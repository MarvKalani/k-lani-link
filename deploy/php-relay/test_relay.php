<?php
/**
 * Checks the PHP relay core without a web server: `php deploy/php-relay/test_relay.php`.
 * Time is simulated by backdating the stored timestamps.
 */

declare(strict_types=1);

require __DIR__ . '/k-lani-relay.php';

$failures = 0;
function check(string $name, bool $ok): void
{
    global $failures;
    echo ($ok ? 'ok   ' : 'FAIL ') . $name . "\n";
    $failures += $ok ? 0 : 1;
}

$dir = sys_get_temp_dir() . '/k-lani-relay-test-' . bin2hex(random_bytes(6));
mkdir($dir, 0700);
$token = str_repeat('t', 43);
$hash = hash('sha256', $token);
$relay = new KLaniRelay(['dir' => $dir, 'tokens' => [$hash => KLaniRelay::limits()], 'max_wait' => 0]);
$b64 = static fn (string $bytes): string => rtrim(strtr(base64_encode($bytes), '+/', '-_'), '=');
$call = static function (string $method, string $path, array $query = [], string $body = '', ?string $type = null, ?string $auth = null) use ($relay, $token): array {
    return $relay->handle($method, $path, $query, $auth ?? "Bearer $token",
        static fn (int $limit): string => substr($body, 0, $limit + 1), null, $type);
};
$record = static fn (int $sequence): string => 'KLI2' . pack('J', $sequence - 1) . random_bytes(32);
$roomDir = "$dir/" . substr($hash, 0, 32);

// Admission.
check('missing token is refused', $call('GET', '/v1/hello/x/client', [], '', null, '')[0] === 401);
check('wrong token is refused', $call('GET', '/v1/hello/x/client', [], '', null, 'Bearer ' . str_repeat('u', 43))[0] === 401);

// Handshake creates a session.
$room = $b64(random_bytes(16));
$hello = 'KLH1' . chr(1) . random_bytes(KLaniRelay::HELLO_BYTES - 5);
$reply = 'KLH1' . chr(2) . random_bytes(KLaniRelay::REPLY_BYTES - 5);
$helloHash = KLaniRelay::helloHash($hello);
check('hello stored', $call('PUT', "/v1/hello/$room/client", [], $hello)[0] === 204);
check('hello read', $call('GET', "/v1/hello/$room/client", ['wait' => '0'])[2] === $hello);
check('reply stored', $call('POST', "/v1/hello/$room/host/$helloHash", [], $reply)[0] === 204);
$session = KLaniRelay::sessionId($hello, $reply);
$box = "/v1/box/$room/$session/c2h";

// Records, acknowledgement and identical retries.
$first = $record(1);
check('record 1 stored', $call('PUT', "$box/1", [], $first)[0] === 204);
check('identical retry accepted', $call('PUT', "$box/1", [], $first)[0] === 204);
check('different bytes refused', $call('PUT', "$box/1", [], $record(1))[0] === 409);
[$status, , $body] = $call('GET', $box, ['from' => '1', 'wait' => '0']);
check('record 1 delivered with length prefix', $status === 200 && $body === pack('N', strlen($first)) . $first);
check('reading from 2 acknowledges 1', $call('GET', $box, ['from' => '2', 'wait' => '0'])[0] === 204);
check('acknowledged retry still accepted', $call('PUT', "$box/1", [], $first)[0] === 204);

// JSON-wrapped write, as sent behind web firewalls.
$second = $record(2);
check('wrapped record 2 stored', $call('POST', "$box/2", [], json_encode(['data' => $b64($second)]), 'application/json')[0] === 204);
[$status, , $body] = $call('GET', $box, ['from' => '2', 'wait' => '0']);
check('wrapped record delivered as raw bytes', $status === 200 && substr($body, 4) === $second);
check('malformed wrapping refused', $call('POST', "$box/3", [], '{"data":"%%"}', 'application/json')[0] === 400);

// Expiry: a record older than 30 s is gone for readers and writers alike.
$third = $record(3);
check('record 3 stored', $call('PUT', "$box/3", [], $third)[0] === 204);
$backdate = static function (string $file, float $seconds): void {
    $state = json_decode((string) file_get_contents($file), true);
    array_walk_recursive($state, static function (&$value, $key) use ($seconds): void {
        if ($key === 'created') {
            $value -= $seconds;
        }
    });
    file_put_contents($file, json_encode($state));
};
$backdate("$roomDir/$room/room.json", 31);
check('expired record is gone, not an empty answer', $call('GET', $box, ['from' => '3', 'wait' => '0'])[0] === 410);
check('expired record cannot be confirmed again', $call('PUT', "$box/3", [], $third)[0] === 410);
check('expired record file deleted', !is_file("$roomDir/$room/$session.0.3"));

