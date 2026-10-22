# Problemas frecuentes

## `pkexec` no muestra el diálogo o falla sin mensaje

El proceso GUI puede no heredar el `PATH` ni tener una sesión polkit completa.
Copia el comando del banner de error (botón *Copiar comando*) y ejecútalo en
una terminal:

```bash
pkexec /usr/bin/systemctl start docker.socket docker
pkexec /usr/bin/systemctl stop docker.socket docker
```

Si `pkexec` no existe, la app cae a `systemctl` directo; necesitas permisos o
`sudo systemctl …` a mano. Comprueba el agente polkit:

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
**Levantar cinnamon** en serie: un servicio cada vez y esperando `healthy`. No
uses `docker compose up -d` a secas para el stack completo.

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

Abre **Ajustes** y escribe la ruta correcta del compose. Se guarda en
`~/.config/cinnamon-docker/settings.json`; si el campo está vacío se usa
`CINNAMON_DOCKER_COMPOSE` y, si no, la ruta por defecto.

```bash
cinnamon-docker --help   # muestra la ruta efectiva
```

Debe ser un `docker-compose.yml` válido y Docker Compose v2 debe estar
instalado (`docker compose version`). Cambiar la ruta no reinicia contenedores.

## La consola no ejecuta o el comando falla

- El botón **Consola** sólo está activo con el contenedor en marcha.
- Pulsa **Ejecutar** o Enter en el campo; si el campo está vacío no se lanza.
- La imagen debe traer `sh`: se ejecuta
  `docker exec <id|name> sh -lc "<comando>"`. Si no existe, prueba otra imagen o
  instala una shell.
- No hay TTY. `top`, `bash` interactivo o `vim` no funcionan; usa comandos no
  interactivos (`ls`, `cat`, `env`, `ps`).
- El código de salida y la salida combinada (stdout+stderr) aparecen en el
  modal; la salida se recorta a las últimas 200 líneas. El comando se
  interrumpe a los 60 s; si necesitas algo más largo, ejecútalo fuera de la
  consola o divídelo.
- Reproduce el comando a mano para aislar permisos o rutas:
  `docker exec <nombre> sh -lc 'echo hola; uname -s'`.

## Ajustes no se guardan

La configuración vive en `~/.config/cinnamon-docker/settings.json` (respeta
`XDG_CONFIG_HOME`). Si no persiste, revisa permisos del directorio y que
`HOME`/`XDG_CONFIG_HOME` estén definidos. Un JSON inválido o campos ausentes
caen en los valores por defecto; los números fuera de rango se recortan a
1–60 s (refresco) y 2–120 s (métricas). El efecto 3D se apaga con
`"depth": false`.

## El efecto 3D no se nota

Comprueba **Ajustes → Efecto 3D**. Con `depth: false` los botones y tarjetas se
pintan planos; el cambio se aplica al guardar. En temas claros el bisel es sutil
por diseño.

## No aparece el icono en la bandeja

La app usa StatusNotifierItem (SNI) vía `ksni`. Comprueba que el escritorio
tiene un watcher:

```bash
busctl --user list | grep -i StatusNotifierWatcher
```

- KDE Plasma lo trae activo (`kded6`).
- GNOME necesita la extensión *AppIndicator and KStatusNotifierItem Support*.
- Cinnamon moderno usa `xapp-sn-watcher`; en versiones antiguas puede no haber
  SNI.
- Si no hay watcher, el log muestra `Bandeja no disponible: …` y cerrar la
  ventana termina la app (no se queda en segundo plano).
- Si el icono no está instalado, se usa el pixmap procedural; no es un fallo.

## Cerrar la ventana la oculta y no sé cómo salir

Con bandeja disponible, cerrar la ventana la oculta y mantiene la app y el
refresco. Para terminar de verdad:

- Pulsa **Salir** en el panel *Actividad* (arriba a la derecha), o
- abre el menú del icono de la bandeja y elige **Salir**.

Si no hay bandeja, cerrar la ventana termina la app normalmente.

## El tema no cambia o no persiste

El toggle **Claro/Oscuro** está en el header y aplica en caliente. La
preferencia se guarda en:

```
~/.config/cinnamon-docker/theme
```

Una línea: `light` o `dark`. Respeta `XDG_CONFIG_HOME`. Si el archivo no se
puede escribir, la app sigue funcionando pero no recuerda el cambio. Revisa
permisos del directorio y que `HOME`/`XDG_CONFIG_HOME` estén definidos.

## El icono SVG no se ve en el lanzador

`install.sh` copia `assets/icon.svg` a:

```
~/.local/share/icons/hicolor/scalable/apps/cinnamon-docker.svg
```

y el `.desktop` usa `Icon=cinnamon-docker`. Si el lanzador sigue mostrando un
icono genérico:

```bash
gtk-update-icon-cache -q -t ~/.local/share/icons/hicolor
update-desktop-database -q ~/.local/share/applications
```

Cierra y vuelve a abrir el menú de aplicaciones. En KDE puede tardar un
refresco de caché. El icono de la bandeja tiene fallback propio y no depende de
este archivo.

## `install.sh` dice que falta el binario

Compila primero:

```bash
cargo build --release -j 2
./install.sh
```

El script no compila por ti. Instala en `~/.local` por defecto; usa
`PREFIX=/otra/ruta ./install.sh` si quieres otro prefijo (sin root).

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
su propio hilo con cadencia configurable en **Ajustes** (2–120 s; defecto 5 s),
para no bloquear la UI ni competir con el refresco de `docker ps` (2 s por
defecto). Para un dato puntual:

```bash
docker stats --no-stream --format json
```
