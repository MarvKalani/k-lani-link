// k-lani-link HTTPS mailbox relay as a Cloudflare Worker.
//
// The same acknowledged multi-record mailbox as the Rust relay (src/mailbox/relay.rs) and
// the PHP relay (deploy/php-relay): it holds only sealed ciphertext, records for 30 seconds
// and sessions for 120 seconds, and cannot read or change content. Every request needs an
// admission token; only SHA-256 hashes of tokens are configured (secret `TOKENS`).
//
// Each token space lives in its own Durable Object, in memory only: nothing is written to
// Cloudflare storage, and a restarted object starts empty (the endpoints then reconnect).
// Pairing by code and the failure brake share one further object.

const WINDOW = 32;
const RETRY_HASHES = 64;
const RECORD_TTL = 30_000;
const SESSION_TTL = 120_000;
const LONG_POLL = 25;
const HELLO_BYTES = 101;
const REPLY_BYTES = 102;
const MAX_PACKET_BYTES = 12 * 1024 + 28;
const PAIR_TTL = 600_000;
const PAIR_PART_BYTES = [64, 128, 8192, 64];
const DOOR_RATE = 20;
const DOOR_BURST = 40;
const WRITES = 8;
const LIMITS = { rooms: 16, sessions_per_room: 8, requests_per_second: 50, request_burst: 200, pair_slots: 4 };
const ID = "[A-Za-z0-9_-]+";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const path = url.pathname;
    const peer = request.headers.get("CF-Connecting-IP") || "";
    const tokens = parseTokens(env.TOKENS);
    const tenant = await authorize(request.headers.get("Authorization"), tokens);
    const pairs = env.PAIRS.get(env.PAIRS.idFromName("pairs"));
    if (path === "/v1/pair" || path.startsWith("/v1/pair/")) {
      // The slots' client side has no token, so these routes authorize per part.
      const headers = { "x-link-peer": peer };
      if (tenant && admits(tokens[tenant])) {
        headers["x-link-space"] = await spaceKey(tenant, tokens[tenant]);
        headers["x-link-slots"] = String(limit(tokens[tenant], "pair_slots"));
        headers["x-link-total"] = String(Math.max(0, Math.min(900, Number(env.PAIR_SLOTS ?? 64))));
      } else if (request.headers.has("Authorization")) {
        headers["x-link-refused"] = "1";
      }
      return pairs.fetch(forward(request, "/pair" + path.slice(8), url.search, headers, env));
    }
    if (!path.startsWith("/v1/")) {
      return reply(404);
    }
    if (!tenant || !admits(tokens[tenant])) {
      const brake = await pairs.fetch(forward(request, "/brake/strike", "", { "x-link-peer": peer }, env, true));
      return reply(brake.status === 429 ? 429 : 401);
    }
    const space = await spaceKey(tenant, tokens[tenant]);
    const object = env.SPACES.get(env.SPACES.idFromName(space));
    const headers = { "x-link-tenant": tenant, "x-link-limits": JSON.stringify({ ...LIMITS, ...tokens[tenant] }) };
    const response = await object.fetch(forward(request, path, url.search, headers, env));
    // Tells the endpoints this relay's limits; they never exceed them (self-tuning).
    const out = new Response(response.body, response);
    out.headers.set("X-K-Lani-Relay", `wait=${LONG_POLL}; writes=${WRITES}`);
    return out;
  },
};

let parsedFrom = null;
let parsed = {};
/** `TOKENS`: JSON mapping lowercase hex SHA-256 of each token to its rules and limits. */
function parseTokens(text) {
  if (text !== parsedFrom) {
    parsedFrom = text;
    try {
      const value = JSON.parse(text || "{}");
      parsed = value && typeof value === "object" && !Array.isArray(value) ? lowerKeys(value) : {};
    } catch {
      parsed = {};
    }
  }
  return parsed;
}
function lowerKeys(value) {
  const out = {};
  for (const [key, rules] of Object.entries(value)) {
    if (/^[0-9a-fA-F]{64}$/.test(key)) out[key.toLowerCase()] = rules && typeof rules === "object" ? rules : {};
  }
  return out;
}

