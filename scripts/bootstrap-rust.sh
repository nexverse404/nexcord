#!/usr/bin/env bash
# Bootstrap a native Rust toolchain from npm tarballs.
#
# Useful when rustup / static.rust-lang.org are unreachable but the npm registry
# is (CI sandboxes, air-gapped proxies, locked-down networks). The official
# rust-installer component tarballs are published to npm under @toolchain-bin.
#
# Usage:  ./scripts/bootstrap-rust.sh [PREFIX]        # default: ~/.rust-npm
# Then:   export PATH="$PREFIX/bin:$PATH"
#         export LD_LIBRARY_PATH="$PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
#
set -euo pipefail

PREFIX="${1:-$HOME/.rust-npm}"
TRIPLE="x86_64-unknown-linux-gnu"
BASE="https://registry.npmjs.org/@toolchain-bin"

# name | published npm version | sha256 of the .tgz
# Checksums are pinned: a compromised or swapped tarball must never become our
# compiler. npm tarballs are immutable, so these do not drift.
declare -a PKGS=(
  "rustc-1.99.0-$TRIPLE|1.99.0|130fcfe7fc2bc95fb097b0c5f58888a1ef7d1d301385d1ad3c203ae1ef745b70"
  "cargo-1.99.0-$TRIPLE|0.100.0|6d600e3e511043adacd0ad1315e293cec58d95787fda59bea0c5e8a333c723bc"
  "rust-std-1.99.0-$TRIPLE|1.99.0-1|e148f60bd8ef105bcc070e112de3ad0519256968cf591dfce3bfe066152080f6"
  "rustfmt-1.99.0-$TRIPLE|1.10.0|b4fbae67150d6843315ed47fbad4f4b058e27e6fac43b67af41d8710caafdab3"
  "clippy-1.99.0-$TRIPLE|0.1.99|04be334fe3814e35e1fb0237eaa6057ee6972df0397404cb82fffa17fb23bb48"
)

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$PREFIX"

i=0
for entry in "${PKGS[@]}"; do
  i=$((i + 1))
  IFS='|' read -r name ver sha <<<"$entry"
  url="$BASE/$name/-/$name-$ver.tgz"
  echo "[$i/${#PKGS[@]}] $name"
  curl --fail --silent --show-error --location \
       --connect-timeout 15 --max-time 300 --retry 3 --retry-delay 1 \
       "$url" -o "$TMP/$name.tgz"
  echo "$sha  $TMP/$name.tgz" | sha256sum --check --status || {
    echo "checksum mismatch for $name — refusing to install" >&2
    exit 1
  }
  mkdir -p "$TMP/x$i"
  tar xzf "$TMP/$name.tgz" -C "$TMP/x$i"
done

# Each tarball contains one rust-installer component directory.
for d in \
  "$TMP/x1/package/rustc" \
  "$TMP/x2/package/cargo" \
  "$TMP/x3/package/rust-std-$TRIPLE" \
  "$TMP/x4/package/rustfmt-preview" \
  "$TMP/x5/package/clippy-preview"; do
  cp -a "$d/." "$PREFIX/"
done

# npm tarballs strip the executable bits.
chmod +x "$PREFIX"/bin/* "$PREFIX"/libexec/* \
  "$PREFIX/lib/rustlib/$TRIPLE/bin/"* \
  "$PREFIX/lib/rustlib/$TRIPLE/bin/gcc-ld/"* 2>/dev/null || true

# Fail loudly on a partial/broken install instead of leaving a half prefix.
LD_LIBRARY_PATH="$PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
  "$PREFIX/bin/rustc" --version >/dev/null
LD_LIBRARY_PATH="$PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
  "$PREFIX/bin/cargo" --version >/dev/null

echo
echo "Rust installed to $PREFIX"
"$PREFIX/bin/rustc" --version
"$PREFIX/bin/cargo" --version
echo
echo "Add to your shell:"
echo "  export PATH=\"$PREFIX/bin:\$PATH\""
echo "  export LD_LIBRARY_PATH=\"$PREFIX/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}\""
