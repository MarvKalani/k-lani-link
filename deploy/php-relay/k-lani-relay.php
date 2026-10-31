<?php
/**
 * k-lani-link HTTPS mailbox relay for ordinary PHP web hosting (PHP 8.2+).
 *
 * The same acknowledged multi-record mailbox as the Rust relay (`src/mailbox/relay.rs`):
 * it stores only sealed ciphertext, expiring records after 30 seconds and sessions after
 * 120 seconds (deleted at the next request or `sweepAll` run, e.g. from cron), and it
 * cannot read or change content. Every request needs an admission token; only SHA-256
 * hashes of the tokens are configured. Storage uses files with locks, no database.
 *
 * Differences forced by PHP hosting: a write into a full window is answered with 429
 * (the client retries) instead of waiting, and long polls wait at most `max_wait`
 * seconds (a short wait keeps few PHP workers and database connections busy).
 */

declare(strict_types=1);

final class KLaniRelay
{
    public const WINDOW = 32;
    public const RETRY_HASHES = 64;
    public const RECORD_TTL = 30;
    public const SESSION_TTL = 120;
    public const LONG_POLL = 25;
    public const HELLO_BYTES = 101;
    public const REPLY_BYTES = 102;
    public const MAX_PACKET_BYTES = 12 * 1024 + 28;
    /** Pairing by code: a slot lives ten minutes; numbers run from 100 to 999. */
    public const PAIR_TTL = 600;
    public const SLOT_NUMBERS = 900;
    /** Opening message, client answer, sealed bundle, sealed acknowledgement. */
    private const PAIR_PART_BYTES = [64, 128, 8192, 64];
    /** Requests per second for everyone without a token together (the slots' client side). */
    private const DOOR_RATE = 20;
    private const DOOR_BURST = 40;

    /** @var array<string, array<string, int>> token hash => limits */
    private array $tenants;
    private string $dir;
    private int $maxWait;
    private int $maxWaiters;
    private int $maxWrites;
    private int $failuresPerMinute;
    private int $banSeconds;
    private int $pairSlots;
    private ?string $salt;

    /**
     * @param array{dir: string, tokens: array<string, array<string, mixed>>, max_wait?: int} $config
     *   `tokens` maps the lowercase hex SHA-256 of each admission token to its limits
     *   (rooms, sessions_per_room, requests_per_second, request_burst) and optional rules:
     *   `space` (tokens with the same name share rooms: one token per device), `expires`
     *   (last valid day `YYYY-MM-DD`, UTC, or Unix seconds from which it is refused),
     *   `allow_ips` (addresses or networks such as `203.0.113.0/24`) and `max_ips_per_day`
     *   (distinct addresses within 24 hours, IPv6 per /64). Address rules use the peer
     *   address passed to `handle`, which must be the client's, not a proxy's.
     *   `failed_auth_per_minute` (0: off) refused requests per address before it gets 429
     *   for `failed_auth_ban_minutes`; a valid token is never held back by it.
     *   `pair_slots` limits open pairing-by-code slots: at the top level for the whole relay
     *   (default 64), in a token's limits for its space (default 4); 0 turns pairing by code off.
     */
    public function __construct(array $config)
    {
        $this->dir = rtrim($config['dir'], '/');
        $this->tenants = $config['tokens'];
        $this->maxWait = max(0, min(self::LONG_POLL, (int) ($config['max_wait'] ?? 5)));
        // Waiting reads hold a PHP worker each; beyond this many, reads answer at once so
        // workers stay free for writes. About half the host's workers is a good start.
        $this->maxWaiters = max(0, (int) ($config['max_waiters'] ?? 3));
        $this->maxWrites = max(1, min(8, (int) ($config['max_writes'] ?? 2)));
        $this->failuresPerMinute = max(0, (int) ($config['failed_auth_per_minute'] ?? 0));
        $this->banSeconds = 60 * max(1, min(1440, (int) ($config['failed_auth_ban_minutes'] ?? 10)));
        $this->pairSlots = max(0, min(self::SLOT_NUMBERS, (int) ($config['pair_slots'] ?? 64)));
        $this->salt = isset($config['salt']) ? (string) $config['salt'] : null;
    }

    /** Default limits for a token; small, for private use on shared hosting. */
    public static function limits(): array
    {
        return ['rooms' => 16, 'sessions_per_room' => 8, 'requests_per_second' => 50, 'request_burst' => 200];
    }

