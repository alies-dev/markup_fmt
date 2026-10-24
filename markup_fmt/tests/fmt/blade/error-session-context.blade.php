<form method="POST">
@csrf
<input name="email" value="{{ old('email') }}">
@error('email')
<span class="error">{{ $message }}</span>
@else
<span class="ok">ok</span>
@enderror
@error('title', 'post')
<span>{{ $message }}</span>
@enderror
</form>

@session('status')
<div class="alert">{{ $value }}</div>
@endsession

@context('canonical')
<link rel="canonical" href="{{ $value }}">
@endcontext
