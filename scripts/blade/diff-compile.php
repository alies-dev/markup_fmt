<?php

declare(strict_types=1);

/*
 * Differential compile gate.
 *
 * For every *.blade.php under <original-dir>, compiles it and its counterpart under <formatted-dir>
 * (same relative path) with fresh Illuminate BladeCompiler instances and compares the resulting PHP
 * token streams. A formatter that only moves whitespace must yield identical streams.
 *
 * Usage:
 *   php diff-compile.php <original-dir> <formatted-dir>
 *        [--autoload=/path/to/vendor/autoload.php]  default: ./vendor/autoload.php (illuminate/view pinned in composer.json)
 *        [--setup=/path/to/setup.php]               file returning fn (BladeCompiler $blade): void, to register custom
 *                                                   directives / component aliases / namespaces of the app under test
 *        [--ws=edge|collapse]                       HTML whitespace normalisation, see below (default: edge)
 *        [--context=N]                              tokens of context around the first difference (default: 6)
 *        [--php-normalize]                          enable the php-normalized tier (for runs with a PHP formatter)
 *        [--show=whitespace]                        also print the first difference of every whitespace-only file
 *        [--limit=N]                                max failing (and, with --show, whitespace-only) files to detail (default: 50; 0 = all)
 *
 * Exit status: 0 no file differs (identical / whitespace-only only), 1 at least one failure, 2 usage/environment error.
 *
 * Determinism
 *   - Str::uuid() (used by @once, @pushOnce, @prependOnce) is replaced by a counter that restarts for each compiled file.
 *   - Component tags (<x-foo>) are compiled without real component classes: the container resolves the view
 *     factory to a stub whose exists() is always true, so every component that has no class resolves as an
 *     anonymous component (the same code path Laravel uses for class-less components). If the autoloader
 *     can load the app's real component classes (--autoload pointing at the app), those are used instead.
 *
 * Token comparison (an approximation, deliberately)
 *   PHP side: positions ignored; T_WHITESPACE dropped; T_OPEN_TAG / T_CLOSE_TAG reduced to "<?php" / "?>"
 *   (the newline PHP swallows after "?>" is not output); whitespace runs inside comments collapsed.
 *   HTML side (T_INLINE_HTML): whitespace runs collapse to one space, except inside <pre>, <textarea>, <script>,
 *   <style> and quoted attribute values, which are compared byte-for-byte. The scanner is a small stateful
 *   HTML tokenizer whose state survives PHP islands (so `<pre><?php echo $a; ?>  x</pre>` stays raw).
 *   --ws=edge additionally drops whitespace directly after ">" and directly before "<" (block-level
 *   reflowing by a formatter is expected); it therefore cannot see a space added/removed between two inline
 *   elements. It also ignores the indentation following a "?>" that swallowed a newline (formatters indent
 *   block children). Use --ws=collapse to see those too (noisier).
 *   Whitespace between a PHP block and text (or two PHP blocks) is otherwise compared (echo adjacency is semantic).
 *
 * Classification (per file)
 *   identical        token streams equal under the normalisation above
 *   whitespace-only  equal after additionally dropping whitespace-only HTML tokens (collapsed to " ", outside raw
 *                    elements / quoted attributes) that touch a tag boundary: the last non-space char of the previous
 *                    HTML token or the first non-space char of the next one (skipping PHP tokens) is ">" / "<", or the
 *                    token is at file start/end. Other whitespace-only tokens (between text / echo output) stay as one
 *                    space, so removing "a <?php echo" -> "a<?php echo" is "differs". The audit at the end of the report
 *                    says which class of whitespace was dropped (between-tags, file-edge, across-php, across-echo).
 *   html-whitespace  equal after removing ALL whitespace inside T_INLINE_HTML outside <pre>/<textarea> (script/style bodies
 *                    and attribute values included: formatter normalisations); PHP tokens still compared exactly. Files where
 *                    whitespace was removed/added between text and/or echo output are listed in a prominent AUDIT block.
 *   php-normalized   (only with --php-normalize) additionally: T_CONSTANT_ENCAPSED_STRING compared by decoded value
 *                    ('x' == "x" without escapes/$), a trailing "," before ")" / "]" ignored. Nothing else (strict).
 *   differs          anything else; only these (and compile/parse failures) make the gate fail.
 */

use Illuminate\Container\Container;
use Illuminate\Contracts\Foundation\Application;
use Illuminate\Contracts\View\Factory;
use Illuminate\Filesystem\Filesystem;
use Illuminate\Support\Str;
use Illuminate\View\Compilers\BladeCompiler;

$positional = [];
$autoload = __DIR__.'/vendor/autoload.php';
$setupFile = null;
$wsMode = 'edge';
$contextSize = 6;
$limit = 50;
$show = null;
$phpNormalize = false;

