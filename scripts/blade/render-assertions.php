<?php

declare(strict_types=1);

/*
 * Render assertions for whitespace-visible Blade behaviour.
 *
 * diff-compile.php compares compiled PHP; this script compares what the templates actually *render*. For every
 * render/cases/<name>.blade.php (optional sibling <name>.json = view data) it
 *   1. formats render/cases/ with the corpus binary (--php-formatter none, and mago too with --mago),
 *   2. renders the original and each formatted version with a real Illuminate view stack
 *      (Factory + EngineResolver + CompilerEngine + FileViewFinder, illuminate/view pinned in composer.json),
 *   3. compares the two outputs and prints PASS/FAIL per case and formatter (with a diff on FAIL).
 *
 * Usage:
 *   php render-assertions.php [--mago] [--filter=SUBSTR] [--cases=DIR] [--views=DIR] [--corpus=BINARY]
 *                             [--autoload=/path/to/vendor/autoload.php] [--keep] [--context=N]
 *
 *   --mago         additionally format with --php-formatter mago (the binary must be built with the mago feature)
 *   --filter       only cases whose name contains SUBSTR
 *   --cases/--views  default render/cases and render/views (view root for anonymous components: components/<name>.blade.php)
 *   --corpus       default corpus/target/release/blade-corpus (build: cd corpus && cargo build --release)
 *   --keep         keep the temp dir (formatted trees, compiled views) and print its path
 *   --context      chars of context around the first differing char (default 40)
 *
 * Exit status: 0 all PASS, 1 at least one FAIL, 2 usage/environment error.
 *
 * Comparison modes (chosen by file name)
 *   <name>.text.blade.php   plain text: outputs are compared byte for byte, apart from leading/trailing whitespace of the
 *                           whole output (a formatter owns the final newline of a file).
 *   <name>.blade.php        HTML: outputs are normalised like a browser would see them, then compared:
 *                             - <pre>/<textarea>/<script>/<style> content is kept byte for byte, except that the single
 *                               newline right after <pre>/<textarea> is dropped (the HTML parser ignores it);
 *                             - elsewhere whitespace runs collapse to one space, attribute whitespace is collapsed (quoted
 *                               values are exact), and whitespace touching a block-level tag (or the output edge) is dropped;
 *                             - whitespace between inline elements / text / echo output stays significant.
 *
 * A case also FAILs when the corpus run reports the file as failed / crashed / non-idempotent, since a refused file is
 * copied unchanged and would otherwise pass trivially.
 *
 * Determinism: Str::uuid() (@once, @pushOnce, ...) is replaced by a counter restarted before every render; PHP warnings and
 * notices raised while rendering are turned into exceptions.
 */

use Illuminate\Container\Container;
use Illuminate\Contracts\Foundation\Application;
use Illuminate\Contracts\View\Factory as FactoryContract;
use Illuminate\Events\Dispatcher;
use Illuminate\Filesystem\Filesystem;
use Illuminate\Support\Str;
use Illuminate\View\Compilers\BladeCompiler;
use Illuminate\View\Engines\CompilerEngine;
use Illuminate\View\Engines\EngineResolver;
use Illuminate\View\Engines\PhpEngine;
use Illuminate\View\Factory;
use Illuminate\View\FileViewFinder;

const BLOCK_TAGS = [
    'address', 'article', 'aside', 'blockquote', 'body', 'dd', 'details', 'dialog', 'div', 'dl', 'dt', 'fieldset',
    'figcaption', 'figure', 'footer', 'form', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'head', 'header', 'hgroup', 'hr',
    'html', 'li', 'link', 'main', 'meta', 'nav', 'ol', 'p', 'pre', 'script', 'section', 'style', 'table', 'tbody', 'td',
    'tfoot', 'th', 'thead', 'title', 'tr', 'ul',
];
const RAW_TAGS = ['pre', 'textarea', 'script', 'style'];
const HTML_WS = " \t\r\n\f";

function usage(string $message): never
{
    fwrite(STDERR, "error: {$message}\n");
    fwrite(STDERR, "usage: php render-assertions.php [--mago] [--filter=SUBSTR] [--cases=DIR] [--views=DIR] [--corpus=BINARY] [--autoload=PATH] [--keep] [--context=N]\n");
    exit(2);
}

$here = __DIR__;
$autoload = $here.'/vendor/autoload.php';
$casesDir = $here.'/render/cases';
$viewsDir = $here.'/render/views';
$corpus = $here.'/corpus/target/release/blade-corpus';
$withMago = false;
$filter = null;
$keep = false;
$context = 40;

