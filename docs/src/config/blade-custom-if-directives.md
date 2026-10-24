# `blade.customIfDirectives`

Names of conditionals registered with `Blade::if()`, written without the leading `@`.

Each name `n` provides these directives:

- `@n(...)`: opens a block.
- `@unlessn(...)`: opens a block that is closed by `@endn`.
- `@elsen(...)`: branch.
- `@else` and `@elseif(...)`: branches, as with any block compiled to a PHP `if`.
- `@endn`: closes the block.

Names are case-sensitive. Arguments are opaque: they're never sent to an external PHP formatter.

Default value is `[]`.

This option only affects Blade files.

## Example

```jsonc
{
  "markup": {
    "blade.customIfDirectives": ["disk"]
  }
}
```

With the configuration above, this template is a balanced block:

```blade
@disk('local')
  <p>local</p>
@elsedisk('s3')
  <p>s3</p>
@else
  <p>other</p>
@enddisk
```
