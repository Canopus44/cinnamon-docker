//! Docker Cinnamon — panel mínimo para arrancar/parar el motor Docker, el
//! stack de docker-compose del proyecto Cinnamon (en serie) y cualquier
//! contenedor del daemon, en serie y sin bloquear la UI.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

/// Servicios del compose, en el orden en que se levantan (bases primero, luego
/// apps: dos `ng serve` en paralelo agotan la RAM y mueren con 137).
const SERVICES: [&str; 8] = [
    "chat-postgres",
    "erp-postgres",
    "mongo",
    "pgadmin",
    "chat-backend",
    "chat-frontend",
    "erp-backend",
    "erp-frontend",
];

const CINNAMON_PROJECT: &str = "cinnamon";
const COMPOSE_FILE: &str = "/mnt/disco-local/home/Github/Cinnamon/docker-compose.yml";
const REFRESH_EVERY: Duration = Duration::from_secs(2);
/// `docker stats --no-stream` tarda ~2 s: cadencia propia y en su hilo.
const STATS_EVERY: Duration = Duration::from_secs(5);
const WAIT_TIMEOUT: Duration = Duration::from_secs(900);
const LOG_CAP: usize = 200;
const LOGS_TAIL: usize = 200;

/// Binarios con ruta absoluta: el proceso GUI no siempre hereda el PATH del
/// shell, y `pkexec`/`systemctl` sueltos fallaban sin mensaje útil.
const PKEXEC: &str = "/usr/bin/pkexec";
const SYSTEMCTL: &str = "/usr/bin/systemctl";

// ── tema ───────────────────────────────────────────────────────────────────

const BG: egui::Color32 = egui::Color32::from_rgb(0x0E, 0x13, 0x16);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(0x15, 0x1C, 0x20);
const CARD: egui::Color32 = egui::Color32::from_rgb(0x1A, 0x23, 0x28);
const HOVER: egui::Color32 = egui::Color32::from_rgb(0x20, 0x2B, 0x31);
const BORDER: egui::Color32 = egui::Color32::from_rgb(0x26, 0x34, 0x3B);
const TEXT: egui::Color32 = egui::Color32::from_rgb(0xE6, 0xED, 0xF0);
const MUTED: egui::Color32 = egui::Color32::from_rgb(0x8B, 0x9C, 0xA5);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x46, 0xC2, 0xA3);
const INFO: egui::Color32 = egui::Color32::from_rgb(0x5B, 0x9D, 0xFF);
const OK: egui::Color32 = egui::Color32::from_rgb(0x3D, 0xD6, 0x8C);
const WARN: egui::Color32 = egui::Color32::from_rgb(0xE3, 0xB3, 0x41);
const ERR: egui::Color32 = egui::Color32::from_rgb(0xF2, 0x6D, 0x6D);

/// Mismo color con otra opacidad: fondos de pills y avisos.
fn tint(c: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.faint_bg_color = CARD;
    v.extreme_bg_color = BG;
    v.code_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.weak_text_color = Some(MUTED);
    v.selection.bg_fill = tint(ACCENT, 90);
    v.selection.stroke = egui::Stroke::new(1.0, TEXT);
    v.hyperlink_color = INFO;
    v.warn_fg_color = WARN;
    v.error_fg_color = ERR;
    v.window_corner_radius = egui::CornerRadius::same(12);
    v.window_stroke = egui::Stroke::new(1.0, BORDER);
    v.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: egui::Color32::from_black_alpha(160),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: egui::Color32::from_black_alpha(120),
    };

    // Botones con radio 8; marcos y contenedores no interactivos con radio 10.
    let button_radius = egui::CornerRadius::same(8);
    let frame_radius = egui::CornerRadius::same(10);
    let border = egui::Stroke::new(1.0, BORDER);

    v.widgets.noninteractive.bg_fill = SURFACE;
    v.widgets.noninteractive.weak_bg_fill = SURFACE;
    v.widgets.noninteractive.bg_stroke = border;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT);
    v.widgets.noninteractive.corner_radius = frame_radius;

    v.widgets.inactive.bg_fill = CARD;
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.inactive.bg_stroke = border;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = button_radius;

    v.widgets.hovered.bg_fill = HOVER;
    v.widgets.hovered.weak_bg_fill = HOVER;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, TEXT);
    v.widgets.hovered.corner_radius = button_radius;

    v.widgets.active.bg_fill = HOVER;
    v.widgets.active.weak_bg_fill = HOVER;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, ACCENT);
    v.widgets.active.corner_radius = button_radius;

    v.widgets.open = v.widgets.active;

    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals(v);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}

/// Ancho real de un texto sin envolver: evita huecos y cortes innecesarios.
fn text_width(ui: &egui::Ui, text: &str, size: f32, mono: bool) -> f32 {
    let font = if mono {
        egui::FontId::monospace(size)
    } else {
        egui::FontId::proportional(size)
    };
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, TEXT)
        .size()
        .x
}

/// Texto truncado en una caja de ancho fijo, alineado a la izquierda (a
/// diferencia de `add_sized`, que centra el contenido y deja huecos).
fn clipped_label(
    ui: &mut egui::Ui,
    rich: egui::RichText,
    width: f32,
    height: f32,
) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| ui.add(egui::Label::new(rich).truncate()),
    )
    .inner
}

/// `Docker version 29.9.2, build abc` → `29.9.2`; si no hay token numérico,
/// corta en la primera coma.
fn short_docker_version(full: &str) -> String {
    full.split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| t.contains('.') && t.starts_with(|c: char| c.is_ascii_digit()))
        .map(|t| t.trim_matches(',').to_string())
        .unwrap_or_else(|| full.split(',').next().unwrap_or(full).trim().to_string())
}

/// Punto de estado de 8 px: mismo color que la pill de estado.
fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// Barra de 44×4: fondo `BORDER` y relleno proporcional al color semántico.
fn meter(ui: &mut egui::Ui, frac: f32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 4.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2, BORDER);
    let filled = rect.width() * frac.clamp(0.0, 1.0);
    if filled > 0.0 {
        let bar = egui::Rect::from_min_size(rect.min, egui::vec2(filled, rect.height()));
        painter.rect_filled(bar, 2, color);
    }
}

/// Chip de métrica para un layout de derecha a izquierda: se añade barra,
/// valor y etiqueta en ese orden porque en RTL el primero queda a la derecha.
fn stat_text(
    ui: &mut egui::Ui,
    label: &str,
    value: String,
    frac: Option<f32>,
    color: egui::Color32,
) {
    if let Some(f) = frac {
        meter(ui, f, color);
    }
    ui.label(
        egui::RichText::new(value)
            .size(11.0)
            .monospace()
            .color(color),
    )
    .on_hover_text("Fuente: docker stats --no-stream");
    ui.label(egui::RichText::new(label).size(10.0).color(MUTED));
}

/// Pill compacta: texto corto sobre el color semántico atenuado.
fn pill(ui: &mut egui::Ui, text: impl Into<String>, fg: egui::Color32) -> egui::Response {
    egui::Frame::new()
        .fill(tint(fg, 28))
        .stroke(egui::Stroke::new(1.0, tint(fg, 70)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text.into()).size(12.0).color(fg))
        })
        .inner
}

/// Pill con punto pintado (la fuente por defecto no trae `●`).
fn dot_pill(ui: &mut egui::Ui, color: egui::Color32, text: &str) -> egui::Response {
    egui::Frame::new()
        .fill(tint(color, 28))
        .stroke(egui::Stroke::new(1.0, tint(color, 70)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                status_dot(ui, color);
                ui.label(egui::RichText::new(text).size(12.0).color(color));
            });
        })
        .response
}

