# cinnamon-docker

Panel de escritorio Linux para el motor Docker, el stack compose de Cinnamon y
todos los contenedores del daemon, sin terminal.

[![CI](https://github.com/Canopus44/cinnamon-docker/actions/workflows/ci.yml/badge.svg)](https://github.com/Canopus44/cinnamon-docker/actions/workflows/ci.yml)
[![Licencia: MIT](https://img.shields.io/badge/license-MIT-46C2A3.svg)](LICENSE)

![Captura de cinnamon-docker](docs/screenshot.png)

## Qué es

Una app nativa en Rust (`egui`/`eframe`, renderer `glow`, X11) que muestra el
estado del motor y de cada contenedor, permite arrancar/parar servicios en
orden y sigue el consumo de CPU/RAM. Un solo binario, sin tokio ni async, sin
daemon propio ni configuración en runtime.

## Características

- **Motor Docker**: Iniciar/Detener vía
  `pkexec /usr/bin/systemctl {start|stop} docker.socket docker`. Detener pide
  confirmación porque apaga **todos** los contenedores de **todos** los
  proyectos. Si no hay `pkexec`, intenta `systemctl` directo.
- **Stack Cinnamon**: 8 servicios fijos (chat-postgres, erp-postgres, mongo,
  pgadmin, chat-backend, chat-frontend, erp-backend, erp-frontend).
  *Levantar todo (serie)* hace `docker compose up -d` servicio a servicio y
  espera `healthy` (timeout 900 s): dos `ng serve` en paralelo agotan la RAM y
  mueren con exit 137.
- **Todos los contenedores del daemon**, agrupados por proyecto compose: Cinnamon
  primero, el resto alfabético y `Sin proyecto` al final. Estado y salud como
  pills; acciones Iniciar/Detener/Reiniciar/Logs por contenedor y por grupo.
- **CPU/RAM** por contenedor (con barras) y totales en el header, más `n/8`
  servicios arriba de Cinnamon.
- **Log lateral** redimensionable, con Copiar, Limpiar y auto-scroll.
- **CLI** para scripts: `--status`, `--containers`, `--help`.
- Tema oscuro, sin dependencias de red y sin telemetría.
- **No borra contenedores ni volúmenes.**

## Requisitos

- Linux con X11 o Wayland (la app corre por XWayland).
- Docker en marcha o instalado como servicio systemd; usuario en el grupo
  `docker` para el CLI.
- `polkit`/`pkexec` para el botón de motor (sin él, `systemctl` directo).
- Rust stable para compilar.

Dependencias de build:

```bash
# Arch
sudo pacman -S --needed base-devel pkgconf libxkbcommon libx11 libxi \
  libxcursor libxrandr mesa fontconfig

# Debian / Ubuntu
sudo apt install build-essential pkg-config libxkbcommon-dev libx11-dev \
  libxi-dev libxcursor-dev libxrandr-dev libgl1-mesa-dev libfontconfig1-dev
```

## Compilar e instalar

```bash
cargo build --release -j 2
install -Dm755 target/release/cinnamon-docker ~/.local/bin/cinnamon-docker
```

Lanzador de escritorio (`~/.local/share/applications/cinnamon-docker.desktop`):

```ini
[Desktop Entry]
Type=Application
Name=Docker Cinnamon
Comment=Panel del motor Docker y del stack Cinnamon
Exec=%h/.local/bin/cinnamon-docker
Icon=docker
Terminal=false
Categories=Development;System;
```

## Uso

GUI: abre la ventana, mira el header (motor, socket, versión, totales) y las
tarjetas. *Levantar todo (serie)* arranca el stack; *Apagar todo (cinnamon)* lo
para; cada tarjeta tiene sus propias acciones.

CLI (sale con 0):

| Comando | Salida |
|---|---|
| `cinnamon-docker` | Ventana. |
| `cinnamon-docker --status` | Motor, socket, versión y tabla de los 8 servicios Cinnamon (estado, salud, detalle). |
| `cinnamon-docker --containers` | Todos los contenedores con proyecto, nombre, estado, salud, imagen y detalle. |
| `cinnamon-docker --help` | Ayuda y ruta del compose. |

Con el motor apagado la CLI no consulta el daemon (el socket lo reactivaría):
muestra el estado del motor y los servicios como `detenido`.

## Configuración

No hay archivo de configuración: son constantes de `src/main.rs`.

| Constante | Valor por defecto | Para qué |
|---|---|---|
| `COMPOSE_FILE` | `/mnt/disco-local/home/Github/Cinnamon/docker-compose.yml` | Compose del stack Cinnamon. **Ruta del autor**: edítala y recompila para otro proyecto. |
| `SERVICES` | los 8 servicios Cinnamon | Orden de arranque serie y columna del CLI. |
| `REFRESH_EVERY` | 2 s | `docker ps -a` y estado del motor. |
| `STATS_EVERY` | 5 s | `docker stats --no-stream`. |
| `WAIT_TIMEOUT` | 900 s | Espera de `healthy` por servicio. |

## Cómo funciona

UI en el hilo principal, refresco y stats en hilos propios, un hilo por acción,
comunicados por `mpsc`. Con el motor apagado no se toca el daemon y al detener
se para antes `docker.socket`. Detalles, invariantes y puntos de extensión en
[docs/architecture.md](docs/architecture.md). Problemas típicos en
[docs/troubleshooting.md](docs/troubleshooting.md).

## Limitaciones y seguridad

- **No borra nada**: sólo `start`/`stop`/`restart`/`logs`/`ps`/`stats` y
  `compose up -d`. No hay `rm`, `down -v` ni borrado de volúmenes.
- **Detener el motor apaga todos los contenedores** de todos los proyectos;
  por eso hay modal de confirmación.
- **La ruta del compose está fija** y es la del autor. Cambiarla exige editar y
  recompilar.
- **Sin daemon remoto**: sólo el Docker local (`DOCKER_HOST` no se usa).
- **`pkexec`** pide autenticación en cada arranque/parada del motor; el
  resultado y el comando exacto se muestran en el banner y el log.
- Requiere sesión gráfica X11 o XWayland; no hay modo headless (usa la CLI).

## Contribuir

Lee [CONTRIBUTING.md](CONTRIBUTING.md). Checks obligatorios: `cargo fmt
--check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
`cargo build --release`.

## Licencia

MIT © 2026 Santiago Molina. Ver [LICENSE](LICENSE).
