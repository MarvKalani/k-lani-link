// Starts the platform binary in place of this script: same terminal, same arguments, its
// exit code and signals passed through. Only error messages are ours, and only on stderr.
import { spawn as nodeSpawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { missingMessage, packageFor, unsupportedMessage } from './platform.js';

const FORWARDED = ['SIGINT', 'SIGTERM', 'SIGHUP'];

/** The binary's path, or an error message; `resolve` is require.resolve or a stand-in. */
export function locate(platform, arch, resolve) {
  const target = packageFor(platform, arch);
  if (!target) return { error: unsupportedMessage(platform, arch) };
  try {
    return { path: resolve(`${target.name}/bin/${target.binary}`) };
  } catch {
    return { error: missingMessage(target.name) };
  }
}

/** Runs the binary; `exit(code)` or `raise(signal)` once it ends. */
export function run({
  platform = process.platform,
  arch = process.arch,
  argv = process.argv.slice(2),
  resolve = createRequire(import.meta.url).resolve,
  spawn = nodeSpawn,
  stderr = process.stderr,
  signals = process,
  exit = (code) => process.exit(code),
  raise = (signal) => process.kill(process.pid, signal),
} = {}) {
  const found = locate(platform, arch, resolve);
  if (found.error) {
    stderr.write(found.error);
    exit(1);
    return null;
  }
  const child = spawn(found.path, argv, { stdio: 'inherit' });
  const forward = (signal) => {
    try {
      child.kill(signal);
    } catch {
      // Windows cannot send every signal; the console reaches the program directly.
    }
  };
  for (const signal of FORWARDED) signals.on(signal, forward);
  child.on('error', () => {
    stderr.write(`k-lani-link could not be started from ${found.path}\n`);
    exit(1);
  });
  child.on('exit', (code, signal) => {
    for (const name of FORWARDED) signals.removeListener(name, forward);
    if (signal) raise(signal);
    else exit(code ?? 1);
  });
  return child;
}
