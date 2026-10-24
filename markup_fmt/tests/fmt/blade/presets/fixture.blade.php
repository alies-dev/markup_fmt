<div>
@livewire('counter')
@livewireStyles
@persist('player')
<audio src="{{ $src }}"></audio>
@endpersist
@script
<script>
    $wire.on('x',   () => {   });
</script>
@endscript
@this
</div>
