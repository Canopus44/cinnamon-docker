# Cómo funciona

Documento técnico de `cinnamon-docker`. Todo el código vive en `src/main.rs`
(~4200 líneas); el binario es único y no hay librería ni módulos. Dependencias:
`eframe`/`egui` (UI), `serde_json` (salida de Docker) y `ksni` (bandeja SNI).

## Mapa de secciones

`src/main.rs` está dividido por comentarios `// ── nombre ──`:

| Sección | Contenido |
|---|---|
| `tema` | `ThemeMode`, `Palette`, `LIGHT_PALETTE`/`DARK_PALETTE`, `CURRENT_MODE`, `pal()`, `theme_path`, `load_theme`/`save_theme`, `apply_theme`, `paint_window_background`, `theme_switch`, `Settings`, `parse_settings`, `load_settings`/`save_settings`, `settings_path`, `depth_surface`, `primary_button`, `paint_bevel`, `DEPTH_ENABLED` y helpers (`tint`, `pill`, `dot_pill`, `meter`, `stat_text`, `soft_button`, `brand_mark`, `summary_card`, `clipped_label`). |
| `ejecución de comandos` | `CmdOut`, `run`, `run_merged`, `docker_bin`, `systemctl_bin`, `pkexec_bin`, `compose`, `engine_command`, `engine_active`, `socket_active`, `engine_hint`. |
| `estado de los servicios y contenedores` | `SvcState`, `Container`, `read_containers` (`docker ps -a`), `health_from`, `services_from`, `state_of`, `is_ready`, `group_key`. |
| `métricas (docker stats)` | `Stats`, `parse_pct`, `parse_bytes`, `read_stats`, `human_bytes`. |
| `mensajes del hilo de trabajo` | `Notice`, `enum Msg`, `spawn_refresh`, `spawn_stats`, `engine_action`, `container_action`, `cinnamon_serial_start`, `wait_ready`, `group_start`, `group_stop`, `exec_action`, `EXEC_OUTPUT_LINES`, `EXEC_HISTORY`. |
| `bandeja` | `TrayAction`, `TrayState`, `CinnamonTray`, `impl ksni::Tray`, `tray_icon`, `installed_icon_name`. |
| `app` | `struct App`, `impl App` (drenaje, header, resumen, tarjetas, log, modal), `impl eframe::App` (`logic` y `ui`). |
| `CLI` | `cli_status`, `cli_containers`, `usage`, `main`. |

## Modelo de hilos

```
main (UI egui: logic + ui)
 ├── spawn_refresh   cada 2 s   → docker ps -a + systemctl is-active + docker --version
 ├── spawn_stats     cada 5 s   → docker stats --no-stream (~2.1 s por muestra)
 ├── un hilo por acción        → engine_action / container_action / group_* / cinnamon_serial_start
 ├── un hilo por comando de Consola → docker exec <id|name> sh -lc <comando>
 └── hilo ksni (SNI)           → D-Bus org.kde.StatusNotifierWatcher + menú
```

- **main**: bucle de `eframe`; `logic` drena el canal y agenda trabajo, `ui` sólo
  pinta. Ningún comando externo corre aquí: `docker ps` o `pkexec` bloquearían la
  ventana.
- **spawn_refresh**: lee motor, socket, versión y contenedores, y manda un solo
  `Msg::Refresh`.
- **spawn_stats**: hilo y cadencia propios porque `docker stats` tarda ~2 s;
  con el refresco de 2 s competirían por el daemon y la UI iría a tirones.
- **acciones**: cada operación abre su hilo, emite `Msg::Log` /
  `Msg::ContainerBusy` / `Msg::Notice` y termina.
- **bandeja**: `ksni::blocking` corre su propio hilo y registra el icono en el
  watcher SNI. El menú sólo envía `Msg::Tray(TrayAction)`; la UI ejecuta la
  acción. El `Sender` es clonado y no bloquea.

