#!/usr/bin/env bash
# Builds the distribution corpus (D-022) from container images: for each image
# in the list, the release files sys-lib reads are copied out of an unstarted
# container (`docker create` + `docker cp`, so images without a shell work),
# and the expected answers are what a shell reads from os-release, the reader
# os-release(5) was written for.
#
# Usage: capture_distros.sh <image-list> <out-dir>
# Runs in CI (the Fixtures workflow); the output is reviewed before commit.
set -uo pipefail

list="$1"
out="$2"
mkdir -p "$out"

# Everything the Linux backend may read for OS identity (src/parse/release.rs),
# plus system-release for the record.
files=(
  etc/os-release usr/lib/os-release etc/lsb-release
  etc/centos-release etc/rocky-release etc/almalinux-release etc/fedora-release
  etc/oracle-release etc/redhat-release etc/system-release etc/SuSE-release
  etc/alpine-release etc/arch-release etc/gentoo-release etc/slackware-version
  etc/debian_version
)

ok=0
skipped=()
while IFS= read -r image; do
  case "$image" in ''|'#'*) continue ;; esac
  slug=$(printf '%s' "$image" | tr '/:@' '---' | tr -cd 'A-Za-z0-9._-')
  dir="$out/$slug"
  if ! docker pull --quiet "$image" >/dev/null 2>&1; then
    skipped+=("$image (pull failed)")
    continue
  fi
  id=$(docker create "$image" /none 2>/dev/null) || { skipped+=("$image (create failed)"); continue; }
  mkdir -p "$dir"
  for f in "${files[@]}"; do
    mkdir -p "$dir/$(dirname "$f")"
    # -L follows the usual /etc/os-release -> ../usr/lib/os-release link.
    docker cp -L "$id:/$f" "$dir/$f" >/dev/null 2>&1 || true
    # A copied directory or an empty leftover is not a release file.
    if [ -d "$dir/$f" ]; then rm -rf -- "${dir:?}/$f"; fi
  done
  docker rm "$id" >/dev/null 2>&1
  find "$dir" -type d -empty -delete

  digest=$(docker image inspect --format '{{index .RepoDigests 0}}' "$image" 2>/dev/null || echo unknown)
  release=""
  for f in etc/os-release usr/lib/os-release; do
    if [ -f "$dir/$f" ]; then release="$dir/$f"; break; fi
  done
  {
    printf 'IMAGE=%s\n' "$image"
    printf 'DIGEST=%s\n' "$digest"
    if [ -n "$release" ]; then
      # A clean shell with nothing but the file sourced into it.
      env -i bash --noprofile --norc -c '
        . "$1" >/dev/null 2>&1
        printf "ID=%s\nNAME=%s\nPRETTY_NAME=%s\nVERSION_ID=%s\nID_LIKE=%s\nVERSION_CODENAME=%s\n" \
          "$ID" "$NAME" "$PRETTY_NAME" "$VERSION_ID" "$ID_LIKE" "$VERSION_CODENAME"
      ' _ "$release"
    else
      printf 'LEGACY=1\n'
    fi
  } > "$dir/EXPECTED.txt"
  ok=$((ok + 1))
  echo "captured $image -> $slug"
done < "$list"

echo
echo "captured: $ok"
for s in "${skipped[@]}"; do echo "skipped: $s"; done
