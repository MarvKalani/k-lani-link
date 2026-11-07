import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { chmodSync, cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { PLATFORMS, binaryName, packageFor, unsupportedMessage } from '../lib/platform.js';
import { locate, run } from '../lib/run.js';

const here = dirname(fileURLToPath(import.meta.url));

test('every supported platform maps to its scoped package and binary name', () => {
  const expected = {
    'linux-x64': '@k-lani/link-linux-x64',
    'linux-arm64': '@k-lani/link-linux-arm64',
    'darwin-x64': '@k-lani/link-darwin-x64',
    'darwin-arm64': '@k-lani/link-darwin-arm64',
    'win32-x64': '@k-lani/link-win32-x64',
    'win32-arm64': '@k-lani/link-win32-arm64',
  };
  assert.equal(PLATFORMS.length, 6);
  for (const [key, name] of Object.entries(expected)) {
    const [platform, arch] = key.split('-');
    assert.deepEqual(packageFor(platform, arch), { name, binary: binaryName(platform) });
  }
  assert.equal(binaryName('win32'), 'k-lani-link.exe');
  assert.equal(binaryName('linux'), 'k-lani-link');
  // npm refuses unscoped names ending in a platform as spam.
  assert.ok(PLATFORMS.every((p) => p.name.startsWith('@k-lani/link-')));
});

test('an unsupported platform gets one clear message and exit code 1, no stack trace', () => {
  assert.equal(packageFor('freebsd', 'x64'), null);
  assert.equal(packageFor('linux', 'ia32'), null);
  let written = '';
  let code;
  const child = run({
    platform: 'freebsd',
    arch: 'x64',
    stderr: { write: (text) => (written += text) },
    exit: (value) => (code = value),
    spawn: () => assert.fail('nothing may start'),
  });
  assert.equal(child, null);
  assert.equal(code, 1);
  assert.equal(written, unsupportedMessage('freebsd', 'x64'));
  assert.match(written, /no build for freebsd-x64/);
  assert.doesNotMatch(written, /at .*\.js:\d+/);
});

test('a missing platform package is named instead of a resolve error', () => {
  const found = locate('linux', 'x64', () => {
    throw new Error('Cannot find module');
  });
  assert.match(found.error, /@k-lani\/link-linux-x64/);
  assert.match(found.error, /npm install -g k-lani-link/);
});

test('arguments, exit code and signals pass through unchanged', () => {
  const calls = [];
  const signals = new EventEmitter();
  const child = new EventEmitter();
  child.kill = (signal) => calls.push(['kill', signal]);
  let code;
  run({
    platform: 'linux',
    arch: 'x64',
    argv: ['models', '--config', 'with space.toml', '--', '-x'],
    resolve: (request) => `/fake/${request}`,
    spawn: (path, args, options) => {
      calls.push(['spawn', path, args, options]);
      return child;
    },
    signals,
    exit: (value) => (code = value),
  });
  assert.deepEqual(calls[0], [
    'spawn',
    '/fake/@k-lani/link-linux-x64/bin/k-lani-link',
    ['models', '--config', 'with space.toml', '--', '-x'],
    { stdio: 'inherit' },
  ]);
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) signals.emit(signal, signal);
  assert.deepEqual(calls.slice(1), [
    ['kill', 'SIGINT'],
    ['kill', 'SIGTERM'],
    ['kill', 'SIGHUP'],
  ]);
  child.emit('exit', 7, null);
  assert.equal(code, 7);
  assert.equal(signals.listenerCount('SIGINT'), 0);

  let raised;
  const other = new EventEmitter();
  other.kill = () => {};
  run({
    platform: 'linux',
    arch: 'x64',
    resolve: () => '/fake',
    spawn: () => other,
    signals: new EventEmitter(),
    raise: (signal) => (raised = signal),
  });
  other.emit('exit', null, 'SIGTERM');
  assert.equal(raised, 'SIGTERM');
});

test('the real starter runs an installed binary and prints nothing of its own', { skip: process.platform === 'win32' }, () => {
  const root = mkdtempSync(join(tmpdir(), 'kl-npm-'));
  try {
    const main = join(root, 'node_modules', 'k-lani-link');
    cpSync(join(here, '..', 'bin'), join(main, 'bin'), { recursive: true });
    cpSync(join(here, '..', 'lib'), join(main, 'lib'), { recursive: true });
    writeFileSync(join(main, 'package.json'), JSON.stringify({ name: 'k-lani-link', type: 'module' }));
    const target = packageFor(process.platform, process.arch);
    if (!target) return;
    const bin = join(root, 'node_modules', ...target.name.split('/'), 'bin');
    mkdirSync(bin, { recursive: true });
    writeFileSync(join(root, 'node_modules', ...target.name.split('/'), 'package.json'), '{}');
    const fake = join(bin, target.binary);
    writeFileSync(fake, '#!/bin/sh\nfor a in "$@"; do printf "[%s]" "$a"; done\nexit 3\n');
    chmodSync(fake, 0o755);
    const result = spawnSync(process.execPath, [join(main, 'bin', 'k-lani-link.js'), 'a b', '--x=1', ''], {
      encoding: 'utf8',
    });
    assert.equal(result.stdout, '[a b][--x=1][]');
    assert.equal(result.stderr, '');
    assert.equal(result.status, 3);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('the starter is valid syntax for Node 18 ESM', () => {
  execFileSync(process.execPath, ['--check', join(here, '..', 'bin', 'k-lani-link.js')]);
});
