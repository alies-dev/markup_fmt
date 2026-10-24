@foreach ($users as $user)
<p>This is user {{ $user->id }}</p>
@endforeach

@for ($i = 0; $i < 10; $i++)
The current value is {{ $i }}
@endfor

@while (true)
<p>I'm looping forever.</p>
@endwhile

<table>
<tbody>
@foreach($rows as $key=>$row)
<tr><td>{{ $key }}</td><td>{{ $row }}</td></tr>
@endforeach
</tbody>
</table>
@foreach ($users as $user) <b>{{ $user->name }}</b> @endforeach