    /**
     * Handles one request below the relay's mount point, e.g. `/v1/box/...`.
     *
     * @param callable(int): ?string $readBody reads at most n+1 bytes of the body
     * @param ?callable(): void $beforeWait releases resources (e.g. a database connection) before a long poll
     * @param ?string $contentType `application/json` marks a record wrapped as {"data":"<base64url>"}
     * @param ?string $peer the client's address (`REMOTE_ADDR`), for address rules and the brake
     * @return array{0: int, 1: array<string, string>, 2: string}
     */
    public function handle(string $method, string $path, array $query, ?string $authorization, callable $readBody, ?callable $beforeWait = null, ?string $contentType = null, ?string $peer = null): array
    {
        if ($path === '/v1/pair' || str_starts_with($path, '/v1/pair/')) {
            // The slots' client side has no token, so these routes authorize per part.
            return $this->pair($method, $path, $query, $authorization, $readBody, $beforeWait, self::address($peer));
        }
        $json = strtolower(trim(explode(';', (string) $contentType)[0])) === 'application/json';
        $readBody = static function (int $limit) use ($readBody, $json): ?string {
            if (!$json) {
                return $readBody($limit);
            }
            // Web firewalls often admit only JSON; the record inside is the same sealed bytes.
            $wrapped = json_decode((string) $readBody(intdiv($limit + 2, 3) * 4 + 16), true);
            $data = is_array($wrapped) && array_keys($wrapped) === ['data'] && is_string($wrapped['data'])
                ? self::decode($wrapped['data']) : null;
            return $data !== null && self::encode($data) === $wrapped['data'] ? $data : null;
        };
        $peer = self::address($peer);
        $tenant = $this->authorize($authorization);
        if ($tenant === null || !$this->admits($tenant, $peer)) {
            return self::reply($this->strike($peer) ? 429 : 401);
        }
        if (!$this->rate($tenant)) {
            return self::reply(429);
        }
        $this->sweepSometimes();
        $result = $this->route($tenant, $method, $path, $query, $readBody, $beforeWait);
        // Tells the endpoints the host's limits; they never exceed them (self-tuning).
        $result[1]['X-K-Lani-Relay'] = "wait={$this->maxWait}; writes={$this->maxWrites}";
        return $result;
    }

    /** @return array{0: int, 1: array<string, string>, 2: string} */
    private function route(string $tenant, string $method, string $path, array $query, callable $readBody, ?callable $beforeWait): array
    {
        $write = $method === 'PUT' || $method === 'POST';
        $id = '[A-Za-z0-9_-]+';
        if (preg_match("#^/v1/hello/($id)/client$#", $path, $m)) {
            return $write ? $this->writeHello($tenant, $m[1], null, $readBody)
                : ($method === 'GET' ? $this->readHello($tenant, $m[1], null, $query, $beforeWait) : self::reply(405));
        }
        if (preg_match("#^/v1/hello/($id)/host/($id)$#", $path, $m)) {
            return $write ? $this->writeHello($tenant, $m[1], $m[2], $readBody)
                : ($method === 'GET' ? $this->readHello($tenant, $m[1], $m[2], $query, $beforeWait) : self::reply(405));
        }
        if (preg_match("#^/v1/box/($id)/($id)/(c2h|h2c)/([0-9]{1,19})$#", $path, $m)) {
            return $write ? $this->putRecord($tenant, $m[1], $m[2], $m[3] === 'c2h' ? 0 : 1, $m[4], $readBody) : self::reply(405);
        }
        if (preg_match("#^/v1/box/($id)/($id)/(c2h|h2c)$#", $path, $m)) {
            return $method === 'GET' ? $this->getRecords($tenant, $m[1], $m[2], $m[3] === 'c2h' ? 0 : 1, $query, $beforeWait) : self::reply(405);
        }
        return self::reply(404);
    }

    /** @return array{0: int, 1: array<string, string>, 2: string} */
    public static function reply(int $status, string $body = ''): array
    {
        $headers = [
            'Cache-Control' => 'no-store, no-transform',
            'Content-Type' => 'application/octet-stream',
            'X-Content-Type-Options' => 'nosniff',
        ];
        if ($status === 429) {
            $headers['Retry-After'] = '1';
        }
        return [$status, $headers, $body];
    }

    /** A full record window: the client retries promptly, without a Retry-After hint. */
    private static function busy(): array
    {
        [$status, $headers, $body] = self::reply(429);
        unset($headers['Retry-After']);
        return [$status, $headers, $body];
    }

    private function authorize(?string $header): ?string
    {
        if ($header === null || !preg_match('/^Bearer ([A-Za-z0-9_-]{32,256})$/', trim($header), $m)) {
            return null;
        }
        $presented = hash('sha256', $m[1]);
        $found = null;
        foreach ($this->tenants as $hash => $_) {
            if (hash_equals(strtolower((string) $hash), $presented)) {
                $found = strtolower((string) $hash);
            }
        }
        return $found;
    }

    /** Expiry, address networks and addresses per day; address rules need a known peer. */
    private function admits(string $tenant, ?string $peer): bool
    {
        $rules = $this->tenants[$tenant];
        $expires = self::expiry($rules['expires'] ?? null);
        if ($expires === false || ($expires !== null && time() >= $expires)) {
            return false;
        }
        $networks = array_map('strval', (array) ($rules['allow_ips'] ?? []));
        $perDay = (int) ($rules['max_ips_per_day'] ?? 0);
        if ($networks === [] && $perDay <= 0) {
            return true;
        }
        if ($peer === null) {
            return false;
        }
        if ($networks !== [] && array_filter($networks, static fn (string $network): bool => self::inNetwork($peer, $network)) === []) {
            return false;
        }
        if ($perDay <= 0) {
            return true;
        }
        $dir = $this->tenantDir($tenant);
        if (!is_dir($dir) && !@mkdir($dir, 0700, true) && !is_dir($dir)) {
            return false;
        }
        // Only salted hashes of addresses are kept, and only for a day.
        $key = 'h' . substr(hash_hmac('sha256', self::subscriber($peer), $this->salt()), 0, 32);
        return (bool) self::locked("$dir/ips-" . substr($tenant, 0, 16) . '.json', static function (array $seen) use ($key, $perDay): array {
            $now = time();
            $seen = array_filter($seen, static fn ($first): bool => (int) $first > $now - 86400);
            if (!isset($seen[$key])) {
                if (count($seen) >= $perDay) {
                    return [$seen, false];
                }
                $seen[$key] = $now;
            }
            return [$seen, true];
        });
    }

