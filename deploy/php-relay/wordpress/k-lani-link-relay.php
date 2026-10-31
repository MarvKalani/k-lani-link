<?php
/**
 * Plugin Name: k-lani-link Relay
 * Description: Encrypted HTTPS mailbox relay for k-lani-link endpoints. Holds only sealed ciphertext, expiring after 30 seconds and deleted at the next request or cron run; every request needs an access token created on its settings page.
 * Version: 0.1.0
 * Requires at least: 6.0
 * Requires PHP: 8.2
 * License: see LICENSING.md of k-lani-link
 */

declare(strict_types=1);

defined('ABSPATH') || exit;

require_once __DIR__ . '/k-lani-relay.php';

const KLANI_RELAY_OPTION = 'k_lani_link_relay';

const KLANI_RELAY_SWEEP = 'k_lani_link_relay_sweep';

/**
 * Each token: created, and optional label, space, expires (YYYY-MM-DD), allow_ips, max_ips_per_day.
 * @return array{tokens: array<string, array<string, mixed>>, max_wait: int, max_waiters: int, max_writes: int, failed_auth_per_minute: int, failed_auth_ban_minutes: int}
 */
function klani_relay_settings(): array
{
    $settings = get_option(KLANI_RELAY_OPTION, []);
    return (is_array($settings) ? $settings : []) + ['tokens' => [], 'max_wait' => 5, 'max_waiters' => 3, 'max_writes' => 2,
        'failed_auth_per_minute' => 0, 'failed_auth_ban_minutes' => 10];
}

/** The relay core's configuration: token hashes with their rules, never the tokens. */
function klani_relay_config(array $settings): array
{
    $tokens = [];
    foreach ($settings['tokens'] as $hash => $entry) {
        $tokens[$hash] = KLaniRelay::limits() + array_filter([
            'space' => (string) ($entry['space'] ?? ''),
            'expires' => (string) ($entry['expires'] ?? ''),
            'allow_ips' => array_values((array) ($entry['allow_ips'] ?? [])),
            'max_ips_per_day' => (int) ($entry['max_ips_per_day'] ?? 0),
        ]);
    }
    return [
        'dir' => klani_relay_storage($settings),
        'tokens' => $tokens,
        'max_wait' => (int) $settings['max_wait'],
        'max_waiters' => (int) $settings['max_waiters'],
        'max_writes' => (int) $settings['max_writes'],
        'failed_auth_per_minute' => (int) $settings['failed_auth_per_minute'],
        'failed_auth_ban_minutes' => (int) $settings['failed_auth_ban_minutes'],
        'salt' => hash_hmac('sha256', 'k-lani-link-relay-addresses', wp_salt('auth')),
    ];
}

/**
 * The storage folder's name: unguessable (it also protects hosts that ignore .htaccess)
 * and derived from the site's secret salt, so it is the same for every request without
 * being stored, and the REST route and the fast path always share it.
 */
function klani_relay_dir_name(): string
{
    return 'k-lani-link-relay-' . substr(hash_hmac('sha256', 'k-lani-link-relay-storage', wp_salt('auth')), 0, 24);
}

function klani_relay_storage(array $settings): string
{
    $dir = wp_upload_dir(null, false)['basedir'] . '/' . klani_relay_dir_name();
    if (!is_dir($dir)) {
        wp_mkdir_p($dir);
        @chmod($dir, 0700);
        file_put_contents("$dir/.htaccess", "Require all denied\n<IfModule !mod_authz_core.c>\nDeny from all\n</IfModule>\n");
        file_put_contents("$dir/index.html", '');
    }
    return $dir;
}

/** Where the fast path (relay.php) reads its settings; a PHP file prints nothing if fetched. */
function klani_relay_config_file(): string
{
    return dirname(__DIR__, 2) . '/k-lani-link-relay-config.php';
}

/** Publishes token hashes and settings for the fast path whenever they change. */
function klani_relay_publish(): void
{
    $config = klani_relay_config(klani_relay_settings());
    $file = klani_relay_config_file();
    $temporary = $file . '.' . bin2hex(random_bytes(6));
    if (file_put_contents($temporary, "<?php\nreturn " . var_export($config, true) . ";\n", LOCK_EX) !== false) {
        @chmod($temporary, 0600);
        rename($temporary, $file);
    }
}
add_action('add_option_' . KLANI_RELAY_OPTION, 'klani_relay_publish');
add_action('update_option_' . KLANI_RELAY_OPTION, 'klani_relay_publish');

function klani_relay_relay(): KLaniRelay
{
    return new KLaniRelay(klani_relay_config(klani_relay_settings()));
}

