@if($a)
<p>a</p>
@elseif ($b)
<p>b</p>
@endif
@foreach($items as $item)
{{ $item }}
@endforeach
@isset ($x)
@endisset
@include ('partials.x')
@csrf
@method ('PUT')
@datetime ($when)
<x-alert @class(['a']) />
