<p>
{{
    $user->name
}}
</p>
<div>
{{ $user->first_name .
   $user->last_name }}
</div>
{!!
  $html
!!}
<span>{{ collect($items)
    ->map(fn ($i) => $i->name)
    ->implode(', ') }}</span>
