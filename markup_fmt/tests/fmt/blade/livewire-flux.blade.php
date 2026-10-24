<div>
<livewire:counter :count="$count" />
<livewire:posts.list wire:key="list" />
<livewire:counter />
<flux:button variant="primary">Save</flux:button>
@livewire('counter')
<input wire:model.live="search" wire:keydown.enter="save">
</div>