Los hilos se comunican con `std::sync::mpsc::channel::<Msg>()`; `App` guarda
`tx` y `rx` y hace `try_recv` en cada `logic`. Sin runtime async propio; `ksni`
usa `async-io` internamente.

## Enum `Msg`

| Variante | Efecto en `App` |
|---|---|
| `Refresh { active, socket_active, version, containers, error }` | Sustituye el estado del motor y la lista de contenedores; limpia stats si el motor está apagado. |
| `Stats { rows, error }` | Sustituye el mapa de métricas (`HashMap<id|nombre, Stats>`). |
| `Log(String)` | Añade una línea al `VecDeque` (cap 200). |
| `EngineBusy(bool)` / `GlobalBusy(bool)` | Deshabilitan botones mientras dura la operación. |
| `ContainerBusy { name, busy }` | Spinner por tarjeta. |
| `Notice(Option<Notice>)` | Banner de éxito/error; un error puede traer el comando copiable. |
| `ExecDone { target, command, output, ok, code }` | Añade una entrada al historial de Consola del contenedor (máx. 20), guarda el código de salida y libera su spinner. |
| `ForceRefresh` | Fuerza un `spawn_refresh` inmediato tras una acción. |
| `Tray(TrayAction)` | `Show`, `EngineStart`, `EngineStop`, `Refresh`, `StartProject(String)`, `StopProject(String)`, `Quit`. `EngineStop` muestra la ventana y abre el modal de confirmación; `Quit` marca `allow_close` y cierra. El String fija el proyecto del snapshot del menú, para que un cambio de selección entre abrir el menú y pulsar no arranque/apague otro proyecto. |

## `logic` vs `ui` (eframe 0.37)

- `fn logic(&mut self, ctx, frame)` corre antes de cada `ui` y **también con la
  ventana oculta** (eframe usa `Context::run_logic`, sin pass de egui). Aquí van:
  `drain()`, timers de refresco/stats, `sync_tray()`, el cierre a bandeja y
  `ctx.request_repaint_after(500 ms)`.
- `fn ui(&mut self, ui, frame)` sólo pinta: fondo, modal, header, panel de
  actividad y cuerpo.
- Cerrar con bandeja disponible: si `close_requested` y `!allow_close` y
  `tray_ok`, se envían `ViewportCommand::CancelClose` y `Visible(false)`; la app
  sigue viva y el icono SNI permanece. Si `tray_ok == false`, el cierre pasa
  normal.
- «Salir» (panel Actividad o menú de bandeja): `allow_close = true` +
  `ViewportCommand::Close`.
- La ventana oculta no ejecuta `ui`, pero `logic` sigue drenando mensajes y
  refrescando el estado; la bandeja pide repintado con `ctx.request_repaint()`.

## Tema claro/oscuro

- `ThemeMode` (`Light`/`Dark`) + `Palette` con tokens (`bg`, `surface`, `card`,
  `text`, `muted`, `accent`, semánticos, sombras y gradiente).
- `CURRENT_MODE: AtomicU8` y `pal()` evitan pasar la paleta por cada función de
  dibujo. Es seguro porque sólo lo lee el hilo de UI; los tests usan
  `palette_for(mode)` directamente.
- `apply_theme(ctx, mode)` parte de `Visuals::light()`/`dark()`, aplica radios,
  spacing, tipografía, sombras y colores de widgets.
- `paint_window_background` pinta un gradiente vertical de dos colores con un
  `Mesh` de 4 vértices/2 triángulos en `LayerId::background()`; los paneles usan
  superficies semitransparentes. No hay blur real en egui.
- Persistencia: `~/.config/cinnamon-docker/theme` (respeta `XDG_CONFIG_HOME`),
  una línea `light`/`dark`; errores de I/O caen en claro. El toggle del header
  aplica y guarda el cambio.
- El icono de bandeja usa los mismos píxeles de marca (`brand_rgba`) que el
  header y la ventana, independientes del tema.
- Ajustes persistentes: `settings.json` (compose efectivo, `refresh_secs`,
  `stats_secs`, `depth`). Se cargan al arrancar; el modal sólo aplica al guardar.
  Los números se recortan a 1–60 s y 2–120 s; JSON inválido cae en defaults.