    /** True for an address or network such as `203.0.113.0/24` or `2001:db8::/48`. */
    public static function validNetwork(string $network): bool
    {
        $base = self::address(explode('/', trim($network), 2)[0]);
        return $base !== null && self::inNetwork($base, $network);
    }

    /** True for a day `YYYY-MM-DD`. */
    public static function validDay(string $day): bool
    {
        return is_int(self::expiry($day));
    }

    /** Unix seconds from which a token is refused; null: no end; false: unreadable (refuse). */
    private static function expiry(mixed $value): int|false|null
    {
        if ($value === null || $value === '' || $value === 0) {
            return null;
        }
        if (is_int($value)) {
            return $value;
        }
        $day = DateTimeImmutable::createFromFormat('!Y-m-d', (string) $value, new DateTimeZone('UTC'));
        return $day !== false && $day->format('Y-m-d') === $value ? $day->getTimestamp() + 86400 : false;
    }

    /** Counts a refused request against its address; true while that address is banned. */
    private function strike(?string $peer): bool
    {
        if ($peer === null || $this->failuresPerMinute <= 0) {
            return false;
        }
        $key = 'h' . substr(hash_hmac('sha256', self::subscriber($peer), $this->salt()), 0, 32);
        return (bool) self::locked($this->dir . '/.strikes.json', function (array $strikes) use ($key): array {
            $now = time();
            $strikes = array_filter($strikes, static fn ($s): bool => is_array($s) && ($s[0] > $now - 60 || $s[2] > $now));
            // Bounded: beyond this many addresses, failures are no longer counted.
            if (!isset($strikes[$key]) && count($strikes) >= 4096) {
                return [$strikes, false];
            }
            $strike = $strikes[$key] ?? [$now, 0, 0];
            if ($strike[2] > $now) {
                return [$strikes, true];
            }
            if ($strike[0] <= $now - 60) {
                $strike = [$now, 0, 0];
            }
            $strike[1]++;
            if ($strike[1] > $this->failuresPerMinute) {
                $strike[2] = $now + $this->banSeconds;
            }
            $strikes[$key] = $strike;
            return [$strikes, $strike[2] > $now];
        });
    }

    /** Binary peer address; IPv4-mapped IPv6 becomes IPv4. */
    private static function address(?string $text): ?string
    {
        $binary = $text === null ? false : @inet_pton(trim($text));
        if ($binary === false) {
            return null;
        }
        return strlen($binary) === 16 && str_starts_with($binary, str_repeat("\0", 10) . "\xff\xff") ? substr($binary, 12) : $binary;
    }

    private static function inNetwork(string $address, string $network): bool
    {
        [$base, $bits] = array_pad(explode('/', trim($network), 2), 2, null);
        $base = self::address($base);
        if ($base === null || strlen($base) !== strlen($address)) {
            return false;
        }
        $bits = $bits === null ? strlen($base) * 8 : (ctype_digit($bits) ? (int) $bits : -1);
        if ($bits < 0 || $bits > strlen($base) * 8) {
            return false;
        }
        $whole = intdiv($bits, 8);
        if (substr($address, 0, $whole) !== substr($base, 0, $whole)) {
            return false;
        }
        $mask = (0xff << (8 - $bits % 8)) & 0xff;
        return $bits % 8 === 0 || (ord($address[$whole]) & $mask) === (ord($base[$whole]) & $mask);
    }

    /** IPv4 per address, IPv6 per /64: one subscriber usually holds a whole /64. */
    private static function subscriber(string $address): string
    {
        return strlen($address) === 16 ? substr($address, 0, 8) . str_repeat("\0", 8) : $address;
    }

    /** Secret for address hashes: from the configuration or a private file in the storage. */
    private function salt(): string
    {
        if ($this->salt === null) {
            $file = $this->dir . '/.salt';
            // Created exclusively: when several requests race, all use the first one's salt.
            $handle = @fopen($file, 'x');
            if ($handle !== false) {
                fwrite($handle, bin2hex(random_bytes(32)));
                fclose($handle);
                @chmod($file, 0600);
            }
            for ($try = 0; $try < 20; $try++) {
                $salt = @file_get_contents($file);
                if (is_string($salt) && strlen($salt) === 64) {
                    break;
                }
                usleep(5000);
            }
            // Without a readable salt, address rules would never recognize an address again.
            if (!is_string($salt) || strlen($salt) !== 64) {
                throw new RuntimeException('relay storage cannot keep its salt');
            }
            $this->salt = $salt;
        }
        return $this->salt;
    }

    private function limit(string $tenant, string $name): int
    {
        return (int) ($this->tenants[$tenant][$name] ?? self::limits()[$name]);
    }

    /**
     * Separate storage per token, or per `space` shared by its tokens: a token can never
     * address rooms outside its own space.
     */
    private function tenantDir(string $tenant): string
    {
        $space = $this->tenants[$tenant]['space'] ?? '';
        return $this->dir . '/' . (is_string($space) && $space !== ''
            ? substr(hash('sha256', "k-lani-link-space:$space"), 0, 32) : substr($tenant, 0, 32));
    }