async function authorize(header, tokens) {
  const match = /^Bearer ([A-Za-z0-9_-]{32,256})$/.exec((header || "").trim());
  if (!match) return null;
  const presented = hex(await sha256(new TextEncoder().encode(match[1])));
  let found = null;
  for (const hash of Object.keys(tokens)) {
    if (same(hash, presented)) found = hash;
  }
  return found;
}

/** `expires`: last valid day (YYYY-MM-DD, UTC) or Unix seconds from which it is refused. */
function admits(rules) {
  const expires = rules.expires;
  if (expires === undefined || expires === null || expires === "" || expires === 0) return true;
  if (typeof expires === "number") return Date.now() / 1000 < expires;
  const day = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(expires));
  if (!day) return false;
  const end = Date.UTC(+day[1], +day[2] - 1, +day[3]) + 86_400_000;
  return Number.isFinite(end) && new Date(end - 1).toISOString().slice(0, 10) === expires && Date.now() < end;
}

function limit(rules, name) {
  const value = Number(rules[name] ?? LIMITS[name]);
  return Number.isFinite(value) && value >= 0 ? value : LIMITS[name];
}

/** Separate storage per token, or per `space` shared by its tokens. */
async function spaceKey(tenant, rules) {
  const space = typeof rules.space === "string" && rules.space !== "" ? rules.space : null;
  if (space === null) return tenant.slice(0, 32);
  return hex(await sha256(new TextEncoder().encode(`k-lani-link-space:${space}`))).slice(0, 32);
}

function forward(request, path, search, headers, env, empty = false) {
  const target = new URL(path + search, "https://relay.internal");
  const copy = new Headers();
  for (const name of ["content-type", "content-length"]) {
    if (request.headers.has(name) && !empty) copy.set(name, request.headers.get(name));
  }
  for (const [name, value] of Object.entries(headers)) copy.set(name, value);
  copy.set("x-link-brake", `${Number(env.FAILED_AUTH_PER_MINUTE ?? 20)};${Number(env.FAILED_AUTH_BAN_MINUTES ?? 10)}`);
  const body = empty || request.method === "GET" || request.method === "HEAD" ? null : request.body;
  return new Request(target, { method: empty ? "POST" : request.method, headers: copy, body });
}

/* ---- shared helpers ---- */

function reply(status, body = null, extra = {}) {
  const headers = {
    "Cache-Control": "no-store, no-transform",
    "Content-Type": "application/octet-stream",
    "X-Content-Type-Options": "nosniff",
    ...extra,
  };
  if (status === 429 && !("Retry-After" in extra)) headers["Retry-After"] = "1";
  if (extra["Retry-After"] === null) delete headers["Retry-After"];
  return new Response(status === 204 ? null : body, { status, headers });
}
/** A full record window: the client retries promptly, without a Retry-After hint. */
function busy() {
  return reply(429, null, { "Retry-After": null });
}

async function sha256(bytes) {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
}
function hex(bytes) {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}
function same(a, b) {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}
function equalBytes(a, b) {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
  return diff === 0;
}
function encode(bytes) {
  let text = "";
  for (const b of bytes) text += String.fromCharCode(b);
  return btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function decode(text) {
  if (!/^[A-Za-z0-9_-]*$/.test(text)) return null;
  try {
    const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (text.length % 4)) % 4));
    const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
    return encode(bytes) === text ? bytes : null;
  } catch {
    return null;
  }
}
function id(value, bytes) {
  const decoded = decode(value);
  return decoded !== null && decoded.length === bytes;
}
function concat(...parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}
function u32(n) {
  const out = new Uint8Array(4);
  new DataView(out.buffer).setUint32(0, n);
  return out;
}

/** The body, at most `max` bytes; null when it is longer or unreadable. */
async function body(request, max) {
  const json = (request.headers.get("content-type") || "").split(";")[0].trim().toLowerCase() === "application/json";
  const cap = json ? Math.floor((max + 2) / 3) * 4 + 16 : max;
  if (!request.body) return new Uint8Array();
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0;
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    size += value.length;
    if (size > cap) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const bytes = concat(...chunks);
  if (!json) return bytes;
  // Web firewalls often admit only JSON; the record inside is the same sealed bytes.
  try {
    const wrapped = JSON.parse(new TextDecoder().decode(bytes));
    const keys = wrapped && typeof wrapped === "object" ? Object.keys(wrapped) : [];
    if (keys.length !== 1 || keys[0] !== "data" || typeof wrapped.data !== "string") return null;
    const data = decode(wrapped.data);
    return data !== null && data.length <= max ? data : null;
  } catch {
    return null;
  }
}