// Expired ciphertext must go even when no relay request comes in. WP-Cron runs on site
// visits; a real system cron calling wp-cron.php makes this independent of traffic.
add_filter('cron_schedules', static function (array $schedules): array {
    $schedules['k_lani_link_minute'] = ['interval' => 60, 'display' => 'Every minute (k-lani-link Relay)'];
    return $schedules;
});
add_action(KLANI_RELAY_SWEEP, static function (): void {
    klani_relay_relay()->sweepAll();
    klani_relay_remove_orphans();
});

/**
 * Removes storage folders this plugin no longer uses: a changed salt (security plugins,
 * `wp config shuffle-salts`) derives a new name, and earlier versions used random names.
 * Only folders matching the plugin's exact naming pattern are touched.
 */
function klani_relay_remove_orphans(): void
{
    $uploads = wp_upload_dir(null, false)['basedir'];
    foreach (glob("$uploads/k-lani-link-relay-*", GLOB_ONLYDIR) ?: [] as $dir) {
        if (preg_match('/^k-lani-link-relay-[0-9a-f]{24}$/', basename($dir)) && basename($dir) !== klani_relay_dir_name()) {
            klani_relay_remove_tree($dir);
        }
    }
}

function klani_relay_remove_tree(string $dir): void
{
    $paths = new RecursiveIteratorIterator(
        new RecursiveDirectoryIterator($dir, FilesystemIterator::SKIP_DOTS),
        RecursiveIteratorIterator::CHILD_FIRST
    );
    foreach ($paths as $path) {
        $path->isDir() ? @rmdir((string) $path) : @unlink((string) $path);
    }
    @rmdir($dir);
}
add_action('init', static function (): void {
    if (!wp_next_scheduled(KLANI_RELAY_SWEEP)) {
        wp_schedule_event(time() + 60, 'k_lani_link_minute', KLANI_RELAY_SWEEP);
    }
});
register_activation_hook(__FILE__, 'klani_relay_publish');
// Deactivation ends the fast path at once: without its settings file it answers 404.
register_deactivation_hook(__FILE__, static function (): void {
    @unlink(klani_relay_config_file());
    wp_clear_scheduled_hook(KLANI_RELAY_SWEEP);
});

add_action('rest_api_init', static function (): void {
    register_rest_route('k-lani-link', '/v1/(?P<path>[A-Za-z0-9_/-]+)', [
        'methods' => ['GET', 'POST', 'PUT', 'DELETE'],
        // The relay checks its own admission token on every request.
        'permission_callback' => '__return_true',
        'callback' => 'klani_relay_serve',
    ]);
});

function klani_relay_serve(WP_REST_Request $request): void
{
    $relay = klani_relay_relay();
    // Some PHP setups only pass Authorization through a rewrite variable.
    $authorization = $request->get_header('authorization')
        ?? ($_SERVER['HTTP_AUTHORIZATION'] ?? $_SERVER['REDIRECT_HTTP_AUTHORIZATION'] ?? null);
    $body = $request->get_body();
    [$status, $headers, $payload] = $relay->handle(
        $request->get_method(),
        '/v1/' . $request['path'],
        $request->get_query_params(),
        $authorization,
        static fn (int $limit): string => substr($body, 0, $limit + 1),
        // A long poll needs no database: free the connection for the site meanwhile.
        static function (): void {
            global $wpdb;
            $wpdb->close();
        },
        $request->get_header('content_type'),
        // The direct peer only; forwarded-for headers are never trusted.
        $_SERVER['REMOTE_ADDR'] ?? null,
    );
    status_header($status);
    foreach ($headers as $name => $value) {
        header("$name: $value");
    }
    header('Content-Length: ' . strlen($payload));
    echo $payload;
    exit;
}

add_action('admin_menu', static function (): void {
    add_options_page('k-lani-link Relay', 'k-lani-link Relay', 'manage_options', 'k-lani-link-relay', 'klani_relay_page');
});

/**
 * Access rules from a form row; invalid parts are dropped and reported.
 * @return array{0: array<string, mixed>, 1: list<string>}
 */
function klani_relay_rules(array $input): array
{
    $problems = [];
    $space = preg_replace('/[^A-Za-z0-9_-]/', '-', trim((string) ($input['space'] ?? '')));
    $expires = trim((string) ($input['expires'] ?? ''));
    if ($expires !== '' && !KLaniRelay::validDay($expires)) {
        $problems[] = "“{$expires}” is not a date";
        $expires = '';
    }
    $networks = [];
    foreach (preg_split('/[\s,;]+/', (string) ($input['allow_ips'] ?? ''), -1, PREG_SPLIT_NO_EMPTY) ?: [] as $network) {
        if (KLaniRelay::validNetwork($network)) {
            $networks[] = $network;
        } else {
            $problems[] = "“{$network}” is not an address or network";
        }
    }
    return [[
        'label' => mb_substr(sanitize_text_field((string) ($input['label'] ?? '')), 0, 60),
        'space' => substr((string) $space, 0, 64),
        'expires' => $expires,
        'allow_ips' => array_slice($networks, 0, 256),
        'max_ips_per_day' => max(0, min(1024, (int) ($input['max_ips_per_day'] ?? 0))),
    ], $problems];
}

