# Docker Cinnamon

<p align="center"><img src="assets/icon.svg" width="112" alt="Icono de Docker Cinnamon"></p>

Panel nativo Linux en Rust/egui para el motor Docker, el stack compose de
Cinnamon y **cualquier** contenedor o proyecto del daemon, sin terminal.

[![CI](https://github.com/Canopus44/cinnamon-docker/actions/workflows/ci.yml/badge.svg)](https://github.com/Canopus44/cinnamon-docker/actions/workflows/ci.yml)
[![Licencia: MIT](https://img.shields.io/badge/license-MIT-46C2A3.svg)](LICENSE)

| Tema claro | Tema oscuro |
|---|---|
| ![Captura clara](docs/screenshot.png) | ![Captura oscura](docs/screenshot-dark.png) |

## Características

- **Motor Docker**: Iniciar/Detener vía
  `pkexec /usr/bin/systemctl start/stop docker.socket docker`. Detener pide
  confirmación porque apaga **todos** los contenedores de **todos** los
  proyectos.
- **Selector de proyecto en el header**: `Levantar <proyecto>` /
  `Apagar <proyecto>` para **cualquier** proyecto compose o contenedores
  sueltos (`Sin proyecto`). Cinnamon conserva el arranque en serie con espera
  `healthy` (timeout 900 s: dos `ng serve` en paralelo agotan la RAM y mueren
  con exit 137); el resto de proyectos usan `docker start` secuencial.
- **Tarjetas por contenedor** con estado y salud, imagen, uptime, CPU/RAM y
  acciones Iniciar/Detener/Reiniciar/Logs y **Consola**.
- **Consola por contenedor**: ejecuta comandos dentro del contenedor con
  `docker exec <id|name> sh -lc <comando>` desde un modal, sin abrir terminal.
  Guarda las últimas 20 ejecuciones por contenedor y recorta la salida a 200
  líneas; Enter o «Ejecutar» lanzan el comando. Cada comando se corta a los
  60 s y la salida se limita a 1 MiB.
- **Resumen arriba**: CPU total, RAM total y contenedores; métricas cada 5 s
  por defecto (configurable en Ajustes).
- **Ajustes en caliente** (botón en el header): ruta del compose, refresco de
  contenedores (1–60 s), métricas (2–120 s) y efecto 3D. Se guardan en
  `~/.config/cinnamon-docker/settings.json`.
- **Bandeja SNI**: Abrir panel, Iniciar/Detener motor, Refrescar,
  Levantar/Apagar proyecto seleccionado, Salir. Cerrar la ventana la oculta en
  la bandeja; si no hay bandeja disponible, la ventana cierra con normalidad.
- **Tema claro/oscuro estilo iOS 2026** con selector en el header y efecto 3D
  sutil en botones, tarjetas y paneles (apagable en Ajustes); la preferencia de
  tema queda persistida.
- **Panel de actividad lateral redimensionable** con Copiar, Limpiar y
  auto-scroll.
- **CLI** para scripts: `--status`, `--containers`, `--help`.
- **No borra contenedores ni volúmenes**, sin telemetría, un solo binario.

## Requisitos

- Linux con X11 o Wayland (la app corre por XWayland).
- Docker Engine + plugin Compose v2.
- `polkit`/`pkexec` para el botón de motor.
- Rust stable para compilar.
- Usuario en el grupo `docker` para CLI y métricas.

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

> Nota: si tu distro no trae `docker compose` v2, instala el plugin oficial de
> Docker; la app ejecuta `docker compose`, no `docker-compose`.

## Instalación rápida

```bash
git clone https://github.com/Canopus44/cinnamon-docker.git
cd cinnamon-docker
cargo build --release -j 2
./install.sh
```

`install.sh` copia a `~/.local` el binario, `icon.svg`, `icon-symbolic.svg` y
un `.desktop` con `Exec` absoluto, y refresca los caches de iconos y escritorio
si existen. No necesita root. Respeta `PREFIX`:

```bash
sudo env PREFIX=/usr/local ./install.sh   # instalación en sistema
```

Asegura que `~/.local/bin` esté en el `PATH`:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Paso a paso, desinstalación y autostart en
[docs/install.md](docs/install.md).

## Paquete y releases

Los releases están en
[github.com/Canopus44/cinnamon-docker/releases](https://github.com/Canopus44/cinnamon-docker/releases).
El paquete portable `cinnamon-docker-<versión>-x86_64-linux.tar.gz` incluye el
binario, los iconos SVG, el `.desktop`, `install.sh`, el README y la licencia.

```bash
tar xzf cinnamon-docker-0.3.0-x86_64-linux.tar.gz
cd cinnamon-docker-0.3.0-x86_64-linux
./install.sh
```

Verifica el checksum desde el directorio que contiene ambos archivos:

```bash
sha256sum -c cinnamon-docker-0.3.0-x86_64-linux.tar.gz.sha256
```

## Uso

**GUI**: ejecuta `cinnamon-docker`, elige el proyecto en el header,
levántalo o apágalo con los botones y usa las acciones de cada tarjeta
(Iniciar/Detener/Reiniciar/Logs).

«Consola» abre un modal para ejecutar comandos dentro del contenedor (sólo si
está en marcha); «Ajustes» abre la configuración en caliente.

**Bandeja**: el clic izquierdo sobre el icono abre el panel; el menú ofrece
Abrir panel, Iniciar/Detener motor, Refrescar, Levantar/Apagar proyecto
seleccionado y Salir. «Salir» termina el proceso; cerrar la ventana sólo la
oculta.

**CLI** (sale con 0):

| Comando | Salida |
|---|---|
| `cinnamon-docker` | Ventana. |
| `cinnamon-docker --status` | Motor, socket, versión y tabla de los 8 servicios Cinnamon (estado, salud, detalle). |
| `cinnamon-docker --containers` | Todos los contenedores con proyecto, nombre, estado, salud, imagen y detalle. |
| `cinnamon-docker --help` | Ayuda y ruta efectiva del compose. |

Con el motor apagado la CLI no consulta el daemon (el socket lo
reactivaría): muestra el estado del motor y los servicios como `detenido`.

## Configuración

### Ajustes y `settings.json`

El botón **Ajustes** del header edita en caliente:

| Campo | Rango / defecto | Efecto |
|---|---|---|
| `compose` | ruta; vacío = entorno/default | Compose usado por `docker compose`. |
| `refresh_secs` | 1–60, defecto 2 | `docker ps -a` y estado del motor. |
| `stats_secs` | 2–120, defecto 5 | `docker stats --no-stream`. |
| `depth` | `true`/`false`, defecto `true` | Efecto 3D en botones, tarjetas y paneles. |

Se guardan en `$XDG_CONFIG_HOME/cinnamon-docker/settings.json` (por defecto
`~/.config/cinnamon-docker/settings.json`) al pulsar **Guardar cambios**.
Campos ausentes o inválidos usan el valor por defecto; los números se recortan
al rango. Si el archivo no se puede escribir, la app sigue funcionando pero no
recuerda los cambios.

La ruta del compose se resuelve así: `compose` de `settings.json` si no está
vacío; si no, `CINNAMON_DOCKER_COMPOSE`; si no, la ruta por defecto del autor.
Cambiar la ruta no reinicia contenedores.

### Tema

El selector del header guarda la preferencia en
`~/.config/cinnamon-docker/theme` (`light` o `dark`). `XDG_CONFIG_HOME` se
respeta.

### Constantes

| Constante | Valor | Para qué |
|---|---|---|
| `SERVICES` | los 8 servicios Cinnamon | Orden de arranque en serie y columna del CLI. |
| `DEFAULT_REFRESH_SECS` | 2 s | Valor inicial de `refresh_secs`; editable en Ajustes. |
| `DEFAULT_STATS_SECS` | 5 s | Valor inicial de `stats_secs`; editable en Ajustes. |
| `WAIT_TIMEOUT` | 900 s | Espera de `healthy` por servicio. |

## Cómo funciona

La UI corre en el hilo principal con `eframe`/`egui` 0.37 (renderer `glow`);
el refresco, las métricas y las acciones corren en hilos propios comunicados
por `mpsc`; la bandeja usa `ksni`. Detalles, invariantes y puntos de extensión
en [docs/architecture.md](docs/architecture.md). Problemas típicos en
[docs/troubleshooting.md](docs/troubleshooting.md).

## Limitaciones y seguridad

- **La app no borra nada**: sus acciones sólo hacen
  `start`/`stop`/`restart`/`logs`/`ps`/`stats` y `compose up -d`/`compose stop`.
  La consola ejecuta lo que tú escribas dentro del contenedor, así que ese
  riesgo es responsabilidad del comando.
- **Detener el motor apaga todos los contenedores** de todos los proyectos;
  por eso hay modal de confirmación.
- **Sin daemon remoto**: sólo el Docker local (`DOCKER_HOST` no se usa).
- **`pkexec`** pide autenticación en cada arranque/parada del motor.
- Requiere sesión gráfica X11 o XWayland; no hay modo headless (usa la CLI).
- La ruta del compose es configurable por variable de entorno, pero el stack
  Cinnamon sigue teniendo sus 8 servicios en `SERVICES`: para otro stack con
  arranque en serie hay que editar `src/main.rs` y recompilar.
- **La consola no es un sandbox**: usa `sh -lc` dentro del contenedor y el
  usuario por defecto de la imagen (a menudo root). No hay TTY: comandos
  interactivos como `top` o `bash` sin `-c` no funcionan. El comando viaja como
  un único argumento a `docker exec`, sin shell del host. Los comandos que no
  terminan se interrumpen a los 60 s.
- **Consola requiere `sh`** en la imagen del contenedor. Si la imagen no lo
  trae, usa otra shell o instala una.

## Contribuir

Lee [CONTRIBUTING.md](CONTRIBUTING.md). Checks obligatorios: `cargo fmt
--check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
`cargo build --release`.

## Licencia

MIT © 2026 Santiago Molina. Ver [LICENSE](LICENSE).