// A full window is a prompt retry (429 without Retry-After), not a wait.
[$status, $headers] = $call('PUT', "$box/" . (4 + KLaniRelay::WINDOW), [], $record(4 + KLaniRelay::WINDOW));
check('write beyond the window is busy', $status === 429 && !isset($headers['Retry-After']));

// Idle cleanup: with no further request, a sweep run removes expired ciphertext.
$fourth = $record(4);
check('record 4 stored', $call('PUT', "$box/4", [], $fourth)[0] === 204);
$backdate("$roomDir/$room/room.json", 31);
$relay->sweepAll();
check('idle sweep deletes expired record files', !is_file("$roomDir/$room/$session.0.4"));
$state = json_decode((string) file_get_contents("$roomDir/$room/room.json"), true);
check('idle sweep keeps no metadata for them', !isset($state['sessions'][$session]['channels'][0]['records'][4]));
// A room unused for two minutes disappears entirely.
$state['updated'] -= 121;
file_put_contents("$roomDir/$room/room.json", json_encode($state));
$relay->sweepAll();
check('idle sweep removes expired rooms', !is_dir("$roomDir/$room"));

// Self-tuning: the relay announces its limits, and busy wait slots mean answering at once.
check('limits are announced', ($call('GET', $box, ['from' => '4', 'wait' => '0'])[1]['X-K-Lani-Relay'] ?? '') === 'wait=0; writes=2');
$waiting = new KLaniRelay(['dir' => $dir, 'tokens' => [$hash => KLaniRelay::limits()], 'max_wait' => 2, 'max_waiters' => 1]);
$room2 = $b64(random_bytes(16));
$read = static function () use ($waiting, $token, $room2): float {
    $started = microtime(true);
    $waiting->handle('GET', "/v1/hello/$room2/client", ['wait' => '2'], "Bearer $token", static fn (): string => '');
    return microtime(true) - $started;
};
check('a free wait slot lets a read wait', $read() >= 1.9);
$held = fopen("$dir/.wait-0", 'c');
flock($held, LOCK_EX);
check('with every wait slot taken a read answers at once', $read() < 0.5);
flock($held, LOCK_UN);
fclose($held);

