#!/usr/bin/env bash
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
target="$(rustc -vV | sed -n 's/^host: //p')"
bundle="orleans-$version-$target"
dist="$root/dist"
archive="$bundle.tar.gz"

if [[ -z "$version" || -z "$target" ]]; then
    echo "could not determine version or target" >&2
    exit 1
fi

cargo build --release --locked

rm -rf "$dist/$bundle"
mkdir -p "$dist/$bundle/docs" "$dist/$bundle/examples"

cp target/release/orleans "$dist/$bundle/"
cp README.md CHANGELOG.md LICENSE-MIT LICENSE-APACHE "$dist/$bundle/"
cp docs/*.md "$dist/$bundle/docs/"
cp examples/coaster.svg "$dist/$bundle/examples/"

tar -czf "$dist/$archive" -C "$dist" "$bundle"
(
    cd "$dist"
    sha256sum "$archive" > "$archive.sha256"
)

echo "created dist/$archive"
echo "created dist/$archive.sha256"