- `DEPTH_ENABLED: AtomicBool` controla el bisel/sombra de `depth_surface` y
  `paint_bevel`. Los modales usan `pal().card` opaco para no dejar ver la UI de
  atrás; el resto de paneles conserva `surface`.

## Bandeja SNI

- `CinnamonTray` implementa `ksni::Tray`: `id`, `title`, `icon_name`,
  `icon_pixmap`, `tool_tip`, `activate` y `menu`.
- `tray_icon()` genera un icono 128×128 ARGB32 en código (cuadrado redondeado +
  ballena blanca + tres contenedores oscuros + punto naranja), sin dependencias
  de imagen.
- `installed_icon_name()` devuelve `"cinnamon-docker"` sólo si el SVG está
  instalado en `XDG_DATA_HOME`/`~/.local/share`/`/usr/share`/`XDG_DATA_DIRS`
  (default `/usr/local/share:/usr/share`, para detectar un `PREFIX=/usr/local`);
  si no, devuelve vacío y el host usa `icon_pixmap` como fallback.
- Menú: Abrir panel, Iniciar/Detener motor, Refrescar, Levantar/Apagar proyecto
  seleccionado, Salir. `sync_tray()` publica sólo cambios de estado.
- `EngineStop` abre la ventana y el modal de confirmación; nunca detiene el
  motor sin confirmación.

## Proyecto seleccionado y arranque genérico

`App::selected_project` (vacío = «Sin proyecto») alimenta el ComboBox y los
botones `Levantar`/`Apagar`. `project_list()` ordena Cinnamon primero, resto
alfabético y `Sin proyecto` al final; incluye siempre `cinnamon` aunque no haya
contenedores. «Levantar cinnamon» usa `can_start_project`, que comprueba los 8
`SERVICES` por nombre (no sólo el número de contenedores).

- Cinnamon: `start_all` → `cinnamon_serial_start` (servicio a servicio,
  `docker compose up -d`, espera `healthy`, timeout 900 s); `stop_all` usa
  `docker compose stop`.
- Otros proyectos: `group_start` hace `docker start` secuencial de los
  contenedores parados; `group_stop` hace `docker stop` de los que están arriba.
- Contenedores sueltos («Sin proyecto») usan las mismas acciones genéricas.

## Compose configurable

`DEFAULT_COMPOSE_FILE` es la ruta del autor. `default_compose()` resuelve
`CINNAMON_DOCKER_COMPOSE` (o el default si falta o está vacía) y
`COMPOSE_FILE: LazyLock<RwLock<String>>` guarda la ruta efectiva, mutable desde
**Ajustes** con `set_compose_file`. `compose()` y `usage()` usan esa ruta;
`--help` muestra la efectiva. No hay fallback silencioso a otra ruta si el
valor apunta a un archivo inexistente. `compose()` ejecuta
`docker compose -f <ruta> -p cinnamon …`, de modo que el nombre de proyecto no
dependa del directorio del archivo.

## Consola por contenedor

`exec_action` corre en un hilo propio y ejecuta
`docker exec <id|name> sh -lc <comando>`. El comando va como un único argv:
no hay shell del host ni interpolación de variables locales. Dentro del
contenedor `sh -lc` permite pipes, redirecciones y variables.

- Sólo se ofrece para contenedores en marcha; el modal se abre desde el botón
  **Consola** de la tarjeta.
- `App` guarda el input por contenedor y un historial de 20 entradas
  (`EXEC_HISTORY`), con salida recortada a las últimas 200 líneas
  (`EXEC_OUTPUT_LINES`) para no crecer sin límite.
- `ExecDone` trae `ok` y `code`; el modal pinta el estado y permite limpiar la
  salida. Enter en el campo ejecuta (patrón `lost_focus && key_pressed(Enter)`);
  también está el botón **Ejecutar**.