function wait(params) {
  const value = params.get("wait") ?? String(LONG_POLL);
  return /^[0-9]{1,3}$/.test(value) && Number(value) <= LONG_POLL ? Number(value) : null;
}
function onlyKeys(params, allowed) {
  for (const key of params.keys()) if (!allowed.includes(key)) return false;
  return true;
}

/** Waiting requests wake on any change; other requests run meanwhile. */
class Changes {
  constructor() {
    this.waiters = new Set();
  }
  notify() {
    for (const wake of this.waiters) wake();
    this.waiters.clear();
  }
  async poll(seconds, probe) {
    const deadline = Date.now() + seconds * 1000;
    for (;;) {
      const result = await probe();
      if (result !== null || Date.now() >= deadline) return result;
      await new Promise((resolve) => {
        const done = () => {
          clearTimeout(timer);
          this.waiters.delete(done);
          resolve();
        };
        const timer = setTimeout(done, Math.min(1000, deadline - Date.now()));
        this.waiters.add(done);
      });
    }
  }
}

/* ---- one token space: rooms, sessions and records ---- */

export class Space {
  constructor(ctx) {
    this.ctx = ctx;
    this.rooms = new Map();
    this.buckets = new Map();
    this.changes = new Changes();
  }

  async fetch(request) {
    const url = new URL(request.url);
    const tenant = request.headers.get("x-link-tenant");
    const limits = JSON.parse(request.headers.get("x-link-limits"));
    if (!this.rate(tenant, limits)) return reply(429);
    this.sweepAll(Date.now());
    const write = request.method === "PUT" || request.method === "POST";
    const path = url.pathname;
    let m;
    if ((m = new RegExp(`^/v1/hello/(${ID})/client$`).exec(path))) {
      return write ? this.writeHello(m[1], null, request, limits)
        : request.method === "GET" ? this.readHello(m[1], null, url.searchParams) : reply(405);
    }
    if ((m = new RegExp(`^/v1/hello/(${ID})/host/(${ID})$`).exec(path))) {
      return write ? this.writeHello(m[1], m[2], request, limits)
        : request.method === "GET" ? this.readHello(m[1], m[2], url.searchParams) : reply(405);
    }
    if ((m = new RegExp(`^/v1/box/(${ID})/(${ID})/(c2h|h2c)/([0-9]{1,19})$`).exec(path))) {
      return write ? this.putRecord(m[1], m[2], m[3] === "c2h" ? 0 : 1, m[4], request) : reply(405);
    }
    if ((m = new RegExp(`^/v1/box/(${ID})/(${ID})/(c2h|h2c)$`).exec(path))) {
      return request.method === "GET" ? this.getRecords(m[1], m[2], m[3] === "c2h" ? 0 : 1, url.searchParams) : reply(405);
    }
    return reply(404);
  }

  rate(tenant, limits) {
    const now = Date.now() / 1000;
    const burst = Number(limits.request_burst);
    const state = this.buckets.get(tenant) ?? { tokens: burst, at: now };
    const tokens = Math.min(burst, state.tokens + (now - state.at) * Number(limits.requests_per_second));
    const allowed = tokens >= 1;
    this.buckets.set(tenant, { tokens: allowed ? tokens - 1 : tokens, at: now });
    return allowed;
  }

  /** Expires one room in place; false when the whole room is gone. */
  sweep(room, now) {
    if (room.updated <= now - SESSION_TTL) return false;
    if (room.client && room.client.created <= now - RECORD_TTL) room.client = null;
    for (const [hash, entry] of room.replies) if (entry.created <= now - RECORD_TTL) room.replies.delete(hash);
    for (const [session, state] of room.sessions) {
      const removed = state.updated <= now - SESSION_TTL;
      for (const channel of state.channels) {
        for (const [index, entry] of channel.records) {
          if (removed || entry.created <= now - RECORD_TTL) {
            channel.records.delete(index);
            channel.floor = Math.max(channel.floor, index + 1);
          }
        }
      }
      if (removed) room.sessions.delete(session);
    }
    return true;
  }
  sweepAll(now) {
    for (const [name, room] of this.rooms) if (!this.sweep(room, now)) this.rooms.delete(name);
  }