foreach (array_slice($argv, 1) as $arg) {
    if (str_starts_with($arg, '--autoload=')) {
        $autoload = substr($arg, 11);
    } elseif (str_starts_with($arg, '--setup=')) {
        $setupFile = substr($arg, 8);
    } elseif (str_starts_with($arg, '--ws=')) {
        $wsMode = substr($arg, 5);
    } elseif (str_starts_with($arg, '--context=')) {
        $contextSize = max(0, (int) substr($arg, 10));
    } elseif (str_starts_with($arg, '--show=')) {
        $show = substr($arg, 7);
    } elseif (str_starts_with($arg, '--limit=')) {
        $limit = max(0, (int) substr($arg, 8));
    } elseif ($arg === '--php-normalize') {
        $phpNormalize = true;
    } elseif (str_starts_with($arg, '--')) {
        usage("unknown option: {$arg}");
    } else {
        $positional[] = $arg;
    }
}

if (count($positional) !== 2) {
    usage('expected <original-dir> <formatted-dir>');
}
if (! in_array($wsMode, ['edge', 'collapse'], true)) {
    usage('--ws must be edge or collapse');
}
if ($show !== null && $show !== 'whitespace') {
    usage('--show must be whitespace');
}
[$originalDir, $formattedDir] = array_map(static fn (string $d) => rtrim($d, '/'), $positional);
foreach ([$originalDir, $formattedDir] as $dir) {
    if (! is_dir($dir)) {
        usage("not a directory: {$dir}");
    }
}
if (! is_file($autoload)) {
    usage("autoload not found: {$autoload} (run `composer install` in scripts/blade or pass --autoload=)");
}

require $autoload;

/** View factory stand-in: every view "exists", so class-less components resolve as anonymous components. */
final class AlwaysExistsViewFactory implements Factory
{
    public function exists($view)
    {
        return true;
    }

    public function file($path, $data = [], $mergeData = [])
    {
        throw new LogicException('not used at compile time');
    }

    public function make($view, $data = [], $mergeData = [])
    {
        throw new LogicException('not used at compile time');
    }

    public function share($key, $value = null)
    {
        return $value;
    }

    public function composer($views, $callback)
    {
        return [];
    }

    public function creator($views, $callback)
    {
        return [];
    }

    public function addNamespace($namespace, $hints)
    {
        return $this;
    }

    public function replaceNamespace($namespace, $hints)
    {
        return $this;
    }
}

$container = new Container;
Container::setInstance($container);
$container->bind(Factory::class, static fn () => new AlwaysExistsViewFactory);
// ComponentTagCompiler::guessClassName() only calls getNamespace() on the application.
$container->bind(Application::class, static fn () => new class {
    public function getNamespace(): string
    {
        return 'App\\';
    }
});

$setup = null;
if ($setupFile !== null) {
    if (! is_file($setupFile)) {
        usage("setup file not found: {$setupFile}");
    }
    $setup = require $setupFile;
    if (! is_callable($setup)) {
        usage('--setup file must return a callable taking a BladeCompiler');
    }
}

function usage(string $message): never
{
    fwrite(STDERR, "error: {$message}\n");
    fwrite(STDERR, "usage: php diff-compile.php <original-dir> <formatted-dir> [--autoload=PATH] [--setup=PATH] [--ws=edge|collapse] [--show=whitespace] [--php-normalize] [--context=N] [--limit=N]\n");
    exit(2);
}

/** @return string compiled PHP; throws on compile errors */
function compileBlade(string $source, ?callable $setup): string
{
    $counter = 0;
    Str::createUuidsUsing(static function () use (&$counter) {
        return sprintf('00000000-0000-4000-8000-%012d', ++$counter);
    });

    $blade = new BladeCompiler(new Filesystem, sys_get_temp_dir());
    if ($setup !== null) {
        $setup($blade);
    }

    return $blade->compileString($source);
}

/** Whether `$php` is syntactically valid PHP (what `php -l` would accept); returns the error text otherwise. */
function phpSyntaxError(string $php): ?string
{
    try {
        PhpToken::tokenize($php, TOKEN_PARSE);
    } catch (ParseError $e) {
        return $e->getMessage().' on compiled line '.$e->getLine();
    }

    return null;
}

const RAW_ELEMENTS = ['pre', 'textarea', 'script', 'style'];

/**
 * Stateful scanner that normalises T_INLINE_HTML chunks; the state persists across PHP islands.
 */
final class HtmlNormalizer
{
    private string $mode = 'text'; // text | tag | comment

    private ?string $quote = null;

    private ?string $raw = null; // open raw element name (pre/textarea/script/style)

