@push('sidebar', 'inline content')
@push('sidebar', '')
<p>empty literal second argument opens a block</p>
@endpush
@prepend('sidebar', $content)
@push("sidebar", "")
<p>double-quoted empty</p>
@endpush
@push('block-only')
<p>one argument</p>
@endpush
