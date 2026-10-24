@component('alert', ['type' => 'error'])
    @slot('title')
        Forbidden
    @endslot

    You are not allowed to access this resource!
@endcomponent

@component('card')
@slot('header', 'Inline slot')
Body
@slot('footer', null, ['class' => 'mt-4'])
<small>block slot</small>
@endslot
@endcomponent

@componentFirst(['custom.alert', 'alert'])
<p>fallback</p>
@endComponentFirst