// ── ejecución de comandos ───────────────────────────────────────────────────

struct CmdOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl CmdOut {
    /// stdout y stderr completos, uno por línea, sin duplicar vacíos.
    fn full_output(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        let out = self.stdout.trim_end();
        let err = self.stderr.trim_end();
        if !out.is_empty() {
            parts.push(out);
        }
        if !err.is_empty() {
            parts.push(err);
        }
        if parts.is_empty() {
            "sin salida".to_string()
        } else {
            parts.join("\n")
        }
    }
}

fn run(program: &str, args: &[&str]) -> CmdOut {
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
    {
        Ok(o) => CmdOut {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        Err(e) => CmdOut {
            ok: false,
            stdout: String::new(),
            stderr: format!("no se pudo ejecutar `{program}`: {e}"),
        },
    }
}

/// Como `run`, pero mezcla stdout y stderr (docker logs escribe el stderr del
/// contenedor en su propio stderr). Sin deadlock: cada pipe se lee en un hilo.
fn run_merged(program: &str, args: &[&str]) -> CmdOut {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return CmdOut {
                ok: false,
                stdout: String::new(),
                stderr: format!("no se pudo ejecutar `{program}`: {e}"),
            }
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    let err_pipe = child.stderr.take();
    let handle = err_pipe.map(|mut pipe| {
        thread::spawn(move || {
            let mut buf = String::new();
            let _ = pipe.read_to_string(&mut buf);
            buf
        })
    });
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(h) = handle {
        stderr = h.join().unwrap_or_default();
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    CmdOut { ok, stdout, stderr }
}

fn cmdline(program: &str, args: &[String]) -> String {
    let mut s = String::from(program);
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

fn docker_bin() -> &'static str {
    static BIN: OnceLock<&'static str> = OnceLock::new();
    BIN.get_or_init(|| {
        ["/usr/local/bin/docker", "/usr/bin/docker"]
            .iter()
            .find(|p| Path::new(p).exists())
            .copied()
            .unwrap_or("docker")
    })
}

fn systemctl_bin() -> &'static str {
    if Path::new(SYSTEMCTL).exists() {
        SYSTEMCTL
    } else {
        "systemctl"
    }
}

fn pkexec_bin() -> Option<&'static str> {
    if Path::new(PKEXEC).exists() {
        Some(PKEXEC)
    } else {
        None
    }
}

fn compose(args: &[&str]) -> CmdOut {
    let mut full = vec!["compose", "-f", COMPOSE_FILE];
    full.extend_from_slice(args);
    run(docker_bin(), &full)
}

fn first_err(o: &CmdOut) -> String {
    let stderr = o.stderr.trim();
    if !stderr.is_empty() {
        stderr.to_string()
    } else {
        let stdout = o.stdout.trim();
        if stdout.is_empty() {
            "falló sin salida".to_string()
        } else {
            stdout.to_string()
        }
    }
}

/// Comando que el usuario puede pegar en una terminal si `pkexec` falla.
fn engine_command(start: bool) -> String {
    format!(
        "{} {} {} docker.socket docker",
        pkexec_bin().unwrap_or("sudo"),
        systemctl_bin(),
        if start { "start" } else { "stop" }
    )
}

fn engine_active() -> bool {
    run(systemctl_bin(), &["is-active", "docker"]).stdout.trim() == "active"
}

fn socket_active() -> bool {
    run(systemctl_bin(), &["is-active", "docker.socket"])
        .stdout
        .trim()
        == "active"
}

/// Si el error es "no hay motor", devuelve el comando para levantarlo.
fn engine_hint(err: &str) -> Option<String> {
    if err.contains("Cannot connect to the Docker daemon")
        || err.contains("Is the docker daemon running")
        || err.contains("error during connect")
    {
        Some(engine_command(true))
    } else {
        None
    }
}

// ── estado de los servicios y contenedores ─────────────────────────────────

#[derive(Clone, Default)]
struct SvcState {
    state: String,
    health: String,
    status: String,
}

#[derive(Clone, Default)]
struct Container {
    id: String,
    name: String,
    image: String,
    state: String,
    health: String,
    status: String,
    project: String,
    service: String,
}

impl Container {
    fn is_up(&self) -> bool {
        matches!(self.state.as_str(), "running" | "paused" | "restarting")
    }
}

fn field(v: &serde_json::Value, k: &str) -> String {
    v.get(k)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

/// `docker ps -a --format json` emite NDJSON: un objeto por línea.
fn read_containers() -> Result<Vec<Container>, String> {
    let out = run(docker_bin(), &["ps", "-a", "--format", "json"]);
    if !out.ok {
        return Err(first_err(&out));
    }
    let mut rows: Vec<Container> = Vec::new();
    for line in out.stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let mut project = String::new();
        let mut service = String::new();
        for label in field(&v, "Labels").split(',') {
            if let Some((k, val)) = label.split_once('=') {
                match k.trim() {
                    "com.docker.compose.project" => project = val.trim().to_string(),
                    "com.docker.compose.service" => service = val.trim().to_string(),
                    _ => {}
                }
            }
        }
        let status = field(&v, "Status");
        let state = field(&v, "State");
        // `HealthStatus` queda congelado al último valor antes de parar el
        // contenedor (p. ej. "starting"/"unhealthy" en uno exited): solo vale
        // mientras el contenedor está vivo.
        let mut health = if matches!(state.as_str(), "running" | "paused" | "restarting") {
            field(&v, "HealthStatus")
        } else {
            String::new()
        };
        if health == "none" {
            health.clear();
        }
        if health.is_empty() {
            health = health_from(&status);
        }
        rows.push(Container {
            id: field(&v, "ID"),
            name: field(&v, "Names"),
            image: short_image(&field(&v, "Image")),
            state,
            health,
            status,
            project,
            service,
        });
    }
    Ok(rows)
}

fn health_from(status: &str) -> String {
    if status.contains("(healthy)") {
        "healthy".to_string()
    } else if status.contains("(unhealthy)") {
        "unhealthy".to_string()
    } else if status.contains("(health: starting)") {
        "starting".to_string()
    } else {
        String::new()
    }
}

fn short_image(image: &str) -> String {
    let base = image.split('@').next().unwrap_or(image);
    match base.rsplit_once('/') {
        Some((head, tail)) if head.contains('.') || head.contains(':') => tail.to_string(),
        _ => base.to_string(),
    }
}

// ── métricas (docker stats) ─────────────────────────────────────────────────

#[derive(Clone, Default)]
struct Stats {
    cpu: f32,
    mem_used: u64,
    mem_limit: u64,
    mem_pct: f32,
    pids: u32,
}

/// "10.81%" → 10.81; vacío o "--" → None.
fn parse_pct(s: &str) -> Option<f32> {
    let s = s.replace('%', "");
    let s = s.trim();
    if s.is_empty() || s.starts_with("--") {
        return None;
    }
    s.parse().ok()
}

/// "556.1MiB" → 583113113. KiB/MiB/GiB/TiB son 1024; kB/MB/GB/TB, 1000 (Docker
/// usa ambas: memoria en binario, red en decimal).
fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let value: f64 = num.trim().parse().ok()?;
    let unit = unit.trim();
    let unit = if unit.is_empty() { "B" } else { unit };
    let mut chars = unit.chars();
    let head = chars.next()?.to_ascii_uppercase();
    let step: f64 = if chars.next().is_some_and(|c| c.eq_ignore_ascii_case(&'i')) {
        1024.0
    } else {
        1000.0
    };
    let mut mult = 1.0;
    for _ in 0..match head {
        'B' => 0,
        'K' => 1,
        'M' => 2,
        'G' => 3,
        'T' => 4,
        _ => return None,
    } {
        mult *= step;
    }
    Some((value * mult) as u64)
}

