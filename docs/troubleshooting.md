# Problemas frecuentes

## `pkexec` no muestra el diálogo o falla sin mensaje

El proceso GUI puede no heredar el `PATH` ni tener una sesión polkit completa.
Copia el comando del banner de error (botón *Copiar comando*) y ejecútalo en
una terminal:

```bash
pkexec /usr/bin/systemctl start docker.socket docker
pkexec /usr/bin/systemctl stop docker.socket docker
```

Si `pkexec` no existe, la app cae a `systemctl` directo; en ese caso necesitas
permisos o `sudo systemctl …` a mano. Comprueba el agente polkit:

```bash
systemctl status polkit
```

## Al arrancar, el socket reactiva `dockerd`

`docker.socket` está activo aunque `docker.service` no: cualquier consulta al
socket (incluido un `docker ps`) arranca el servicio. Por eso la app no
consulta el daemon con el motor apagado y **Detener motor** ejecuta
`stop docker.socket docker` en ese orden. Si el motor vuelve solo:

```bash
systemctl status docker.socket
sudo systemctl stop docker.socket
```

## Contenedores que mueren con exit 137

Exit 137 = `SIGKILL`, casi siempre OOM. En Cinnamon, dos `ng serve`
(`chat-frontend` y `erp-frontend`) en paralelo agotan la RAM. La app usa
*Levantar todo (serie)*: un servicio cada vez y esperando `healthy`. No uses
`docker compose up -d` a secas para el stack completo.

Revisa el consumo por contenedor en las tarjetas (CPU/RAM) o:

```bash
docker stats --no-stream
free -h
```

## CPU/RAM muestran `—`

- El contenedor está parado: no hay métricas.
- Aún no llegó la primera muestra: `docker stats --no-stream` tarda ~2 s y se
  consulta cada 5 s; espera un ciclo o pulsa *Refrescar*.
- El motor está apagado: la app no consulta el daemon a propósito.
- `docker stats` falló (permisos, daemon caído): el motivo aparece en el log.

## `permission denied` en `/var/run/docker.sock`

El usuario debe pertenecer al grupo `docker`:

```bash
sudo usermod -aG docker "$USER"
```

Cierra la sesión y vuelve a entrar (o `newgrp docker`) para que el grupo se
aplique. La app no usa `sudo` para el CLI de Docker: sólo para `systemctl`.

## La ruta de compose no existe

`COMPOSE_FILE` está fijada a:

```
/mnt/disco-local/home/Github/Cinnamon/docker-compose.yml
```

Si el proyecto está en otra ruta, *Levantar todo* y las acciones de grupo
fallan con el error de `docker compose`. Edita la constante en `src/main.rs` y
recompila:

```bash
cargo build --release -j 2
install -Dm755 target/release/cinnamon-docker ~/.local/bin/cinnamon-docker
```

No hay variable de entorno ni archivo de configuración en runtime.

## Wayland: ventana vacía o error de EGL/GL

La app usa el renderer `glow` con backend `x11`. En Wayland corre vía
XWayland; si el compositor lo bloquea o falta GL:

```bash
# forzar X11/XWayland
WAYLAND_DISPLAY= ./target/release/cinnamon-docker
```

Asegúrate de tener `libgl1-mesa`/`mesa` y `libxkbcommon` instalados. Los
errores de arranque de `eframe` van a la terminal desde la que lanzas el
binario.

## Las stats van 5 s por detrás

Es intencionado: `docker stats --no-stream` tarda ~2 s por muestra y corre en
su propio hilo cada 5 s (`STATS_EVERY`), para no bloquear la UI ni competir con
el refresco de `docker ps` (2 s). Para un dato puntual:

```bash
docker stats --no-stream --format json
```