    private ?string $openingRaw = null; // raw element whose start tag is currently being scanned

    private bool $trimLeading = true; // swallow whitespace here (edge mode: after ">" / at file start)

    /** @var list<int> strip mode: offsets (in the text returned by normalize()) where text-mode whitespace was removed */
    public array $gaps = [];

    public function __construct(private readonly bool $edge, private readonly bool $strip = false) {}

    /**
     * Call for every PHP (non-HTML) token that is emitted. `$endsLine`: the token is a "?>" that swallowed a
     * newline; in edge mode the indentation that follows it is formatter-controlled and ignored.
     */
    public function phpIsland(bool $endsLine = false): void
    {
        // Strip mode keeps whitespace after a newline-swallowing close tag: only one newline is swallowed, the rest
        // is output (Blade doubles the newline after echoes), so it is judged by the tag-boundary rule instead.
        $this->trimLeading = $endsLine && $this->edge && ! $this->strip;
    }

    /** Whether whitespace at the current position is significant byte for byte (raw element, quoted attribute, comment). */
    public function inSensitiveState(): bool
    {
        return $this->raw !== null || $this->quote !== null || $this->mode === 'comment';
    }

    public function normalize(string $s): string
    {
        $this->gaps = [];
        $out = '';
        $floor = 0; // trailing-space trimming never reaches into raw element content
        $len = strlen($s);
        $i = 0;

        while ($i < $len) {
            if ($this->raw !== null) {
                $end = $this->findRawEnd($s, $i);
                $piece = $end === null ? substr($s, $i) : substr($s, $i, $end - $i);
                $out .= $this->keep($piece, ! in_array($this->raw, ['pre', 'textarea'], true));
                $this->trimLeading = false;
                if ($end === null) {
                    return $out;
                }
                $floor = strlen($out);
                $i = $end;
                $this->raw = null;

                continue;
            }

            if ($this->mode === 'comment') {
                $end = strpos($s, '-->', $i);
                if ($end === false) {
                    return $out.$this->keep(substr($s, $i), true);
                }
                $out .= $this->keep(substr($s, $i, $end + 3 - $i), true);
                $i = $end + 3;
                $this->mode = 'text';
                $this->trimLeading = $this->edge;

                continue;
            }

            if ($this->mode === 'tag') {
                $i = $this->scanTag($s, $i, $out);

                continue;
            }

            // text mode
            $c = $s[$i];
            if ($c === '<') {
                if (substr($s, $i, 4) === '<!--') {
                    $this->mode = 'comment';
                    $out = $this->edge ? self::rtrimSpaces($out, $floor) : $out;
                    $out .= '<!--';
                    $i += 4;
                    $this->trimLeading = false;

                    continue;
                }
                if ($i + 1 < $len && (ctype_alpha($s[$i + 1]) || $s[$i + 1] === '/' || $s[$i + 1] === '!')) {
                    if ($this->edge) {
                        $out = self::rtrimSpaces($out, $floor);
                    }
                    $this->mode = 'tag';
                    $this->quote = null;
                    $this->openingRaw = null;
                    if (preg_match('/\G<([A-Za-z][A-Za-z0-9]*)/', $s, $m, 0, $i)) {
                        $name = strtolower($m[1]);
                        if (in_array($name, RAW_ELEMENTS, true)) {
                            $this->openingRaw = $name;
                        }
                    }
                    $out .= '<';
                    $i++;
                    $this->trimLeading = false;

                    continue;
                }
            }

            if (ctype_space($c)) {
                $j = $i;
                while ($j < $len && ctype_space($s[$j])) {
                    $j++;
                }
                if ($this->strip) {
                    if (! $this->trimLeading) {
                        $this->gaps[] = strlen($out);
                    }
                } elseif (! ($this->trimLeading && $this->edge) && ! ($out !== '' && str_ends_with($out, ' '))) {
                    $out .= ' ';
                }
                $i = $j;

                continue;
            }

            $out .= $c;
            $this->trimLeading = false;
            $i++;
        }

        return $out;
    }

    private function keep(string $piece, bool $stripWhitespace): string
    {
        return $this->strip && $stripWhitespace ? (string) preg_replace('/\s+/', '', $piece) : $piece;
    }

    private static function rtrimSpaces(string $out, int $floor): string
    {
        $n = strlen($out);
        while ($n > $floor && $out[$n - 1] === ' ') {
            $n--;
        }

        return substr($out, 0, $n);
    }

