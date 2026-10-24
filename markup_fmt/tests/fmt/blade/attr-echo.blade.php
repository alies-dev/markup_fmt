<div {{ $attributes->merge(['class' => 'alert alert-'.$type]) }}>
<input {{ $attributes }} type="text">
<a href="#" {!! $extra !!}>x</a>
<span {{$attributes->class(['a'])}} id="x"></span>
<p {{{ $legacy }}} data-a="1">y</p>
</div>
