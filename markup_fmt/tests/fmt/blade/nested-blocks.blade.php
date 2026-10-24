@foreach ($sections as $section)
<section>
@if ($section->visible)
<h2>{{ $section->title }}</h2>
@forelse ($section->items as $item)
@if ($loop->first) <hr> @endif
@switch($item->type)
@case('a')
<p>A: {{ $item->name }}</p>
@break
@default
<p>{{ $item->name }}</p>
@endswitch
@empty
<p>No items</p>
@endforelse
@elseif ($section->hidden)
@can('view', $section)
@push('hidden')
<div>{{ $section->id }}</div>
@endpush
@endcan
@else
@auth
@guest
<span>never</span>
@endguest
@endauth
@endif
</section>
@endforeach