- No hay TTY: los comandos interactivos no funcionan.
- `run_merged_timeout` mata el proceso a los 60 s y `read_capped` limita cada
  stream a 1 MiB; el recorte a 200 líneas se hace después.

## Ajustes en runtime

`Settings` (`compose`, `refresh_secs`, `stats_secs`, `depth`) se carga de
`$XDG_CONFIG_HOME/cinnamon-docker/settings.json` al crear `App`. Al guardar:
`set_compose_file` actualiza el `RwLock` que usa `compose()`, `set_depth_enabled`
cambia el flag de dibujo y `save_settings` escribe JSON indentado. Un fallo de
I/O no rompe la app.

La ruta del compose se resuelve en cascada: campo `compose` no vacío →
`CINNAMON_DOCKER_COMPOSE` → `DEFAULT_COMPOSE_FILE`. `--help` muestra la efectiva.

## Invariantes

1. **Con el motor apagado no se toca el daemon.** `spawn_refresh`, `spawn_stats`
   y la CLI sólo ejecutan `docker ps`/`docker stats` si `engine_active()` es
   cierto. Una consulta por el socket reactivaría `docker.service`.
2. **Socket antes que servicio al detener.** `systemctl stop docker.socket docker`:
   si se para sólo el servicio, la siguiente petición al socket lo relanza.
3. **Arranque serie + `healthy` en Cinnamon.** Dos `ng serve` a la vez agotan la
   RAM y mueren con exit 137.
4. **Detener el motor confirma.** Apaga todos los contenedores de todos los
   proyectos; el modal `confirm_stop_engine` es obligatorio.
5. **No se borra nada.** Sólo `start`/`stop`/`restart`/`logs`/`ps`/`stats`/
   `compose up -d`/`compose stop`. Nunca `rm`, `down -v` ni `volume rm`.
6. **Cerrar oculta, no mata.** Con bandeja disponible, cerrar la ventana la
   desmapea y mantiene proceso, SNI y refresco. Sin bandeja, cierra normal.
7. **El tema no afecta la lógica.** La paleta es sólo presentación; el icono de
   bandeja conserva el color de marca.
8. **Consola sin shell del host.** `docker exec` recibe el comando como un solo
   argumento; la única shell es `sh -lc` dentro del contenedor.
9. **La consola no borra por sí sola.** La app no ejecuta `rm`/`down -v`; sólo
   corre el comando que escribe el usuario.

## Merge de stats

`read_stats` indexa cada muestra dos veces: por `ID` (12 hex) y por `Name`.
`App::stats_for` busca primero `self.stats.get(&c.id)` y cae a
`self.stats.get(&c.name)` si el ID no está. `stats_total` suma CPU/RAM de los
contenedores arriba y toma el mayor `mem_limit`.

## Agrupación y orden

`BTreeMap<String, Vec<Container>>` por `project`: iteración determinista. Las
claves se ordenan con `project_rank`: Cinnamon primero, resto alfabético y
`Sin proyecto` al final. Dentro de Cinnamon los contenedores siguen `SERVICES`;
en el resto, el nombre.

## Puntos de extensión

- **Otra ruta compose**: `CINNAMON_DOCKER_COMPOSE` (sin recompilar). Para otro
  stack con orden serie propio, añadir sus servicios a `SERVICES`.
- **Otro tema/token**: ampliar `Palette` y ambas paletas; no hardcodear colores.
- **Nueva operación de contenedor**: variante en `ContainerOp`, rama en
  `container_action` y botón en la tarjeta.
- **Nuevo dato del daemon**: campo en `Container` + lectura en `read_containers`.
- **Otra métrica**: campo en `Stats` y parseo en `read_stats`; su refresco va en
  el hilo de stats.
- **Nueva acción de bandeja**: variante en `TrayAction`, ítem en `menu()` y rama
  en `drain`.
- **Nuevo ajuste**: campo en `Settings`, parseo/guardado y control en el modal.
- **Historial de consola**: `EXEC_HISTORY`/`EXEC_OUTPUT_LINES` y el mapa
  `exec_entries`.
