<x-profile :$user :$team />
<x-card :$title ::class="{ 'a': b }" :count="$items->count() > 0" data-x='{{ $y }}' />
<x-input :value="old('name')" :disabled="$a  &&  $b" name="name" name="again"/>
<x-card title="Hi {{ $name }}" :show="$count > 0">x</x-card>
<x-alert @class(['p-4', 'bg-red' => $e]) @style(['color: red']) @click="x = 1">Alert</x-alert>
<x-button   type='submit'   class="a"   id=btn>Go</x-button>
<x-chart :data="$series->map(fn ($s) => $s->value)->all()" />