/// `docker stats --no-stream --format json` emite NDJSON. Cada Stats se indexa
/// por ID (12 hex) y también por nombre, por si `docker ps` no trae ID.
fn read_stats() -> Result<HashMap<String, Stats>, String> {
    let out = run(docker_bin(), &["stats", "--no-stream", "--format", "json"]);
    if !out.ok {
        return Err(first_err(&out));
    }
    let mut map: HashMap<String, Stats> = HashMap::new();
    for line in out.stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let mem = field(&v, "MemUsage");
        let (mem_used, mem_limit) = mem
            .split_once('/')
            .map(|(u, l)| (parse_bytes(u).unwrap_or(0), parse_bytes(l).unwrap_or(0)))
            .unwrap_or((0, 0));
        let st = Stats {
            cpu: parse_pct(&field(&v, "CPUPerc")).unwrap_or(0.0),
            mem_used,
            mem_limit,
            mem_pct: parse_pct(&field(&v, "MemPerc")).unwrap_or(0.0),
            pids: field(&v, "PIDs").trim().parse().unwrap_or(0),
        };
        let id = field(&v, "ID");
        let name = field(&v, "Name");
        if !id.is_empty() {
            map.insert(id, st.clone());
        }
        if !name.is_empty() {
            map.insert(name, st.clone());
        }
    }
    Ok(map)
}

/// Bytes humanos: <1 GiB en MiB enteros, si no GiB con un decimal.
fn human_bytes(b: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    let b = b as f64;
    if b < 1024.0 * MIB {
        format!("{:.0} MiB", b / MIB)
    } else {
        format!("{:.1} GiB", b / (1024.0 * MIB))
    }
}

fn find_service<'a>(rows: &'a [Container], svc: &str) -> Option<&'a Container> {
    rows.iter().find(|c| {
        (c.project == CINNAMON_PROJECT || c.project.is_empty())
            && (c.service == svc || (c.service.is_empty() && c.name == svc))
    })
}

fn services_from(rows: &[Container]) -> Vec<(String, SvcState)> {
    let mut out = Vec::new();
    for svc in SERVICES {
        if let Some(c) = find_service(rows, svc) {
            out.push((
                svc.to_string(),
                SvcState {
                    state: c.state.clone(),
                    health: c.health.clone(),
                    status: c.status.clone(),
                },
            ));
        }
    }
    out
}

fn state_of<'a>(rows: &'a [(String, SvcState)], name: &str) -> Option<&'a SvcState> {
    rows.iter().find(|(n, _)| n == name).map(|(_, s)| s)
}

/// Un servicio está listo si su healthcheck dice `healthy`, o si corre y no
/// tiene healthcheck (p. ej. pgadmin).
fn is_ready(st: &SvcState) -> bool {
    st.health == "healthy" || (st.health.is_empty() && st.state == "running")
}

fn is_failed(st: &SvcState) -> bool {
    st.health == "unhealthy" || st.state == "exited" || st.state == "dead"
}

fn group_key(project: &str) -> String {
    if project.is_empty() {
        "Sin proyecto".to_string()
    } else {
        project.to_string()
    }
}

// ── mensajes del hilo de trabajo ───────────────────────────────────────────

#[derive(Clone)]
struct Notice {
    text: String,
    command: Option<String>,
    error: bool,
}

enum Msg {
    Refresh {
        active: bool,
        socket_active: bool,
        version: String,
        containers: Vec<Container>,
        error: Option<String>,
    },
    Log(String),
    EngineBusy(bool),
    GlobalBusy(bool),
    ContainerBusy {
        name: String,
        busy: bool,
    },
    Notice(Option<Notice>),
    ForceRefresh,
    Stats {
        rows: HashMap<String, Stats>,
        error: Option<String>,
    },
}

fn log_line(tx: &Sender<Msg>, text: impl Into<String>) {
    let _ = tx.send(Msg::Log(text.into()));
}

fn log_output(tx: &Sender<Msg>, out: &CmdOut) {
    for line in out.full_output().lines() {
        log_line(tx, format!("  {line}"));
    }
}

fn fail(tx: &Sender<Msg>, text: String, command: Option<String>) {
    log_line(tx, format!("✖ {text}"));
    let _ = tx.send(Msg::Notice(Some(Notice {
        text,
        command,
        error: true,
    })));
}

fn ok_notice(tx: &Sender<Msg>, text: impl Into<String>) {
    log_line(tx, format!("✔ {}", text.into()));
    let _ = tx.send(Msg::Notice(None));
}

fn spawn_refresh(tx: Sender<Msg>) {
    thread::spawn(move || {
        let active = engine_active();
        let socket = if active { true } else { socket_active() };
        let version = run(docker_bin(), &["--version"]).stdout.trim().to_string();
        // Sin motor no se consulta el daemon: `docker ps` por el socket lo
        // volvería a arrancar (docker.socket está activo aunque el .service no).
        let (containers, error) = if active {
            match read_containers() {
                Ok(rows) => (rows, None),
                Err(e) => (Vec::new(), Some(e)),
            }
        } else {
            (Vec::new(), None)
        };
        let _ = tx.send(Msg::Refresh {
            active,
            socket_active: socket,
            version,
            containers,
            error,
        });
    });
}

/// Stats en hilo propio: los ~2 s de `docker stats` no esperan al refresco de
/// `docker ps`. Con el motor apagado no se toca el daemon (el socket lo
/// levantaría).
fn spawn_stats(tx: Sender<Msg>) {
    thread::spawn(move || {
        if !engine_active() {
            return;
        }
        let (rows, error) = match read_stats() {
            Ok(rows) => (rows, None),
            Err(e) => (HashMap::new(), Some(e)),
        };
        let _ = tx.send(Msg::Stats { rows, error });
    });
}

