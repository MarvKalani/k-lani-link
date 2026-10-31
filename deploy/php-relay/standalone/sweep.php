<?php
/**
 * Deletes expired ciphertext even when no request comes in. Run it from the host's cron,
 * e.g. every minute: `php /path/to/k-lani-relay/sweep.php`. Command line only.
 */

declare(strict_types=1);

PHP_SAPI === 'cli' || exit;
require __DIR__ . '/k-lani-relay.php';
(new KLaniRelay(require __DIR__ . '/config.php'))->sweepAll();
