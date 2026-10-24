@use('App\Models\User')
@use('App\Models\User', 'U')
@use('App\Models\{User, Post}')
@use(function App\Helpers\format)
@use(const App\LIMIT)
<p>{{ User::count() }}</p>
