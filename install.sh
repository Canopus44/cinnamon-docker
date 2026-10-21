#!/usr/bin/env bash
# Instalador de usuario: copia binario, iconos y .desktop bajo $PREFIX.
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
if [[ -x "$root/target/release/cinnamon-docker" ]]; then
    bin="$root/target/release/cinnamon-docker"
elif [[ -x "$root/cinnamon-docker" ]]; then
    bin="$root/cinnamon-docker"
else
    echo "Compila primero: cargo build --release -j 2" >&2
    exit 1
fi

install -Dm755 "$bin" "$prefix/bin/cinnamon-docker"

icons="$prefix/share/icons/hicolor/scalable/apps"
install -Dm644 "$root/assets/icon.svg" "$icons/cinnamon-docker.svg"
install -Dm644 "$root/assets/icon-symbolic.svg" "$icons/cinnamon-docker-symbolic.svg"

apps="$prefix/share/applications"
tmp="$(mktemp)"
awk -v exec="$prefix/bin/cinnamon-docker" \
    '/^Exec=/{print "Exec=" exec; next} {print}' \
    "$root/packaging/cinnamon-docker.desktop" >"$tmp"
install -Dm644 "$tmp" "$apps/cinnamon-docker.desktop"
rm -f "$tmp"

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q "$apps" || true
fi

echo "Instalado en $prefix:"
echo "  $prefix/bin/cinnamon-docker"
echo "  $icons/cinnamon-docker.svg"
echo "  $icons/cinnamon-docker-symbolic.svg"
echo "  $apps/cinnamon-docker.desktop"
echo
echo "Recuerda que $prefix/bin debe estar en PATH."
