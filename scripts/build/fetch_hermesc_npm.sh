#!/usr/bin/env bash
# Add the modern Hermes compilers (HBC 97 and above) to the corpus toolchains.
#
# facebook/hermes stopped publishing CLI release assets after v0.13.0 (HBC 96).
# Since then the compiler React Native uses ships on npm as `hermes-compiler`,
# with prebuilt hermesc binaries for macOS, Linux and Windows in one package.
# This fetches the given versions from the npm registry, checks each tarball
# against the registry's sha1, probes the HBC version it emits, and appends it
# to the same manifest fetch_hermesc.sh writes. Run it after fetch_hermesc.sh,
# which starts a fresh manifest.
#
# Usage: bash scripts/build/fetch_hermesc_npm.sh [version ...]
#   default: 250829098.0.16 (HBC 98) 260318099.0.4 (HBC 99)
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TOOLDIR="$ROOT/examples/react-native/.toolchains"
MANIFEST="$TOOLDIR/manifest.tsv"
REGISTRY="https://registry.npmjs.org/hermes-compiler"
mkdir -p "$TOOLDIR"
touch "$MANIFEST"

[ "$#" -eq 0 ] && set -- 250829098.0.16 260318099.0.4

case "$(uname -s)" in
  Darwin) BIN="osx-bin/hermesc" ;;
  Linux) BIN="linux64-bin/hermesc" ;;
  MINGW* | MSYS* | CYGWIN*) BIN="win64-bin/hermesc.exe" ;;
  *) echo "unsupported host: $(uname -s)" >&2; exit 1 ;;
esac

PROBE="$(mktemp -d)/probe.js"
printf 'var x = 1 + 2; print(x);\n' > "$PROBE"

status=0
for ver in "$@"; do
  tag="hermes-v$ver"
  dest="$TOOLDIR/$tag"
  if [ ! -d "$dest" ]; then
    echo "Downloading hermes-compiler@$ver ..."
    meta="$(curl -fsSL "$REGISTRY/$ver")" || { echo "  WARN: no such version $ver" >&2; status=1; continue; }
    want="$(printf '%s' "$meta" | python3 -I -c 'import sys,json;print(json.load(sys.stdin)["dist"]["shasum"])')"
    tgz="$(mktemp)"
    if ! curl -fsSL "$REGISTRY/-/hermes-compiler-$ver.tgz" -o "$tgz"; then
      echo "  WARN: download failed for $ver" >&2; rm -f "$tgz"; status=1; continue
    fi
    got="$(shasum "$tgz" 2>/dev/null || sha1sum "$tgz")"
    if [ "${got%% *}" != "$want" ]; then
      echo "  WARN: sha1 mismatch for $ver" >&2; rm -f "$tgz"; status=1; continue
    fi
    mkdir -p "$dest"
    tar xzf "$tgz" -C "$dest" "package/hermesc" || {
      echo "  WARN: extract failed for $ver" >&2; rm -rf "$dest" "$tgz"; status=1; continue
    }
    rm -f "$tgz"
  fi

  compiler="$dest/package/hermesc/$BIN"
  [ -f "$compiler" ] || { echo "  WARN: $BIN missing in $ver" >&2; status=1; continue; }
  chmod +x "$compiler" 2>/dev/null

  out="$(mktemp).hbc"
  if ! "$compiler" -emit-binary -out "$out" "$PROBE" >/dev/null 2>&1; then
    echo "  WARN: $ver failed to compile the probe" >&2; status=1; continue
  fi
  hbc="$(python3 -I -c 'import sys,struct;print(struct.unpack("<I",open(sys.argv[1],"rb").read()[8:12])[0])' "$out")"
  rm -f "$out"
  echo "  $tag -> HBC $hbc"
  # Replace any earlier line for this tag, then keep the manifest sorted.
  grep -v -F "$(printf '\t%s\t' "$tag")" "$MANIFEST" > "$MANIFEST.tmp"
  printf '%s\t%s\t%s\n' "$hbc" "$tag" "$compiler" >> "$MANIFEST.tmp"
  sort -t$'\t' -k1,1n -k2,2V "$MANIFEST.tmp" -o "$MANIFEST"
  rm -f "$MANIFEST.tmp"
done

echo "Manifest: $MANIFEST"
exit "$status"