    private function rate(string $tenant): bool
    {
        $dir = $this->tenantDir($tenant);
        if (!is_dir($dir) && !@mkdir($dir, 0700, true) && !is_dir($dir)) {
            return false;
        }
        return (bool) self::locked($dir . '/rate.json', function (array $state) use ($tenant): array {
            $now = microtime(true);
            $burst = (float) $this->limit($tenant, 'request_burst');
            $tokens = min($burst, (float) ($state['tokens'] ?? $burst) + ($now - (float) ($state['at'] ?? $now)) * $this->limit($tenant, 'requests_per_second'));
            if ($tokens < 1.0) {
                return [['tokens' => $tokens, 'at' => $now], false];
            }
            return [['tokens' => $tokens - 1.0, 'at' => $now], true];
        });
    }

    private static function id(string $value, int $bytes): bool
    {
        $decoded = self::decode($value);
        return $decoded !== null && strlen($decoded) === $bytes && self::encode($decoded) === $value;
    }

    private static function decode(string $value): ?string
    {
        $decoded = base64_decode(strtr($value, '-_', '+/') . str_repeat('=', (4 - strlen($value) % 4) % 4), true);
        return $decoded === false ? null : $decoded;
    }

    private static function encode(string $bytes): string
    {
        return rtrim(strtr(base64_encode($bytes), '+/', '-_'), '=');
    }

    public static function helloHash(string $hello): string
    {
        return self::encode(hash('sha256', $hello, true));
    }

    public static function sessionId(string $hello, string $reply): string
    {
        return self::encode(hash('sha256', 'k-lani-link/mailbox/transcript/v1'
            . pack('N', strlen($hello)) . $hello . pack('N', strlen($reply)) . $reply, true));
    }

    /**
     * Runs $work on a JSON state file under an exclusive lock and saves what it returns.
     *
     * @param callable(array): array{0: ?array, 1: mixed} $work new state (null: delete) and result
     */
    private static function locked(string $file, callable $work): mixed
    {
        $handle = @fopen($file, 'c+');
        if ($handle === false) {
            return null;
        }
        try {
            if (!flock($handle, LOCK_EX)) {
                return null;
            }
            $text = stream_get_contents($handle);
            $state = $text === '' || $text === false ? [] : (json_decode($text, true) ?: []);
            [$next, $result] = $work($state);
            if ($next === null) {
                ftruncate($handle, 0);
            } elseif ($next !== $state) {
                ftruncate($handle, 0);
                rewind($handle);
                fwrite($handle, json_encode($next, JSON_UNESCAPED_SLASHES));
                fflush($handle);
            }
            return $result;
        } finally {
            flock($handle, LOCK_UN);
            fclose($handle);
        }
    }

    private function roomDir(string $tenant, string $room): string
    {
        return $this->tenantDir($tenant) . '/' . $room;
    }

    /** Expires one room in place; false when the whole room is gone. */
    private function sweep(string $dir, array &$room, float $now): bool
    {
        if (($room['updated'] ?? 0) <= $now - self::SESSION_TTL) {
            return false;
        }
        if (isset($room['client']) && $room['client']['created'] <= $now - self::RECORD_TTL) {
            unset($room['client']);
            @unlink("$dir/client.hello");
        }
        // Work on the room itself: `$room['sessions'] ?? []` would iterate a copy, delete the
        // files and keep their metadata, so expired records would still be acknowledged.
        $room += ['replies' => [], 'sessions' => []];
        foreach ($room['replies'] as $hash => $entry) {
            if ($entry['created'] <= $now - self::RECORD_TTL) {
                unset($room['replies'][$hash]);
                @unlink("$dir/reply.$hash");
            }
        }
        $ended = [];
        foreach (array_keys($room['sessions']) as $session) {
            $state = &$room['sessions'][$session];
            $removed = $state['updated'] <= $now - self::SESSION_TTL;
            foreach (array_keys($state['channels']) as $dirIndex) {
                $channel = &$state['channels'][$dirIndex];
                foreach ($channel['records'] as $index => $entry) {
                    if ($removed || $entry['created'] <= $now - self::RECORD_TTL) {
                        unset($channel['records'][$index]);
                        $channel['floor'] = max($channel['floor'], (int) $index + 1);
                        @unlink("$dir/$session.$dirIndex.$index");
                    }
                }
                unset($channel);
            }
            unset($state);
            if ($removed) {
                $ended[] = $session;
            }
        }
        foreach ($ended as $session) {
            unset($room['sessions'][$session]);
        }
        return true;
    }

    /**
     * Removes expired records and rooms of every token. Requests trigger it at most every
     * few seconds; cron should call it too, because without any request nothing runs.
     */
    public function sweepAll(): void
    {
        if (is_file($this->pairFile())) {
            self::locked($this->pairFile(), static function (array $slots): array {
                $live = self::livePairs($slots);
                return [$live === [] ? null : $live, null];
            });
        }
        $now = microtime(true);
        foreach (glob($this->dir . '/*', GLOB_ONLYDIR) ?: [] as $tenantDir) {
            foreach (glob("$tenantDir/*", GLOB_ONLYDIR) ?: [] as $dir) {
                $gone = false;
                self::locked("$dir/room.json", function (array $state) use ($dir, $now, &$gone): array {
                    // A room another request is just creating has no state yet; keep it.
                    if ($state === []) {
                        $gone = @filemtime($dir) < $now - self::SESSION_TTL;
                        return [$state, null];
                    }
                    if (!$this->sweep($dir, $state, $now)) {
                        $gone = true;
                        return [null, null];
                    }
                    return [$state, null];
                });
                if ($gone) {
                    self::removeDir($dir);
                }
            }
        }
    }