    private function scanTag(string $s, int $i, string &$out): int
    {
        $len = strlen($s);

        while ($i < $len) {
            $c = $s[$i];

            if ($this->quote !== null) {
                $end = strpos($s, $this->quote, $i);
                if ($end === false) {
                    $out .= $this->keep(substr($s, $i), true);

                    return $len;
                }
                $out .= $this->keep(substr($s, $i, $end + 1 - $i), true);
                $i = $end + 1;
                $this->quote = null;

                continue;
            }

            if ($c === '"' || $c === "'") {
                $this->quote = $c;
                $out .= $c;
                $i++;

                continue;
            }

            if ($c === '>') {
                $out = rtrim($out, ' ');
                $out .= '>';
                $i++;
                $this->mode = 'text';
                $this->trimLeading = $this->edge;
                if ($this->openingRaw !== null && ! str_ends_with(substr($out, -2), '/>')) {
                    $this->raw = $this->openingRaw;
                }
                $this->openingRaw = null;

                return $i;
            }

            if (ctype_space($c)) {
                while ($i < $len && ctype_space($s[$i])) {
                    $i++;
                }
                if (! $this->strip && ! str_ends_with($out, ' ')) {
                    $out .= ' ';
                }

                continue;
            }

            $out .= $c;
            $i++;
        }

        return $i;
    }

    private function findRawEnd(string $s, int $from): ?int
    {
        $pos = stripos($s, '</'.$this->raw, $from);

        return $pos === false ? null : $pos;
    }
}

/**
 * Strip mode (`$strip`): all whitespace inside HTML except <pre>/<textarea> is removed (script/style bodies and
 * attribute values included). Tokens then carry [3] = text-mode whitespace was removed right before this token and,
 * for HTML tokens, [4] = offsets of text-mode whitespace removed inside the text (see buildUnits()).
 *
 * @return list<array{0: string, 1: string, 2: int, 3?: bool, 4?: list<int>}> [kind, text, compiled line, droppable-whitespace flag (HTML only) / gap flag]
 */
function normalizedTokens(string $php, bool $edge, bool $strip = false): array
{
    $html = new HtmlNormalizer($edge || $strip, $strip);
    $tokens = [];
    $pending = false;

    foreach (PhpToken::tokenize($php) as $token) {
        $before = count($tokens);
        switch ($token->id) {
            case T_INLINE_HTML:
                $sensitive = $html->inSensitiveState();
                $text = $html->normalize($token->text);
                if ($strip) {
                    $len = strlen($text);
                    $lead = false;
                    $trail = false;
                    $inner = [];
                    foreach ($html->gaps as $g) {
                        if ($text === '' || $g >= $len) {
                            $trail = true;
                        } elseif ($g === 0) {
                            $lead = true;
                        } else {
                            $inner[] = $g;
                        }
                    }
                    if ($text !== '') {
                        $tokens[] = ['HTML', $text, $token->line, $pending || $lead, $inner];
                        $pending = $trail;
                    } else {
                        $pending = $pending || $trail;
                    }
                } elseif ($text !== '') {
                    // 4th element: droppable candidate (collapsed whitespace-only token outside sensitive regions)
                    $tokens[] = ['HTML', $text, $token->line, ! $sensitive && $text === ' ' && ctype_space($token->text)];
                }

                break;
            case T_WHITESPACE:
                break;
            case T_OPEN_TAG:
                $html->phpIsland();
                $tokens[] = [$token->getTokenName(), '<?php', $token->line];

                break;
            case T_OPEN_TAG_WITH_ECHO:
                $html->phpIsland();
                $tokens[] = [$token->getTokenName(), '<?=', $token->line];

                break;
            case T_CLOSE_TAG:
                $html->phpIsland(str_ends_with($token->text, "\n"));
                $tokens[] = [$token->getTokenName(), '?>', $token->line];

                break;
            case T_COMMENT:
            case T_DOC_COMMENT:
                $html->phpIsland();
                $tokens[] = [$token->getTokenName(), trim(preg_replace('/\s+/', ' ', $token->text)), $token->line];

                break;
            default:
                $html->phpIsland();
                $tokens[] = [$token->getTokenName() ?? 'CHAR', $token->text, $token->line];
        }

        if ($strip && count($tokens) > $before && $tokens[$before][0] !== 'HTML') {
            $tokens[$before][3] = $pending;
            $pending = false;
        }
    }

    return $tokens;
}

/**
 * Text/echo units of a strip-mode token stream: every HTML char is one unit, every PHP token is "\0" (open/close
 * tag of an island that echoes: "\1", opaque like text). Gaps are the unit positions where text-mode whitespace was removed.
 *
 * @return array{units: string, gaps: array<int, true>, labels: array<int, string>}
 */
