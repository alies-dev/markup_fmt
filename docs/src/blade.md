# Laravel Blade

markup_fmt formats [Laravel Blade](https://laravel.com/docs/blade) templates (`*.blade.php`) with the same HTML engine
that handles Jinja, Twig and Vento. Blade directives, echoes and raw PHP are parsed the way Laravel's
`BladeCompiler` recognizes them, and formatting never changes what a template compiles to.

When using markup_fmt as a Rust crate, `detect_language` maps file names ending with `.blade.php`
(ASCII case-insensitive) to `Language::Blade`. Plain `.php` files are not affected.

## Using with dprint

dprint chooses a plugin by a file's last extension (`php`) or its exact file name. A compound extension such as
`blade.php` is never matched by itself, and markup_fmt does not claim every `.php` file because that would take PHP files away
from other plugins. So Blade files must be associated with this plugin explicitly:

```jsonc
{
  "markup": {
    "associations": ["**/*.blade.php"]
  },
  "excludes": ["resources/views/vendor/mail/**"]
}
```

Files that can't be formatted as HTML-based Blade must be excluded by path, see [Limitations](#limitations).

## Supported syntax

- Echoes: `{{ expr }}`, `{!! expr !!}` and the legacy `{{{ expr }}}`. They are printed with exactly one space inside the delimiters
  and never converted from one form to another.
- Comments: `{{-- … --}}` are kept verbatim.
- Escapes: `@{{ … }}`, `@{!! … !!}`, `@{{{ … }}}` and `@@name` are plain text.
- Directives: `@name`, `@name(args)` and `@name (args)`. Built-in names are case-insensitive and keep their source spelling.
  Unknown `@name`, such as Alpine's `@click` or CSS's `@media`, is plain text.
- Block directives: `@if … @elseif … @else … @endif`, `@unless`, `@isset`, `@empty`, `@auth`, `@guest`, `@can`, `@env`,
  `@production`, `@error`, `@session`, `@switch`, `@foreach`, `@forelse … @empty … @endforelse`, `@for`, `@while`,
  `@section`, `@push`, `@prepend`, `@pushOnce`, `@once`, `@fragment`, `@component`, `@slot`, `@lang`, and more.
  Children are indented, and branches and closers are aligned with the opener.
  Directives such as `@section`, `@slot`, `@push`, `@prepend`, `@lang` and `@php` are blocks or inline directives depending on their
  arguments, following the rules of the compiler (for example `@section('title', 'Home')` is inline while
  `@section('title') … @endsection` is a block).
- Raw PHP: `<?php … ?>`, `<?= … ?>` and `@php … @endphp`. `@verbatim … @endverbatim` is kept as-is.
- Directives, echoes and comments in attribute position (`<div {{ $attributes }}>`, `<img @class([...])>`,
  `<option @if ($a) selected @endif>`) and inside quoted attribute values.
- Components: `<x-name>`, `<x:name>`, `<x-pkg::name>`, slots (`<x-slot:name>`, `<x-slot name="…">`, closed by either `</x-slot>` or `</x-slot:name>`),
  the `:attr` / `::attr` / `:$short` attribute forms, and `<{{ $tag }}>` tag names.
  `component.selfClosing` and `component.whitespaceSensitivity` apply to components.
- `<script>` and `<style>` bodies that contain Blade syntax are printed verbatim; others are formatted as usual.

Whitespace right before an `@` directive and right after an echo is never added or removed when it is absent
in the source, because it changes the meaning (`user@example.com` is text, `word@if(...)` is not a directive).

## External formatter contract

markup_fmt owns the Blade-to-PHP framing, so the host only formats PHP and needs no Blade knowledge.
The host callback receives complete PHP snippets with the hint `ext: "php"`, together with `print_width` (already
reduced by indentation and delimiters), `indent_level` and `attr`. markup_fmt unwraps the result and validates it.

| Source | Snippet sent (illustrative) |
|---|---|
| `{{ e }}`, `{!! e !!}`, `{{{ e }}}`, component `:attr="e"` | `<?php (e);` |
| `@if` / `@elseif` / `@unless (c)` | `<?php if (c) {}` |
| `@foreach` / `@forelse (a as b)` | `<?php foreach (a as b) {}` |
| `@for`, `@while`, `@switch`, `@case` | the matching construct |
| `@isset`, `@empty`, `@unset` | `<?php isset(...);` etc. |
| other built-in directives with arguments | `<?php f(ARGS);` |
| `<?php … ?>` and `@php … @endphp` body | the body after `<?php` |
| `<?= … ?>` | not sent (printed as written, as are unclosed `<?php` tags) |
| `@use`, `@verbatim`, custom directives, `Blade::if()` arguments | not sent (opaque) |

The source is kept unchanged when the callback returns an error, returns the input untouched, or when the result can't be
mapped back unambiguously or breaks a postcondition:

- `@foreach` / `@forelse` keep a single line with single spaces around `as`, which the compiler matches textually;
- `@lang` keeps the character after `(`;
- directives that split arguments on commas textually (`@json`, `@js`, `@pushIf`, `@elsePushIf`, `@elsePush`, `@inject`,
  `@pushOnce`, `@prependOnce`, `@each`, `@section`, `@push`, `@prepend`, `@slot`, `@break`, `@continue`) get no trailing comma and keep their comma count;
- the result doesn't contain `}}`, `!!}`, `--}}`, `@endphp`, `?>` (except for raw PHP) or a new directive token;
- a `//` comment keeps its terminating newline;
- string, heredoc and nowdoc contents stay byte-identical.

Without an external formatter, every snippet stays as written. A multi-line echo or directive argument list keeps its own layout
(including the delimiters), but moves together with the node: its continuation lines keep their indentation relative to the line
the node starts on. Lines inside string literals are never touched, and a snippet containing a heredoc is printed byte-for-byte.

Line terminators inside raw PHP (`<?php`, `<?=`, `@php`), `@verbatim`, string/heredoc/nowdoc literals of echoes and directive
arguments, and `<script>` / `<style>` bodies that contain Blade syntax are never rewritten, whatever `lineBreak` is: `\r\n` stays `\r\n`
and a bare `\n` stays `\n`, because a terminator inside a literal is part of its value. `lineBreak` only applies to the layout
markup_fmt produces itself. Results returned by the external formatter are accepted only if their literals are byte-identical
to the source (a result that changes `\r\n` inside a literal is rejected like any other changed literal).

In dprint, snippets are sent to the host with a file name like `view.blade.php#.php`, so another dprint plugin that formats `.php` files
(for example an `exec` plugin running a PHP formatter) is needed to format PHP code; otherwise PHP stays as-is.

## Limitations

- `Blade::extend()` callbacks, `Blade::precompiler()` and `prepareStringsForCompilationUsing()` run arbitrary code over the template,
  so they can't be modeled. Files that depend on them must be excluded, or marked with an ignore-file comment
  (`dprint-ignore-file` in dprint, `markup-fmt-ignore-file` otherwise; see [`ignoreFileCommentDirective`](./config/ignore-file-comment-directive.md)).
- Markdown mail templates (`@component('mail::message')`, `<x-mail::message>`, files under `resources/views/vendor/mail/`) have Markdown bodies,
  where indentation creates code blocks. Re-indenting changes the rendered mail, so exclude them by path.
- Non-HTML Blade (plain-text mail, Slack messages, XML feeds) must be excluded by path as well.
- HTML that is unbalanced across directive branches (for example `@if ($a) <div> @else <div class="x"> @endif … </div>`) or across `@include`
  boundaries is valid Blade but can't be represented as a tree. It is reported as a syntax error, and the file is left unchanged.
- Whether `@section`, `@slot`, `@push` and `@prepend` open a block is decided by the compiler at run time from argument values.
  markup_fmt classifies by literal arguments: `@section('x', $content)` (non-literal second argument) is treated as inline.
- Parenthesis balancing of directive arguments is aware of PHP strings and comments, while Laravel counts parentheses and extends the match.
  Templates where the two disagree are broken or very unusual Blade.
- `@verbatim` and `@php … @endphp` are cut out by the compiler before anything else, even inside Blade comments. An inline `@php($x = 1)` followed
  later by `@endphp` is swallowed into that block by the compiler, and markup_fmt keeps the whole span verbatim. Such raw markers that start inside another construct
  (echo, directive arguments, tag, HTML comment) are reported as syntax errors.
- A directive opener, branch or closer inside an HTML comment (`<!-- @if ($x) -->`) is still compiled by Blade, so it is reported as unsupported.
- Custom directives must be configured (`blade.customDirectives`, `blade.customIfDirectives`, `blade.customBlockDirectives`) or they are plain text.
- Tailwind class sorting and attribute sorting are not performed.

## Compatibility

- Directive inventory: Laravel v13.38.0 (`Illuminate\View\Compilers`). Every `compile*` directive of that version is known to the parser.
- PHP: PHP 8.x. markup_fmt only lexes PHP (strings, comments, heredocs) to find boundaries; syntax is checked by your PHP formatter.
- Components: generic `<x-…>` / `<x:…>` tags.
- Packages: Livewire 3, Volt, Inertia, Ziggy and Filament directives via [`blade.presets`](./config/blade-presets.md).
  `<livewire:…>` and `<flux:…>` tags (compiled by package precompilers) are formatted as ordinary elements; `wire:*` attributes are ordinary attributes.

## Options

- [`blade.customDirectives`](./config/blade-custom-directives.md)
- [`blade.customIfDirectives`](./config/blade-custom-if-directives.md)
- [`blade.customBlockDirectives`](./config/blade-custom-block-directives.md)
- [`blade.directiveSpace`](./config/blade-directive-space.md)
- [`blade.presets`](./config/blade-presets.md)
