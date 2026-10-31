# PHP relay for ordinary web hosting (WordPress plugin or plain PHP)

The same acknowledged HTTPS mailbox as the Rust relay, written for cheap web hosting
where no own server process can run: a WordPress plugin, or one PHP folder on plain web
space. It stores only sealed ciphertext, which expires after 30 s and is deleted at the
next request or cron run, and it cannot read content. It is meant
for **one or two concurrent users** on a small hosting plan, not as a shared service.
German setup guide for site owners: [ANLEITUNG.de.md](ANLEITUNG.de.md).

| File | Purpose |
| --- | --- |
| `k-lani-relay.php` | Relay core (PHP 8.2+), shared by both variants |
| `wordpress/` | Plugin: fast path `relay.php` (no WordPress start), REST route `/wp-json/k-lani-link/v1/...`, settings page for access tokens, cron sweep, deactivation and uninstall hooks |
| `standalone/` | Plain web space: `index.php`, `.htaccess`, `config.example.php`, `sweep.php` for cron, denied `data/` folder |
| `test_relay.php` | Core checks without a web server: `php deploy/php-relay/test_relay.php` |
| `lab.py` | Docker lab: WordPress + MariaDB behind OWASP ModSecurity CRS with TLS, driven by the actual endpoints |

## Protection against misuse

- Every request needs an admission token (256 random bits). Without it: `401`, nothing
  else. The one exception is the client side of pairing by code (`offer-https`/`join-https`),
  as on the Rust relay: a host with a token opens a slot for ten minutes; the client, which
  knows only the code, reads the opening, writes one answer, reads the sealed bundle and
  writes the acknowledgement. Unknown, expired and used slots answer `404` and count against
  the failure brake; tokenless requests share a budget of 20 per second. `pair_slots` limits
  open slots (top level: whole relay, default 64; in a token's limits: its space, default 4;
  0 turns pairing by code off). Only SHA-256 hashes of tokens are stored and compared in
  constant time.
- Each token has its own storage namespace, room/session limits and a request-rate
  bucket (`429` with `Retry-After` beyond it), unless tokens share a `space`.
- Optional per-token rules, as in the Rust relay: `space` (tokens with the same name
  share rooms: one token per device, each revocable alone), `expires` (last valid day),
  `allow_ips` (addresses/networks) and `max_ips_per_day` (distinct addresses within
  24 hours, IPv6 per /64, stored as salted hashes). A failure brake
  (`failed_auth_per_minute`, `failed_auth_ban_minutes`) answers `429` to guessing
  addresses but never to a valid token. The plugin page creates a host+client token pair
  and edits these rules; plain web space sets them in `config.php`. Address rules use
  `REMOTE_ADDR` only; behind a CDN or proxy that is the proxy's address, so leave them
  empty there. Forwarded-for headers are never trusted.
- Records expire after 30 s, sessions after 120 s; acknowledged records are deleted at
  once. Expired files are removed by any request (at most every 5 s, all tokens) and by
  a sweep: the plugin schedules one every minute via WP-Cron, plain web space runs
  `sweep.php` from cron. **PHP cannot act on its own:** with no request and no cron run,
  expired ciphertext stays on disk until the next one. Use a real system cron (calling
  `wp-cron.php` or `sweep.php` every minute) where the host offers it.
- The storage folder is denied by `.htaccess` and, for the plugin, has an unguessable
  name derived from the site's secret salt (for hosts that ignore `.htaccess`).
- Deactivating the plugin removes the fast path's settings file at once; the fast path
  then answers `404`. Uninstalling removes settings, token hashes and stored files.
- The relay never sees keys or plaintext. A site administrator sees only ciphertext.

## Differences from the Rust relay

- A write into a full 32-record window gets `429` without `Retry-After` (the client
  retries at once) instead of waiting; PHP must not hold a worker for it.
- Long polls wait at most `max_wait` seconds (default 5). Each waiting request holds one
  PHP worker; the plugin closes its database connection before waiting, because small
  plans allow few connections (one tested host: 18).
- Limits are per token: rooms (16), sessions per room (8), 50 requests/s, burst 200.

## Endpoint settings

The relay URL may carry the mount path: the plugin's fast path
`https://example.org/wp-content/plugins/k-lani-link-relay/relay.php` (preferred), its REST
route `https://example.org/wp-json/k-lani-link` (when a host or security plugin blocks
direct PHP files in plugin folders), or `https://example.org/k-lani-relay` for plain web
space. Endpoints adapt to web firewalls by themselves and
keep the same sealed bytes:

1. `PUT` (default), then on `403`/`405`/`501`
2. `POST` with the raw record, then on `403`/`415`
3. `POST` with `{"data":"<base64url record>"}` as `application/json`

Each step stays for the rest of that client. `post_writes = true` in `[https]` starts at
step 2.