function klani_relay_page(): void
{
    if (!current_user_can('manage_options')) {
        return;
    }
    $settings = klani_relay_settings();
    $created = [];
    $problems = [];
    if ($_SERVER['REQUEST_METHOD'] === 'POST' && check_admin_referer('k-lani-link-relay')) {
        $post = wp_unslash($_POST);
        $token = static fn (): string => rtrim(strtr(base64_encode(random_bytes(32)), '+/', '-_'), '=');
        if (isset($post['create']) || isset($post['pair'])) {
            [$rules, $problems] = klani_relay_rules((array) ($post['new'] ?? []));
            if (isset($post['pair'])) {
                // Host and client get their own tokens in one shared space: either can be revoked alone.
                $rules['space'] = $rules['space'] !== '' ? $rules['space'] : 'pair-' . wp_date('Ymd-His');
                $base = $rules['label'] !== '' ? $rules['label'] . ' ' : '';
                foreach (['host', 'client'] as $role) {
                    $created[$role] = $token();
                    $settings['tokens'][hash('sha256', $created[$role])] = ['created' => time(), 'label' => $base . $role] + $rules;
                }
            } else {
                $created['token'] = $token();
                $settings['tokens'][hash('sha256', $created['token'])] = ['created' => time()] + $rules;
            }
        } elseif (isset($post['revoke']) && is_string($post['revoke'])) {
            unset($settings['tokens'][sanitize_key($post['revoke'])]);
        } elseif (isset($post['save_rules'])) {
            foreach ((array) ($post['rules'] ?? []) as $hash => $input) {
                $hash = sanitize_key((string) $hash);
                if (isset($settings['tokens'][$hash])) {
                    [$rules, $found] = klani_relay_rules((array) $input);
                    $settings['tokens'][$hash] = $rules + $settings['tokens'][$hash];
                    $problems = array_merge($problems, $found);
                }
            }
        } elseif (isset($post['max_wait'])) {
            $settings['max_wait'] = max(0, min(KLaniRelay::LONG_POLL, (int) $post['max_wait']));
            $settings['max_waiters'] = max(0, min(64, (int) ($post['max_waiters'] ?? 3)));
            $settings['max_writes'] = max(1, min(8, (int) ($post['max_writes'] ?? 2)));
        } elseif (isset($post['failed_auth_per_minute'])) {
            $settings['failed_auth_per_minute'] = max(0, min(10000, (int) $post['failed_auth_per_minute']));
            $settings['failed_auth_ban_minutes'] = max(1, min(1440, (int) ($post['failed_auth_ban_minutes'] ?? 10)));
        }
        update_option(KLANI_RELAY_OPTION, $settings, false);
    }
    $field = static fn (string $name, string $value, string $attributes = ''): string => '<input name="' . esc_attr($name)
        . '" value="' . esc_attr($value) . '" ' . $attributes . '>';
    echo '<div class="wrap"><h1>k-lani-link Relay</h1>';
    echo '<p>Relay URL for <code>k-lani-link init-https --relay-url</code>, fast (no WordPress start per request): <code>'
        . esc_html(plugins_url('relay.php', __FILE__)) . '</code></p>'
        . '<p>If the host or a security plugin blocks direct PHP files in plugin folders, use the REST URL: <code>'
        . esc_html(untrailingslashit(rest_url('k-lani-link'))) . '</code></p>';
    foreach ($problems as $problem) {
        echo '<div class="notice notice-error"><p>' . esc_html($problem) . ' – ignored.</p></div>';
    }
    if ($created !== []) {
        echo '<div class="notice notice-warning"><p><strong>New access token(s), shown only once.</strong> Only SHA-256 hashes are stored here.</p>';
        foreach ($created as $role => $value) {
            $file = $role === 'client' ? 'client-token' : ($role === 'host' ? 'host-token' : 'relay-token');
            echo '<p>' . esc_html($role) . ' → file <code>' . esc_html($file) . '</code>: <code>' . esc_html($value) . '</code></p>';
        }
        if (isset($created['client'])) {
            echo '<p><code>k-lani-link init-https --dir new-pairing --relay-url … --admission-token-file host-token --client-admission-token-file client-token</code></p>';
        }
        echo '</div>';
    }
    // Revoke buttons belong to a form of their own: Enter in a rule field must save the
    // rules, never submit the first row's Revoke.
    echo '<h2>Access tokens</h2><form method="post" id="k-lani-link-revoke">';
    wp_nonce_field('k-lani-link-relay');
    echo '</form><form method="post">';
    wp_nonce_field('k-lani-link-relay');
    echo '<button name="save_rules" value="1" hidden></button>';
    echo '<table class="widefat"><thead><tr><th>Name</th><th>Token (start)</th><th>Space</th><th>Valid until</th><th>Allowed addresses/networks</th><th>Addresses per day</th><th>Created</th><th></th></tr></thead><tbody>';
    foreach ($settings['tokens'] as $hash => $entry) {
        $row = 'rules[' . $hash . ']';
        echo '<tr><td>' . $field("{$row}[label]", (string) ($entry['label'] ?? ''), 'size="10"')
            . '</td><td><code>' . esc_html(substr((string) $hash, 0, 8)) . '…</code></td><td>'
            . $field("{$row}[space]", (string) ($entry['space'] ?? ''), 'size="12" pattern="[A-Za-z0-9_-]*"')
            . '</td><td>' . $field("{$row}[expires]", (string) ($entry['expires'] ?? ''), 'type="date"')
            . '</td><td>' . $field("{$row}[allow_ips]", implode(', ', (array) ($entry['allow_ips'] ?? [])), 'size="24"')
            . '</td><td>' . $field("{$row}[max_ips_per_day]", (string) ($entry['max_ips_per_day'] ?? 0), 'type="number" min="0" max="1024" style="width:5em"')
            . '</td><td>' . esc_html(wp_date('Y-m-d', (int) $entry['created']))
            . '</td><td><button class="button" form="k-lani-link-revoke" name="revoke" value="' . esc_attr((string) $hash) . '">Revoke</button></td></tr>';
    }
    echo '</tbody></table>';
    if ($settings['tokens'] !== []) {
        echo '<p><button class="button" name="save_rules" value="1">Save rules</button></p>';
    }
    echo '</form><h3>New access</h3><form method="post">';
    wp_nonce_field('k-lani-link-relay');
    echo '<table class="form-table"><tbody>'
        . '<tr><th>Name</th><td>' . $field('new[label]', '', 'placeholder="e.g. Laptop"') . '</td></tr>'
        . '<tr><th>Space</th><td>' . $field('new[space]', '', 'pattern="[A-Za-z0-9_-]*"') . ' Tokens with the same space share their connections (one token per device). Empty: a space of its own; for a pair, a new shared one.</td></tr>'
        . '<tr><th>Valid until</th><td>' . $field('new[expires]', '', 'type="date"') . ' Last valid day (UTC). Empty: no end.</td></tr>'
        . '<tr><th>Allowed addresses</th><td>' . $field('new[allow_ips]', '', 'size="40" placeholder="203.0.113.0/24, 2001:db8::/48"') . ' Empty: from anywhere.</td></tr>'
        . '<tr><th>Addresses per day</th><td>' . $field('new[max_ips_per_day]', '0', 'type="number" min="0" max="1024"') . ' Different addresses within 24 hours (IPv6 per /64); 0: no limit. Stored only as salted hashes.</td></tr>'
        . '</tbody></table>'
        . '<p><button class="button button-primary" name="pair" value="1">Create token pair (host + client)</button> '
        . '<button class="button" name="create" value="1">Create single token</button></p></form>'
        . '<p class="description">Address rules see the address that connects to this site. Behind a CDN or proxy (e.g. Cloudflare) that is the proxy\'s address: leave them empty there.</p>';
    echo '<h2>Protection against guessing</h2><form method="post">';
    wp_nonce_field('k-lani-link-relay');
    echo '<p><label>Refused requests per minute and address: ' . $field('failed_auth_per_minute', (string) $settings['failed_auth_per_minute'], 'type="number" min="0" max="10000"')
        . '</label> then answer <code>429</code> for ' . $field('failed_auth_ban_minutes', (string) $settings['failed_auth_ban_minutes'], 'type="number" min="1" max="1440" style="width:5em"')
        . ' minutes. 0: off. Valid tokens are never held back.</p><p><button class="button">Save</button></p></form>';
    echo '<h2>Long poll</h2><form method="post">';
    wp_nonce_field('k-lani-link-relay');
    echo '<p><label>Maximum wait per request (seconds, 0–25): <input type="number" min="0" max="25" name="max_wait" value="'
        . esc_attr((string) $settings['max_wait']) . '"></label></p>'
        . '<p><label>Requests that may wait at once: <input type="number" min="0" max="64" name="max_waiters" value="'
        . esc_attr((string) $settings['max_waiters']) . '"></label> About half of the PHP workers your plan offers; further reads answer at once.</p>'
        . '<p><label>Parallel writes per endpoint: <input type="number" min="1" max="8" name="max_writes" value="'
        . esc_attr((string) $settings['max_writes']) . '"></label> Endpoints never exceed this and slow down further on overload.</p>'
        . '<p><button class="button">Save</button></p></form></div>';
}