  async writeHello(roomId, hash, request, limits) {
    const expected = hash === null ? HELLO_BYTES : REPLY_BYTES;
    if (!id(roomId, 16) || (hash !== null && !id(hash, 32))) return reply(400);
    const bytes = await body(request, expected);
    const magic = [0x4b, 0x4c, 0x48, 0x31]; // "KLH1"
    if (bytes === null || bytes.length !== expected || magic.some((b, i) => bytes[i] !== b) || bytes[4] !== (hash === null ? 1 : 2)) {
      return reply(400);
    }
    const now = Date.now();
    let room = this.rooms.get(roomId);
    if (!room) {
      if (this.rooms.size >= Number(limits.rooms)) return reply(429);
      room = { updated: now, client: null, replies: new Map(), sessions: new Map() };
      this.rooms.set(roomId, room);
    }
    const old = hash === null ? room.client?.bytes : room.replies.get(hash)?.bytes;
    if (old) {
      if (equalBytes(old, bytes)) return reply(204);
      if (hash !== null) return reply(409);
    }
    if (hash !== null) {
      const hello = room.client?.bytes;
      if (!hello || encode(await sha256(hello)) !== hash) return reply(410);
      const session = encode(await sha256(concat(new TextEncoder().encode("k-lani-link/mailbox/transcript/v1"), u32(hello.length), hello, u32(bytes.length), bytes)));
      if (!room.sessions.has(session)) {
        if (room.sessions.size >= Number(limits.sessions_per_room)) return reply(429);
        const channel = () => ({ floor: 1, offered: 0, records: new Map(), acknowledged: [] });
        room.sessions.set(session, { updated: now, channels: [channel(), channel()] });
      }
      room.replies.set(hash, { created: now, bytes });
    } else {
      room.client = { created: now, bytes };
    }
    room.updated = now;
    this.changes.notify();
    return reply(204);
  }

  async readHello(roomId, hash, params) {
    const seconds = wait(params);
    if (!id(roomId, 16) || (hash !== null && !id(hash, 32)) || seconds === null || !onlyKeys(params, ["wait"])) return reply(400);
    const found = await this.changes.poll(seconds, () => {
      const room = this.rooms.get(roomId);
      if (!room || !this.sweep(room, Date.now())) return null;
      room.updated = Date.now();
      const entry = hash === null ? room.client : room.replies.get(hash);
      return entry ? entry.bytes : null;
    });
    return found === null ? reply(204) : reply(200, found);
  }

  async putRecord(roomId, session, direction, sequence, request) {
    if (!id(roomId, 16) || !id(session, 32) || sequence[0] === "0" || sequence.length > 18) return reply(400);
    const index = Number(sequence);
    const bytes = await body(request, MAX_PACKET_BYTES);
    if (bytes === null || bytes.length < 28 || bytes[0] !== 0x4b || bytes[1] !== 0x4c || bytes[2] !== 0x49 || bytes[3] !== 0x32
      || new DataView(bytes.buffer, bytes.byteOffset + 4, 8).getBigUint64(0) !== BigInt(sequence) - 1n) {
      return reply(400);
    }
    const room = this.rooms.get(roomId);
    const state = room && this.sweep(room, Date.now()) ? room.sessions.get(session) : null;
    if (!state) return reply(410);
    const now = Date.now();
    const channel = state.channels[direction];
    const digest = hex(await sha256(bytes));
    if (index < channel.floor) {
      const acked = channel.acknowledged.find(([at]) => at === index);
      return reply(acked ? (acked[1] === digest ? 204 : 409) : 410);
    }
    const held = channel.records.get(index);
    if (held) return reply(held.sha === digest ? 204 : 409);
    // Keep room for a missing earlier record; the client retries a later one.
    if (index - channel.floor >= WINDOW || channel.records.size >= WINDOW) return busy();
    channel.records.set(index, { created: now, sha: digest, bytes });
    state.updated = now;
    room.updated = now;
    this.changes.notify();
    return reply(204);
  }

