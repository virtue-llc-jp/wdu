#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd -- "$script_directory/.." && pwd)"
cd "$repository_root"

version="${WDU_VERSION:-$(awk -F'"' '/^version = / { print $2 }' Cargo.toml)}"
release_base_url="${WDU_RELEASE_BASE_URL:-https://github.com/virtue-llc-jp/wdu/releases/download/v${version}}"
arm_sha256="${WDU_ARM_SHA256:-}"
intel_sha256="${WDU_INTEL_SHA256:-}"
output_path="${1:-$repository_root/packaging/homebrew/wdu.rb}"

if [[ -z "$version" || -z "$arm_sha256" || -z "$intel_sha256" ]]; then
    printf '%s\n' "WDU_ARM_SHA256 and WDU_INTEL_SHA256 are required" >&2
    exit 1
fi

template_path="$repository_root/packaging/homebrew/wdu.rb.in"
arm_url="$release_base_url/wdu-${version}-aarch64-apple-darwin.tar.gz"
intel_url="$release_base_url/wdu-${version}-x86_64-apple-darwin.tar.gz"

mkdir -p "$(dirname -- "$output_path")"
sed \
    -e "s|@VERSION@|$version|g" \
    -e "s|@ARM_URL@|$arm_url|g" \
    -e "s|@ARM_SHA256@|$arm_sha256|g" \
    -e "s|@INTEL_URL@|$intel_url|g" \
    -e "s|@INTEL_SHA256@|$intel_sha256|g" \
    "$template_path" > "$output_path"

printf 'rendered %s\n' "$output_path"