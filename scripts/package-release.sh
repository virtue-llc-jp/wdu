#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd -- "$script_directory/.." && pwd)"
cd "$repository_root"

version="${WDU_VERSION:-$(awk -F'"' '/^version = / { print $2; exit }' Cargo.toml)}"
target="${1:-$(rustc -vV | awk '/^host:/ { print $2 }')}"
dist_directory="${DIST_DIR:-$repository_root/dist}"

if [[ -z "$version" || -z "$target" ]]; then
    printf '%s\n' "failed to determine version or target" >&2
    exit 1
fi

archive_root="wdu-${version}-${target}"
archive_name="${archive_root}.tar.gz"
stage_directory="$(mktemp -d "${TMPDIR:-/tmp}/wdu-release.XXXXXX")"
trap 'rm -rf "$stage_directory"' EXIT

cargo build --release --locked --target "$target"

mkdir -p "$stage_directory/$archive_root/bin" \
    "$stage_directory/$archive_root/share/doc/wdu" \
    "$dist_directory"
install -m 0755 "target/$target/release/wdu" "$stage_directory/$archive_root/bin/wdu"
install -m 0755 "target/$target/release/wdu-daemon" \
    "$stage_directory/$archive_root/bin/wdu-daemon"
install -m 0644 README.md "$stage_directory/$archive_root/share/doc/wdu/README.md"
install -m 0644 docs/architecture.md "$stage_directory/$archive_root/share/doc/wdu/architecture.md"
install -m 0644 docs/data-model.md "$stage_directory/$archive_root/share/doc/wdu/data-model.md"
install -m 0644 docs/development.md "$stage_directory/$archive_root/share/doc/wdu/development.md"
install -m 0644 docs/homebrew.md "$stage_directory/$archive_root/share/doc/wdu/homebrew.md"

archive_path="$dist_directory/$archive_name"
tar -C "$stage_directory" -czf "$archive_path" "$archive_root"
shasum -a 256 "$archive_path" > "$archive_path.sha256"

printf 'created %s\n' "$archive_path"
printf 'created %s\n' "$archive_path.sha256"