function buildUnits(array $tokens): array
{
    $units = '';
    $gaps = [];
    $labels = [];
    $open = null;
    $echo = false;

    foreach ($tokens as $t) {
        if ($t[3] ?? false) {
            $gaps[strlen($units)] = true;
        }
        if ($t[0] === 'HTML') {
            foreach ($t[4] as $g) {
                $gaps[strlen($units) + $g] = true;
            }
            $units .= $t[1];

            continue;
        }

        $i = strlen($units);
        $units .= "\0";
        $labels[$i] = $t[1];
        if ($t[0] === 'T_OPEN_TAG' || $t[0] === 'T_OPEN_TAG_WITH_ECHO') {
            $open = $i;
            $echo = $t[0] === 'T_OPEN_TAG_WITH_ECHO';
        } elseif ($t[0] === 'T_ECHO' || $t[0] === 'T_PRINT') {
            $echo = true;
        } elseif ($t[0] === 'T_CLOSE_TAG') {
            if ($echo && $open !== null) {
                $units[$open] = "\1";
                $units[$i] = "\1";
            }
            $open = null;
            $echo = false;
        }
    }

    return ['units' => $units, 'gaps' => $gaps, 'labels' => $labels];
}

/**
 * Whitespace present in one stream only, between text/echo output: both neighbours (skipping PHP statements, but not
 * echoing islands) are neither a tag end ">" / tag start "<" nor the file edge. Such removals change rendered text.
 *
 * @return list<string>
 */
function textGapRisks(array $ua, array $ub): array
{
    $risks = [];
    foreach ([[$ua, $ub, 'original'], [$ub, $ua, 'formatted']] as [$x, $y, $side]) {
        $u = $x['units'];
        $n = strlen($u);
        foreach (array_diff_key($x['gaps'], $y['gaps']) as $p => $_) {
            $l = $p - 1;
            while ($l >= 0 && $u[$l] === "\0") {
                $l--;
            }
            $r = $p;
            while ($r < $n && $u[$r] === "\0") {
                $r++;
            }
            $c1 = $l >= 0 ? $u[$l] : null;
            $c2 = $r < $n ? $u[$r] : null;
            if ($c1 === null || $c1 === '>' || $c2 === null || $c2 === '<') {
                continue;
            }

            $from = max(0, $p - 25);
            $text = '';
            for ($k = $from; $k < min($n, $p + 25); $k++) {
                $text .= $k === $p ? '|' : '';
                if ($u[$k] === "\0" || $u[$k] === "\1") {
                    $label = $x['labels'][$k] ?? '';
                    $text .= $label === '<?php' || $label === '<?=' ? '⟨'.$label.'…' : ($label === '?>' ? '?>⟩' : '');
                } else {
                    $text .= $u[$k];
                }
            }
            $risks[] = "whitespace only in {$side} (|): ".($from > 0 ? '…' : '').$text;
            if (count($risks) >= 200) {
                return $risks;
            }
        }
    }

    return $risks;
}

/** Tier 4: ignore a trailing "," before ")" / "]" and compare plain string literals by decoded value. */
function phpNormalize(array $tokens): array
{
    $out = [];
    $n = count($tokens);
    for ($i = 0; $i < $n; $i++) {
        $t = $tokens[$i];
        if ($t[0] === ',' && isset($tokens[$i + 1]) && ($tokens[$i + 1][0] === ')' || $tokens[$i + 1][0] === ']')) {
            continue;
        }
        if ($t[0] === 'T_CONSTANT_ENCAPSED_STRING') {
            $inner = substr($t[1], 1, -1);
            if ($t[1][0] === "'") {
                $t[1] = 'S:'.strtr($inner, ['\\\\' => '\\', "\\'" => "'"]);
            } elseif (! str_contains($inner, '\\') && ! str_contains($inner, '$')) {
                $t[1] = 'S:'.$inner;
            }
        }
        $out[] = $t;
    }

    return $out;
}

/** One-line description of the first difference (HTML tokens are windowed around the first differing char). */
function describeDifference(array $a, array $b, int $at): string
{
    $x = $a[$at] ?? null;
    $y = $b[$at] ?? null;
    if ($x !== null && $y !== null && $x[0] === 'HTML' && $y[0] === 'HTML') {
        $p = 0;
        $max = min(strlen($x[1]), strlen($y[1]));
        while ($p < $max && $x[1][$p] === $y[1][$p]) {
            $p++;
        }
        $from = max(0, $p - 25);

        return sprintf('L%d html "%s" vs "%s"', $x[2], substr($x[1], $from, 55), substr($y[1], $from, 55));
    }
    $fmt = static fn (?array $t): string => $t === null ? '<end of stream>' : sprintf('%s %s', $t[0], substr($t[1], 0, 50));

    return sprintf('L%d %s vs %s', ($x ?? $y)[2] ?? 0, $fmt($x), $fmt($y));
}

