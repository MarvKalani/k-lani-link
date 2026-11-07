#!/usr/bin/env bash
# Packs a published GitHub release of k-lani-link as npm packages, without building anything
# and without publishing:
#
#   scripts/npm-pack.sh v1.2611.4
#
# Downloads the release's binaries, checks SHA256SUMS against its Ed25519 signature (the
# updater's public key in src/update.rs) and every file against SHA256SUMS, then writes one
# package per platform (@k-lani/link-<os>-<cpu>) and the main package k-lani-link to
# .artifacts/npm/<version>/ with a SHA256SUMS of the archives. Stops on the first failed check.
# A version folder that already exists is never touched: npm never accepts a version twice.
set -euo pipefail

REPOSITORY=MarvKalani/k-lani-link
die() { printf 'npm-pack: %s\n' "$*" >&2; exit 1; }

[[ $# -eq 1 ]] || die "usage: scripts/npm-pack.sh <release-tag, e.g. v1.2611.4>"
tag=$1
[[ $tag =~ ^v([0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?)$ ]] || die "not a release tag: $tag"
version=${BASH_REMATCH[1]}

for tool in gh git node npm openssl base64 xxd; do
  command -v "$tool" >/dev/null || die "missing tool: $tool"
done
if command -v sha256sum >/dev/null; then sha() { sha256sum "$@"; }; else sha() { shasum -a 256 "$@"; }; fi

root=$(git rev-parse --show-toplevel)
source=$root/npm/k-lani-link
out=$root/.artifacts/npm/$version
[[ ! -e $out ]] || die "$out already exists; a packed version is never packed again"

[[ $(gh release view "$tag" -R "$REPOSITORY" --json isDraft --jq .isDraft) == false ]] \
  || die "$tag is a draft, not a published release"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
release=$work/release
mkdir "$release"

# Platform table from the starter itself: node platform, arch, package name, release suffix.
mapfile -t platforms < <(node --input-type=module -e "
  const { PLATFORMS, binaryName } = await import(process.argv[1]);
  for (const p of PLATFORMS) console.log([p.platform, p.arch, p.name, p.release, binaryName(p.platform)].join('\t'));
" "$source/lib/platform.js")
[[ ${#platforms[@]} -gt 0 ]] || die "no platforms in npm/k-lani-link/lib/platform.js"

patterns=(-p SHA256SUMS -p SHA256SUMS.sig -p LICENSE.txt -p RUST-LICENSES.txt)
for row in "${platforms[@]}"; do
  IFS=$'\t' read -r _ _ _ suffix _ <<<"$row"
  patterns+=(-p "k-lani-link-$version-$suffix")
done
gh release download "$tag" -R "$REPOSITORY" -D "$release" "${patterns[@]}"
for ((i = 1; i < ${#patterns[@]}; i += 2)); do
  [[ -f $release/${patterns[i]} ]] || die "${patterns[i]} did not arrive from $tag"
done

# 1. The checksum list carries the release key's signature.
key=$(sed -n 's/^const PUBLIC_KEY: &str = "\(.*\)";$/\1/p' "$root/src/update.rs")
[[ -n $key ]] || die "no PUBLIC_KEY in src/update.rs"
{ printf '302a300506032b6570032100' | xxd -r -p; printf '%s' "$key" | base64 -d; } >"$work/key.der"
openssl pkey -pubin -inform DER -in "$work/key.der" -out "$work/key.pem" 2>/dev/null \
  || die "the release public key is not an Ed25519 key"
base64 -d <"$release/SHA256SUMS.sig" >"$work/sums.sig" || die "SHA256SUMS.sig is not base64"
openssl pkeyutl -verify -pubin -inkey "$work/key.pem" -rawin -in "$release/SHA256SUMS" \
  -sigfile "$work/sums.sig" >/dev/null 2>&1 || die "SHA256SUMS.sig does not match SHA256SUMS: stopped"

# 2. Every downloaded file has exactly one matching line in the signed list.
(
  cd "$release"
  for file in *; do
    [[ $file == SHA256SUMS || $file == SHA256SUMS.sig ]] && continue
    lines=$(awk -v f="$file" '$2 == f || $2 == "*" f' SHA256SUMS | wc -l)
    [[ $lines -eq 1 ]] || die "$file is not listed once in SHA256SUMS"
    awk -v f="$file" '$2 == f || $2 == "*" f' SHA256SUMS | sha -c --status - \
      || die "$file does not match SHA256SUMS: stopped"
  done
)
echo "Release $tag: signature and checksums verified."

git -C "$root" show "$tag:LICENSING.md" >"$work/LICENSING.md" 2>/dev/null \
  || die "LICENSING.md is missing at $tag"

licenses() {
  cp "$release/LICENSE.txt" "$release/RUST-LICENSES.txt" "$work/LICENSING.md" "$1/"
}

# package.json files: written by node, so the JSON is always valid.
manifest() {
  node -e "
    const [kind, version, name, os, cpu] = process.argv.slice(1);
    const common = {
      version,
      license: 'SEE LICENSE IN LICENSE.txt',
      homepage: 'https://github.com/MarvKalani/k-lani-link#readme',
      repository: { type: 'git', url: 'git+https://github.com/MarvKalani/k-lani-link.git' },
      bugs: { url: 'https://github.com/MarvKalani/k-lani-link/issues' },
    };
    const label = { linux: 'Linux', darwin: 'macOS', win32: 'Windows' }[os];
    const doc = kind === 'platform'
      ? { name, ...common, description: 'The k-lani-link program for ' + label + ' ' + cpu + '; install k-lani-link instead.',
          os: [os], cpu: [cpu], files: ['bin/', 'LICENSE.txt', 'RUST-LICENSES.txt', 'LICENSING.md'],
          publishConfig: { access: 'public' } }
      : { name: 'k-lani-link', ...common,
          description: 'Use the AI models on your own computers from anywhere, end-to-end encrypted.',
          keywords: ['ai', 'llm', 'local-ai', 'self-hosted', 'end-to-end-encryption', 'webrtc', 'openai-api'],
          type: 'module', bin: { 'k-lani-link': 'bin/k-lani-link.js' }, engines: { node: '>=18' },
          files: ['bin/', 'lib/', 'LICENSE.txt', 'RUST-LICENSES.txt', 'LICENSING.md'],
          optionalDependencies: Object.fromEntries(process.argv.slice(6).map((n) => [n, version])) };
    process.stdout.write(JSON.stringify(doc, null, 2) + '\n');
  " "$@"
}

stage=$work/stage
packed=$work/packed
mkdir "$stage" "$packed"
names=()
for row in "${platforms[@]}"; do
  IFS=$'\t' read -r os cpu name suffix binary <<<"$row"
  dir=$stage/${name#@k-lani/}
  mkdir -p "$dir/bin"
  cp "$release/k-lani-link-$version-$suffix" "$dir/bin/$binary"
  chmod 0755 "$dir/bin/$binary"
  licenses "$dir"
  printf '# %s\n\nInstall k-lani-link instead of this package: `npm install -g k-lani-link`\n' "$name" >"$dir/README.md"
  manifest platform "$version" "$name" "$os" "$cpu" >"$dir/package.json"
  names+=("$name")
done

main=$stage/k-lani-link
mkdir -p "$main"
cp -R "$source/bin" "$source/lib" "$source/README.md" "$main/"
chmod 0755 "$main/bin/k-lani-link.js"
licenses "$main"
manifest main "$version" "" "" "" "${names[@]}" >"$main/package.json"

for dir in "$stage"/*; do
  (cd "$dir" && npm pack --ignore-scripts --silent --pack-destination "$packed" >/dev/null)
done

# Only a complete set appears under .artifacts; mkdir fails if the folder came into being meanwhile.
mkdir -p "$(dirname "$out")"
mkdir "$out" || die "$out already exists; a packed version is never packed again"
mv "$packed"/*.tgz "$out/"
(cd "$out" && sha *.tgz >SHA256SUMS)

echo "Packed into $out:"
(cd "$out" && for file in *.tgz; do printf '  %-40s %9s bytes\n' "$file" "$(wc -c <"$file" | tr -d ' ')"; done)
first=$(cd "$out" && ls *.tgz | grep -v "^k-lani-link-$version.tgz$")
publish="cd .artifacts/npm/$version"
for file in $first; do publish+=" && npm publish ./$file --tag latest"; done
publish+=" && npm publish ./k-lani-link-$version.tgz --tag latest"
echo "Publish (platform packages first, the main package last):"
echo "  $publish"
