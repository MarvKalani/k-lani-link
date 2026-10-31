<?php
// Removes the relay's settings, its fast-path settings file and its ciphertext folders,
// including ones left by an earlier salt or version (same exact naming pattern only).
defined('WP_UNINSTALL_PLUGIN') || exit;
wp_clear_scheduled_hook('k_lani_link_relay_sweep');
@unlink(dirname(__DIR__, 2) . '/k-lani-link-relay-config.php');
foreach (glob(wp_upload_dir(null, false)['basedir'] . '/k-lani-link-relay-*', GLOB_ONLYDIR) ?: [] as $dir) {
    if (!preg_match('/^k-lani-link-relay-[0-9a-f]{24}$/', basename($dir))) {
        continue;
    }
    $paths = new RecursiveIteratorIterator(
        new RecursiveDirectoryIterator($dir, FilesystemIterator::SKIP_DOTS),
        RecursiveIteratorIterator::CHILD_FIRST
    );
    foreach ($paths as $path) {
        $path->isDir() ? @rmdir((string) $path) : @unlink((string) $path);
    }
    @rmdir($dir);
}
delete_option('k_lani_link_relay');
