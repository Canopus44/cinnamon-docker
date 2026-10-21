# Contribuir a cinnamon-docker

Gracias por el interés. Es una app de escritorio pequeña y de un solo binario:
los cambios deben caber en `src/main.rs` sin añadir dependencias salvo que sea
imprescindible.

## Reportar un bug

Abre un issue con este contenido:

```
Versión de cinnamon-docker:   (cargo metadata / git describe)
Versión de Docker:            docker --version
Motor / socket:               systemctl is-active docker docker.socket
Distro y escritorio:          (p. ej. Arch + Cinnamon X11 / Wayland)
Rust:                         cargo --version && rustc --version

Pasos para reproducir:
1.
2.
3.

Esperado:
Obtenido:

Log:                          (copia el panel Log o /tmp/cd.log)
```

Si el problema es de la UI, adjunta una captura. No pegues rutas de tu home ni
tokens; el `COMPOSE_FILE` del repo es específico del autor (ver abajo).

## Entorno

- Linux con X11 o Wayland (vía XWayland).
- Docker con el daemon accesible por el usuario (`grupo docker`).
- `polkit`/`pkexec` para iniciar/detener el motor.
- Rust stable; dependencias de build en el README.

## Build y checks

Antes de enviar un PR, todo esto debe pasar en limpio:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

- No dejes warnings de Clippy ni `#[allow(...)]` para silenciarlos: corrige el
  código.
- No uses `unwrap()`/`expect()` en rutas que ejecuta la UI (comandos, parseo de
  salida de Docker, pintado). En tests y constantes sí es aceptable.
- Añade un test cuando toques parseo (`parse_pct`, `parse_bytes`,
  `health_from`, `short_image`, `group_key`) o cualquier rama nueva.

## Estilo

- UI, mensajes, comentarios y documentación **en español**.
- Sigue la estructura existente: secciones `// ── nombre ──`, helpers de tema
  (`pill`, `meter`, `stat_text`) en vez de estilos ad hoc.
- Comentarios sólo para el *porqué* (p. ej. socket antes que servicio); el
  *qué* lo dice el código.
- Nada de `tokio`/async: hilos + `mpsc`, como el resto.
- No borres contenedores ni volúmenes ni añadas operaciones destructivas.

## Commits y ramas

- Ramas: `feat/<tema>`, `fix/<tema>`, `docs/<tema>`.
- Commits en [Conventional Commits](https://www.conventionalcommits.org/):
  `feat: agrupa tarjetas por proyecto`, `fix: no consulta el daemon con el
  motor apagado`.
- Un commit = un cambio coherente; no mezcles formateo masivo con lógica.

## Checklist de PR

- [ ] `cargo fmt --check`, Clippy, tests y build release en verde.
- [ ] Captura antes/después si cambia la UI.
- [ ] Sin secretos, rutas personales nuevas ni logs.
- [ ] La CLI (`--status`, `--containers`, `--help`) sigue funcionando y sale
      con 0.
- [ ] No se rompe ningún invariante de `docs/architecture.md` (socket,
      arranque serie + `healthy`, confirmación al detener el motor).

## Nota sobre `COMPOSE_FILE`

`COMPOSE_FILE` está hardcodeada a la ruta del proyecto Cinnamon del autor y
`SERVICES` a sus 8 servicios. Un PR no debe cambiar esos valores por los
tuyos: si necesitas otra ruta, hazlo en tu fork y recompila.