  async getRecords(roomId, session, direction, params) {
    const seconds = wait(params);
    const fromText = params.get("from");
    if (!id(roomId, 16) || !id(session, 32) || seconds === null || fromText === null
      || !/^[1-9][0-9]{0,17}$/.test(fromText) || !onlyKeys(params, ["from", "wait"])) {
      return reply(400);
    }
    const from = Number(fromText);
    const result = await this.changes.poll(seconds, () => {
      const room = this.rooms.get(roomId);
      const state = room && this.sweep(room, Date.now()) ? room.sessions.get(session) : null;
      if (!state) return 410;
      const channel = state.channels[direction];
      if (from < channel.floor) return 410;
      if (from > channel.offered + 1) return 400;
      // Everything below `from` reached the reader: drop it and remember its hash.
      for (const [index, entry] of channel.records) {
        if (index < from) {
          channel.records.delete(index);
          channel.acknowledged.push([index, entry.sha]);
        }
      }
      channel.acknowledged = channel.acknowledged.slice(-RETRY_HASHES);
      channel.floor = from;
      const parts = [];
      for (let index = from; index < from + WINDOW && channel.records.has(index); index++) {
        const packet = channel.records.get(index).bytes;
        parts.push(u32(packet.length), packet);
        channel.offered = Math.max(channel.offered, index);
      }
      state.updated = Date.now();
      room.updated = Date.now();
      return parts.length ? concat(...parts) : null;
    });
    if (typeof result === "number") return reply(result);
    return result === null ? reply(204) : reply(200, result);
  }
}

/* ---- pairing by code and the failure brake ---- */

// As in the Rust relay (src/mailbox/relay/slots.rs): a host with a token opens a slot and
// gets a three-digit number. The client knows only the code, so its side of a slot is the
// one route without a token: it reads the opening (a), writes ONE answer (b), reads the
// sealed bundle (c) and writes the sealed acknowledgement (d). Unknown, expired and used
// slots all answer 404 and count against the failure brake.
export class Pairs {
  constructor(ctx) {
    this.ctx = ctx;
    this.slots = new Map();
    this.strikes = new Map();
    this.door = { tokens: DOOR_BURST, at: Date.now() / 1000 };
    this.changes = new Changes();
    this.salt = crypto.getRandomValues(new Uint8Array(32));
  }

  async fetch(request) {
    const url = new URL(request.url);
    const peer = request.headers.get("x-link-peer") || "";
    const [perMinute, banMinutes] = (request.headers.get("x-link-brake") || "20;10").split(";").map(Number);
    this.brake = { perMinute: Math.max(0, perMinute || 0), ban: 60_000 * Math.max(1, Math.min(1440, banMinutes || 10)) };
    if (url.pathname === "/brake/strike") return reply((await this.strike(peer)) ? 429 : 401);
    if (request.headers.has("x-link-refused")) return reply((await this.strike(peer)) ? 429 : 401);
    return this.pair(request, url, peer);
  }

  live() {
    const now = Date.now();
    for (const [number, slot] of this.slots) if (slot.created <= now - PAIR_TTL) this.slots.delete(number);
  }

  async key(peer) {
    // IPv4 per address, IPv6 per /64; only salted hashes are kept, in memory.
    const subscriber = peer.includes(":") ? peer.split(":").slice(0, 4).join(":") : peer;
    return hex(await sha256(concat(this.salt, new TextEncoder().encode(subscriber)))).slice(0, 32);
  }

  /** Counts a refused request against its address; true while that address is banned. */
  async strike(peer) {
    if (!peer || this.brake.perMinute <= 0) return false;
    const key = await this.key(peer);
    const now = Date.now();
    for (const [k, s] of this.strikes) if (s.start <= now - 60_000 && s.until <= now) this.strikes.delete(k);
    if (!this.strikes.has(key) && this.strikes.size >= 4096) return false;
    let strike = this.strikes.get(key) ?? { start: now, count: 0, until: 0 };
    if (strike.until > now) return true;
    if (strike.start <= now - 60_000) strike = { start: now, count: 0, until: 0 };
    strike.count++;
    if (strike.count > this.brake.perMinute) strike.until = now + this.brake.ban;
    this.strikes.set(key, strike);
    return strike.until > now;
  }

  async banned(peer) {
    if (!peer || this.brake.perMinute <= 0) return false;
    const strike = this.strikes.get(await this.key(peer));
    return !!strike && strike.until > Date.now();
  }