// Access rules: shared spaces, expiry, address networks, addresses per day, failure brake.
$tokenA = str_repeat('a', 43);
$tokenB = str_repeat('b', 43);
$tokenC = str_repeat('c', 43);
$rules = static fn (array $extra): array => KLaniRelay::limits() + $extra;
$guarded = new KLaniRelay(['dir' => $dir, 'max_wait' => 0, 'failed_auth_per_minute' => 3, 'tokens' => [
    hash('sha256', $tokenA) => $rules(['space' => 'me', 'allow_ips' => ['192.0.2.0/24', '2001:db8::/48']]),
    hash('sha256', $tokenB) => $rules(['space' => 'me', 'max_ips_per_day' => 2, 'expires' => gmdate('Y-m-d', time() + 86400)]),
    hash('sha256', $tokenC) => $rules(['expires' => gmdate('Y-m-d', time() - 86400)]),
]]);
$ask = static function (string $method, string $path, string $token, ?string $peer, string $body = '') use ($guarded): int {
    return $guarded->handle($method, $path, ['wait' => '0'], "Bearer $token",
        static fn (int $limit): string => substr($body, 0, $limit + 1), null, null, $peer)[0];
};
$room3 = $b64(random_bytes(16));
check('allowed network admits', $ask('PUT', "/v1/hello/$room3/client", $tokenA, '192.0.2.10', $hello) === 204);
check('a second token of the same space reads the same room', $ask('GET', "/v1/hello/$room3/client", $tokenB, '198.51.100.1') === 200);
check('the first token\'s own space differs from the old per-token folder', !is_dir("$dir/" . substr(hash('sha256', $tokenA), 0, 32)));
check('IPv4-mapped peer matches its IPv4 network', $ask('GET', "/v1/hello/$room3/client", $tokenA, '::ffff:192.0.2.11') === 200);
check('IPv6 network admits', $ask('GET', "/v1/hello/$room3/client", $tokenA, '2001:db8:0:7::1') === 200);
check('address outside the networks is refused', $ask('GET', "/v1/hello/$room3/client", $tokenA, '198.51.100.2') === 401);
check('unknown peer is refused when rules need it', $ask('GET', "/v1/hello/$room3/client", $tokenA, null) === 401);
check('second address of the day admitted', $ask('GET', "/v1/hello/$room3/client", $tokenB, '2001:db8:1:2::1') === 200);
check('same IPv6 /64 counts once', $ask('GET', "/v1/hello/$room3/client", $tokenB, '2001:db8:1:2::ffff') === 200);
check('third address of the day refused', $ask('GET', "/v1/hello/$room3/client", $tokenB, '203.0.113.5') === 401);
check('known address still admitted', $ask('GET', "/v1/hello/$room3/client", $tokenB, '198.51.100.1') === 200);
$ips = (string) file_get_contents(glob("$dir/*/ips-*.json")[0]);
check('addresses are stored only as hashes', !str_contains($ips, '198.51') && !str_contains($ips, '2001'));
check('expired token refused', $ask('GET', "/v1/hello/$room3/client", $tokenC, '192.0.2.10') === 401);
for ($i = 0; $i < 2; $i++) {
    $ask('GET', "/v1/hello/$room3/client", str_repeat('z', 43), '192.0.2.99');
}
check('failures within the limit stay 401', $ask('GET', "/v1/hello/$room3/client", str_repeat('z', 43), '192.0.2.99') === 401);
check('beyond the limit the address gets 429', $ask('GET', "/v1/hello/$room3/client", str_repeat('z', 43), '192.0.2.99') === 429);
check('other addresses are unaffected', $ask('GET', "/v1/hello/$room3/client", str_repeat('z', 43), '203.0.113.10') === 401);
check('a valid token from a banned address still works', $ask('GET', "/v1/hello/$room3/client", $tokenA, '192.0.2.99') === 200);
$once = static fn (): int => (new KLaniRelay(['dir' => $dir, 'tokens' => [hash('sha256', $tokenA) => $rules(['space' => 'salted', 'max_ips_per_day' => 1])]]))
    ->handle('GET', "/v1/hello/$room3/client", ['wait' => '0'], "Bearer $tokenA", static fn (): string => '', null, null, '203.0.113.77')[0];
check('separate relay instances share the stored salt', $once() === 204 && $once() === 204);
check('the stored salt is private', (fileperms("$dir/.salt") & 0077) === 0);
check('a malformed expiry refuses the token', (new KLaniRelay(['dir' => $dir, 'tokens' => [hash('sha256', $tokenA) => $rules(['expires' => '31.12.2099'])]]))
    ->handle('GET', "/v1/hello/$room3/client", ['wait' => '0'], "Bearer $tokenA", static fn (): string => '')[0] === 401);

// Pairing by code: slots with a token side and a tokenless side.
$pairDir = "$dir/pair";
mkdir($pairDir, 0700);
$hostToken = str_repeat('h', 43);
$otherToken = str_repeat('o', 43);
$pairRelay = static fn (array $global = [], array $own = []): KLaniRelay => new KLaniRelay($global + ['dir' => $pairDir, 'max_wait' => 0,
    'failed_auth_per_minute' => 3, 'tokens' => [hash('sha256', $hostToken) => KLaniRelay::limits() + $own,
        hash('sha256', $otherToken) => KLaniRelay::limits()]]);
$pairCall = static function (KLaniRelay $relay, string $method, string $path, ?string $token, string $body = '', string $peer = '192.0.2.50'): array {
    return $relay->handle($method, $path, ['wait' => '0'], $token === null ? null : "Bearer $token",
        static fn (int $limit): string => substr($body, 0, $limit + 1), null, null, $peer);
};
$p = $pairRelay();
check('GET /v1/pair says pairing exists without a token', $pairCall($p, 'GET', '/v1/pair', null)[0] === 405);
check('opening a slot needs a token', $pairCall($p, 'POST', '/v1/pair', null, 'open')[0] === 401);
[$status, $headers, $number] = $pairCall($p, 'POST', '/v1/pair', $hostToken, 'opening');
check('a host opens a slot with a three-digit number', $status === 201 && preg_match('/^[1-9][0-9]{2}$/', $number) === 1
    && $headers['Content-Type'] === 'text/plain');
