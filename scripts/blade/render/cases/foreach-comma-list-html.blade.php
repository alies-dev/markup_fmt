<div><p>
Tags:
@foreach ($tags as $tag)
<a href="/t/{{ $tag }}">{{ $tag }}</a>@if (! $loop->last), @endif
@endforeach
</p></div>