fn engine_action(tx: Sender<Msg>, start: bool) {
    thread::spawn(move || {
        let fallback = engine_command(start);
        if start && engine_active() {
            log_line(&tx, "· el motor ya estaba activo: no hace falta arrancarlo");
            let _ = tx.send(Msg::Notice(Some(Notice {
                text: "El motor ya estaba activo.".to_string(),
                command: None,
                error: false,
            })));
            let _ = tx.send(Msg::EngineBusy(false));
            let _ = tx.send(Msg::ForceRefresh);
            return;
        }
        if !start && !engine_active() && !socket_active() {
            log_line(&tx, "· el motor ya estaba detenido");
            let _ = tx.send(Msg::Notice(Some(Notice {
                text: "El motor ya estaba detenido.".to_string(),
                command: None,
                error: false,
            })));
            let _ = tx.send(Msg::EngineBusy(false));
            let _ = tx.send(Msg::ForceRefresh);
            return;
        }

        // `docker.socket` está activo aunque el servicio no: cualquier cliente
        // que toque /run/docker.sock vuelve a levantar dockerd. Por eso se
        // detiene/arranca junto con el servicio.
        let args: Vec<String> = vec![
            if start { "start" } else { "stop" }.to_string(),
            "docker.socket".to_string(),
            "docker".to_string(),
        ];
        let (program, argv) = match pkexec_bin() {
            Some(pk) => {
                let mut argv = vec![systemctl_bin().to_string()];
                argv.extend(args.iter().cloned());
                (pk, argv)
            }
            None => {
                log_line(
                    &tx,
                    format!("· {PKEXEC} no existe: se intentará {SYSTEMCTL} directo"),
                );
                (systemctl_bin(), args.clone())
            }
        };
        let line = cmdline(program, &argv);
        log_line(&tx, format!("$ {line}"));
        let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        let out = run(program, &refs);
        log_output(&tx, &out);

        if !out.ok {
            // stdout y stderr completos, también en el banner.
            let detail = out.full_output();
            fail(
                &tx,
                format!(
                    "motor: {} falló (código de salida ≠ 0)\n{}\nEjecuta en terminal: {fallback}",
                    if start { "iniciar" } else { "detener" },
                    detail
                ),
                Some(fallback),
            );
        } else {
            let active = engine_active();
            let socket = socket_active();
            if start && !active {
                fail(
                    &tx,
                    "el motor no quedó activo (systemctl start devolvió 0 pero is-active no dice active)\nEjecuta en terminal: "
                        .to_string()
                        + &fallback,
                    Some(fallback),
                );
            } else if !start && (active || socket) {
                let estado = format!(
                    "docker={} docker.socket={}",
                    if active { "activo" } else { "detenido" },
                    if socket { "activo" } else { "detenido" }
                );
                fail(
                    &tx,
                    format!("el motor sigue activo tras detenerlo ({estado})\nEjecuta en terminal: {fallback}"),
                    Some(fallback),
                );
            } else if start {
                ok_notice(&tx, "motor activo");
            } else {
                ok_notice(&tx, "motor detenido (socket incluido)");
            }
        }
        let _ = tx.send(Msg::EngineBusy(false));
        let _ = tx.send(Msg::ForceRefresh);
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContainerOp {
    Start,
    Stop,
    Restart,
    Logs,
}

impl ContainerOp {
    fn verb(self) -> &'static str {
        match self {
            ContainerOp::Start => "start",
            ContainerOp::Stop => "stop",
            ContainerOp::Restart => "restart",
            ContainerOp::Logs => "logs",
        }
    }
}

fn container_action(tx: Sender<Msg>, c: Container, op: ContainerOp) {
    thread::spawn(move || {
        let target = if c.id.is_empty() {
            c.name.clone()
        } else {
            c.id.clone()
        };
        if op == ContainerOp::Logs {
            log_line(
                &tx,
                format!("═══ logs {} (últimas {LOGS_TAIL}) ═══", c.name),
            );
            let out = run_merged(
                docker_bin(),
                &["logs", "--tail", &LOGS_TAIL.to_string(), target.as_str()],
            );
            let text = out.full_output();
            for line in text.lines() {
                log_line(&tx, format!("{} │ {line}", c.name));
            }
            if !out.ok {
                let e = first_err(&out);
                let hint = engine_hint(&e);
                fail(&tx, format!("logs {}: {e}", c.name), hint);
            }
            let _ = tx.send(Msg::ContainerBusy {
                name: c.name.clone(),
                busy: false,
            });
            return;
        }

        let verb = op.verb();
        log_line(
            &tx,
            format!("$ {} {verb} {target}  ({})", docker_bin(), c.name),
        );
        let out = run(docker_bin(), &[verb, target.as_str()]);
        log_output(&tx, &out);
        if out.ok {
            ok_notice(&tx, format!("{}: {verb} ok", c.name));
        } else {
            let e = first_err(&out);
            let hint = engine_hint(&format!("{e}\n{}", out.full_output()));
            fail(&tx, format!("{}: {e}", c.name), hint);
        }
        let _ = tx.send(Msg::ContainerBusy {
            name: c.name.clone(),
            busy: false,
        });
        let _ = tx.send(Msg::ForceRefresh);
    });
}

/// Arranque en serie del proyecto Cinnamon: bases primero y esperando
/// `healthy` entre servicios (dos `ng serve` a la vez agotan la RAM → 137).
fn cinnamon_serial_start(tx: &Sender<Msg>) -> bool {
    log_line(tx, "═══ cinnamon: arranque en serie ═══");
    for (i, svc) in SERVICES.iter().enumerate() {
        let rows = read_containers().unwrap_or_default();
        if let Some(c) = find_service(&rows, svc) {
            if c.state == "running" && (c.health.is_empty() || c.health == "healthy") {
                log_line(tx, format!("[{}/8] {svc}: ya corre, se omite", i + 1));
                continue;
            }
        }
        log_line(tx, format!("[{}/8] {svc}: docker compose up -d", i + 1));
        let out = compose(&["up", "-d", svc]);
        if !out.ok {
            let e = first_err(&out);
            let hint = engine_hint(&format!("{e}\n{}", out.full_output()));
            fail(tx, format!("{svc}: {e}"), hint);
            log_line(tx, "✖ serie detenida");
            return false;
        }
        match wait_ready(tx, svc) {
            Ok(()) => log_line(tx, format!("✔ {svc} listo")),
            Err(e) => {
                fail(tx, format!("{svc}: {e}"), None);
                log_line(tx, "✖ serie detenida");
                return false;
            }
        }
    }
    log_line(tx, "✔ stack Cinnamon arriba");
    true
}

fn wait_ready(tx: &Sender<Msg>, svc: &str) -> Result<(), String> {
    let started = Instant::now();
    let mut last_note = 0u64;
    loop {
        let rows = read_containers().map_err(|e| format!("docker ps falló: {e}"))?;
        let services = services_from(&rows);
        if let Some(st) = state_of(&services, svc) {
            if is_ready(st) {
                return Ok(());
            }
            if is_failed(st) {
                return Err(format!(
                    "contenedor {}",
                    if st.state.is_empty() {
                        "?"
                    } else {
                        st.state.as_str()
                    }
                ));
            }
        }
        let elapsed = started.elapsed().as_secs();
        if elapsed > WAIT_TIMEOUT.as_secs() {
            return Err(format!("timeout tras {elapsed}s esperando healthy"));
        }
        if elapsed / 30 > last_note {
            last_note = elapsed / 30;
            let detail = state_of(&services, svc).map(|s| {
                format!(
                    "{}{}",
                    if s.state.is_empty() {
                        "sin contenedor"
                    } else {
                        s.state.as_str()
                    },
                    if s.health.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", s.health)
                    }
                )
            });
            log_line(
                tx,
                format!(
                    "   … {svc}: {} · {elapsed}s",
                    detail.unwrap_or_else(|| "sin contenedor".to_string())
                ),
            );
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn group_start(tx: Sender<Msg>, project: String, items: Vec<Container>) {
    thread::spawn(move || {
        let label = group_key(&project);
        let ok = if project == CINNAMON_PROJECT {
            cinnamon_serial_start(&tx)
        } else {
            log_line(&tx, format!("═══ {label}: arranque secuencial ═══"));
            let mut ok = true;
            for c in items.iter().filter(|c| !c.is_up()) {
                let target = if c.id.is_empty() {
                    c.name.clone()
                } else {
                    c.id.clone()
                };
                log_line(&tx, format!("$ docker start {target}  ({})", c.name));
                let out = run(docker_bin(), &["start", target.as_str()]);
                log_output(&tx, &out);
                if !out.ok {
                    let e = first_err(&out);
                    let hint = engine_hint(&format!("{e}\n{}", out.full_output()));
                    fail(&tx, format!("{}: {e}", c.name), hint);
                    ok = false;
                    break;
                }
                log_line(&tx, format!("✔ {} arrancado", c.name));
            }
            ok
        };
        if ok {
            ok_notice(&tx, "grupo arrancado");
        }
        let _ = tx.send(Msg::GlobalBusy(false));
        let _ = tx.send(Msg::ForceRefresh);
    });
}

fn group_stop(tx: Sender<Msg>, project: String, items: Vec<Container>) {
    thread::spawn(move || {
        let ids: Vec<String> = items
            .iter()
            .filter(|c| c.is_up())
            .map(|c| {
                if c.id.is_empty() {
                    c.name.clone()
                } else {
                    c.id.clone()
                }
            })
            .collect();
        if ids.is_empty() {
            log_line(&tx, format!("· {project}: no hay contenedores corriendo"));
            let _ = tx.send(Msg::Notice(Some(Notice {
                text: format!("{project}: no hay contenedores corriendo."),
                command: None,
                error: false,
            })));
            let _ = tx.send(Msg::GlobalBusy(false));
            let _ = tx.send(Msg::ForceRefresh);
            return;
        }
        let mut argv: Vec<&str> = vec!["stop"];
        argv.extend(ids.iter().map(|s| s.as_str()));
        log_line(&tx, format!("$ docker {}  ({project})", argv.join(" ")));
        let out = run(docker_bin(), &argv);
        log_output(&tx, &out);
        if out.ok {
            ok_notice(
                &tx,
                format!("{project}: {} contenedor(es) detenido(s)", ids.len()),
            );
        } else {
            let e = first_err(&out);
            let hint = engine_hint(&format!("{e}\n{}", out.full_output()));
            fail(&tx, format!("{project}: {e}"), hint);
        }
        let _ = tx.send(Msg::GlobalBusy(false));
        let _ = tx.send(Msg::ForceRefresh);
    });
}

fn start_all(tx: Sender<Msg>, items: Vec<Container>) {
    group_start(tx, CINNAMON_PROJECT.to_string(), items)
}

fn stop_all(tx: Sender<Msg>) {
    thread::spawn(move || {
        log_line(&tx, "═══ Apagar todo: docker compose stop ═══");
        let out = compose(&["stop"]);
        log_output(&tx, &out);
        if out.ok {
            ok_notice(&tx, "stack Cinnamon detenido");
        } else {
            let e = first_err(&out);
            let hint = engine_hint(&format!("{e}\n{}", out.full_output()));
            fail(&tx, format!("stop: {e}"), hint);
        }
        let _ = tx.send(Msg::GlobalBusy(false));
        let _ = tx.send(Msg::ForceRefresh);
    });
}

// ── app ────────────────────────────────────────────────────────────────────

struct App {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    engine_active: Option<bool>,
    socket_active: Option<bool>,
    docker_version: String,
    containers: Vec<Container>,
    log: VecDeque<String>,
    engine_busy: bool,
    global_busy: bool,
    busy_containers: HashSet<String>,
    refreshing: bool,
    last_refresh: Instant,
    compose_error: Option<String>,
    notice: Option<Notice>,
    confirm_stop_engine: bool,
    stats: HashMap<String, Stats>,
    stats_at: Option<Instant>,
    stats_refreshing: bool,
    stats_error: Option<String>,
}

impl App {
    fn new(tx: Sender<Msg>, rx: Receiver<Msg>) -> Self {
        let mut app = Self {
            tx,
            rx,
            engine_active: None,
            socket_active: None,
            docker_version: String::new(),
            containers: Vec::new(),
            log: VecDeque::new(),
            engine_busy: false,
            global_busy: false,
            busy_containers: HashSet::new(),
            refreshing: true,
            last_refresh: Instant::now(),
            compose_error: None,
            notice: None,
            confirm_stop_engine: false,
            stats: HashMap::new(),
            stats_at: None,
            stats_refreshing: false,
            stats_error: None,
        };
        spawn_refresh(app.tx.clone());
        app.log_push("Panel listo. Refresco automático cada 2s.");
        if pkexec_bin().is_none() {
            app.log_push(format!(
                "Aviso: {PKEXEC} no existe; iniciar/detener motor usará systemctl directo."
            ));
        }
        app
    }

    fn log_push(&mut self, line: impl Into<String>) {
        self.log.push_back(line.into());
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
    }

    fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Refresh {
                    active,
                    socket_active,
                    version,
                    containers,
                    error,
                } => {
                    self.engine_active = Some(active);
                    self.socket_active = Some(socket_active);
                    if !version.is_empty() {
                        self.docker_version = version;
                    }
                    self.containers = containers;
                    self.compose_error = error;
                    self.refreshing = false;
                    if !active {
                        // Motor apagado: las métricas previas ya no valen.
                        self.stats.clear();
                        self.stats_at = None;
                        self.stats_refreshing = false;
                    }
                }
                Msg::Log(line) => self.log_push(line),
                Msg::EngineBusy(b) => self.engine_busy = b,
                Msg::GlobalBusy(b) => self.global_busy = b,
                Msg::ContainerBusy { name, busy } => {
                    if busy {
                        self.busy_containers.insert(name);
                    } else {
                        self.busy_containers.remove(&name);
                    }
                }
                Msg::Notice(n) => self.notice = n,
                Msg::Stats { rows, error } => {
                    self.stats = rows;
                    self.stats_at = Some(Instant::now());
                    self.stats_refreshing = false;
                    if error != self.stats_error {
                        if let Some(e) = &error {
                            self.log_push(format!("docker stats: {e}"));
                        }
                        self.stats_error = error;
                    }
                }
                Msg::ForceRefresh => {
                    if !self.refreshing {
                        self.refreshing = true;
                        self.last_refresh = Instant::now();
                        spawn_refresh(self.tx.clone());
                    }
                }
            }
        }
    }

    fn request_refresh(&mut self) {
        if !self.refreshing {
            self.refreshing = true;
            self.last_refresh = Instant::now();
            spawn_refresh(self.tx.clone());
        }
    }

    /// Stats de un contenedor vivo; `None` si está parado o aún sin datos.
    fn stats_for(&self, c: &Container) -> Option<&Stats> {
        if !c.is_up() {
            return None;
        }
        self.stats.get(&c.id).or_else(|| self.stats.get(&c.name))
    }

    /// Totales del host: CPU sumada, RAM sumada y límite mayor (los 8 comparten
    /// host). `None` si todavía no hay ninguna stats de un contenedor arriba.
    fn stats_total(&self) -> Option<(f32, u64, u64, f32)> {
        let mut any = false;
        let mut cpu = 0.0;
        let mut used = 0u64;
        let mut limit = 0u64;
        let mut pct = 0.0;
        for c in self.containers.iter().filter(|c| c.is_up()) {
            if let Some(s) = self.stats_for(c) {
                any = true;
                cpu += s.cpu;
                used += s.mem_used;
                limit = limit.max(s.mem_limit);
                pct += s.mem_pct;
            }
        }
        any.then_some((cpu, used, limit, pct))
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let mut do_start = false;
        let mut ask_stop = false;
        let mut do_refresh = false;
        let mut want_start_all = false;
        let mut want_stop_all = false;
        let narrow = ui.available_width() < 1100.0;

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Docker Cinnamon")
                        .size(20.0)
                        .strong()
                        .color(TEXT),
                );
                let version = if self.docker_version.is_empty() {
                    "docker —".to_string()
                } else if narrow {
                    format!("Docker {}", short_docker_version(&self.docker_version))
                } else {
                    self.docker_version.clone()
                };
                ui.label(egui::RichText::new(version).size(11.0).color(MUTED));
            });
            ui.add_space(12.0);
            match self.engine_active {
                Some(true) => dot_pill(ui, OK, "Motor activo"),
                Some(false) => dot_pill(ui, ERR, "Motor detenido"),
                None => dot_pill(ui, MUTED, "Motor desconocido"),
            };
            match (self.socket_active, self.engine_active) {
                (Some(true), Some(false)) => pill(ui, "socket activo", WARN),
                (Some(true), _) => pill(ui, "socket activo", MUTED),
                (Some(false), _) => pill(ui, "socket detenido", MUTED),
                (None, _) => pill(ui, "socket —", MUTED),
            };
            match self.stats_total() {
                Some((cpu, used, limit, pct)) => {
                    pill(ui, format!("CPU {cpu:.1}%"), MUTED);
                    pill(
                        ui,
                        format!(
                            "RAM {} / {} ({pct:.1}%)",
                            human_bytes(used),
                            human_bytes(limit)
                        ),
                        MUTED,
                    );
                }
                None => {
                    pill(ui, "CPU —", MUTED);
                    pill(ui, "RAM —", MUTED);
                }
            }
            let up = self.containers.iter().filter(|c| c.is_up()).count();
            pill(ui, format!("{up}/{} arriba", self.containers.len()), MUTED);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Refrescar").clicked() {
                    do_refresh = true;
                }
                let idle = !self.engine_busy;
                if ui
                    .add_enabled(
                        idle && (self.engine_active == Some(true)
                            || self.socket_active == Some(true)),
                        egui::Button::new("Detener motor"),
                    )
                    .clicked()
                {
                    ask_stop = true;
                }
                if ui
                    .add_enabled(
                        idle && self.engine_active != Some(true),
                        egui::Button::new("Iniciar motor"),
                    )
                    .clicked()
                {
                    do_start = true;
                }
                if self.engine_busy {
                    ui.label(
                        egui::RichText::new("esperando polkit…")
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.spinner();
                }
            });
        });

        ui.horizontal(|ui| {
            let idle = !self.global_busy;
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(
                        egui::RichText::new("Levantar todo (serie)")
                            .color(BG)
                            .strong(),
                    )
                    .fill(ACCENT)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(8),
                )
                .on_hover_text("Arranca postgres → mongo → apps esperando healthy")
                .clicked()
            {
                want_start_all = true;
            }
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Apagar todo ({})", CINNAMON_PROJECT)),
                )
                .on_hover_text("docker compose stop del stack Cinnamon")
                .clicked()
            {
                want_stop_all = true;
            }
            if self.global_busy {
                ui.spinner();
                ui.label(
                    egui::RichText::new("operación en curso…")
                        .size(11.0)
                        .color(MUTED),
                );
            }
            ui.separator();
            ui.label(
                egui::RichText::new(format!("{} contenedor(es)", self.containers.len()))
                    .size(11.0)
                    .color(MUTED),
            );
        });

        if do_refresh {
            self.request_refresh();
        }
        if do_start {
            self.engine_busy = true;
            engine_action(self.tx.clone(), true);
        }
        if ask_stop {
            // "Detener motor" apaga TODOS los contenedores: confirmación explícita.
            self.confirm_stop_engine = true;
        }
        if want_start_all {
            self.global_busy = true;
            let items = self
                .containers
                .iter()
                .filter(|c| c.project == CINNAMON_PROJECT)
                .cloned()
                .collect();
            start_all(self.tx.clone(), items);
        }
        if want_stop_all {
            self.global_busy = true;
            stop_all(self.tx.clone());
        }
    }

    /// Modal de confirmación de "Detener motor". Devuelve true si se confirmó.
    fn confirm_stop_modal(&mut self, ctx: &egui::Context) -> bool {
        if !self.confirm_stop_engine {
            return false;
        }
        let mut choice: Option<bool> = None;
        egui::Modal::new(egui::Id::unique("confirm_stop_engine")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.label(
                egui::RichText::new("¿Detener el motor Docker?")
                    .size(16.0)
                    .strong(),
            );
            ui.add_space(6.0);
            ui.label(
                "Se detendrá el motor y se apagarán TODOS los contenedores de TODOS \
                 los proyectos (no solo Cinnamon). Los datos en volúmenes se conservan.",
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(engine_command(false))
                    .monospace()
                    .weak(),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Sí, detener todo").clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancelar").clicked() {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => {
                self.confirm_stop_engine = false;
                self.engine_busy = true;
                engine_action(self.tx.clone(), false);
                false
            }
            Some(false) => {
                self.confirm_stop_engine = false;
                false
            }
            None => false,
        }
    }

    fn body(&mut self, ui: &mut egui::Ui) {
        let containers = self.containers.clone();
        let mut req_group_start: Option<(String, Vec<Container>)> = None;
        let mut req_group_stop: Option<(String, Vec<Container>)> = None;
        let mut container_action_req: Option<(Container, ContainerOp)> = None;

        if let Some(notice) = self.notice.clone() {
            ui.add_space(2.0);
            egui::Frame::new()
                .inner_margin(8.0)
                .corner_radius(8)
                .stroke(egui::Stroke::new(
                    1.0,
                    tint(if notice.error { ERR } else { OK }, 70),
                ))
                .fill(tint(if notice.error { ERR } else { OK }, 28))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(&notice.text)
                                .color(if notice.error { ERR } else { OK })
                                .monospace(),
                        );
                        if ui.small_button("ocultar").clicked() {
                            self.notice = None;
                        }
                    });
                    if let Some(cmd) = notice.command.clone() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&cmd).monospace().strong());
                            if ui.small_button("Copiar comando").clicked() {
                                ui.ctx().copy_text(cmd.clone());
                                self.log_push(format!("copiado al portapapeles: {cmd}"));
                            }
                        });
                        ui.label(
                            egui::RichText::new(
                                "Pégalo en una terminal (Konsole) si polkit no muestra el diálogo.",
                            )
                            .weak(),
                        );
                    }
                });
        }
        if let Some(err) = self.compose_error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("docker ps: {err}")).color(ERR));
            });
        }

        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // ── todos los contenedores del daemon ──────────────────────
                ui.label(
                    egui::RichText::new("Contenedores (docker ps -a)")
                        .strong()
                        .size(15.0),
                );
                ui.label(
                    egui::RichText::new(
                        "Todos los proyectos del daemon. Agrupados por proyecto compose; «Sin proyecto» para contenedores sueltos.",
                    )
                    .weak(),
                );
                ui.add_space(4.0);
                if containers.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        egui::Frame::new()
                            .fill(CARD)
                            .stroke(egui::Stroke::new(1.0, BORDER))
                            .corner_radius(10)
                            .inner_margin(egui::Margin::symmetric(12, 16))
                            .show(ui, |ui| {
                                ui.set_width(360.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        egui::RichText::new("Sin contenedores (o motor detenido).")
                                            .size(14.0)
                                            .color(TEXT),
                                    );
                                    ui.label(
                                        egui::RichText::new(
                                            "Arranca el motor o levanta el stack para ver las tarjetas.",
                                        )
                                        .size(11.0)
                                        .color(MUTED),
                                    );
                                });
                            });
                    });
                }

                let mut groups: BTreeMap<String, Vec<Container>> = BTreeMap::new();
                for c in &containers {
                    groups
                        .entry(c.project.clone())
                        .or_default()
                        .push(c.clone());
                }
                let mut keys: Vec<String> = groups.keys().cloned().collect();
                keys.sort_by_key(|k| {
                    if k == CINNAMON_PROJECT {
                        format!("0{k}")
                    } else if k.is_empty() {
                        format!("2{k}")
                    } else {
                        format!("1{k}")
                    }
                });

                let mut first_group = true;
                for key in keys {
                    let mut items = groups.remove(&key).unwrap_or_default();
                    let label = group_key(&key);
                    let up = items.iter().filter(|c| c.is_up()).count();
                    let can_start = !self.global_busy && items.iter().any(|c| !c.is_up());
                    let can_stop = !self.global_busy && up > 0;
                    // Cinnamon sigue el orden de arranque serie; el resto, alfabético.
                    if key == CINNAMON_PROJECT {
                        items.sort_by_key(|c| {
                            SERVICES
                                .iter()
                                .position(|s| *s == c.service || *s == c.name)
                                .unwrap_or(SERVICES.len())
                        });
                    } else {
                        items.sort_by(|a, b| a.name.cmp(&b.name));
                    }
                    if first_group {
                        first_group = false;
                    } else {
                        ui.separator();
                    }
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&label)
                                .strong()
                                .color(if key == CINNAMON_PROJECT { INFO } else { TEXT }),
                        );
                        ui.label(
                            egui::RichText::new(format!("{up}/{} arriba", items.len()))
                                .size(11.0)
                                .color(MUTED),
                        );
                        ui.add_space(8.0);
                        if ui
                            .add_enabled(can_start, egui::Button::new("Arrancar todos"))
                            .on_hover_text(if key == CINNAMON_PROJECT {
                                "Serie con espera healthy: postgres → mongo → apps"
                            } else {
                                "docker start de los contenedores parados"
                            })
                            .clicked()
                        {
                            req_group_start = Some((key.clone(), items.clone()));
                        }
                        if ui
                            .add_enabled(can_stop, egui::Button::new("Detener todos"))
                            .clicked()
                        {
                            req_group_stop = Some((key.clone(), items.clone()));
                        }
                        if key == CINNAMON_PROJECT {
                            ui.label(
                                egui::RichText::new("orden serie + healthy")
                                    .size(11.0)
                                    .color(MUTED),
                            );
                            let svc_up = SERVICES
                                .iter()
                                .filter(|s| {
                                    items
                                        .iter()
                                        .any(|c| c.is_up() && (c.service == **s || c.name == **s))
                                })
                                .count();
                            ui.label(
                                egui::RichText::new(format!(
                                    "{svc_up} de {} servicios",
                                    SERVICES.len()
                                ))
                                .size(11.0)
                                .color(MUTED),
                            );
                        }
                    });

                    for c in &items {
                        let busy = self.busy_containers.contains(&c.name);
                        let blocked = busy || self.global_busy;
                        let stats = self.stats_for(c);
                        egui::Frame::new()
                            .fill(CARD)
                            .stroke(egui::Stroke::new(1.0, BORDER))
                            .corner_radius(10)
                            .inner_margin(egui::Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.x = 6.0;
                                // Línea 1: estado, nombre, chips y acciones.
                                ui.horizontal(|ui| {
                                    let name_w =
                                        text_width(ui, &c.name, 13.0, true).min(220.0);
                                    status_dot(ui, container_state_color(c));
                                    clipped_label(
                                        ui,
                                        egui::RichText::new(&c.name)
                                            .monospace()
                                            .size(13.0)
                                            .strong()
                                            .color(TEXT),
                                        name_w,
                                        16.0,
                                    )
                                    .on_hover_text(&c.name);
                                    pill(
                                        ui,
                                        if c.service.is_empty() {
                                            "—"
                                        } else {
                                            c.service.as_str()
                                        },
                                        INFO,
                                    )
                                    .on_hover_text("servicio compose");
                                    pill(
                                        ui,
                                        if c.state.is_empty() {
                                            "—"
                                        } else {
                                            c.state.as_str()
                                        },
                                        container_state_color(c),
                                    )
                                    .on_hover_text(&c.status);
                                    if !c.health.is_empty() {
                                        pill(ui, c.health.clone(), health_color_str(&c.health))
                                            .on_hover_text("healthcheck");
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if busy {
                                                ui.spinner();
                                            }
                                            if ui
                                                .add_enabled(
                                                    !blocked,
                                                    egui::Button::new("Logs").small(),
                                                )
                                                .clicked()
                                            {
                                                container_action_req =
                                                    Some((c.clone(), ContainerOp::Logs));
                                            }
                                            if c.is_up() {
                                                if ui
                                                    .add_enabled(
                                                        !blocked,
                                                        egui::Button::new("Reiniciar").small(),
                                                    )
                                                    .clicked()
                                                {
                                                    container_action_req =
                                                        Some((c.clone(), ContainerOp::Restart));
                                                }
                                                if ui
                                                    .add_enabled(
                                                        !blocked,
                                                        egui::Button::new("Detener").small(),
                                                    )
                                                    .clicked()
                                                {
                                                    container_action_req =
                                                        Some((c.clone(), ContainerOp::Stop));
                                                }
                                            } else if ui
                                                .add_enabled(
                                                    !blocked,
                                                    egui::Button::new("Iniciar").small(),
                                                )
                                                .clicked()
                                            {
                                                container_action_req =
                                                    Some((c.clone(), ContainerOp::Start));
                                            }
                                        },
                                    );
                                });
                                // Línea 2: imagen · uptime a la izquierda, métricas a la derecha.
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    let budget = (ui.available_width() - 340.0).max(80.0);
                                    let img_w =
                                        text_width(ui, &c.image, 11.0, false).min(budget * 0.6);
                                    let status_w = (budget - img_w - 14.0).max(24.0);
                                    clipped_label(
                                        ui,
                                        egui::RichText::new(&c.image)
                                            .size(11.0)
                                            .color(MUTED),
                                        img_w,
                                        14.0,
                                    )
                                    .on_hover_text(&c.image);
                                    ui.label(
                                        egui::RichText::new("·").size(11.0).color(MUTED),
                                    );
                                    clipped_label(
                                        ui,
                                        egui::RichText::new(&c.status)
                                            .size(11.0)
                                            .color(MUTED),
                                        status_w,
                                        14.0,
                                    )
                                    .on_hover_text(&c.status);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            match stats {
                                                Some(st) => {
                                                    if st.pids > 0 {
                                                        ui.label(
                                                            egui::RichText::new(format!(
                                                                "{} pids",
                                                                st.pids
                                                            ))
                                                            .size(10.0)
                                                            .color(MUTED),
                                                        );
                                                    }
                                                    stat_text(
                                                        ui,
                                                        "RAM",
                                                        format!(
                                                            "{} {:.1}%",
                                                            human_bytes(st.mem_used),
                                                            st.mem_pct
                                                        ),
                                                        Some(st.mem_pct / 100.0),
                                                        ACCENT,
                                                    );
                                                    stat_text(
                                                        ui,
                                                        "CPU",
                                                        format!("{:.1}%", st.cpu),
                                                        Some(st.cpu / 100.0),
                                                        ACCENT,
                                                    );
                                                }
                                                None => {
                                                    stat_text(
                                                        ui,
                                                        "RAM",
                                                        "—".to_string(),
                                                        None,
                                                        MUTED,
                                                    );
                                                    stat_text(
                                                        ui,
                                                        "CPU",
                                                        "—".to_string(),
                                                        None,
                                                        MUTED,
                                                    );
                                                }
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(6.0);
                    }
                }
            });

        if let Some((project, items)) = req_group_start {
            self.global_busy = true;
            group_start(self.tx.clone(), group_tag(&project), items);
        }
        if let Some((project, items)) = req_group_stop {
            self.global_busy = true;
            group_stop(self.tx.clone(), group_tag(&project), items);
        }
        if let Some((c, op)) = container_action_req {
            self.busy_containers.insert(c.name.clone());
            container_action(self.tx.clone(), c, op);
        }
    }

    fn log_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Log").strong().color(TEXT));
            if ui.small_button("Limpiar").clicked() {
                self.log.clear();
            }
            if ui
                .small_button("Copiar")
                .on_hover_text("Copia todo el log al portapapeles")
                .clicked()
            {
                ui.ctx()
                    .copy_text(self.log.iter().cloned().collect::<Vec<_>>().join("\n"));
            }
            ui.label(
                egui::RichText::new(format!("{} / {LOG_CAP}", self.log.len()))
                    .size(11.0)
                    .color(MUTED),
            );
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in &self.log {
                    ui.label(egui::RichText::new(line).monospace().size(11.5).color(TEXT));
                }
            });
    }
}

