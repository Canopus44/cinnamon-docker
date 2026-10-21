# Contribuir a cinnamon-docker

Gracias por el interés. Es una app de escritorio pequeña y de un solo binario:
los cambios deben caber en `src/main.rs` sin añadir dependencias salvo que sea
imprescindible. Dependencias actuales: `eframe`/`egui`, `serde_json` y `ksni`.

## Reportar un bug

Abre un issue con este contenido:

```
Versión de cinnamon-docker:   (git describe / cargo metadata)
Versión de Docker:            docker --version
Compose:                      docker compose version
Motor / socket:               systemctl is-active docker docker.socket
Distro y escritorio:          (p. ej. Arch + KDE Wayland)
Rust:                         cargo --version && rustc --version

Pasos para reproducir:
1.
2.
3.

Esperado:
Obtenido:

Log:                          (copia el panel Actividad)
```

Si el problema es de la UI, adjunta una captura. No pegues rutas de tu home ni
tokens. Para dudas de bandeja incluye
`busctl --user list | grep -i StatusNotifierWatcher`.

## Entorno

- Linux con X11 o Wayland (vía XWayland).
- Docker Engine + Compose v2 (`docker compose version`).
- `polkit`/`pkexec` para iniciar/detener el motor.
- Rust stable; dependencias de build en el README.
- Watcher StatusNotifierItem para la bandeja (KDE Plasma; GNOME con
  AppIndicator; Cinnamon con `xapp-sn-watcher`).

## Build y checks

Antes de enviar un PR, todo esto debe pasar en limpio:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
xmllint --noout assets/*.svg
desktop-file-validate packaging/cinnamon-docker.desktop
```

Si `xmllint` o `desktop-file-validate` no están instalados, dilo en el PR; no
los sustituyas por validaciones inventadas.

- No dejes warnings de Clippy ni `#[allow(...)]` para silenciarlos: corrige el
  código.
- No uses `unwrap()`/`expect()` en rutas que ejecuta la UI (comandos, parseo de
  salida de Docker, pintado). En tests y constantes sí es aceptable.
- Añade un test cuando toques parseo (`parse_pct`, `parse_bytes`,
  `health_from`, `short_image`, `group_key`, `ThemeMode::parse`,
  `resolve_compose_file`) o cualquier rama nueva.

## Estilo de UI

- Usa `pal()`/`Palette` para todos los colores; no hardcodees `Color32` en
  widgets. Si añades un token, añádelo a ambas paletas y a su test de contraste.
- Prueba **claro y oscuro**. Capturas mínimas: 1320×840 y 980×560. Revisa que
  el header no se solape, que el log se pueda redimensionar y que las tarjetas
  no recorten acciones.
- Mantén el contraste AA para texto y texto secundario sobre tarjetas; los
  estados deshabilitados pueden ser más tenues.
- Reutiliza `pill`, `dot_pill`, `soft_button`, `summary_card`, `meter` y
  `stat_text` antes de inventar estilos.
- No añadas blur real: egui no lo tiene; usa superficies/alpha como el resto.
- Iconos de app en SVG (`assets/`); los iconos de acciones in-app son texto
  accesible, no SVG en runtime (evita dependencias pesadas de renderizado).

## Pruebas manuales de bandeja

Con la app abierta y un watcher SNI:

1. El icono aparece en la bandeja (`busctl --user list | grep StatusNotifierItem`).
2. Cerrar la ventana la oculta; el proceso y el icono siguen vivos.
3. *Abrir panel* o clic izquierdo restaura la ventana.
4. *Salir* termina el proceso.
5. Sin watcher (por ejemplo `DBUS_SESSION_BUS_ADDRESS` inválido), cerrar la
   ventana debe terminar la app.

No dispares *Detener motor* en las pruebas: apaga todos los contenedores.

## Estilo de código

- UI, mensajes, comentarios y documentación **en español**.
- Sigue la estructura existente: secciones `// ── nombre ──`, helpers de tema
  (`pill`, `meter`, `stat_text`, `soft_button`) en vez de estilos ad hoc.
- Comentarios sólo para el *porqué* (p. ej. socket antes que servicio); el
  *qué* lo dice el código.
- Hilos + `mpsc`; no añadas un runtime async propio. `ksni` ya usa `async-io`
  internamente.
- No borres contenedores ni volúmenes ni añadas operaciones destructivas.

## Commits y ramas

- Ramas: `feat/<tema>`, `fix/<tema>`, `docs/<tema>`.
- Commits en [Conventional Commits](https://www.conventionalcommits.org/):
  `feat: selector de proyecto genérico`, `fix: no consulta el daemon con el
  motor apagado`.
- Un commit = un cambio coherente; no mezcles formateo masivo con lógica.

## Checklist de PR

- [ ] `cargo fmt --check`, Clippy, tests y build release en verde.
- [ ] SVG y `.desktop` validados (si hay herramientas).
- [ ] Capturas antes/después en claro y oscuro si cambia la UI.
- [ ] Sin secretos, rutas personales nuevas ni logs.
- [ ] La CLI (`--status`, `--containers`, `--help`) sigue funcionando y sale
      con 0.
- [ ] `CINNAMON_DOCKER_COMPOSE` y `--help` muestran la ruta correcta.
- [ ] Bandeja: cerrar oculta, reabrir y Salir probados.
- [ ] No se rompe ningún invariante de `docs/architecture.md` (socket,
      arranque serie + `healthy`, confirmación al detener el motor, no borrar).

## Nota sobre el compose por defecto

`DEFAULT_COMPOSE_FILE` es la ruta del proyecto Cinnamon del autor y `SERVICES`
sus 8 servicios. No cambies esos valores por los tuyos en un PR: usa
`CINNAMON_DOCKER_COMPOSE` para la ruta. Para otro stack con orden serie propio,
propón el cambio en un issue antes de tocar `SERVICES`.
