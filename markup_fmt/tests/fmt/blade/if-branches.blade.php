<div class="card">
@if ($user->isAdmin())
<p>Admin</p>
@elseif ($user->isEditor() && $user->active)
        <p>Editor</p>
@elseif($user->isGuest())
<p>Guest</p>
@else
<p>Member</p>
@endif
</div>
@if(count($items) > 0 )
<ul>
@foreach($items as $item)
<li>{{$item->name}}</li>
@endforeach
</ul>
@endif
@if ($a) <span>short form</span> @endif
@if ($a)
<span>no else</span>
@endif
@if ($b)
@else
<p>empty first branch</p>
@endif
@IF ($upper)
<p>built-ins match case-insensitively</p>
@ENDIF
