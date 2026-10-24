@unless (Auth::check())
<a href="/login">Sign in</a>
@else
<a href="/logout">Sign out</a>
@endunless

@isset($records)
<p>{{ count($records) }} records</p>
@else
<p>none</p>
@endisset

@isset($a, $b)
    <span>both</span>
@elseif (isset($a))
<span>only a</span>
@endisset

@empty($records)
<p>No records</p>
@endempty

@empty ($users)
<p>No users</p>
@else
<p>Some users</p>
@endempty

@unless ($hidden)
@elseif ($other)
<p>elseif inside unless compiles to PHP if</p>
@endunless