  /** A banned address gets 429 first; others pay one token of the shared tokenless budget. */
  async through(peer) {
    if (await this.banned(peer)) return false;
    const now = Date.now() / 1000;
    const tokens = Math.min(DOOR_BURST, this.door.tokens + (now - this.door.at) * DOOR_RATE);
    const allowed = tokens >= 1;
    this.door = { tokens: allowed ? tokens - 1 : tokens, at: now };
    return allowed;
  }

  async noSlot(space, peer) {
    return reply(space === null && (await this.strike(peer)) ? 429 : 404);
  }

  async pair(request, url, peer) {
    const m = /^\/pair(?:\/([^/]+)(?:\/([^/]+))?)?$/.exec(url.pathname);
    if (!m) return reply(404);
    const [, number, name] = m;
    const tenantSpace = request.headers.get("x-link-space");
    this.live();
    if (number === undefined) {
      // GET answers 405 without a token: clients use it to see that pairing exists.
      if (request.method !== "POST") return reply(405);
      if (tenantSpace === null) return reply((await this.strike(peer)) ? 429 : 401);
      const opening = await body(request, PAIR_PART_BYTES[0]);
      if (opening === null || opening.length === 0) return reply(400);
      const total = Number(request.headers.get("x-link-total"));
      const perSpace = Math.max(0, Math.min(900, Number(request.headers.get("x-link-slots"))));
      if (total === 0 || perSpace === 0) return reply(403);
      const own = [...this.slots.values()].filter((slot) => slot.space === tenantSpace).length;
      if (this.slots.size >= total || own >= perSpace) return reply(429);
      const free = [];
      for (let n = 100; n <= 999; n++) if (!this.slots.has(n)) free.push(n);
      const pick = new Uint32Array(1);
      crypto.getRandomValues(pick);
      const slot = free[pick[0] % free.length];
      this.slots.set(slot, { space: tenantSpace, created: Date.now(), parts: [opening, null, null, null] });
      return reply(201, String(slot), { "Content-Type": "text/plain" });
    }
    const slot = /^[1-9][0-9]{2}$/.test(number) ? Number(number) : null;
    if (name === undefined) {
      if (request.method !== "DELETE") return reply(405);
      if (tenantSpace === null) return reply((await this.strike(peer)) ? 429 : 401);
      const entry = slot === null ? null : this.slots.get(slot);
      if (!entry || entry.space !== tenantSpace) return reply(404);
      this.slots.delete(slot);
      return reply(204);
    }
    const index = ["a", "b", "c", "d"].indexOf(name);
    const write = request.method === "PUT" || request.method === "POST";
    if (index < 0 || (write && index === 0)) return reply(404);
    if (!write && request.method !== "GET") return reply(405);
    // Parts the client uses without a token: a (read), b (write), c (read), d (write).
    let space;
    if ((index === 0 && !write) || (index === 1 && write) || (index === 2 && !write) || (index === 3 && write)) {
      if (!(await this.through(peer))) return reply(429);
      space = null;
    } else {
      if (tenantSpace === null) return reply((await this.strike(peer)) ? 429 : 401);
      space = tenantSpace;
    }
    if (write) {
      if (slot === null) return this.noSlot(space, peer);
      const bytes = await body(request, PAIR_PART_BYTES[index]);
      if (bytes === null || bytes.length === 0) return reply(400);
      const entry = this.slots.get(slot);
      // Each part is written once, and only after the part before it.
      if (!entry || (space !== null && entry.space !== space) || entry.parts[index] !== null || entry.parts[index - 1] === null) {
        return this.noSlot(space, peer);
      }
      entry.parts[index] = bytes;
      this.changes.notify();
      return reply(204);
    }
    const seconds = wait(url.searchParams);
    if (seconds === null) return reply(400);
    if (slot === null) return this.noSlot(space, peer);
    // Tokenless reads never wait, so a stranger who guessed the number cannot hold the
    // client's place; the client polls. The host side may wait.
    const found = await this.changes.poll(space === null ? 0 : seconds, () => {
      this.live();
      const entry = this.slots.get(slot);
      // The opening is offered only until someone answered it: one attempt per slot.
      if (!entry || (space !== null && entry.space !== space) || (index === 0 && entry.parts[1] !== null)) return false;
      return entry.parts[index];
    });
    if (found === false) return this.noSlot(space, peer);
    return found === null ? reply(204) : reply(200, found);
  }
}