check('the client reads the opening without a token', $pairCall($p, 'GET', "/v1/pair/$number/a", null) === [200, KLaniRelay::reply(200)[1], 'opening']);
check('the client cannot read part c before it exists', $pairCall($p, 'GET', "/v1/pair/$number/c", null)[0] === 204);
check('part c cannot be written before part b', $pairCall($p, 'POST', "/v1/pair/$number/c", $hostToken, 'bundle')[0] === 404);
check('the client writes its answer', $pairCall($p, 'POST', "/v1/pair/$number/b", null, 'answer')[0] === 204);
check('a second answer is refused', $pairCall($p, 'POST', "/v1/pair/$number/b", null, 'other', '192.0.2.51')[0] === 404);
check('the opening is gone once answered', $pairCall($p, 'GET', "/v1/pair/$number/a", null, '', '192.0.2.52')[0] === 404);
check('another space cannot read the answer', $pairCall($p, 'GET', "/v1/pair/$number/b", $otherToken)[0] === 404);
check('the host reads the answer', $pairCall($p, 'GET', "/v1/pair/$number/b", $hostToken)[2] === 'answer');
check('the host writes the bundle', $pairCall($p, 'PUT', "/v1/pair/$number/c", $hostToken, 'bundle')[0] === 204);
check('the client reads the bundle', $pairCall($p, 'GET', "/v1/pair/$number/c", null)[2] === 'bundle');
check('the client acknowledges', $pairCall($p, 'POST', "/v1/pair/$number/d", null, 'ack')[0] === 204);
check('the host reads the acknowledgement', $pairCall($p, 'GET', "/v1/pair/$number/d", $hostToken)[2] === 'ack');
check('another space cannot close the slot', $pairCall($p, 'DELETE', "/v1/pair/$number", $otherToken)[0] === 404);
check('the host closes the slot', $pairCall($p, 'DELETE', "/v1/pair/$number", $hostToken)[0] === 204);
check('a closed slot is gone', $pairCall($p, 'GET', "/v1/pair/$number/c", null, '', '192.0.2.53')[0] === 404);
check('an oversized opening is refused', $pairCall($p, 'POST', '/v1/pair', $hostToken, str_repeat('x', 65))[0] === 400);
check('the host cannot write the client parts', $pairCall($p, 'POST', '/v1/pair/123/a', $hostToken, 'x')[0] === 404);
for ($i = 0; $i < 3; $i++) {
    $pairCall($p, 'GET', '/v1/pair/' . (100 + $i) . '/a', null, '', '198.51.100.200');
}
check('guessing slot numbers runs into the brake', $pairCall($p, 'GET', '/v1/pair/104/a', null, '', '198.51.100.200')[0] === 429);
check('a banned address is refused at the door', $pairCall($p, 'POST', '/v1/pair/104/b', null, 'x', '198.51.100.200')[0] === 429);
$opened = [];
for ($i = 0; $i < 4; $i++) {
    $opened[] = $pairCall($p, 'POST', '/v1/pair', $hostToken, "o$i")[0];
}
check('four open slots per space by default', $opened === [201, 201, 201, 201]
    && $pairCall($p, 'POST', '/v1/pair', $hostToken, 'o5')[0] === 429);
check('another space still has room', $pairCall($p, 'POST', '/v1/pair', $otherToken, 'x')[0] === 201);
$slots = json_decode((string) file_get_contents("$pairDir/.pairs.json"), true);
foreach ($slots as $key => $slot) {
    $slots[$key]['created'] -= KLaniRelay::PAIR_TTL + 1;
}
file_put_contents("$pairDir/.pairs.json", json_encode($slots));
check('slots expire after ten minutes', $pairCall($p, 'POST', '/v1/pair', $hostToken, 'fresh')[0] === 201
    && count(json_decode((string) file_get_contents("$pairDir/.pairs.json"), true)) === 1);
check('the relay-wide limit applies', $pairCall($pairRelay(['pair_slots' => 1]), 'POST', '/v1/pair', $otherToken, 'x')[0] === 429);
check('pair_slots 0 on the relay turns pairing off', $pairCall($pairRelay(['pair_slots' => 0]), 'POST', '/v1/pair', $hostToken, 'x')[0] === 403);
check('pair_slots 0 on a token turns it off for its space', $pairCall($pairRelay([], ['pair_slots' => 0]), 'POST', '/v1/pair', $hostToken, 'x')[0] === 403);

foreach (new RecursiveIteratorIterator(new RecursiveDirectoryIterator($dir, FilesystemIterator::SKIP_DOTS), RecursiveIteratorIterator::CHILD_FIRST) as $path) {
    $path->isDir() ? rmdir((string) $path) : unlink((string) $path);
}
rmdir($dir);
echo $failures === 0 ? "PASS\n" : "$failures FAILED\n";
exit($failures === 0 ? 0 : 1);
