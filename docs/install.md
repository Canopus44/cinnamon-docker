# Instalación paso a paso

Guía completa de instalación de Docker Cinnamon en Linux. Resumen rápido en
[README.md](../README.md#instalación-rápida).

## 1. Requisitos y verificación

Necesitas Linux con X11 o Wayland (XWayland), Rust stable y Docker con el
plugin Compose v2. Comprueba el entorno:

```bash
docker --version
docker compose version
pkexec --version
systemctl is-active docker
```

Si `docker compose version` falla, tu distro no trae el plugin v2: instala el
plugin oficial de Docker. La app ejecuta `docker compose`, no `docker-compose`.

Dependencias de build:

```bash
# Arch
sudo pacman -S --needed base-devel rust docker docker-compose polkit \
  libxkbcommon libx11 libxi libxcursor libxrandr mesa fontconfig

# Debian / Ubuntu
sudo apt install build-essential pkg-config libxkbcommon-dev libx11-dev \
  libxi-dev libxcursor-dev libxrandr-dev libgl1-mesa-dev libfontconfig1-dev \
  docker.io docker-compose-v2 policykit-1
```

## 2. Clonar y compilar

```bash
git clone https://github.com/Canopus44/cinnamon-docker.git
cd cinnamon-docker
cargo build --release -j 2
```

El binario queda en `target/release/cinnamon-docker`.

## 3. Instalar

```bash
./install.sh
```

El instalador no necesita root y deja, por defecto, bajo `~/.local`:

| Archivo | Destino |
|---|---|
| binario | `~/.local/bin/cinnamon-docker` |
| icono | `~/.local/share/icons/hicolor/scalable/apps/cinnamon-docker.svg` |
| icono simbólico | `~/.local/share/icons/hicolor/scalable/apps/cinnamon-docker-symbolic.svg` |
| lanzador | `~/.local/share/applications/cinnamon-docker.desktop` |

El `.desktop` se genera con `Exec` absoluto apuntando al binario instalado, y
el instalador refresca `gtk-update-icon-cache` y `update-desktop-database` si
existen.

Para instalar en sistema en vez de en el usuario:

```bash
sudo env PREFIX=/usr/local ./install.sh
```

`PREFIX` cambia la raíz de todo lo anterior (`$PREFIX/bin`,
`$PREFIX/share/...`).

## 4. PATH y lanzador

Asegura que el directorio de binarios esté en el `PATH` (añádelo a tu shell:
`~/.bashrc`, `~/.zshrc`, etc.):

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Comprobaciones opcionales del lanzador:

```bash
desktop-file-validate ~/.local/share/applications/cinnamon-docker.desktop
gtk-update-icon-cache -q -t ~/.local/share/icons/hicolor
update-desktop-database ~/.local/share/applications
```

## 5. Primer arranque

```bash
cinnamon-docker
```

- Elige tema claro/oscuro con el selector del header; queda guardado en
  `~/.config/cinnamon-docker/theme`.
- Cerrar la ventana la deja oculta en la bandeja; «Salir» en el menú del
  icono termina el proceso.
- En KDE o GNOME con AppIndicator puedes comprobar que la bandeja existe:

  ```bash
  busctl --user list | grep -i StatusNotifierWatcher
  ```

  Si no hay `StatusNotifierWatcher`, la ventana cierra con normalidad en vez
  de ocultarse.

## 6. Docker sin sudo

El CLI y las métricas hablan con el daemon por el socket de Docker; para no
usar `sudo`:

```bash
sudo usermod -aG docker "$USER"
newgrp docker    # aplica el grupo en la sesión actual
```

Cierra sesión y vuelve a entrar para que quede fijo.

El botón de motor usa `pkexec /usr/bin/systemctl start/stop docker.socket
docker` porque arrancar/parar el servicio es una operación de sistema que
requiere privilegios: por eso pide autenticación en cada arranque/parada. Con
el usuario en el grupo `docker`, el resto de acciones (ps, stats, start/stop
de contenedores, compose) no necesita privilegios.

## 7. Compose de Cinnamon

Por defecto la app usa la ruta del autor. Apunta la variable de entorno a tu
compose:

```bash
export CINNAMON_DOCKER_COMPOSE="$HOME/proyectos/Cinnamon/docker-compose.yml"
```

Hazlo persistente en `~/.config/environment.d/` o en la configuración de tu
shell. Verifica la ruta efectiva con:

```bash
cinnamon-docker --help
```

Se lee al arrancar; cambia la variable y vuelve a lanzar la app.

## 8. Autostart opcional

```bash
mkdir -p ~/.config/autostart
cp ~/.local/share/applications/cinnamon-docker.desktop ~/.config/autostart/
```

Nota: la app no tiene arranque automático propio; con este lanzador abre la
ventana al iniciar sesión, y cerrarla la deja en la bandeja.

## 9. Desinstalación

```bash
rm -f ~/.local/bin/cinnamon-docker
rm -f ~/.local/share/applications/cinnamon-docker.desktop
rm -f ~/.local/share/icons/hicolor/scalable/apps/cinnamon-docker.svg
rm -f ~/.local/share/icons/hicolor/scalable/apps/cinnamon-docker-symbolic.svg
rm -rf ~/.config/cinnamon-docker
```

Con `PREFIX=/usr/local` usa las mismas rutas bajo ese prefijo. Esto **no toca
Docker, contenedores ni volúmenes**: la app no los crea ni los borra, y el
desinstalador tampoco.

## 10. Problemas rápidos

Fallos de bandeja, motor, permisos o métricas: ver
[troubleshooting.md](troubleshooting.md). Arquitectura e invariantes en
[architecture.md](architecture.md).
