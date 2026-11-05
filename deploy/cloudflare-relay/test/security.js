import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import test from "node:test";
import { Pairs } from "../src/index.js";

globalThis.crypto ??= webcrypto;

test("compressed and expanded IPv6 addresses share their actual /64", async () => {
  const pairs = new Pairs({});
  for (const addresses of [
    ["2001:db8::1", "2001:db8::2", "2001:0DB8:0000:0::abcd", "2001:db8:0:0:ffff:ffff:ffff:ffff"],
    ["::1", "::2", "0:0:0:0:0:0:0:1"],
    ["::ffff:192.0.2.1", "::ffff:c000:201", "0:0:0:0:0:ffff:c000:202"],
  ]) {
    const keys = await Promise.all(addresses.map((address) => pairs.key(address)));
    assert.equal(new Set(keys).size, 1);
  }
});

test("different IPv6 subnets and IPv4 addresses retain separate counters", async () => {
  const pairs = new Pairs({});
  const keys = await Promise.all([
    "2001:db8::1", "2001:db8:0:1::1", "2001:db8:1::1", "192.0.2.1", "192.0.2.2",
  ].map((address) => pairs.key(address)));
  assert.equal(new Set(keys).size, keys.length);
});

test("a ban applies across a compressed IPv6 /64 but valid admission still works", async () => {
  const pairs = new Pairs({});
  const miss = (peer) => pairs.fetch(new Request("https://relay.internal/pair/999/a?wait=0", {
    headers: { "x-link-peer": peer, "x-link-brake": "1;10" },
  }));
  assert.equal((await miss("2001:db8::1")).status, 404);
  assert.equal((await miss("2001:db8::1")).status, 429);
  assert.equal((await miss("2001:db8::2")).status, 429);
  assert.equal((await miss("2001:0DB8:0000:0000::3")).status, 429);
  assert.equal((await miss("2001:db8:0:1::1")).status, 404);
  const admitted = await pairs.fetch(new Request("https://relay.internal/pair", {
    method: "POST", body: new Uint8Array(33),
    headers: { "x-link-peer": "2001:db8::2", "x-link-brake": "1;10", "x-link-space": "synthetic",
      "x-link-total": "64", "x-link-slots": "4" },
  }));
  assert.equal(admitted.status, 201);
  assert.match(await admitted.text(), /^[1-9][0-9]{2}$/);
});

test("malformed peer spellings do not create arbitrary new address buckets", async () => {
  const pairs = new Pairs({});
  assert.equal(await pairs.key("bad::address"), await pairs.key("also::invalid"));
  assert.equal(await pairs.key("bad::address"), await pairs.key("::1]/ignored"));
});
