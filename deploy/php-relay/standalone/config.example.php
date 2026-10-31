<?php
// Copy to config.php. Never put the tokens themselves here, only their SHA-256 hashes:
//   php -r 'echo hash("sha256", trim(file_get_contents("relay-token"))), "\n";'
return [
    // Outside the web root if your host allows it; otherwise the bundled, denied `data` folder.
    'dir' => __DIR__ . '/data',
    'tokens' => [
        // 'lowercase-hex-sha256-of-the-token' => KLaniRelay::limits(),
        // Optional rules per token; tokens with the same 'space' share rooms (one per device):
        // 'sha256-of-host-token' => KLaniRelay::limits() + ['space' => 'me', 'max_ips_per_day' => 4],
        // 'sha256-of-client-token' => KLaniRelay::limits() + ['space' => 'me', 'expires' => '2027-06-30',
        //     'allow_ips' => ['203.0.113.0/24', '2001:db8::/48'], 'max_ips_per_day' => 8],
    ],
    // Refused requests per minute and address before it gets 429 (0: off); valid tokens pass.
    'failed_auth_per_minute' => 0,
    'failed_auth_ban_minutes' => 10,
    // Seconds a waiting request may hold a PHP worker (0-25).
    'max_wait' => 5,
    // Reads that may hold a PHP worker at once (about half the workers); others answer at once.
    'max_waiters' => 3,
    // Parallel writes each endpoint may use; announced to the endpoints.
    'max_writes' => 2,
    // Open pairing-by-code slots on the whole relay (0: pairing by code off). Per space the
    // default is 4; set 'pair_slots' => N in a token's limits to change it.
    'pair_slots' => 64,
];
