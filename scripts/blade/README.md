# Blade tooling

Developer tooling for markup_fmt's `Language::Blade`. Nothing here ships with the crate.

| Piece | Purpose |
|---|---|
| `corpus/` | Rust binary: formats a directory tree of `*.blade.php`, reports failures / crashes / non-idempotent output, optionally writes the formatted tree. |
| `diff-compile.php` | Differential gate: compiles original and formatted trees with Laravel's `BladeCompiler` and compares the PHP token streams. |
| `render-assertions.php`, `render/` | Renders curated whitespace-visible cases (original vs formatted) with a real Illuminate view engine and compares the output. |
| `drift.php`, `known-compile-methods.txt` | Detects Laravel adding/removing directives (`compile*` methods) the directive table was not reviewed against. |
| `composer.json` / `composer.lock` | Pins `illuminate/view` to **v13.38.0**, the reference compiler. |

PHP ≥ 8.3 and Composer are needed for the PHP scripts (`cd scripts/blade && composer install`; `vendor/` is git-ignored).

## Corpus runner

```sh
cd scripts/blade/corpus
cargo run --release -- <dir>... [--php-formatter none|mago] [--out <dir>] \
                                [--print-width 180] [--indent-width 4] [--examples 5] [--strict]
```

The crate is standalone (its own empty `[workspace]`), depends on `markup_fmt` by path, and is built from this directory only.

Without PHP formatting (Blade structure only; every PHP snippet is returned unchanged):

```sh
cargo run --release -- /path/to/app/resources/views --php-formatter none --out /tmp/blade-none
```