/// El grupo "Sin proyecto" viaja como cadena vacía hacia los hilos.
fn group_tag(project: &str) -> String {
    if project == "Sin proyecto" {
        String::new()
    } else {
        project.to_string()
    }
}

fn container_state_color(c: &Container) -> egui::Color32 {
    match c.state.as_str() {
        "running" => OK,
        "exited" | "dead" => ERR,
        "restarting" | "paused" | "created" => WARN,
        _ => MUTED,
    }
}

fn health_color_str(health: &str) -> egui::Color32 {
    match health {
        "healthy" => OK,
        "unhealthy" => ERR,
        "starting" => WARN,
        _ => MUTED,
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain();
        self.confirm_stop_modal(ui.ctx());

        if !self.refreshing && self.last_refresh.elapsed() >= REFRESH_EVERY {
            self.request_refresh();
        }
        if self.engine_active == Some(true)
            && !self.stats_refreshing
            && self.stats_at.is_none_or(|t| t.elapsed() >= STATS_EVERY)
        {
            self.stats_refreshing = true;
            spawn_stats(self.tx.clone());
        }
        ui.ctx().request_repaint_after(Duration::from_millis(500));

        let header_frame = egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .stroke(egui::Stroke::new(1.0, BORDER));
        egui::Panel::top("header")
            .frame(header_frame)
            .show(ui, |ui| self.header(ui));

        let max_log = ui.ctx().content_rect().width() * 0.55;
        let log_frame = egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(egui::Margin::same(10))
            .stroke(egui::Stroke::new(1.0, BORDER));
        egui::Panel::right("log")
            .resizable(true)
            .default_size(360.0)
            .min_size(240.0)
            .max_size(max_log)
            .frame(log_frame)
            .show(ui, |ui| self.log_panel(ui));

        egui::CentralPanel::default().show(ui, |ui| self.body(ui));
    }
}

