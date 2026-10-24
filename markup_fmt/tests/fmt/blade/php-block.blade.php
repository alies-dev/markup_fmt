@php
    /** @var \App\Models\User $user */
    /** @var string $slug */
        $slug   = Str::slug($user->name);
    $count = count( $items );
@endphp
<p>{{ $slug }}</p>
@if ($show)
<div>
@php
$inner = [1,
   2];
@endphp
</div>
@endif
@php $one = 1; @endphp
