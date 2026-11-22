#!/bin/sh
# Minimal Spore v0 lock/pack driver.
#
#   scripts/spore-pack.sh lock DIR
#   scripts/spore-pack.sh sign DIR --key KEY
#   scripts/spore-pack.sh pack DIR [-o FILE.spore]
#
# `sign` writes an embedded signatures/<keyid>.sig record over spore.lock.
# `pack` includes signatures/ and excludes it from the lock.
set -eu

cmd=${1:-}
shift || true

usage() {
  echo "usage: spore-pack.sh lock DIR | sign DIR --key KEY | pack DIR [-o FILE.spore]" >&2
  exit 2
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

manifest_field() {
  file=$1
  name=$2
  sed -n "s/^[[:space:]]*($name \"\\(.*\\)\").*/\\1/p" "$file" | head -1
}

lock_dir() {
  dir=$1
  manifest="$dir/spore.soil"
  [ -f "$manifest" ] || { echo "spore-pack: missing $manifest" >&2; exit 1; }
  id=$(manifest_field "$manifest" id)
  version=$(manifest_field "$manifest" version)
  [ -n "$id" ] || { echo "spore-pack: spore.soil has no id" >&2; exit 1; }
  [ -n "$version" ] || { echo "spore-pack: spore.soil has no version" >&2; exit 1; }

  dir_abs=$(CDPATH= cd "$dir" && pwd)
  lock="$dir/spore.lock"
  tmp="$lock.tmp"
  {
    echo "spore-lock 1"
    echo "kind spore"
    echo "id $id"
    echo "version $version"
    (cd "$dir" && find . -type f \
      ! -path './spore.lock' ! -path './spore.lock.tmp' \
      ! -path './signatures/*' ! -name '*.spore' -print | sed 's|^\./||' | LC_ALL=C sort |
      while IFS= read -r path; do
        printf 'file %s %s %s\n' "$(sha256 "$dir_abs/$path")" "$(wc -c < "$dir_abs/$path" | tr -d ' ')" "$path"
      done)
  } > "$tmp"
  digest=$(sha256 "$tmp")
  {
    cat "$tmp"
    echo "digest $digest"
  } > "$lock"
  rm -f "$tmp"
  echo "$lock"
}

sign_dir() {
  dir=$1
  key=$2
  command -v gpg >/dev/null 2>&1 || { echo "spore-pack: gpg is required to sign" >&2; exit 1; }
  lock_dir "$dir" >/dev/null
  lock="$dir/spore.lock"
  keyid=$(gpg --list-keys --with-colons "$key" 2>/dev/null | awk -F: '/^pub/{print $5; exit}')
  [ -n "$keyid" ] || { echo "spore-pack: unknown key $key" >&2; exit 1; }
  mkdir -p "$dir/signatures"
  sig="$dir/signatures/$keyid.sig"
  armor=$(mktemp "${TMPDIR:-/tmp}/spore-sig.XXXXXX")
  trap 'rm -f "$armor"' EXIT HUP INT TERM
  gpg --armor --detach-sign --local-user "$key" --output "$armor" "$lock"
  {
    echo "spore-signature 1"
    echo "key $keyid"
    echo "digest $(sha256 "$lock")"
    cat "$armor"
  } > "$sig"
  echo "$sig"
}

pack_dir() {
  dir=$1
  out=$2
  lock_dir "$dir" >/dev/null
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/spore-pack.XXXXXX")
  trap 'rm -rf "$tmp"' EXIT HUP INT TERM
  mkdir -p "$tmp/payload"
  (cd "$dir" && find . -type f ! -name '*.spore' -print | while IFS= read -r path; do
    dest="$tmp/payload/$path"
    mkdir -p "$(dirname "$dest")"
    cp -p "$path" "$dest"
  done)
  find "$tmp/payload" -type f -exec touch -t 197001010000.00 {} +
  mkdir -p "$(dirname "$out")"
  case "$out" in
    /*) ;;
    *) out=$(pwd)/$out ;;
  esac
  (cd "$tmp/payload" && find . -type f -print | LC_ALL=C sort |
    tar --uid 0 --gid 0 --uname root --gname wheel -cf "$out" -T -)
  echo "$out"
}

case "$cmd" in
  lock) [ "$#" -eq 1 ] || usage; lock_dir "$1" ;;
  sign)
    [ "$#" -eq 3 ] && [ "$2" = "--key" ] || usage
    sign_dir "$1" "$3"
    ;;
  pack)
    [ "$#" -ge 1 ] || usage
    dir=$1
    shift
    out="$dir/$(manifest_field "$dir/spore.soil" id | tr ':/' '--').spore"
    if [ "$#" -gt 0 ]; then
      [ "$1" = "-o" ] && [ "$#" -eq 2 ] || usage
      out=$2
    fi
    pack_dir "$dir" "$out"
    ;;
  *) usage ;;
esac