// ── CLI ────────────────────────────────────────────────────────────────────

fn cli_status() -> i32 {
    let active = engine_active();
    println!("Motor: {}", if active { "activo" } else { "detenido" });
    println!(
        "Socket: {}",
        if socket_active() {
            "activo"
        } else {
            "detenido"
        }
    );
    let version = run(docker_bin(), &["--version"]).stdout.trim().to_string();
    if !version.is_empty() {
        println!("Docker: {version}");
    }
    let containers = if active {
        match read_containers() {
            Ok(rows) => Some(rows),
            Err(e) => {
                eprintln!("docker ps -a: {e}");
                None
            }
        }
    } else {
        None
    };
    let rows = services_from(containers.as_deref().unwrap_or(&[]));
    println!("{:<15} {:<10} {:<9} DETALLE", "SERVICIO", "ESTADO", "SALUD");
    for svc in SERVICES {
        match state_of(&rows, svc) {
            Some(st) => println!(
                "{:<15} {:<10} {:<9} {}",
                svc,
                if st.state.is_empty() {
                    "—"
                } else {
                    st.state.as_str()
                },
                if st.health.is_empty() {
                    "—"
                } else {
                    st.health.as_str()
                },
                st.status
            ),
            None => {
                let reason = if active {
                    "sin contenedor"
                } else {
                    "motor detenido"
                };
                println!("{:<15} {:<10} {:<9} {}", svc, "detenido", "—", reason)
            }
        }
    }
    0
}