/**
 * Drops whitespace-only T_INLINE_HTML tokens (those the HTML scanner reduced to a single " " outside raw
 * elements / quoted attributes) when HTML collapses them anyway: the last non-space char of the previous
 * HTML token or the first non-space char of the next one (skipping PHP tokens) is a tag boundary (">" / "<"),
 * or the token is at file start/end. Anything else is significant whitespace between text/echo output and is kept.
 *
 * @param list<array{0: string, 1: string, 2: int, 3?: bool}> $tokens
 *
 * @return array{0: list<array{0: string, 1: string, 2: int, 3?: bool}>, 1: array<int, string>} kept tokens, dropped index => class
 */
function lenientStream(array $tokens): array
{
    $n = count($tokens);
    $isEcho = static fn (string $kind): bool => in_array($kind, ['T_ECHO', 'T_OPEN_TAG_WITH_ECHO', 'T_PRINT'], true);

    // [context char (null = file edge), crossed PHP, crossed echo output]
    $prevCtx = [];
    $state = [null, false, false];
    for ($k = 0; $k < $n; $k++) {
        if ($tokens[$k][0] !== 'HTML') {
            $state[1] = true;
            $state[2] = $state[2] || $isEcho($tokens[$k][0]);
        } elseif ($tokens[$k][3] ?? false) {
            $prevCtx[$k] = $state;
        } else {
            $last = substr(rtrim($tokens[$k][1], " \t\r\n\f"), -1);
            $state = [$last === '' ? 'x' : $last, false, false];
        }
    }

    $nextCtx = [];
    $state = [null, false, false];
    for ($k = $n - 1; $k >= 0; $k--) {
        if ($tokens[$k][0] !== 'HTML') {
            $state[1] = true;
            $state[2] = $state[2] || $isEcho($tokens[$k][0]);
        } elseif ($tokens[$k][3] ?? false) {
            $nextCtx[$k] = $state;
        } else {
            $rest = ltrim($tokens[$k][1], " \t\r\n\f");
            $state = [preg_match('/^<[A-Za-z\/!]/', $rest) === 1 ? '<' : 'x', false, false];
        }
    }

    $kept = [];
    $drops = [];
    foreach ($tokens as $k => $token) {
        if (isset($prevCtx[$k]) && ($prevCtx[$k][0] === null || $prevCtx[$k][0] === '>' || $nextCtx[$k][0] === null || $nextCtx[$k][0] === '<')) {
            $crossedEcho = $prevCtx[$k][2] || $nextCtx[$k][2];
            $crossedPhp = $prevCtx[$k][1] || $nextCtx[$k][1];
            $atEdge = $prevCtx[$k][0] === null || $nextCtx[$k][0] === null;
            $drops[$k] = $crossedEcho ? 'across-echo' : ($crossedPhp ? 'across-php' : ($atEdge ? 'file-edge' : 'between-tags'));

            continue;
        }
        $kept[] = $token;
    }

    return [$kept, $drops];
}

const WHITESPACE_CLASSES = [
    'between-tags' => 'no PHP in between, a tag boundary on at least one side',
    'file-edge' => 'at file start/end, no PHP crossed',
    'across-php' => 'tag boundary found by skipping PHP islands without output (directive statements)',
    'across-echo' => 'tag boundary found by skipping PHP islands that echo (riskiest: output may be text)',
];

/**
 * Classes of the whitespace-only tokens dropped on one side only (greedy alignment of the two streams).
 *
 * @return list<string>
 */
function droppedWhitespaceClasses(array $a, array $b, array $dropsA, array $dropsB): array
{
    $classes = [];
    $i = $j = 0;
    $na = count($a);
    $nb = count($b);
    while ($i < $na || $j < $nb) {
        if ($i < $na && $j < $nb && $a[$i][0] === $b[$j][0] && $a[$i][1] === $b[$j][1]) {
            $i++;
            $j++;
        } elseif ($i < $na && isset($dropsA[$i])) {
            $classes[] = $dropsA[$i++];
        } elseif ($j < $nb && isset($dropsB[$j])) {
            $classes[] = $dropsB[$j++];
        } else {
            break;
        }
    }

    return $classes;
}

/** Index of the first differing token (or of the first extra one when one stream is a prefix), null if equal. */
function firstDifference(array $a, array $b): ?int
{
    $n = min(count($a), count($b));
    for ($i = 0; $i < $n; $i++) {
        if ($a[$i][0] !== $b[$i][0] || $a[$i][1] !== $b[$i][1]) {
            return $i;
        }
    }

    return count($a) === count($b) ? null : $n;
}

/**
 * `$other`: the stream compared against; the differing token is windowed around its first differing character
 * ("…" marks cut text) so long collapsed HTML tokens show where they diverge.
 */