    /** Runs `sweepAll` when the last run is older than a few seconds. */
    private function sweepSometimes(): void
    {
        $marker = $this->dir . '/.swept';
        $last = @filemtime($marker);
        if ($last !== false && $last > time() - 5) {
            return;
        }
        @touch($marker);
        $this->sweepAll();
    }

    private static function removeDir(string $dir): void
    {
        foreach (glob("$dir/*") ?: [] as $file) {
            @unlink($file);
        }
        @unlink("$dir/room.json");
        @rmdir($dir);
    }

    /**
     * Runs $work on the addressed room after expiring it, under the room lock.
     *
     * @param callable(array, string): array{0: ?array, 1: mixed} $work
     */
    private function withRoom(string $tenant, string $room, bool $create, callable $work): mixed
    {
        $dir = $this->roomDir($tenant, $room);
        if (!is_dir($dir)) {
            if (!$create) {
                return $work(null, $dir)[1];
            }
            if ($this->roomCount($tenant) >= $this->limit($tenant, 'rooms')) {
                return 'full';
            }
            if (!@mkdir($dir, 0700, true) && !is_dir($dir)) {
                return null;
            }
        }
        $now = microtime(true);
        $gone = false;
        $result = self::locked("$dir/room.json", function (array $state) use ($dir, $now, $work, $create, &$gone): array {
            if ($state === [] && !$create) {
                return [$state, $work(null, $dir)[1]];
            }
            $state += ['updated' => $now, 'replies' => [], 'sessions' => []];
            if (!$this->sweep($dir, $state, $now)) {
                $gone = true;
                if (!$create) {
                    return [null, $work(null, $dir)[1]];
                }
                // A new handshake in an expired room starts empty; drop the old records.
                foreach (glob("$dir/*") ?: [] as $file) {
                    if (basename($file) !== 'room.json') {
                        @unlink($file);
                    }
                }
                $state = ['updated' => $now, 'replies' => [], 'sessions' => []];
            }
            return $work($state, $dir);
        });
        if ($gone && !$create) {
            self::removeDir($dir);
        }
        return $result;
    }

    /** Counts live rooms of a token and removes expired ones (a rare path: handshakes only). */
    private function roomCount(string $tenant): int
    {
        $count = 0;
        foreach (glob($this->tenantDir($tenant) . '/*', GLOB_ONLYDIR) ?: [] as $dir) {
            if (@filemtime("$dir/room.json") < time() - self::SESSION_TTL) {
                self::removeDir($dir);
            } else {
                $count++;
            }
        }
        return $count;
    }

    private static function body(callable $readBody, int $limit): ?string
    {
        $body = $readBody($limit);
        return $body === null || strlen($body) > $limit ? null : $body;
    }

    private function writeHello(string $tenant, string $room, ?string $hash, callable $readBody): array
    {
        $expected = $hash === null ? self::HELLO_BYTES : self::REPLY_BYTES;
        if (!self::id($room, 16) || ($hash !== null && !self::id($hash, 32))) {
            return self::reply(400);
        }
        $body = self::body($readBody, $expected);
        if ($body === null || strlen($body) !== $expected || substr($body, 0, 4) !== 'KLH1'
            || ord($body[4]) !== ($hash === null ? 1 : 2)) {
            return self::reply(400);
        }
        $limit = $this->limit($tenant, 'sessions_per_room');
        $status = $this->withRoom($tenant, $room, true, function (array $state, string $dir) use ($hash, $body, $limit): array {
            $now = microtime(true);
            $file = $hash === null ? "$dir/client.hello" : "$dir/reply.$hash";
            $old = ($hash === null ? isset($state['client']) : isset($state['replies'][$hash])) ? @file_get_contents($file) : false;
            if ($old !== false) {
                if (hash_equals($old, $body)) {
                    return [$state, 204];
                }
                if ($hash !== null) {
                    return [$state, 409];
                }
            }
            if ($hash !== null) {
                $hello = isset($state['client']) ? @file_get_contents("$dir/client.hello") : false;
                if ($hello === false || !hash_equals(self::helloHash($hello), $hash)) {
                    return [$state, 410];
                }
                $session = self::sessionId($hello, $body);
                if (!isset($state['sessions'][$session])) {
                    if (count($state['sessions']) >= $limit) {
                        return [$state, 429];
                    }
                    $channel = ['floor' => 1, 'offered' => 0, 'records' => [], 'acknowledged' => []];
                    $state['sessions'][$session] = ['updated' => $now, 'channels' => [$channel, $channel]];
                }
                $state['replies'][$hash] = ['created' => $now];
            } else {
                $state['client'] = ['created' => $now];
            }
            if (file_put_contents($file, $body, LOCK_EX) !== strlen($body)) {
                return [$state, 503];
            }
            $state['updated'] = $now;
            return [$state, 204];
        });
        return self::reply($status === 'full' ? 429 : (int) ($status ?? 503));
    }

