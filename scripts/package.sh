#!/usr/bin/env bash
# Empaqueta un tarball portable con binario, assets, .desktop, install.sh, README y licencia.
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)"
name="cinnamon-docker-$version-x86_64-linux"
dist="$root/dist"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

cd "$root"
cargo build --release --locked -j 2

pkg="$stage/$name"
mkdir -p "$pkg/packaging"
install -Dm755 target/release/cinnamon-docker "$pkg/cinnamon-docker"
cp -a assets "$pkg/assets"
cp -a packaging/cinnamon-docker.desktop "$pkg/packaging/cinnamon-docker.desktop"
install -Dm755 install.sh "$pkg/install.sh"
cp -a README.md LICENSE "$pkg/"

mkdir -p "$dist"
tar -C "$stage" -czf "$dist/$name.tar.gz" "$name"

if command -v sha256sum >/dev/null 2>&1; then
    (cd "$dist" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
else
    (cd "$dist" && shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256")
fi

echo "Paquete: $dist/$name.tar.gz"
echo "Tamaño:  $(du -h "$dist/$name.tar.gz" | cut -f1)"
echo "SHA256:  $(cut -d' ' -f1 "$dist/$name.tar.gz.sha256")"
echo "Checksum: $dist/$name.tar.gz.sha256"