foreach (array_slice($argv, 1) as $arg) {
    if ($arg === '--mago') {
        $withMago = true;
    } elseif ($arg === '--keep') {
        $keep = true;
    } elseif (str_starts_with($arg, '--filter=')) {
        $filter = substr($arg, 9);
    } elseif (str_starts_with($arg, '--cases=')) {
        $casesDir = substr($arg, 8);
    } elseif (str_starts_with($arg, '--views=')) {
        $viewsDir = substr($arg, 8);
    } elseif (str_starts_with($arg, '--corpus=')) {
        $corpus = substr($arg, 9);
    } elseif (str_starts_with($arg, '--autoload=')) {
        $autoload = substr($arg, 11);
    } elseif (str_starts_with($arg, '--context=')) {
        $context = max(0, (int) substr($arg, 10));
    } else {
        usage("unknown option: {$arg}");
    }
}

if (! is_file($autoload)) {
    usage("autoload not found: {$autoload} (run `composer install` in scripts/blade or pass --autoload=)");
}
if (! is_dir($casesDir)) {
    usage("cases directory not found: {$casesDir}");
}
if (! is_file($corpus) || ! is_executable($corpus)) {
    usage("corpus binary not found: {$corpus} (build it: cd scripts/blade/corpus && cargo build --release)");
}
$casesDir = rtrim((string) realpath($casesDir), '/');
$viewsDir = is_dir($viewsDir) ? rtrim((string) realpath($viewsDir), '/') : null;

require $autoload;

/** Minimal Application stand-in: ComponentTagCompiler only asks for the root namespace. */
final class StubApplication
{
    public function getNamespace(): string
    {
        return 'App\\';
    }
}

/**
 * Renders one template file with a fresh Illuminate view stack (own compile cache, so no state leaks between renders).
 *
 * @param array<string, mixed> $data
 *
 * @throws Throwable
 */
function renderBlade(string $file, array $data, ?string $viewsDir, string $cacheDir): string
{
    if (! is_dir($cacheDir)) {
        mkdir($cacheDir, 0777, true);
    }

    $files = new Filesystem;
    $container = new Container;
    Container::setInstance($container);
    $container->bind(Application::class, static fn () => new StubApplication);

    $compiler = new BladeCompiler($files, $cacheDir);
    $resolver = new EngineResolver;
    $resolver->register('blade', static fn () => new CompilerEngine($compiler, $files));
    $resolver->register('php', static fn () => new PhpEngine($files));
    $finder = new FileViewFinder($files, $viewsDir === null ? [] : [$viewsDir]);
    $factory = new Factory($resolver, $finder, new Dispatcher($container));
    $container->instance(FactoryContract::class, $factory);
    $container->instance('view', $factory);

    $counter = 0;
    Str::createUuidsUsing(static function () use (&$counter) {
        return sprintf('00000000-0000-4000-8000-%012d', ++$counter);
    });

    set_error_handler(static function (int $no, string $message, string $path, int $line): bool {
        throw new ErrorException($message, 0, $no, $path, $line);
    });
    try {
        return $factory->file($file, $data)->render();
    } finally {
        restore_error_handler();
    }
}

/**
 * Browser-like normal form of rendered HTML (see the header comment).
 */