**Self-tuning.** Every authorized answer carries `X-K-Lani-Relay: wait=<s>; writes=<n>`.
Endpoints never wait longer or keep more writes in flight than announced, halve their
write parallelism on overload (429, 503, timeouts) and grow it back by one per window of
successes, and pause up to 2 s between reads while empty answers come back early. The
relay lets only `max_waiters` reads wait at once (default 3, about half the host's PHP
workers); further reads answer at once, so workers stay free for writes. Settings:
plugin page or `config.php` (`max_wait`, `max_waiters`, `max_writes`).

## Lab findings (2026-10-30)

`python3 deploy/php-relay/lab.py` with WordPress 6 (PHP 8.2, mod_php), 8 Apache workers,
`max_execution_time = 30`, OWASP CRS 4 at paranoia 1 in front with TLS, synthetic model.
One internal Docker network, no WAN; real hosters differ and must be measured.

| Load (tokens/s × tunnels) | Received | p50 | p99 | Slowest first token |
| --- | ---: | ---: | ---: | ---: |
| 50 × 1 | 1,000 / 1,000 | 108 ms | 163 ms | 5.5 s |
| 100 × 1 | 3,000 / 3,000 | 139 ms | 249 ms | 0.6 s |
| 50 × 2 | 3,000 / 3,000 | 166 ms | 1,126 ms | 1.4 s |
| 50 × 4 | 6,000 / 6,000 | 1,059 ms | 6,135 ms | 3.0 s |
| 50 × 10 | 3,808 / 15,000 | 26 s | 43 s | 18.9 s (failed) |
| 50 × 10, waits 1 s | 14,094 / 15,000 | 13 s | 31 s | 9.6 s (5 failed) |

With the fast path and self-tuning (`max_wait` 5, `max_writes` 2), same host:

| Load | `max_waiters` | Received | p50 | p99 | Slowest first token |
| --- | ---: | ---: | ---: | ---: | ---: |
| 50 × 1 | 3 | 1,500 / 1,500 | 94 ms | 145 ms | 0.9 s |
| 50 × 4 | 2 | 6,000 / 6,000 | 765 ms | 8.0 s | 1.7 s |
| 50 × 4 | 3 | 6,000 / 6,000 | 805 ms | 9.4 s | 2.1 s |
| 50 × 4 | 4 | 6,000 / 6,000 | 600 ms | 6.1 s | 2.4 s |
| 50 × 10 | 3 | 15,000 / 15,000 | 9.2 s | 22 s | 4.0 s |

Nothing is lost any more, even at 10 tunnels, but 8 workers cannot carry four or more
live streams smoothly: delays grow instead of tunnels failing. One run each. The fast
path alone, without the wait cap, delivered only 40 % at 4 tunnels, because waiting reads
took all workers. A first tuned run also showed the client's read pause not resetting
after data, which built a backlog; fixed before these numbers.

Pitfalls found, so the next test does not fall into them again:

- **CRS blocks PUT** (rule 911100) **and `application/octet-stream` bodies** (rule
  920420), both with `403`, not `405`. Hence the three-step write ladder above. Relays
  answer a bad token with `401`, so `403` on a write means a firewall rule.
- **Pretty permalinks are required** for `/wp-json/...`; with "plain" permalinks the REST
  API is only reachable through `?rest_route=`, which the relay URL cannot carry.
- **Worker count is the limit, not bandwidth.** Every tunnel holds about two waiting
  requests (one per endpoint). With 8 workers, 1–2 tunnels are smooth, 4 degrade, 10
  fail. Shorter waits help only partly and multiply requests.
- **Each request boots WordPress** (and passes the WAF), costing far more than the relay
  logic itself. A first token can take seconds while the endpoints probe the ladder.
- **Key files must stay private** (`0600`); the endpoints refuse readable token files.
- **The fast path is about 7× cheaper per request** (60 ms instead of 406 ms for a single
  request through the WAF), but it does not remove the worker limit: with 4 tunnels the
  waiting reads still occupied all 8 workers and writes queued behind them. The relay
  needs a cap on concurrent waits (planned with self-tuning).
- Fast path and REST route must share one storage folder; an earlier version stored a
  random folder name and could create two. The name is now derived from the salt.
- Review findings fixed: expiry kept metadata of deleted records (an expired record was
  answered with `204` instead of `410`); expired files stayed without traffic (now cron
  and request sweeps); deactivation left the fast path open. `test_relay.php` covers them.
- PHP CGI/FPM setups may drop `Authorization`; the standalone `.htaccess` and the plugin
  pass it through `HTTP_AUTHORIZATION`/`REDIRECT_HTTP_AUTHORIZATION`. Not yet tested on a
  real CGI host.

Not tested: real shared web hosts, CDN or hoster rate limiting, security plugins
that restrict the REST API, the standalone variant and PHP 8.3/8.4.