    private static function wait(array $query): ?int
    {
        $wait = $query['wait'] ?? (string) self::LONG_POLL;
        return is_string($wait) && preg_match('/^[0-9]{1,3}$/', $wait) && (int) $wait <= self::LONG_POLL ? (int) $wait : null;
    }

    /**
     * Polls $probe until it yields a value or the wait ends. The lock is held only per probe.
     *
     * @param callable(): mixed $probe null: nothing yet
     */
    private function poll(int $wait, ?callable $beforeWait, callable $probe): mixed
    {
        // Only `max_waiters` reads may hold a worker at once; the others answer right away.
        $slot = $wait > 0 ? $this->waitSlot() : null;
        $deadline = microtime(true) + ($slot === null ? 0 : min($wait, $this->maxWait));
        $released = false;
        try {
            while (true) {
                $result = $probe();
                if ($result !== null || microtime(true) >= $deadline) {
                    return $result;
                }
                if (!$released && $beforeWait !== null) {
                    $beforeWait();
                    $released = true;
                }
                usleep(100_000);
            }
        } finally {
            if ($slot !== null) {
                flock($slot, LOCK_UN);
                fclose($slot);
            }
        }
    }

    /** One of `max_waiters` lock files, held while a read waits; null when all are taken. */
    private function waitSlot(): mixed
    {
        for ($index = 0; $index < $this->maxWaiters; $index++) {
            $handle = @fopen("{$this->dir}/.wait-$index", 'c');
            if ($handle !== false && flock($handle, LOCK_EX | LOCK_NB)) {
                return $handle;
            }
            if ($handle !== false) {
                fclose($handle);
            }
        }
        return null;
    }

    private function readHello(string $tenant, string $room, ?string $hash, array $query, ?callable $beforeWait): array
    {
        $wait = self::wait($query);
        if (!self::id($room, 16) || ($hash !== null && !self::id($hash, 32)) || $wait === null
            || array_diff(array_keys($query), ['wait']) !== []) {
            return self::reply(400);
        }
        $found = $this->poll($wait, $beforeWait, fn () => $this->withRoom($tenant, $room, false, function (?array $state, string $dir) use ($hash): array {
            if ($state === null) {
                return [$state, null];
            }
            $present = $hash === null ? isset($state['client']) : isset($state['replies'][$hash]);
            $bytes = $present ? @file_get_contents($hash === null ? "$dir/client.hello" : "$dir/reply.$hash") : false;
            $state['updated'] = microtime(true);
            return [$state, $bytes === false ? null : $bytes];
        }));
        return $found === null ? self::reply(204) : self::reply(200, $found);
    }

    private function putRecord(string $tenant, string $room, string $session, int $direction, string $sequence, callable $readBody): array
    {
        if (!self::id($room, 16) || !self::id($session, 32) || $sequence === '0' || $sequence[0] === '0'
            || strlen($sequence) > 18) {
            return self::reply(400);
        }
        $index = (int) $sequence;
        $body = self::body($readBody, self::MAX_PACKET_BYTES);
        if ($body === null || strlen($body) < 28 || substr($body, 0, 4) !== 'KLI2'
            || unpack('J', substr($body, 4, 8))[1] !== $index - 1) {
            return self::reply(400);
        }
        $status = $this->withRoom($tenant, $room, false, function (?array $state, string $dir) use ($session, $direction, $index, $body): array {
            if ($state === null || !isset($state['sessions'][$session])) {
                return [$state, 410];
            }
            $now = microtime(true);
            $channel = &$state['sessions'][$session]['channels'][$direction];
            $digest = hash('sha256', $body);
            if ($index < $channel['floor']) {
                foreach ($channel['acknowledged'] as [$acked, $hash]) {
                    if ($acked === $index) {
                        return [$state, hash_equals($hash, $digest) ? 204 : 409];
                    }
                }
                return [$state, 410];
            }
            if (isset($channel['records'][$index])) {
                return [$state, hash_equals($channel['records'][$index]['sha'], $digest) ? 204 : 409];
            }
            // Keep room for a missing earlier record; the client retries a later one.
            if ($index - $channel['floor'] >= self::WINDOW || count($channel['records']) >= self::WINDOW) {
                return [$state, 'busy'];
            }
            if (file_put_contents("$dir/$session.$direction.$index", $body, LOCK_EX) !== strlen($body)) {
                return [$state, 503];
            }
            $channel['records'][$index] = ['created' => $now, 'sha' => $digest];
            $state['sessions'][$session]['updated'] = $now;
            $state['updated'] = $now;
            return [$state, 204];
        });
        return $status === 'busy' ? self::busy() : self::reply((int) ($status ?? 503));
    }