fn cli_containers() -> i32 {
    let active = engine_active();
    println!("Motor: {}", if active { "activo" } else { "detenido" });
    if !active {
        println!("No hay motor: no se consulta el daemon (evita reactivarlo por el socket).");
        println!(
            "{:<16} {:<26} {:<11} {:<10} SIN DATOS",
            "PROYECTO", "CONTENEDOR", "ESTADO", "SALUD"
        );
        for svc in SERVICES {
            println!(
                "{:<16} {:<26} {:<11} {:<10} sin contenedor",
                CINNAMON_PROJECT, svc, "detenido", "—"
            );
        }
        return 0;
    }
    match read_containers() {
        Ok(rows) => {
            if rows.is_empty() {
                println!("Sin contenedores.");
                return 0;
            }
            println!(
                "{:<16} {:<26} {:<11} {:<10} {:<26} DETALLE",
                "PROYECTO", "CONTENEDOR", "ESTADO", "SALUD", "IMAGEN"
            );
            let mut sorted = rows.clone();
            sorted.sort_by(|a, b| {
                (group_key(&a.project), &a.name).cmp(&(group_key(&b.project), &b.name))
            });
            for c in sorted {
                let health = if c.health.is_empty() {
                    "—"
                } else {
                    c.health.as_str()
                };
                println!(
                    "{:<16} {:<26} {:<11} {:<10} {:<26} {}",
                    group_key(&c.project),
                    c.name,
                    if c.state.is_empty() {
                        "—"
                    } else {
                        c.state.as_str()
                    },
                    health,
                    c.image,
                    c.status
                );
            }
            println!("\nTotal: {} contenedor(es)", rows.len());
        }
        Err(e) => {
            eprintln!("docker ps -a: {e}");
        }
    }
    0
}