function normalizeRenderedHtml(string $html): string
{
    $tokens = []; // [kind, text, block?]  kind: text | tag | raw
    $len = strlen($html);
    $i = 0;
    $raw = null;

    while ($i < $len) {
        if ($raw !== null) {
            $end = stripos($html, '</'.$raw, $i);
            $end = $end === false ? $len : $end;
            if ($end > $i) {
                $tokens[] = ['raw', substr($html, $i, $end - $i), false];
            }
            $i = $end;
            $raw = null;

            continue;
        }

        if (substr($html, $i, 4) === '<!--') {
            $end = strpos($html, '-->', $i);
            $end = $end === false ? $len : $end + 3;
            $tokens[] = ['raw', substr($html, $i, $end - $i), false];
            $i = $end;

            continue;
        }

        if ($html[$i] === '<' && preg_match('/\G<(\/?)([A-Za-z][A-Za-z0-9-]*)/', $html, $m, 0, $i)) {
            $closing = $m[1] === '/';
            $name = strtolower($m[2]);
            $tag = '<'.$m[1].$m[2];
            $i += strlen($m[0]);
            $quote = null;
            while ($i < $len) {
                $c = $html[$i];
                if ($quote !== null) {
                    $tag .= $c;
                    $quote = $c === $quote ? null : $quote;
                    $i++;
                } elseif ($c === '"' || $c === "'") {
                    $quote = $c;
                    $tag .= $c;
                    $i++;
                } elseif ($c === '>') {
                    $tag = rtrim($tag, ' ').'>';
                    $i++;

                    break;
                } elseif (strpbrk($c, HTML_WS) !== false) {
                    while ($i < $len && strpbrk($html[$i], HTML_WS) !== false) {
                        $i++;
                    }
                    $tag .= ' ';
                } else {
                    $tag .= $c;
                    $i++;
                }
            }
            $tokens[] = ['tag', $tag, in_array($name, BLOCK_TAGS, true)];
            if (! $closing && in_array($name, RAW_TAGS, true) && ! str_ends_with($tag, '/>')) {
                $raw = $name;
                if (in_array($name, ['pre', 'textarea'], true)) {
                    if (substr($html, $i, 2) === "\r\n") {
                        $i += 2;
                    } elseif (($html[$i] ?? '') === "\n") {
                        $i++;
                    }
                }
            }

            continue;
        }

        // Text up to the next tag start (a lone "<" is text).
        $end = preg_match('/<(?:[A-Za-z\/!])/', $html, $m, PREG_OFFSET_CAPTURE, $i + 1) ? $m[0][1] : $len;
        $text = substr($html, $i, $end - $i);
        $tokens[] = ['text', (string) preg_replace('/['.HTML_WS.']+/', ' ', $text), false];
        $i = $end;
    }

    // Whitespace facing a block-level tag (or the output edge) collapses away.
    $n = count($tokens);
    for ($k = 0; $k < $n; $k++) {
        if ($tokens[$k][0] !== 'text') {
            continue;
        }
        if ($k === 0 || ($tokens[$k - 1][0] === 'tag' && $tokens[$k - 1][2])) {
            $tokens[$k][1] = ltrim($tokens[$k][1], ' ');
        }
        if ($k === $n - 1 || ($tokens[$k + 1][0] === 'tag' && $tokens[$k + 1][2])) {
            $tokens[$k][1] = rtrim($tokens[$k][1], ' ');
        }
    }

    // Adjacent text tokens (a dropped tag in between cannot happen, but trimmed-empty ones can) merge into one run.
    $out = '';
    foreach ($tokens as [, $text]) {
        $out .= $text;
    }

    return trim($out, HTML_WS);
}

/** @return array{string, string} [expected, actual] normalised according to the case mode */
function normalizePair(bool $text, string $expected, string $actual): array
{
    if ($text) {
        return [trim($expected, HTML_WS), trim($actual, HTML_WS)];
    }

    return [normalizeRenderedHtml($expected), normalizeRenderedHtml($actual)];
}

function visible(string $s): string
{
    return strtr($s, ["\n" => '\n', "\r" => '\r', "\t" => '\t']);
}

/** Window of both strings around the first differing byte, whitespace made visible. */
function renderDifference(string $expected, string $actual, int $context): string
{
    $p = 0;
    $max = min(strlen($expected), strlen($actual));
    while ($p < $max && $expected[$p] === $actual[$p]) {
        $p++;
    }
    $from = max(0, $p - $context);
    $window = static fn (string $s): string => ($from > 0 ? '…' : '')
        .visible(substr($s, $from, $context * 2)).(strlen($s) > $from + $context * 2 ? '…' : '');

    return sprintf(
        "      first difference at byte %d\n      original : %s\n      formatted: %s",
        $p,
        $window($expected),
        $window($actual),
    );
}

/** Source text with visible newlines, one numbered row per line. */
function showSource(string $label, string $source): string
{
    $rows = [];
    foreach (explode("\n", $source) as $n => $line) {
        $rows[] = sprintf('        %2d| %s', $n + 1, str_replace("\t", '\t', $line));
    }

    return "      {$label}:\n".implode("\n", $rows).(str_ends_with($source, "\n") ? '' : "\n        (no trailing newline)");
}

/**
 * Runs the corpus binary on `$casesDir`, writing the formatted tree to `$out`.
 *
 * @return array<string, list<string>> corpus-reported problems (failed / crash / non-idempotent): case file name => messages
 */
function formatCases(string $corpus, string $casesDir, string $out, string $formatter): array
{
    $cmd = [$corpus, $casesDir, '--php-formatter', $formatter, '--out', $out, '--examples', '100000'];
    $proc = proc_open($cmd, [1 => ['pipe', 'w'], 2 => ['pipe', 'w']], $pipes);
    if (! is_resource($proc)) {
        usage('cannot start '.$corpus);
    }
    $stdout = (string) stream_get_contents($pipes[1]);
    $stderr = (string) stream_get_contents($pipes[2]);
    fclose($pipes[1]);
    fclose($pipes[2]);
    $status = proc_close($proc);
    if ($status !== 0 && $status !== 1) {
        usage("corpus run with --php-formatter {$formatter} failed (exit {$status}): ".trim($stderr.$stdout));
    }

    $problems = [];
    $section = null;
    $category = '';
    foreach (explode("\n", $stdout) as $line) {
        if (preg_match('/^(failed|crashes|non-idempotent):/', $line, $m)) {
            $section = $m[1];
            $category = '';
        } elseif (preg_match('/^(formatted|unchanged)/', $line)) {
            $section = null;
        } elseif ($section !== null && preg_match('/^  (\w+): \d+$/', $line, $m)) {
            $category = $m[1].' ';
        } elseif ($section !== null && preg_match('/^    (.+?\.blade\.php)(?::\d+(?::\d+)?)?\s+(.*)$/', $line, $m) && str_starts_with($m[1], $casesDir.'/')) {
            $problems[substr($m[1], strlen($casesDir) + 1)][] = "{$section}: {$category}{$m[2]}";
        }
    }

    return $problems;
}

