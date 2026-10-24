# `blade.customDirectives`

Names of directives registered with `Blade::directive()`, written without the leading `@`.

An unknown `@name` is plain text in Blade files, so a custom directive must be listed to be
recognized as a directive. Names are case-sensitive (unlike built-in directives) and override a built-in directive
that has exactly the same spelling.

Arguments of custom directives are opaque: they're never sent to an external PHP formatter.

A name consists of letters, digits and underscores, optionally followed by `::` and another word.

Default value is `[]`.

This option only affects Blade files.

## Example

```jsonc
{
  "markup": {
    "blade.customDirectives": ["datetime", "money"]
  }
}
```

With the configuration above, `@datetime($date)` and `@money($amount)` are directives.
`@unknown($x)` stays text.