fn usage() {
    println!("cinnamon-docker — panel de Docker y del stack Cinnamon");
    println!();
    println!("Uso:");
    println!("  cinnamon-docker              abre la ventana");
    println!("  cinnamon-docker --status     motor + 8 servicios Cinnamon, sale con 0");
    println!(
        "  cinnamon-docker --containers todos los contenedores (proyecto/estado/salud), sale con 0"
    );
    println!("  cinnamon-docker --help       esta ayuda");
    println!();
    println!("Compose: {COMPOSE_FILE}");
}

fn main() -> eframe::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--status") | Some("-s") => std::process::exit(cli_status()),
        Some("--containers") | Some("-c") => std::process::exit(cli_containers()),
        Some("--help") | Some("-h") => {
            usage();
            return Ok(());
        }
        _ => {}
    }

    let (tx, rx) = channel();
    let app = App::new(tx, rx);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Docker Cinnamon")
            .with_inner_size([1320.0, 840.0])
            .with_min_inner_size([980.0, 560.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "Docker Cinnamon",
        options,
        Box::new(move |cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_bytes_binarios_y_decimales() {
        // Valores reales de `docker stats`.
        assert_eq!(parse_bytes("556.1MiB"), Some(583_113_113));
        assert_eq!(parse_bytes("15.56GiB"), Some(16_707_422_781));
        assert_eq!(parse_bytes("0B"), Some(0));
        assert_eq!(parse_bytes("123B"), Some(123));
        assert_eq!(parse_bytes("1.2kB"), Some(1200));
        assert_eq!(parse_bytes("1.79GB"), Some(1_790_000_000));
        assert_eq!(parse_bytes("12MB"), Some(12_000_000));
        assert_eq!(parse_bytes(""), None);
        assert_eq!(parse_bytes("--"), None);
    }

    #[test]
    fn parsea_porcentajes() {
        assert_eq!(parse_pct("10.81%"), Some(10.81));
        assert_eq!(parse_pct("0.00%"), Some(0.0));
        assert_eq!(parse_pct(" 3.5 % "), Some(3.5));
        assert_eq!(parse_pct("--"), None);
        assert_eq!(parse_pct(""), None);
    }

    #[test]
    fn split_de_memusage() {
        let (used, limit) = "556.1MiB / 15.56GiB"
            .split_once('/')
            .map(|(u, l)| (parse_bytes(u).unwrap_or(0), parse_bytes(l).unwrap_or(0)))
            .unwrap();
        assert_eq!(used, 583_113_113);
        assert_eq!(limit, 16_707_422_781);
    }

    #[test]
    fn bytes_humanos() {
        assert_eq!(human_bytes(583_113_113), "556 MiB");
        assert_eq!(human_bytes(16_707_422_781), "15.6 GiB");
    }
}
