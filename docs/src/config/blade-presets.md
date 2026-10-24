# `blade.presets`

Directive sets of common packages to recognize in Blade files, so you don't have to list them by hand
with [`blade.customDirectives`](./blade-custom-directives.md) and
[`blade.customBlockDirectives`](./blade-custom-block-directives.md).

Available preset values:

- `"livewire"`: Livewire 3. Inline: `@livewire`, `@livewireStyles`, `@livewireScripts`, `@livewireScriptConfig`,
  `@entangle`, `@this`. Blocks: `@persist … @endpersist`, `@teleport … @endteleport`, `@assets … @endassets`,
  and `@script … @endscript` whose body is raw JavaScript.
- `"volt"`: Livewire Volt. Block: `@volt … @endvolt`.
- `"inertia"`: Inertia. Inline: `@inertia`, `@inertiaHead`.
- `"ziggy"`: Ziggy. Inline: `@routes`.
- `"filament"`: Filament. Inline: `@filamentStyles`, `@filamentScripts`.

Default value is `[]`.

This option only affects Blade files. Tags such as `<livewire:counter />` and `<flux:button>` don't need a preset:
they're ordinary elements.

## Example

```jsonc
{
  "markup": {
    "blade.presets": ["livewire", "inertia"]
  }
}
```
