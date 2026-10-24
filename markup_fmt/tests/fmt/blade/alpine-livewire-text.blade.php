<div x-data="{ open: false }" @click.away="open = false" @keydown.escape.window="open = false">
<button @click="open = !open" @keydown.enter.prevent="submit" x-on:click.prevent="go" :class="{ 'a': open }">Toggle</button>
<input wire:model.live="search" wire:keydown.enter="save" x-bind:value="v">
<p x-text="msg" @custom-event.window="handle($event)"></p>
</div>
