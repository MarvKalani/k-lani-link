// Which npm package carries the k-lani-link binary for this computer, and under which name.
// scripts/npm-pack.sh reads the same table to build the packages, so they cannot drift apart.

/** Node's platform/arch → platform package and the release file it is built from. */
export const PLATFORMS = Object.freeze([
  { platform: 'linux', arch: 'x64', name: '@k-lani/link-linux-x64', release: 'linux-x64' },
  { platform: 'linux', arch: 'arm64', name: '@k-lani/link-linux-arm64', release: 'linux-arm64' },
  { platform: 'darwin', arch: 'x64', name: '@k-lani/link-darwin-x64', release: 'macos-x64' },
  { platform: 'darwin', arch: 'arm64', name: '@k-lani/link-darwin-arm64', release: 'macos-arm64' },
  { platform: 'win32', arch: 'x64', name: '@k-lani/link-win32-x64', release: 'windows-x64.exe' },
  { platform: 'win32', arch: 'arm64', name: '@k-lani/link-win32-arm64', release: 'windows-arm64.exe' },
]);

/** The executable's file name inside a platform package's bin/ folder. */
export function binaryName(platform) {
  return platform === 'win32' ? 'k-lani-link.exe' : 'k-lani-link';
}

/** The platform package for this platform and processor, or null when there is none. */
export function packageFor(platform, arch) {
  const entry = PLATFORMS.find((p) => p.platform === platform && p.arch === arch);
  return entry ? { name: entry.name, binary: binaryName(platform) } : null;
}

export function unsupportedMessage(platform, arch) {
  const supported = PLATFORMS.map((p) => `${p.platform}-${p.arch}`).join(', ');
  return (
    `k-lani-link has no build for ${platform}-${arch}. Supported: ${supported}.\n` +
    'Other systems can build it from source: https://github.com/MarvKalani/k-lani-link\n'
  );
}

export function missingMessage(name) {
  return (
    `The k-lani-link program for this computer (${name}) is not installed.\n` +
    'Install again without skipping optional packages: npm install -g k-lani-link\n'
  );
}
