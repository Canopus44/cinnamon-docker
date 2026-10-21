# Cómo funciona

Documento técnico de `cinnamon-docker`. Todo el código vive en `src/main.rs`
(~2100 líneas); el binario es único y no hay librería ni módulos.

## Mapa de secciones

`src/main.rs` está dividido por comentarios `// ── nombre ──`:

| Sección | Contenido |
|---|---|
| `tema` | Paleta (`BG`, `SURFACE`, `CARD`, …), `apply_theme`, helpers de pintado (`pill`, `dot_pill`, `meter`, `stat_text`, `clipped_label`). |
| `ejecución de comandos` | `CmdOut`, `run`, `run_merged`, `docker_bin`, `systemctl_bin`, `pkexec_bin`, `compose`, `engine_command`, `engine_active`, `socket_active`, `engine_hint`. |
| `estado de los servicios y contenedores` | `SvcState`, `Container`, `read_containers` (`docker ps -a`), `health_from`, `services_from`, `state_of`, `is_ready`, `group_key`. |
| `métricas (docker stats)` | `Stats`, `parse_pct`, `parse_bytes`, `read_stats`, `human_bytes`. |
| `mensajes del hilo de trabajo` | `Notice`, `enum Msg`, `spawn_refresh`, `spawn_stats`, `engine_action`, `container_action`, `cinnamon_serial_start`, `wait_ready`, `group_start`, `group_stop`. |
| `app` | `struct App`, `impl App` (drenaje de mensajes, header, tarjetas, log, modal), `impl eframe::App`. |
| `CLI` | `cli_status`, `cli_containers`, `usage`, `main`. |

## Modelo de hilos

```
main (UI egui)
 ├── spawn_refresh   cada 2 s   → docker ps -a + systemctl is-active + docker --version
 ├── spawn_stats     cada 5 s   → docker stats --no-stream (~2.1 s por muestra)
 └── un hilo por acción        → engine_action / container_action / group_* / cinnamon_serial_start
```

- **main**: bucle de `eframe`; sólo pinta y drena el canal. Ningún comando
  externo corre aquí: `docker ps` o `pkexec` bloquearían la ventana.
- **spawn_refresh**: lee motor, socket, versión y contenedores, y manda un solo
  `Msg::Refresh`.
- **spawn_stats**: hilo y cadencia propios porque `docker stats` tarda ~2 s;
  con el refresco de 2 s competirían por el daemon y la UI iría a tirones.
- **acciones**: cada operación (iniciar/detener motor, arrancar/detener/
  reiniciar/logs de un contenedor o de un grupo) abre su hilo, emite
  `Msg::Log`/`Msg::ContainerBusy`/`Msg::Notice` y termina.

Los hilos se comunican con `std::sync::mpsc::channel::<Msg>()`; `App` guarda
`tx` y `rx` y hace `try_recv` en cada frame. Sin tokio ni async.

## Enum `Msg`

| Variante | Efecto en `App` |
|---|---|
| `Refresh { active, socket_active, version, containers, error }` | Sustituye el estado del motor y la lista de contenedores; limpia stats si el motor está apagado. |
| `Stats { rows, error }` | Sustituye el mapa de métricas (`HashMap<id|nombre, Stats>`). |
| `Log(String)` | Añade una línea al `VecDeque` (cap 200). |
| `EngineBusy(bool)` / `GlobalBusy(bool)` | Deshabilitan botones mientras dura la operación. |
| `ContainerBusy { name, busy }` | Spinner por tarjeta. |
| `Notice(Option<Notice>)` | Banner de éxito/error; un error puede traer el comando copiable. |
| `ForceRefresh` | Fuerza un `spawn_refresh` inmediato tras una acción. |

El refresco periódico lo dispara `impl eframe::App::ui` (API de eframe 0.37):
si pasaron 2 s (`REFRESH_EVERY`) y no hay uno en vuelo, llama a
`spawn_refresh`; igual con `STATS_EVERY` para `spawn_stats`. La UI se repinta
cada 500 ms.

## Invariantes

1. **Con el motor apagado no se toca el daemon.** `spawn_refresh`, `spawn_stats`
   y la CLI sólo ejecutan `docker ps`/`docker stats` si `engine_active()` es
   cierto. Una consulta por el socket reactivaría `docker.service`.
2. **Socket antes que servicio al detener.** `systemctl stop docker.socket docker`:
   si se para sólo el servicio, la siguiente petición al socket lo relanza.
3. **Arranque serie + `healthy`.** `cinnamon_serial_start` levanta los 8
   servicios de `SERVICES` de uno en uno con `docker compose up -d <svc>` y
   espera `healthy` (`wait_ready`, timeout 900 s). Dos `ng serve` a la vez
   agotan la RAM y mueren con exit 137.
4. **Detener el motor confirma.** Apaga todos los contenedores de todos los
   proyectos; el modal `confirm_stop_engine` es obligatorio.
5. **No se borra nada.** El código sólo usa `start`/`stop`/`restart`/`logs`/
   `ps`/`stats`/`compose up -d`. Nunca `rm`, `down -v` ni `volume rm`.

## Merge de stats

`read_stats` indexa cada muestra dos veces: por `ID` (12 hex) y por `Name`.
`App::stats_for` busca primero `self.stats.get(&c.id)` y cae a
`self.stats.get(&c.name)` si el ID no está (por ejemplo si `docker ps` devolvió
el nombre pero no el ID). `stats_total` suma CPU y RAM de los contenedores
arriba y toma el mayor `mem_limit` (todos comparten host).

## Agrupación y orden

`BTreeMap<String, Vec<Container>>` por `project`: iteración determinista sin
volver a ordenar el mapa. Luego se reordenan las claves con una clave sintética
`0cinnamon`, `1<otros>`, `2""`: Cinnamon primero, resto alfabético y
`Sin proyecto` al final. Dentro de Cinnamon los contenedores siguen el orden de
`SERVICES`; en el resto, el nombre.

## Puntos de extensión

- **Otro proyecto/ruta compose**: `COMPOSE_FILE` (constante) y `SERVICES`.
  Requiere recompilar; no hay configuración en runtime.
- **Nuevo servicio Cinnamon**: añadirlo a `SERVICES` en el orden de arranque.
- **Nueva operación de contenedor**: variante en `ContainerOp` y su rama en
  `container_action`; la UI sólo necesita un botón que la invoque.
- **Nuevo dato del daemon**: campo en `Container` + lectura en
  `read_containers` (formato JSON de `docker ps`).
- **Otra métrica**: campo en `Stats` y parseo en `read_stats`; el refresco va
  en su propio hilo.
