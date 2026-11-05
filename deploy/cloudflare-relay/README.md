# Relay on Cloudflare Workers (free plan)

Your own k-lani-link HTTPS relay in a free Cloudflare account, without a server. It is the
same acknowledged mailbox as the Rust relay and the [PHP relay](../php-relay/README.md):
it holds only sealed ciphertext, records for 30 s and sessions for 120 s, and it cannot
read or change content. Each token space lives in its own Durable Object **in memory
only**; nothing is written to Cloudflare storage. German guide: [ANLEITUNG.de.md](ANLEITUNG.de.md).

## Set up (about five minutes)

You need Node.js 22 or newer and a Cloudflare account.

```sh
cd deploy/cloudflare-relay
npm ci
npx wrangler login
node tokens.js ~/k-lani-relay-tokens        # writes host-token and client-token
npx wrangler secret put TOKENS               # paste the JSON line tokens.js printed
npx wrangler deploy                          # prints https://k-lani-relay.<you>.workers.dev
```

`tokens.js` writes two private token files and prints the `TOKENS` secret, which holds
only their SHA-256 hashes. Keep the folder private; the host needs both files.

Then, on the model computer:

```sh
k-lani-link offer-https --dir ~/k-lani-pairing \
  --relay-url https://k-lani-relay.<you>.workers.dev \
  --admission-token-file ~/k-lani-relay-tokens/host-token \
  --client-admission-token-file ~/k-lani-relay-tokens/client-token
```

and type the relay address and the code into the control panel on the other computer.
`--qr` works as well. More tokens, e.g. one per laptop so each can be revoked alone: run
`tokens.js` again with the same space name and add the new hashes to `TOKENS`.

A [Deploy to Cloudflare](https://deploy.workers.cloudflare.com/?url=https://github.com/MarvKalani/k-lani-link/tree/main/deploy/cloudflare-relay)
link creates the Worker from this folder in your account; set the `TOKENS` secret
afterwards under the Worker's **Settings → Variables and Secrets**. This route has not
been tried yet; the commands above have.

## Settings

| Setting | Where | Default |
| --- | --- | --- |
| `TOKENS` | secret | `{}`: JSON, token SHA-256 → rules |
| `FAILED_AUTH_PER_MINUTE`, `FAILED_AUTH_BAN_MINUTES` | `wrangler.toml` | 20, 10 |
| `PAIR_SLOTS` (open pairing-by-code slots, 0: off) | `wrangler.toml` | 64 |

Per-token rules in `TOKENS`: `space` (tokens with the same name share rooms), `expires`
(last valid day `YYYY-MM-DD`, UTC, or Unix seconds), `rooms` (16), `sessions_per_room`
(8), `requests_per_second` (50), `request_burst` (200) and `pair_slots` (4). The address
rules of the other relays (`allow_ips`, `max_ips_per_day`) are not supported here.

## Limits of the free plan

As listed by Cloudflare in November 2026 (check
[Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/) and
[Durable Objects limits](https://developers.cloudflare.com/durable-objects/platform/limits/)
before relying on them): 100,000 Worker requests and 100,000 Durable Object requests a
day, reset at 00:00 UTC. Every poll and every record write is one of each. A connected
pair that is idle polls about every 25 s on each side, roughly 7,000 requests a day; a
streamed answer adds tens of requests. That is ample for a few people; beyond the limit,
requests fail until the next day, and the paid plan raises it.

## Differences from the Rust relay

- A write into a full 32-record window gets `429` without `Retry-After`, as on the PHP
  relay; the client retries at once.
- State is memory only. When Cloudflare restarts an object (a new deployment, maintenance),
  its sessions end and the endpoints reconnect with their pairing chain; nothing is
  replayed.
- Cloudflare terminates TLS. It sees request metadata (addresses, times, sizes, paths)
  and sealed records, never keys or plaintext. Workers observability is off in
  `wrangler.toml`.

## Validation

The development toolchain is pinned to Wrangler 4.157.0 with Undici 7.30.1.
`npm test` checks IPv6 subnet grouping and the failure brake, including compressed
and IPv4-embedded addresses. `npm audit` checks the complete lockfile, including
development tools; both checks also run in CI. These Node dependencies are local
development/deployment tools, not runtime imports of the deployed Worker.

Run on 2 November 2026 with `wrangler dev` 4.151.1 (local workerd, HTTPS with a private
test CA) and two k-lani-link 1.2611.2 peers: pairing by code through the Worker, the
connection, streamed answers from Qwen3.8 Flash Next (Basalt) and from Mellum 2.1 loaded
on demand (157 tokens/s through the relay), refused tokens answered `401` and, after 20
refusals in a minute, `429` while a valid token still passed. Not yet run against a
deployment on Cloudflare's network.