    private function getRecords(string $tenant, string $room, string $session, int $direction, array $query, ?callable $beforeWait): array
    {
        $wait = self::wait($query);
        $from = $query['from'] ?? null;
        if (!self::id($room, 16) || !self::id($session, 32) || $wait === null || !is_string($from)
            || !preg_match('/^[1-9][0-9]{0,17}$/', $from) || array_diff(array_keys($query), ['from', 'wait']) !== []) {
            return self::reply(400);
        }
        $from = (int) $from;
        $result = $this->poll($wait, $beforeWait, fn () => $this->withRoom($tenant, $room, false, function (?array $state, string $dir) use ($session, $direction, $from): array {
            if ($state === null || !isset($state['sessions'][$session])) {
                return [$state, 410];
            }
            $channel = &$state['sessions'][$session]['channels'][$direction];
            if ($from < $channel['floor']) {
                return [$state, 410];
            }
            if ($from > $channel['offered'] + 1) {
                return [$state, 400];
            }
            // Everything below `from` reached the reader: drop it and remember its hash.
            foreach ($channel['records'] as $index => $entry) {
                if ((int) $index < $from) {
                    unset($channel['records'][$index]);
                    @unlink("$dir/$session.$direction.$index");
                    $channel['acknowledged'][] = [(int) $index, $entry['sha']];
                }
            }
            $channel['acknowledged'] = array_slice($channel['acknowledged'], -self::RETRY_HASHES);
            $channel['floor'] = $from;
            $body = '';
            for ($index = $from; $index < $from + self::WINDOW && isset($channel['records'][$index]); $index++) {
                $packet = @file_get_contents("$dir/$session.$direction.$index");
                if ($packet === false) {
                    break;
                }
                $body .= pack('N', strlen($packet)) . $packet;
                $channel['offered'] = max($channel['offered'], $index);
            }
            $state['sessions'][$session]['updated'] = microtime(true);
            $state['updated'] = microtime(true);
            return [$state, $body === '' ? null : $body];
        }));
        if (is_int($result)) {
            return self::reply($result);
        }
        return $result === null ? self::reply(204) : self::reply(200, $result);
    }

    /*
     * Pairing by code, as in the Rust relay (`src/mailbox/relay/slots.rs`). A host with a token
     * opens a slot and gets a three-digit number. The client knows only the code, so its side
     * of a slot is the one route without a token: it reads the opening (part a), writes ONE
     * answer (b), reads the sealed bundle (c) and writes the sealed acknowledgement (d).
     * Unknown, expired and used slots all answer 404 and count against the failure brake.
     * Everything stored is ciphertext or a public key-exchange message.
     */

    /** @return array{0: int, 1: array<string, string>, 2: string} */
    private function pair(string $method, string $path, array $query, ?string $authorization, callable $readBody, ?callable $beforeWait, ?string $peer): array
    {
        if (!preg_match('#^/v1/pair(?:/([^/]+)(?:/([^/]+))?)?$#', $path, $m)) {
            return self::reply(404);
        }
        $number = $m[1] ?? null;
        $name = $m[2] ?? null;
        if ($number === null) {
            // GET answers 405 without a token: clients use it to see that pairing exists.
            if ($method !== 'POST') {
                return self::reply(405);
            }
            $tenant = $this->host($authorization, $peer);
            if (is_array($tenant)) {
                return $tenant;
            }
            $opening = self::body($readBody, self::PAIR_PART_BYTES[0]);
            if ($opening === null || $opening === '') {
                return self::reply(400);
            }
            [$status, $slot] = $this->openSlot($tenant, $opening);
            if ($status !== 201) {
                return self::reply($status);
            }
            [, $headers] = self::reply(201);
            $headers['Content-Type'] = 'text/plain';
            return [201, $headers, (string) $slot];
        }
        if ($name === null) {
            if ($method !== 'DELETE') {
                return self::reply(405);
            }
            $tenant = $this->host($authorization, $peer);
            if (is_array($tenant)) {
                return $tenant;
            }
            $slot = self::slotNumber($number);
            return self::reply($slot !== null && $this->closeSlot($this->spaceKey($tenant), $slot) ? 204 : 404);
        }
        $index = array_search($name, ['a', 'b', 'c', 'd'], true);
        $write = $method === 'PUT' || $method === 'POST';
        if ($index === false || ($write && $index === 0)) {
            return self::reply(404);
        }
        if (!$write && $method !== 'GET') {
            return self::reply(405);
        }
        // Parts the client writes or reads without a token: a (read), b (write), c (read), d (write).
        if (in_array([$index, $write], [[0, false], [1, true], [2, false], [3, true]], true)) {
            if (!$this->door($peer)) {
                return self::reply(429);
            }
            $space = null;
        } else {
            $tenant = $this->host($authorization, $peer);
            if (is_array($tenant)) {
                return $tenant;
            }
            $space = $this->spaceKey($tenant);
        }
        if ($write) {
            $slot = self::slotNumber($number);
            if ($slot === null) {
                return $this->noSlot($space, $peer);
            }
            $bytes = self::body($readBody, self::PAIR_PART_BYTES[$index]);
            if ($bytes === null || $bytes === '') {
                return self::reply(400);
            }
            $written = self::locked($this->pairFile(), function (array $slots) use ($slot, $index, $space, $bytes): array {
                $slots = self::livePairs($slots);
                $entry = $slots[(string) $slot] ?? null;
                // Each part is written once, and only after the part before it.
                if ($entry === null || ($space !== null && $entry['space'] !== $space)
                    || $entry['parts'][$index] !== null || $entry['parts'][$index - 1] === null) {
                    return [$slots, false];
                }
                $slots[(string) $slot]['parts'][$index] = self::encode($bytes);
                return [$slots, true];
            });
            if ($written === null) {
                return self::reply(503);
            }
            return $written ? self::reply(204) : $this->noSlot($space, $peer);
        }
        $wait = self::wait($query);
        if ($wait === null) {
            return self::reply(400);
        }
        $slot = self::slotNumber($number);
        if ($slot === null) {
            return $this->noSlot($space, $peer);
        }
        // Tokenless reads never wait, so a stranger who guessed the number cannot hold the
        // client's place; the client polls. The host side may wait.
        $found = $this->poll($space === null ? 0 : $wait, $beforeWait, fn () => self::locked($this->pairFile(), static function (array $slots) use ($slot, $index, $space): array {
            $slots = self::livePairs($slots);
            $entry = $slots[(string) $slot] ?? null;
            if ($entry === null || ($space !== null && $entry['space'] !== $space)
                // The opening is offered only until someone answered it: one attempt per slot.
                || ($index === 0 && $entry['parts'][1] !== null)) {
                return [$slots, false];
            }
            // Reading never removes anything: a lost answer can be read again. The host closes
            // the slot once it has checked the acknowledgement, or the slot expires.
            $part = $entry['parts'][$index];
            return [$slots, $part === null ? null : self::decode($part)];
        }));
        if ($found === false) {
            return $this->noSlot($space, $peer);
        }
        return $found === null ? self::reply(204) : self::reply(200, $found);
    }

