# `blade.customBlockDirectives`

Explicit metadata for custom block directives.

The value is an array of objects. Each object has these properties:

- `name` (required, string): name of the opening directive, without the leading `@`.
- `branches` (optional, array of strings): directives that continue the block. Default is `[]`.
- `closers` (required, array of strings): directives that close the block. At least one is needed.
- `raw` (optional, boolean): whether the body is raw text (for example JavaScript) that must be kept
  byte-for-byte. Default is `false`.

A block is never inferred from the existence of `@endname`: if it isn't listed here, `@name` is an inline directive
(when listed in [`blade.customDirectives`](./blade-custom-directives.md)) or plain text.

Names are case-sensitive and override a built-in directive with exactly the same spelling.
Invalid entries (not an object, missing `name` or `closers`, invalid names, unknown properties) are reported as
configuration diagnostics and ignored.

Default value is `[]`.

This option only affects Blade files.

## Example

```jsonc
{
  "markup": {
    "blade.customBlockDirectives": [
      { "name": "markdown", "closers": ["endmarkdown"] },
      { "name": "feature", "branches": ["elsefeature"], "closers": ["endfeature"] },
      { "name": "script", "closers": ["endscript"], "raw": true }
    ]
  }
}
```

With the configuration above:

```blade
@feature('beta')
<p>Beta</p>
@elsefeature
<p>Stable</p>
@endfeature
```

is formatted as a block, so its children are indented:

```blade
@feature('beta')
  <p>Beta</p>
@elsefeature
  <p>Stable</p>
@endfeature
```

while the body of `@script … @endscript` is printed as-is.
