<div>
@if ($a)
<p>{{ $a }}</p>
@else
<p>none</p>
@endif
{{-- multi
line --}}
@php
    $x = 1;
@endphp
</div>
<?php
$y = 2;
?>
<p>end</p>
