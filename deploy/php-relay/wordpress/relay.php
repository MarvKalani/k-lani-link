<?php
/**
 * Fast path of the k-lani-link Relay plugin: serves the relay without starting WordPress or
 * opening a database connection, so each request costs a fraction of the REST route.
 * Relay URL: https://<site>/wp-content/plugins/k-lani-link-relay/relay.php
 * The plugin publishes token hashes and settings to wp-content/k-lani-link-relay-config.php;
 * without that file this answers 404 and the REST route remains the way in.
 */

declare(strict_types=1);

require __DIR__ . '/k-lani-relay.php';

$config = is_file(dirname(__DIR__, 2) . '/k-lani-link-relay-config.php')
    ? include dirname(__DIR__, 2) . '/k-lani-link-relay-config.php' : null;
if (!is_array($config) || ($_SERVER['PATH_INFO'] ?? '') === '') {
    [$status, $headers, $payload] = KLaniRelay::reply(404);
} else {
    $authorization = $_SERVER['HTTP_AUTHORIZATION'] ?? $_SERVER['REDIRECT_HTTP_AUTHORIZATION']
        ?? (function_exists('getallheaders') ? (array_change_key_case(getallheaders())['authorization'] ?? null) : null);
    [$status, $headers, $payload] = (new KLaniRelay($config))->handle(
        $_SERVER['REQUEST_METHOD'] ?? 'GET',
        (string) $_SERVER['PATH_INFO'],
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