function renderContext(array $tokens, int $at, int $size, ?array $other = null): string
{
    $from = max(0, $at - $size);
    $to = min(count($tokens) - 1, $at + $size);
    $lines = [];
    for ($i = $from; $i <= $to; $i++) {
        $marker = $i === $at ? '>>' : '  ';
        $text = $tokens[$i][1];
        $prefix = '';
        if ($i === $at && $other !== null && isset($other[$at]) && $other[$at][0] === $tokens[$i][0]) {
            $p = 0;
            $max = min(strlen($text), strlen($other[$at][1]));
            while ($p < $max && $text[$p] === $other[$at][1][$p]) {
                $p++;
            }
            if ($p > 40) {
                $text = substr($text, $p - 40);
                $prefix = '…';
            }
        }
        $text = $prefix.str_replace(["\n", "\r", "\t"], ['\n', '\r', '\t'], $text);
        if (strlen($text) > 100) {
            $text = substr($text, 0, 100).'…';
        }
        $lines[] = sprintf('    %s L%-4d %-24s %s', $marker, $tokens[$i][2], $tokens[$i][0], $text);
    }
    if ($at >= count($tokens)) {
        $lines[] = '    >> <end of stream>';
    }

    return implode("\n", $lines);
}

$files = [];
$iterator = new RecursiveIteratorIterator(new RecursiveDirectoryIterator($originalDir, FilesystemIterator::SKIP_DOTS));
foreach ($iterator as $file) {
    if ($file->isFile() && str_ends_with(strtolower($file->getFilename()), '.blade.php')) {
        $files[] = substr($file->getPathname(), strlen($originalDir) + 1);
    }
}
sort($files);

$counts = [
    'identical' => 0,
    'whitespace-only' => 0,
    'html-whitespace' => 0,
    'php-normalized' => 0,
    'differs' => 0,
    'parse-error-only-in-formatted' => 0,
    'compile-error-in-formatted' => 0,
    'missing-formatted' => 0,
    'skipped-original-does-not-compile' => 0,
];
$originalInvalidPhp = 0;
$riskFiles = [];
$differsSummary = [];
$failures = [];
$whitespaceOnly = [];
/** @var array<string, array{0: int, 1: array<string, true>}> class => [dropped tokens, files] */
$droppedByClass = [];

foreach ($files as $rel) {
    $originalSource = file_get_contents($originalDir.'/'.$rel);
    $formattedPath = $formattedDir.'/'.$rel;

    if (! is_file($formattedPath)) {
        $counts['missing-formatted']++;
        $failures[] = [$rel, 'missing-formatted', 'no counterpart in '.$formattedDir];

        continue;
    }

    try {
        $originalPhp = compileBlade($originalSource, $setup);
    } catch (Throwable $e) {
        $counts['skipped-original-does-not-compile']++;

        continue;
    }

    try {
        $formattedPhp = compileBlade(file_get_contents($formattedPath), $setup);
    } catch (Throwable $e) {
        $counts['compile-error-in-formatted']++;
        $failures[] = [$rel, 'compile-error-in-formatted', get_class($e).': '.$e->getMessage()];

        continue;
    }

    $originalError = phpSyntaxError($originalPhp);
    $formattedError = phpSyntaxError($formattedPhp);
    if ($originalError !== null) {
        $originalInvalidPhp++;
    }

    $a = normalizedTokens($originalPhp, $wsMode === 'edge');
    $b = normalizedTokens($formattedPhp, $wsMode === 'edge');
    $at = firstDifference($a, $b);

    // Tier 2: equal once droppable whitespace-only tokens are removed => "whitespace-only".
    $la = $lb = $dropsA = $dropsB = null;
    $lenientAt = null;
    if ($at !== null) {
        [$baseA, $baseB] = $wsMode === 'edge' ? [$a, $b] : [normalizedTokens($originalPhp, true), normalizedTokens($formattedPhp, true)];
        [$la, $dropsA] = lenientStream($baseA);
        [$lb, $dropsB] = lenientStream($baseB);
        $lenientAt = firstDifference($la, $lb);
    }

    if ($formattedError !== null && $originalError === null) {
        $counts['parse-error-only-in-formatted']++;
        $detail = "formatted output does not parse: {$formattedError}";
        if ($at !== null) {
            $detail .= "\n  first differing token (#{$at}):\n  original:\n".renderContext($a, $at, $contextSize, $b)
                ."\n  formatted:\n".renderContext($b, $at, $contextSize, $a);
        }
        $failures[] = [$rel, 'parse-error-only-in-formatted', $detail];
    } elseif ($at === null) {
        $counts['identical']++;
    } elseif ($lenientAt === null) {
        $counts['whitespace-only']++;
        $classes = [];
        foreach (droppedWhitespaceClasses($baseA, $baseB, $dropsA, $dropsB) as $class) {
            $droppedByClass[$class][0] = ($droppedByClass[$class][0] ?? 0) + 1;
            $droppedByClass[$class][1][$rel] = true;
            $classes[$class] = true;
        }
        $classes = array_keys($classes);
        $whitespaceOnly[] = [$rel, $classes, "first differing token (#{$at}):\n  original:\n".renderContext($baseA, $at, $contextSize, $baseB)
            ."\n  formatted:\n".renderContext($baseB, $at, $contextSize, $baseA)];
    } else {
        // Tier 3: all whitespace inside HTML (outside <pre>/<textarea>) removed, PHP compared exactly.
        $sa = normalizedTokens($originalPhp, true, true);
        $sb = normalizedTokens($formattedPhp, true, true);
        $da = $sa;
        $db = $sb;
        $dAt = firstDifference($sa, $sb);
        $tier = $dAt === null ? 'html-whitespace' : null;
        $how = 'after removing all HTML whitespace outside <pre>/<textarea>';
        if ($tier === null && $phpNormalize) {
            // Tier 4: PHP formatter normalisations on top.
            $da = phpNormalize($sa);
            $db = phpNormalize($sb);
            $dAt = firstDifference($da, $db);
            $tier = $dAt === null ? 'php-normalized' : null;
            $how .= ' and normalising PHP';
        }

        if ($tier !== null) {
            $counts[$tier]++;
            $risks = textGapRisks(buildUnits($da), buildUnits($db));
            if ($risks !== []) {
                $riskFiles[] = [$rel, $tier, $risks];
            }
        } else {
            $counts['differs']++;
            $failures[] = [$rel, 'differs', "first differing token (#{$dAt}, {$how}):\n  original:\n".renderContext($da, $dAt, $contextSize, $db)
                ."\n  formatted:\n".renderContext($db, $dAt, $contextSize, $da)];
            $differsSummary[] = [$rel, describeDifference($da, $db, $dAt)];
        }
    }
}

