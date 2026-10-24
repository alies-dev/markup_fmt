# `blade.directiveSpace`

Control the space between a directive name and its opening parenthesis in Blade files.

Available option values:

- `"preserve"`: Keep the source spelling.
- `"normalize"`: Built-in control-flow directives (`@if`, `@elseif`, `@unless`, `@isset`, `@empty`, `@foreach`,
  `@forelse`, `@for`, `@while`, `@switch`, `@case`) get exactly one space before `(`.
  All other built-in directives get none.

`"normalize"` never changes custom directives, nor `@class` and `@style` in component start tags,
because a space there changes how the template compiles.

Default value is `"preserve"`.

This option only affects Blade files.

## Example for `"preserve"`

Input:

```blade
@if($a)
  @include ('partial')
@endif
```

Output:

```blade
@if($a)
  @include ('partial')
@endif
```

## Example for `"normalize"`

Input:

```blade
@if($a)
  @include ('partial')
@endif
```

Output:

```blade
@if ($a)
  @include('partial')
@endif
```