// ---------------------------------------------------------------------------------------------------------------------

$caseFiles = glob($casesDir.'/*.blade.php') ?: [];
sort($caseFiles);
$cases = [];
foreach ($caseFiles as $path) {
    $rel = basename($path);
    $name = substr($rel, 0, -strlen('.blade.php'));
    if ($filter !== null && ! str_contains($name, $filter)) {
        continue;
    }
    $dataFile = $casesDir.'/'.$name.'.json';
    $data = [];
    if (is_file($dataFile)) {
        $data = json_decode((string) file_get_contents($dataFile), true);
        if (! is_array($data)) {
            usage("invalid JSON in {$dataFile}");
        }
    }
    $cases[] = ['name' => $name, 'rel' => $rel, 'path' => $path, 'text' => str_ends_with($name, '.text'), 'data' => $data];
}
if ($cases === []) {
    usage('no cases found in '.$casesDir.($filter !== null ? " matching '{$filter}'" : ''));
}

$tmp = sys_get_temp_dir().'/blade-render-'.bin2hex(random_bytes(4));
mkdir($tmp, 0777, true);
register_shutdown_function(static function () use ($tmp, $keep): void {
    if ($keep) {
        fwrite(STDERR, "kept: {$tmp}\n");

        return;
    }
    (new Filesystem)->deleteDirectory($tmp);
});

$formatters = $withMago ? ['none', 'mago'] : ['none'];
$formatted = [];
foreach ($formatters as $formatter) {
    $formatted[$formatter] = formatCases($corpus, $casesDir, $tmp.'/formatted-'.$formatter, $formatter);
}

$rows = [];
$failed = 0;

foreach ($cases as $case) {
    try {
        $original = renderBlade($case['path'], $case['data'], $viewsDir, $tmp.'/cache/original-'.$case['name']);
    } catch (Throwable $e) {
        foreach ($formatters as $formatter) {
            $rows[] = [$case['name'], $formatter, 'FAIL', "original does not render (broken case): {$e->getMessage()}"];
            $failed++;
        }

        continue;
    }

    $source = (string) file_get_contents($case['path']);
    foreach ($formatters as $formatter) {
        $formattedPath = $tmp.'/formatted-'.$formatter.'/'.$case['rel'];
        $problems = $formatted[$formatter][$case['rel']] ?? [];
        $detail = [];

        if (! is_file($formattedPath)) {
            $detail[] = 'no formatted file was written by the corpus run';
        } else {
            $formattedSource = (string) file_get_contents($formattedPath);
            foreach ($problems as $problem) {
                $detail[] = "corpus run: {$problem}";
            }

            try {
                $actual = renderBlade($formattedPath, $case['data'], $viewsDir, $tmp.'/cache/'.$formatter.'-'.$case['name']);
                [$a, $b] = normalizePair($case['text'], $original, $actual);
                if ($a !== $b) {
                    $detail[] = "rendered output differs ({$formatter}; ".($case['text'] ? 'exact' : 'html-normalised').")\n"
                        .renderDifference($a, $b, $context);
                }
            } catch (Throwable $e) {
                $detail[] = 'formatted template does not render: '.$e->getMessage();
            }

            if ($detail !== []) {
                $detail[] = showSource('original source', $source);
                $detail[] = showSource('formatted source', $formattedSource);
            }
        }

        $ok = $detail === [];
        $failed += $ok ? 0 : 1;
        $rows[] = [$case['name'], $formatter, $ok ? 'PASS' : 'FAIL', implode("\n", $detail)];
    }
}

$width = max(array_map(static fn (array $r): int => strlen($r[0]), $rows));
foreach ($rows as [$name, $formatter, $status, $detail]) {
    printf("%s  %-{$width}s  %s\n", $status, $name, $formatter);
    if ($detail !== '') {
        echo '      ', ltrim($detail), "\n";
    }
}

$total = count($rows);
printf("\n%d/%d PASS, %d FAIL (%d cases × %d formatter%s)\n", $total - $failed, $total, $failed, count($cases), count($formatters), count($formatters) === 1 ? '' : 's');

exit($failed > 0 ? 1 : 0);
