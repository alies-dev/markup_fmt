<div><pre>@php
    $n = 41;
@endphp
value: {{ $n + 1 }}
@php($m = 2)  after-inline-php {{ $m }}
</pre></div>