$shown = 0;
foreach ($failures as [$rel, $kind, $detail]) {
    if ($limit !== 0 && $shown >= $limit) {
        echo '... ', count($failures) - $shown, " more failing files not shown (--limit={$limit})\n";

        break;
    }
    echo "FAIL [{$kind}] {$rel}\n  {$detail}\n\n";
    $shown++;
}

if ($show === 'whitespace') {
    $shown = 0;
    foreach ($whitespaceOnly as [$rel, $classes, $detail]) {
        if ($limit !== 0 && $shown >= $limit) {
            echo '... ', count($whitespaceOnly) - $shown, " more whitespace-only files not shown (--limit={$limit})\n";

            break;
        }
        echo 'WHITESPACE-ONLY [', implode(', ', $classes), "] {$rel}\n  {$detail}\n\n";
        $shown++;
    }
}

if ($riskFiles !== []) {
    echo str_repeat('=', 78), "\n";
    echo 'AUDIT: whitespace added/removed between text and/or echo output (', count($riskFiles), " files; tier 3/4, not failures)\n";
    echo str_repeat('=', 78), "\n";
    foreach ($riskFiles as [$rel, $tier, $risks]) {
        echo "  [{$tier}] {$rel}\n";
        foreach (array_slice($risks, 0, 3) as $risk) {
            echo "      {$risk}\n";
        }
        if (count($risks) > 3) {
            echo '      (+', count($risks) - 3, " more)\n";
        }
    }
    echo "\n";
}

echo 'compared files: ', count($files), "\n";
foreach ($counts as $label => $n) {
    printf("  %-36s %d\n", $label, $n);
}
echo "  (informational) original compiled PHP is itself invalid: {$originalInvalidPhp}\n";
echo '  (informational) html-whitespace / php-normalized files with text/echo whitespace audit entries: ', count($riskFiles), "\n";

echo "\nwhitespace-only audit (whitespace-only HTML tokens dropped on one side only; {$counts['whitespace-only']} files)\n";
if ($droppedByClass === []) {
    echo "  (none)\n";
}
foreach ($droppedByClass as $class => [$tokens, $classFiles]) {
    printf("  %-14s %6d tokens in %5d files   %s\n", $class, $tokens, count($classFiles), WHITESPACE_CLASSES[$class]);
}
if ($show !== 'whitespace' && $counts['whitespace-only'] > 0) {
    echo "  (rerun with --show=whitespace to print examples)\n";
}

if ($differsSummary !== []) {
    echo "\ndiffers: first difference per file (", count($differsSummary), " files)\n";
    foreach ($differsSummary as [$rel, $line]) {
        echo "  {$rel}: {$line}\n";
    }
}

exit(count($failures) > 0 ? 1 : 0);