With PHP formatting by [mago](https://mago.carthage.software) (in-process through the `mago-formatter` crate, pinned to `=1.58.2`; the `mago-*` crates are versioned in lockstep, so bump `mago-formatter`, `mago-php-version` and `mago-allocator` together; needs rustc >= 1.99). It exercises the framing/unframing/postcondition path of the external formatter bridge:

```sh
cargo run --release -- /path/to/app/resources/views --php-formatter mago --out /tmp/blade-mago
```

`--no-default-features` builds without mago (then only `--php-formatter none` works) and avoids compiling its dependency tree.

Script/style (and any non-`php`) snippets always use an identity formatter, so JS/CSS formatting never masks Blade problems.

With several input directories `--out` mirrors each under its own directory name (names must be distinct); with one, the tree is mirrored directly under `--out`. Files that fail or crash are copied unchanged, so the output tree is always complete.

### Reading the output

```
formatted (changed):  files that formatted successfully and differ from the source
unchanged:            files that formatted successfully and are already formatted
failed:               files rejected with a syntax error, by category (see below)
crashes:              files whose formatting panicked (always a bug)
non-idempotent:       formatted successfully but formatting the result again changes it (a bug)
```

`formatted + unchanged + failed + crashes` is the total number of files. Non-idempotent files are also counted in `formatted`/`unchanged`.

Failure categories are the `SyntaxErrorKind` variant name (e.g. `ExpectBladeBlockEnd`, `BladeUnexpectedDirective`, `ExpectCloseTag`); `BladeUnsupported` is split by its reason. `External` is an error returned by the callback for a script/style body (not expected here). `Unreadable` is I/O or non-UTF-8 input. Each category lists up to `--examples` `file:line:column` entries; a failed parse never produces output, so these are the "formatter refused" cases — they are expected for genuinely broken templates, but each one on a template that Laravel compiles deserves a look. Non-idempotent examples show the first differing line of the first and second pass.

Exit status: `1` when there are crashes or non-idempotent files (or any failure with `--strict`), `2` on usage errors, otherwise `0`.

## Differential compile gate

A correct Blade formatter only moves whitespace, so compiling the original and the formatted template must give the same PHP. `diff-compile.php` checks that with the real compiler.

```sh
cd scripts/blade && composer install        # once
# 1. format an app's views (writes the mirrored tree)
(cd corpus && cargo run --release -- /path/to/app/resources/views --php-formatter none --out /tmp/blade-none)
# 2. compile both trees and compare
php diff-compile.php /path/to/app/resources/views /tmp/blade-none
```

Useful options:

- `--autoload=/path/to/app/vendor/autoload.php`: use the app's own Laravel and let the app's real component classes resolve (default: the pinned `illuminate/view` in `scripts/blade/vendor`).
- `--setup=file.php`: a file returning `fn (BladeCompiler $blade): void`, to register the app's custom directives (`$blade->directive(...)`, `if`, component aliases/namespaces). Unregistered custom directives simply stay text on both sides, which hides nothing about whitespace but also does not exercise their output.
- `--ws=edge|collapse`, `--show=whitespace`, `--php-normalize`, `--context=N`, `--limit=N` (see below).

Compile determinism: `Str::uuid()` (used by `@once`, `@pushOnce`, `@prependOnce`) is replaced by a counter restarted for every file; class-less component tags (`<x-foo>`) are compiled through a stub view factory (`exists()` is always true) so they resolve as anonymous components without needing the app's component classes.

Per file, the compiled PHP of both versions is tokenized (`PhpToken::tokenize`) and `TOKEN_PARSE`-checked (equivalent to `php -l` on the compiled output). Token streams are compared ignoring positions; `T_WHITESPACE` is dropped, `<?php`/`?>` are normalised (the newline PHP swallows after `?>` is not output), whitespace inside PHP comments is collapsed.

Inline HTML (`T_INLINE_HTML`) is normalised by a small stateful scanner — an **approximation of HTML whitespace semantics**:

- whitespace runs collapse to one space; **not** inside `<pre>`, `<textarea>`, `<script>`, `<style>` or quoted attribute values, which are compared byte for byte (state survives PHP islands inside them);
- `--ws=edge` (default) also ignores whitespace directly after `>` or before `<`, and the indentation after a `?>` that swallowed a newline — formatters re-indent/re-flow block structure, so everything else would be noise. It cannot see a space added or removed between two inline elements;
- `--ws=collapse` only collapses runs (use it to audit inline whitespace; expect more output);
- whitespace between a PHP block and text, or between PHP blocks, is otherwise compared exactly — adjacency around echoes and directives is semantic in Blade.

With `--php-formatter mago` the PHP code inside directive arguments/echoes is rewritten by mago, so token streams legitimately differ (quote style, trailing commas, `array()` → `[]`, …). Use the `none` corpus run for the strict gate and the `mago` run to review what PHP formatting changes.

### Whitespace-only classification

Each compared file lands in exactly one tier, so a formatter that merely re-indents or normalises HTML is separated from one that changes output: `identical` → `whitespace-only` → `html-whitespace` → `php-normalized` → `differs`.

- `identical`: token streams equal under the normalisation above.
- `whitespace-only`: equal after a second pass that drops *whitespace-only* `T_INLINE_HTML` tokens (collapsed to one space, outside `<pre>`/`<textarea>`/`<script>`/`<style>`/quoted attribute values/comments) when HTML collapses them anyway: the last non-space char of the previous HTML token **or** the first non-space char of the next one (skipping PHP tokens) is a tag boundary (`>` / `<`), or the token is at file start/end. Typical case: indentation between two `@stack`/`@yield` lines (`?>`, spaces, `<?php`) that the formatter moved or removed.
- `html-whitespace`: equal after removing **all** whitespace inside `T_INLINE_HTML` outside `<pre>`/`<textarea>` — script/style bodies and attribute values included (JS/CSS indentation, `a, b` vs `a,b` are formatter normalisations) — while PHP tokens are still compared exactly. Files where whitespace was added/removed between text and/or echo output (`a <?php echo` → `a<?php echo`; neighbours skipping PHP statements but not echoing islands are neither `>` nor `<` nor a file edge) are printed in a prominent `AUDIT` block: they stay in this tier (not a failure) but need a human look.
- `php-normalized` (only with `--php-normalize`, for runs with a PHP formatter): additionally `T_CONSTANT_ENCAPSED_STRING` is compared by decoded value (`'x'` equals `"x"` when the double-quoted one has no escapes/`$`) and a trailing `,` before `)`/`]` is ignored. Redundant parentheses etc. stay differences.
- `differs`: anything else; the only tier (besides compile/parse failures) that fails the gate.

The report ends with an audit of the whitespace-only tier (tokens dropped on one side only, grouped by class): `between-tags` (no PHP in between), `file-edge`, `across-php` (tag boundary found by skipping directive statements) and `across-echo` (skipping an island that echoes: the riskiest class, because the echoed value may be text, so review those). `--show=whitespace` prints the first difference of every whitespace-only file (subject to `--limit`). For `differs` files the printed difference is computed on the last tier that applied (HTML whitespace removed, plus PHP normalisation with `--php-normalize`); the differing token is windowed around its first differing character (`…` marks cut text).

### Reading the output

```
identical                          token streams equal
whitespace-only                    equal once whitespace-only HTML tokens next to a tag boundary / file edge are dropped (not a failure; audit at the end of the report)
html-whitespace                    equal after removing ALL whitespace in inline HTML outside <pre>/<textarea>; PHP tokens exact (not a failure; see AUDIT block)
php-normalized                     (--php-normalize only) additionally equal after PHP normalisations (not a failure)
differs                            first differing token printed with context (`>>` marks it; line numbers are lines of the compiled PHP)
parse-error-only-in-formatted      compiled formatted output is not valid PHP but the original's is
compile-error-in-formatted         the formatted template makes BladeCompiler throw
missing-formatted                  no counterpart in the formatted directory
skipped-original-does-not-compile  original already fails to compile; not compared
```

The last informational lines count originals whose compiled PHP is already invalid and the `html-whitespace`/`php-normalized` files that have audit entries. Exit status `1` when any file fails (all counts except `identical`, `whitespace-only`, `html-whitespace`, `php-normalized` and `skipped-…`), `2` on usage errors. At the very end a one-line first difference per `differs` file is listed.

## Render assertions

`diff-compile.php` compares compiled PHP, which is blind to whether a whitespace change *renders* differently (the newline PHP swallows after `?>`, the newline Blade adds after an echo, a space between an echo and text, ...). `render-assertions.php` closes that gap for a curated set of whitespace-visible cases: it formats `render/cases/` with the corpus binary, **renders** original and formatted templates with a real Illuminate view stack (`Factory` + `EngineResolver` + `CompilerEngine` + `FileViewFinder` from the pinned `illuminate/view`) and compares the output.

```sh
cd scripts/blade && composer install                 # once
(cd corpus && cargo build --release)                 # once; the script runs corpus/target/release/blade-corpus
php render-assertions.php                            # --php-formatter none
php render-assertions.php --mago                     # also --php-formatter mago (binary built with the mago feature)
php render-assertions.php --filter=pre- --keep       # subset; keep the temp dir (formatted trees, compiled views)
```

Output is one `PASS`/`FAIL` line per case and formatter; a FAIL prints the first differing byte (original vs formatted, whitespace made visible) and both sources. Exit status: `0` all pass, `1` any FAIL, `2` usage/environment error (missing binary, autoload, bad JSON).

Cases are `render/cases/<name>.blade.php` with optional `<name>.json` (view data, decoded to an array). `render/views/` is the view root, so `<x-alert>` resolves to the anonymous component `render/views/components/alert.blade.php`; it is rendered, not formatted. The file name picks the comparison:

- `<name>.text.blade.php` — plain text: outputs must be byte-identical, ignoring only leading/trailing whitespace of the whole output (a formatter owns the file's final newline).
- `<name>.blade.php` — HTML: both outputs are normalised the way a browser sees them, then compared. `<pre>`/`<textarea>`/`<script>`/`<style>` content is exact (except the single newline right after `<pre>`/`<textarea>`, which the HTML parser drops); elsewhere whitespace runs collapse to one space and whitespace touching a block-level tag (or the output edge) disappears. Whitespace between inline elements, text and echo output stays significant.

A case also fails when the corpus run reports its file as failed, crashed or non-idempotent (a refused file is copied unchanged and would otherwise pass trivially), or when the original does not render at all (broken case). `Str::uuid()` is replaced by a counter restarted per render, and PHP warnings while rendering become errors.

Covered: echo followed by a newline inside `<pre>`/`<textarea>` (escaped and `{!! !!}`), `a{{ $x }}b` / `a {{ $x }} b` / echo sequences, `Hello @if($vip) dear @endif {{ $name }},` (true and false), `<?php ?>` followed by newline vs space, `@php` blocks, `@verbatim` and directives (`@if`, `@foreach`) inside `<pre>`, `@{{ x }}` / `@@if` escapes, `@foreach` building a comma list (text and HTML), `@switch` with whitespace between `@switch` and the first `@case`, inline comments, inline elements, an anonymous component. To add a case, drop the template (and JSON) into `render/cases/`.

## Drift check

```sh
cd scripts/blade && composer install
php drift.php                                  # against the pinned illuminate/view
php drift.php --autoload=/path/to/app/vendor/autoload.php
php drift.php --print                          # inventory in known-compile-methods.txt format
```

It reflects over `BladeCompiler` (traits included), lowercases every `compile*` method except compiler plumbing (`compile`, `compileString`, `compileComponentTags`, `compileExtensions`, `compileStatements`, `compileStatement`, `compileEchos`, `compileRegularEchos`, `compileRawEchos`, `compileEscapedEchos`, `compileComments`, `compileClassComponentOpening`, `compileEndComponentClass`) and diffs against `known-compile-methods.txt` (`#` lines ignored; the header names the Laravel version it was generated from). Exit `1` on any added/removed method. When it fires: review `markup_fmt/src/blade.rs`, then regenerate the list with `--print` and update the version header.

`@verbatim` has no `compile*` method (it is handled by the compiler's raw-block pre-pass) and so is not in the list.

`.github/workflows/blade-drift.yml` runs it on relevant PRs (pinned version), weekly and on demand (pinned version, then the latest `illuminate/view`); a failing job is the signal for manual review.
