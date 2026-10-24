<?php

declare(strict_types=1);

use App\Models\User;

/** @var \Illuminate\Support\Collection $users */
/** @var string $title */
$count   = count($users);
?>
<h1>{{ $title }}</h1>
<?php foreach ($users as $user): ?>
<p><?= e($user->name) ?></p>
<?php endforeach; ?>
