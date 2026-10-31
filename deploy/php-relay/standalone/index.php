<?php
/**
 * k-lani-link relay for plain PHP web space (no WordPress). Upload this folder together with
 * `k-lani-relay.php`, copy `config.example.php` to `config.php` and add your token hashes.
 * The relay URL is this folder's URL, e.g. https://example.org/k-lani-relay
 */

declare(strict_types=1);

require __DIR__ . '/k-lani-relay.php';
$config = require __DIR__ . '/config.php';

$base = rtrim(str_replace('\\', '/', dirname($_SERVER['SCRIPT_NAME'] ?? '/')), '/');
$path = (string) parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH);
$relay = new KLaniRelay($config);
if (!str_starts_with($path, $base . '/v1/')) {
    [$status, $headers, $payload] = KLaniRelay::reply(404);
} else {
    $authorization = $_SERVER['HTTP_AUTHORIZATION'] ?? $_SERVER['REDIRECT_HTTP_AUTHORIZATION']
        ?? (function_exists('getallheaders') ? (array_change_key_case(getallheaders())['authorization'] ?? null) : null);
    [$status, $headers, $payload] = $relay->handle(
        $_SERVER['REQUEST_METHOD'] ?? 'GET',
        substr($path, strlen($base)),
        $_GET,
        $authorization,
        static fn (int $limit): string => (string) file_get_contents('php://input', false, null, 0, $limit + 1),
        null,
        $_SERVER['CONTENT_TYPE'] ?? null,
        // The direct peer only; forwarded-for headers are never trusted.
        $_SERVER['REMOTE_ADDR'] ?? null,
    );
}
http_response_code($status);
foreach ($headers as $name => $value) {
    header("$name: $value");
}
header('Content-Length: ' . strlen($payload));
echo $payload;
