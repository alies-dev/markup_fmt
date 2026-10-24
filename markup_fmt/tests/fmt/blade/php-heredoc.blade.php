<?php
$sql = <<<SQL
SELECT * FROM users WHERE name = '?>' -- not the end
SQL;
$doc = <<<'EOT'
?> {{ $notEcho }} @if
EOT;
$quoted = <<<"Q"
  a {$b} ?>
  Q;
echo $sql, $doc;
?>
<p>{{ $sql }}</p>