    /** The token holder's side: its tenant, or the refusal to send. */
    private function host(?string $authorization, ?string $peer): string|array
    {
        $tenant = $this->authorize($authorization);
        if ($tenant === null || !$this->admits($tenant, $peer)) {
            return self::reply($this->strike($peer) ? 429 : 401);
        }
        return $this->rate($tenant) ? $tenant : self::reply(429);
    }

    /** One answer for an unknown, expired or used slot: 404, counted against the brake without a token. */
    private function noSlot(?string $space, ?string $peer): array
    {
        return self::reply($space === null && $this->strike($peer) ? 429 : 404);
    }

    /** A banned address gets 429 before anything else; others pay one token of the shared tokenless budget. */
    private function door(?string $peer): bool
    {
        if ($this->banned($peer)) {
            return false;
        }
        return (bool) self::locked($this->dir . '/.door.json', static function (array $state): array {
            $now = microtime(true);
            $tokens = min((float) self::DOOR_BURST, (float) ($state['tokens'] ?? self::DOOR_BURST)
                + ($now - (float) ($state['at'] ?? $now)) * self::DOOR_RATE);
            if ($tokens < 1.0) {
                return [['tokens' => $tokens, 'at' => $now], false];
            }
            return [['tokens' => $tokens - 1.0, 'at' => $now], true];
        });
    }

    /** Whether an address is banned by the failure brake, without counting anything. */
    private function banned(?string $peer): bool
    {
        if ($peer === null || $this->failuresPerMinute <= 0 || !is_file($this->dir . '/.strikes.json')) {
            return false;
        }
        $key = 'h' . substr(hash_hmac('sha256', self::subscriber($peer), $this->salt()), 0, 32);
        return (bool) self::locked($this->dir . '/.strikes.json', static fn (array $strikes): array
            => [$strikes, is_array($strikes[$key] ?? null) && $strikes[$key][2] > time()]);
    }

    /** @return array{0: int, 1: ?int} status (201 when opened) and the slot number */
    private function openSlot(string $tenant, string $opening): array
    {
        $total = $this->pairSlots;
        $perSpace = max(0, min(self::SLOT_NUMBERS, (int) ($this->tenants[$tenant]['pair_slots'] ?? 4)));
        if ($total === 0 || $perSpace === 0) {
            return [403, null];
        }
        $space = $this->spaceKey($tenant);
        $result = self::locked($this->pairFile(), static function (array $slots) use ($total, $perSpace, $space, $opening): array {
            $slots = self::livePairs($slots);
            $own = count(array_filter($slots, static fn (array $slot): bool => $slot['space'] === $space));
            if (count($slots) >= $total || $own >= $perSpace) {
                return [$slots, [429, null]];
            }
            $free = array_values(array_diff(range(100, 999), array_map('intval', array_keys($slots))));
            $slot = $free[random_int(0, count($free) - 1)];
            $slots[(string) $slot] = ['space' => $space, 'created' => microtime(true),
                'parts' => [self::encode($opening), null, null, null]];
            return [$slots, [201, $slot]];
        });
        return $result ?? [503, null];
    }

    private function closeSlot(string $space, int $slot): bool
    {
        return (bool) self::locked($this->pairFile(), static function (array $slots) use ($space, $slot): array {
            if (($slots[(string) $slot]['space'] ?? null) !== $space) {
                return [$slots, false];
            }
            unset($slots[(string) $slot]);
            return [$slots, true];
        });
    }

    /** Slots expire after ten minutes; the code shown to the user is useless afterwards. */
    private static function livePairs(array $slots): array
    {
        $now = microtime(true);
        return array_filter($slots, static fn ($slot): bool => is_array($slot) && $slot['created'] > $now - self::PAIR_TTL);
    }

    private static function slotNumber(string $text): ?int
    {
        return preg_match('/^[1-9][0-9]{2}$/', $text) ? (int) $text : null;
    }

    /** Tokens of one space share their slots, as they share rooms. */
    private function spaceKey(string $tenant): string
    {
        return basename($this->tenantDir($tenant));
    }

    private function pairFile(): string
    {
        return $this->dir . '/.pairs.json';
    }
}
