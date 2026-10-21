//! Docker Cinnamon — panel mínimo para arrancar/parar el motor Docker, el
//! stack de docker-compose del proyecto Cinnamon (en serie) y cualquier
//! contenedor del daemon, en serie y sin bloquear la UI.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
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
const DEFAULT_COMPOSE_FILE: &str = "/mnt/disco-local/home/Github/Cinnamon/docker-compose.yml";

/// Valor no vacío de `CINNAMON_DOCKER_COMPOSE`, o la ruta por defecto.
fn resolve_compose_file(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_COMPOSE_FILE)
        .to_string()
}

fn compose_file() -> &'static str {
    static FILE: OnceLock<String> = OnceLock::new();
    FILE.get_or_init(|| {
        let env = std::env::var("CINNAMON_DOCKER_COMPOSE").ok();
        resolve_compose_file(env.as_deref())
    })
}

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

/// Modo de color conmutable en caliente; se persiste en `theme`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ThemeMode {
    Light,
    Dark,
}

impl ThemeMode {
    fn key(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Light => "Claro",
            Self::Dark => "Oscuro",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "light" | "claro" => Some(Self::Light),
            "dark" | "oscuro" => Some(Self::Dark),
            _ => None,
        }
    }

    fn egui_theme(self) -> egui::Theme {
        match self {
            Self::Light => egui::Theme::Light,
            Self::Dark => egui::Theme::Dark,
        }
    }
}

#[derive(Clone, Copy)]
struct Palette {
    bg: egui::Color32,
    surface: egui::Color32,
    card: egui::Color32,
    hover: egui::Color32,
    border: egui::Color32,
    text: egui::Color32,
    muted: egui::Color32,
    accent: egui::Color32,
    on_accent: egui::Color32,
    info: egui::Color32,
    ok: egui::Color32,
    warn: egui::Color32,
    err: egui::Color32,
    shadow: egui::Color32,
    /// Sombra suave de tarjetas: más marcada en oscuro, donde el borde solo no
    /// separa la superficie del fondo.
    card_shadow: egui::Color32,
    gradient_top: egui::Color32,
    gradient_bottom: egui::Color32,
}

const LIGHT_PALETTE: Palette = Palette {
    bg: egui::Color32::from_rgb(0xF2, 0xF2, 0xF7),
    surface: egui::Color32::from_rgba_unmultiplied_const(255, 255, 255, 236),
    card: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
    hover: egui::Color32::from_rgb(0xE8, 0xE8, 0xED),
    border: egui::Color32::from_rgb(0xD1, 0xD1, 0xD6),
    text: egui::Color32::from_rgb(0x1C, 0x1C, 0x1E),
    muted: egui::Color32::from_rgb(0x6C, 0x6C, 0x70),
    accent: egui::Color32::from_rgb(0x0A, 0x7C, 0x6B),
    on_accent: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
    info: egui::Color32::from_rgb(0x00, 0x66, 0xCC),
    ok: egui::Color32::from_rgb(0x24, 0x8A, 0x3D),
    warn: egui::Color32::from_rgb(0xB2, 0x50, 0x00),
    err: egui::Color32::from_rgb(0xD7, 0x00, 0x15),
    shadow: egui::Color32::from_rgba_unmultiplied_const(0, 0, 0, 26),
    card_shadow: egui::Color32::from_rgba_unmultiplied_const(0, 0, 0, 18),
    gradient_top: egui::Color32::from_rgb(0xEA, 0xF7, 0xF4),
    gradient_bottom: egui::Color32::from_rgb(0xF2, 0xF2, 0xF7),
};

const DARK_PALETTE: Palette = Palette {
    bg: egui::Color32::from_rgb(0x0B, 0x0F, 0x12),
    surface: egui::Color32::from_rgba_unmultiplied_const(21, 27, 31, 242),
    card: egui::Color32::from_rgb(0x1C, 0x24, 0x29),
    hover: egui::Color32::from_rgb(0x23, 0x2D, 0x33),
    border: egui::Color32::from_rgb(0x2C, 0x36, 0x3C),
    text: egui::Color32::from_rgb(0xF5, 0xF7, 0xFA),
    muted: egui::Color32::from_rgb(0x9A, 0xA6, 0xAD),
    accent: egui::Color32::from_rgb(0x2D, 0xD4, 0xBF),
    on_accent: egui::Color32::from_rgb(0x06, 0x23, 0x1F),
    info: egui::Color32::from_rgb(0x64, 0xD2, 0xFF),
    ok: egui::Color32::from_rgb(0x30, 0xD1, 0x58),
    warn: egui::Color32::from_rgb(0xFF, 0xD6, 0x0A),
    err: egui::Color32::from_rgb(0xFF, 0x45, 0x3A),
    shadow: egui::Color32::from_rgba_unmultiplied_const(0, 0, 0, 160),
    card_shadow: egui::Color32::from_rgba_unmultiplied_const(0, 0, 0, 60),
    gradient_top: egui::Color32::from_rgb(0x0E, 0x1A, 0x1C),
    gradient_bottom: egui::Color32::from_rgb(0x0B, 0x0F, 0x12),
};

/// Colores de marca: el mismo degradado y motivos del icono instalado, sin
/// depender del tema.
const ICON_TOP: egui::Color32 = egui::Color32::from_rgb(0x2D, 0xD4, 0xBF);
const ICON_BOTTOM: egui::Color32 = egui::Color32::from_rgb(0x0A, 0x7C, 0x6B);
const ICON_INK: egui::Color32 = egui::Color32::from_rgb(0x06, 0x4E, 0x46);
const ICON_ORANGE: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x8A, 0x4A);

/// La UI se dibuja entera en el hilo principal; el atómico solo evita pasar la
/// paleta por cada función de dibujo. No hay otro hilo leyéndolo.
static CURRENT_MODE: AtomicU8 = AtomicU8::new(0);

fn palette_for(mode: ThemeMode) -> &'static Palette {
    match mode {
        ThemeMode::Light => &LIGHT_PALETTE,
        ThemeMode::Dark => &DARK_PALETTE,
    }
}

fn current_mode() -> ThemeMode {
    if CURRENT_MODE.load(Ordering::Relaxed) == 1 {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}

fn set_current_mode(mode: ThemeMode) {
    let v = match mode {
        ThemeMode::Light => 0,
        ThemeMode::Dark => 1,
    };
    CURRENT_MODE.store(v, Ordering::Relaxed);
}

fn pal() -> &'static Palette {
    palette_for(current_mode())
}

fn theme_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("cinnamon-docker").join("theme"))
}

/// Preferencia guardada; cualquier error cae en el tema claro.
fn load_theme() -> ThemeMode {
    theme_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| ThemeMode::parse(&s))
        .unwrap_or(ThemeMode::Light)
}

/// Guarda la preferencia sin ruido: si falla, la app sigue con el tema actual.
fn save_theme(mode: ThemeMode) {
    let Some(path) = theme_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, mode.key());
}

/// Mismo color con otra opacidad: fondos de pills y avisos.
fn tint(c: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

fn apply_theme(ctx: &egui::Context, mode: ThemeMode) {
    let p = palette_for(mode);
    let mut v = match mode {
        ThemeMode::Light => egui::Visuals::light(),
        ThemeMode::Dark => egui::Visuals::dark(),
    };
    // El gradiente de `paint_window_background` se pinta detrás.
    v.panel_fill = egui::Color32::TRANSPARENT;
    v.window_fill = p.surface;
    v.faint_bg_color = p.card;
    v.extreme_bg_color = p.bg;
    v.code_bg_color = p.card;
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.muted);
    v.selection.bg_fill = tint(p.accent, 70);
    v.selection.stroke = egui::Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.info;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.err;
    v.window_corner_radius = egui::CornerRadius::same(20);
    v.window_stroke = egui::Stroke::new(1.0, p.border);
    v.window_shadow = egui::Shadow {
        offset: [0, 10],
        blur: 30,
        spread: 0,
        color: p.shadow,
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: p.shadow,
    };

    // Botones con radio 12; marcos y contenedores no interactivos con radio 16.
    let button_radius = egui::CornerRadius::same(12);
    let frame_radius = egui::CornerRadius::same(16);
    let border = egui::Stroke::new(1.0, p.border);

    v.widgets.noninteractive.bg_fill = p.surface;
    v.widgets.noninteractive.weak_bg_fill = p.surface;
    v.widgets.noninteractive.bg_stroke = border;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.text);
    v.widgets.noninteractive.corner_radius = frame_radius;

    v.widgets.inactive.bg_fill = p.card;
    v.widgets.inactive.weak_bg_fill = p.card;
    v.widgets.inactive.bg_stroke = border;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, p.text);
    v.widgets.inactive.corner_radius = button_radius;

    v.widgets.hovered.bg_fill = p.hover;
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, p.accent);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, p.text);
    v.widgets.hovered.corner_radius = button_radius;

    v.widgets.active.bg_fill = p.hover;
    v.widgets.active.weak_bg_fill = p.hover;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, p.accent);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, p.accent);
    v.widgets.active.corner_radius = button_radius;

    v.widgets.open = v.widgets.active;

    ctx.set_theme(mode.egui_theme());
    ctx.set_visuals(v);
    ctx.style_mut_of(mode.egui_theme(), |style| {
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 7.0);
        style.spacing.interact_size.y = 32.0;
        style.text_styles = [
            (egui::TextStyle::Heading, egui::FontId::proportional(24.0)),
            (egui::TextStyle::Body, egui::FontId::proportional(13.5)),
            (egui::TextStyle::Button, egui::FontId::proportional(13.0)),
            (egui::TextStyle::Small, egui::FontId::proportional(11.0)),
            (egui::TextStyle::Monospace, egui::FontId::monospace(12.0)),
        ]
        .into_iter()
        .collect();
    });
}

/// Gradiente vertical de dos colores en la capa de fondo.
fn paint_window_background(ctx: &egui::Context) {
    let p = pal();
    let rect = ctx.content_rect();
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), p.gradient_top);
    mesh.colored_vertex(rect.right_top(), p.gradient_top);
    mesh.colored_vertex(rect.left_bottom(), p.gradient_bottom);
    mesh.colored_vertex(rect.right_bottom(), p.gradient_bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(2, 1, 3);
    ctx.layer_painter(egui::LayerId::background())
        .add(egui::Shape::mesh(mesh));
}

/// Control segmentado Claro/Oscuro; devuelve el modo nuevo solo si cambia.
fn theme_switch(ui: &mut egui::Ui, current: ThemeMode) -> Option<ThemeMode> {
    let mut chosen = None;
    egui::Frame::new()
        .fill(tint(pal().muted, 18))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
                for mode in [ThemeMode::Light, ThemeMode::Dark] {
                    let selected = mode == current;
                    let button = egui::Button::new(
                        egui::RichText::new(mode.label())
                            .size(11.0)
                            .color(if selected { pal().text } else { pal().muted }),
                    )
                    .fill(if selected {
                        pal().card
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(8);
                    if ui.add(button).clicked() && !selected {
                        chosen = Some(mode);
                    }
                }
            });
        });
    chosen
}

/// Ancho real de un texto sin envolver: evita huecos y cortes innecesarios.
fn text_width(ui: &egui::Ui, text: &str, size: f32, mono: bool) -> f32 {
    let font = if mono {
        egui::FontId::monospace(size)
    } else {
        egui::FontId::proportional(size)
    };
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, pal().text)
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

/// Barra de 44×4: fondo del borde del tema y relleno proporcional al color semántico.
fn meter(ui: &mut egui::Ui, frac: f32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 4.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2, pal().border);
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
    ui.label(egui::RichText::new(label).size(10.0).color(pal().muted));
}

/// Pill compacta: texto corto sobre el color semántico atenuado. Radio 255 =
/// extremo redondeado (CornerRadius es u8).
fn pill(ui: &mut egui::Ui, text: impl Into<String>, fg: egui::Color32) -> egui::Response {
    egui::Frame::new()
        .fill(tint(fg, 22))
        .corner_radius(255)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text.into()).size(11.5).color(fg))
        })
        .inner
}

/// Pill con punto pintado (la fuente por defecto no trae `●`).
fn dot_pill(ui: &mut egui::Ui, color: egui::Color32, text: &str) -> egui::Response {
    egui::Frame::new()
        .fill(tint(color, 22))
        .corner_radius(255)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                status_dot(ui, color);
                ui.label(egui::RichText::new(text).size(11.5).color(color));
            });
        })
        .response
}

/// Botón suave: fondo tintado, sin borde y radio completo. El padding se fija
/// en un scope para no alterar el resto de la fila.
fn soft_button(
    ui: &mut egui::Ui,
    label: &str,
    color: egui::Color32,
    enabled: bool,
) -> egui::Response {
    let text =
        egui::RichText::new(label)
            .size(11.5)
            .color(if enabled { color } else { pal().muted });
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = egui::vec2(9.0, 4.0);
        ui.add_enabled(
            enabled,
            egui::Button::new(text)
                .fill(tint(color, if enabled { 18 } else { 8 }))
                .stroke(egui::Stroke::NONE)
                .corner_radius(255)
                .min_size(egui::vec2(0.0, 26.0)),
        )
    })
    .inner
}

/// Marca de la app: los mismos píxeles que el icono de bandeja y de ventana.
fn brand_mark(ui: &mut egui::Ui, size: f32) {
    static RGBA: LazyLock<Vec<u8>> = LazyLock::new(|| brand_rgba(128));
    let tex = ui.ctx().load_texture_cached(
        "brand-mark",
        egui::Id::unique("brand-mark"),
        egui::ColorImage::from_rgba_unmultiplied([128, 128], &RGBA),
        egui::TextureOptions::LINEAR,
    );
    ui.add(egui::Image::new(egui::load::SizedTexture::new(
        tex.id(),
        egui::vec2(size, size),
    )))
    .on_hover_text("Docker Cinnamon");
}

/// Tarjeta de resumen: etiqueta, valor grande y detalle. Ocupa el ancho de su
/// columna; el valor baja a 22 px cuando la columna es estrecha.
fn summary_card(ui: &mut egui::Ui, label: &str, value: &str, detail: &str) {
    let value_size = if ui.available_width() < 170.0 {
        22.0
    } else {
        26.0
    };
    egui::Frame::new()
        .fill(pal().card)
        .corner_radius(20)
        .inner_margin(egui::Margin::same(16))
        .stroke(egui::Stroke::new(1.0, pal().border))
        .shadow(egui::Shadow {
            offset: [0, 2],
            blur: 12,
            spread: 0,
            color: pal().card_shadow,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(label).size(11.0).color(pal().muted));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(value)
                    .size(value_size)
                    .strong()
                    .color(pal().text),
            );
            ui.add_space(2.0);
            ui.label(egui::RichText::new(detail).size(11.0).color(pal().muted));
        });
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
    let mut full = vec!["compose", "-f", compose_file(), "-p", CINNAMON_PROJECT];
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
        c.project == CINNAMON_PROJECT
            && (c.service == svc || (c.service.is_empty() && c.name == svc))
    })
}

/// ¿Queda algo por arrancar en este proyecto? Cinnamon cuenta sus 8 servicios
/// por nombre, no por número de contenedores: un duplicado/huérfano no debe
/// deshabilitar el arranque si falta un servicio real.
fn can_start_project(project: &str, items: &[Container]) -> bool {
    if project == CINNAMON_PROJECT {
        SERVICES.iter().any(|svc| {
            items
                .iter()
                .find(|c| c.service == *svc || c.name == *svc)
                .is_none_or(|c| !c.is_up())
        })
    } else {
        items.iter().any(|c| !c.is_up())
    }
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

/// Orden de proyectos: cinnamon primero, resto alfabético y «Sin proyecto» al
/// final.
fn project_rank(project: &str) -> String {
    if project == CINNAMON_PROJECT {
        format!("0{project}")
    } else if project.is_empty() {
        format!("2{project}")
    } else {
        format!("1{project}")
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
    Tray(TrayAction),
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

fn start_all(tx: Sender<Msg>) {
    group_start(tx, CINNAMON_PROJECT.to_string(), Vec::new())
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

// ── bandeja ────────────────────────────────────────────────────────────────

use ksni::blocking::TrayMethods;
use ksni::menu::StandardItem;

#[derive(Clone, PartialEq, Eq)]
enum TrayAction {
    Show,
    EngineStart,
    EngineStop,
    Refresh,
    StartProject(String),
    StopProject(String),
    Quit,
}

#[derive(Clone, PartialEq, Default)]
struct TrayState {
    engine_active: Option<bool>,
    socket_active: Option<bool>,
    busy: bool,
    selected_project: String,
    selected_running: usize,
    selected_stopped: usize,
    selected_can_start: bool,
}

struct CinnamonTray {
    tx: Sender<Msg>,
    state: Arc<Mutex<TrayState>>,
    ctx: Option<egui::Context>,
}

impl CinnamonTray {
    fn send(&self, action: TrayAction) {
        let _ = self.tx.send(Msg::Tray(action));
    }

    fn wake(&self) {
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    fn show(&self) {
        if let Some(ctx) = &self.ctx {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.wake();
    }

    fn snapshot(&self) -> TrayState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl ksni::Tray for CinnamonTray {
    fn id(&self) -> String {
        "cinnamon-docker".to_string()
    }

    fn title(&self) -> String {
        let s = self.snapshot();
        let total = s.selected_running + s.selected_stopped;
        let mut title = format!("Docker Cinnamon · {}/{total} arriba", s.selected_running);
        match s.engine_active {
            Some(true) => title.push_str(" · motor activo"),
            Some(false) => title.push_str(" · motor detenido"),
            None => {}
        }
        title
    }

    fn icon_name(&self) -> String {
        installed_icon_name()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![tray_icon()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let s = self.snapshot();
        let engine = match s.engine_active {
            Some(true) => "motor activo",
            Some(false) => "motor detenido",
            None => "motor —",
        };
        ksni::ToolTip {
            title: self.title(),
            description: format!(
                "Proyecto: {} · {}/{} arriba · {engine}",
                group_key(&s.selected_project),
                s.selected_running,
                s.selected_running + s.selected_stopped
            ),
            icon_pixmap: vec![tray_icon()],
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.show();
        self.send(TrayAction::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let s = self.snapshot();
        let project = group_key(&s.selected_project);
        let item = |label: String, icon: &str, enabled: bool, action: TrayAction| {
            StandardItem {
                label,
                icon_name: icon.to_string(),
                enabled,
                // `TrayAction` ya no es `Copy`: el clon ocurre al pulsar.
                activate: Box::new(move |t: &mut Self| t.send(action.clone())),
                ..Default::default()
            }
            .into()
        };
        vec![
            StandardItem {
                label: "Abrir panel".to_string(),
                icon_name: "window-new".to_string(),
                activate: Box::new(|t: &mut Self| {
                    t.show();
                    t.send(TrayAction::Show);
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            item(
                "Iniciar motor".to_string(),
                "media-playback-start",
                !s.busy && s.engine_active != Some(true),
                TrayAction::EngineStart,
            ),
            item(
                "Detener motor".to_string(),
                "media-playback-stop",
                !s.busy && (s.engine_active == Some(true) || s.socket_active == Some(true)),
                TrayAction::EngineStop,
            ),
            item(
                "Refrescar".to_string(),
                "view-refresh",
                !s.busy,
                TrayAction::Refresh,
            ),
            ksni::MenuItem::Separator,
            item(
                format!("Levantar {project}"),
                "media-playback-start",
                !s.busy && s.selected_can_start,
                TrayAction::StartProject(s.selected_project.clone()),
            ),
            item(
                format!("Apagar {project}"),
                "media-playback-stop",
                !s.busy && s.selected_running > 0,
                TrayAction::StopProject(s.selected_project.clone()),
            ),
            ksni::MenuItem::Separator,
            item(
                "Salir".to_string(),
                "application-exit",
                true,
                TrayAction::Quit,
            ),
        ]
    }
}

/// Rectángulo redondeado en coordenadas normalizadas 0..1; el suavizado lo
/// aporta el supersampling de `brand_rgba`.
fn inside_rounded(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let dx = (x0 + r - x).max(x - (x1 - r)).max(0.0);
    let dy = (y0 + r - y).max(y - (y1 - r)).max(0.0);
    dx * dx + dy * dy <= r * r
}

fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    egui::Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

fn inside_ellipse(x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32) -> bool {
    let dx = (x - cx) / rx;
    let dy = (y - cy) / ry;
    dx * dx + dy * dy <= 1.0
}

/// Ray casting: cuenta los cruces del rayo horizontal con cada arista.
fn inside_polygon(x: f32, y: f32, points: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Un píxel de la marca en coordenadas 0..1, en el mismo orden que el SVG.
fn brand_pixel(x: f32, y: f32) -> [u8; 4] {
    if !inside_rounded(x, y, 0.039, 0.039, 0.961, 0.961, 0.219) {
        return [0, 0, 0, 0];
    }
    let bg = mix(ICON_TOP, ICON_BOTTOM, ((x + y) * 0.5).clamp(0.0, 1.0));
    let tail = inside_polygon(
        x,
        y,
        &[
            (0.721, 0.631),
            (0.900, 0.420),
            (0.869, 0.631),
            (0.920, 0.820),
            (0.721, 0.721),
        ],
    );
    let fin = inside_polygon(x, y, &[(0.381, 0.779), (0.480, 0.920), (0.551, 0.801)]);
    let white = tail
        || fin
        || inside_ellipse(x, y, 0.459, 0.660, 0.301, 0.150)
        || inside_ellipse(x, y, 0.350, 0.629, 0.131, 0.109);
    if white {
        return [255, 255, 255, 255];
    }
    let bars = inside_rounded(x, y, 0.310, 0.369, 0.691, 0.424, 0.027)
        || inside_rounded(x, y, 0.310, 0.449, 0.691, 0.504, 0.027)
        || inside_rounded(x, y, 0.310, 0.529, 0.691, 0.584, 0.027);
    if bars || inside_ellipse(x, y, 0.270, 0.619, 0.0215, 0.0215) {
        return [ICON_INK.r(), ICON_INK.g(), ICON_INK.b(), 255];
    }
    if inside_ellipse(x, y, 0.801, 0.199, 0.045, 0.045) {
        return [ICON_ORANGE.r(), ICON_ORANGE.g(), ICON_ORANGE.b(), 255];
    }
    [bg.r(), bg.g(), bg.b(), 255]
}

/// RGBA con supersampling 2×2; el color se promedia ponderado por alpha para
/// que los bordes no arrastren negro.
fn brand_rgba(size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    let step = 1.0 / size as f32;
    for py in 0..size {
        for px in 0..size {
            let (mut acc_r, mut acc_g, mut acc_b, mut acc_a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..2 {
                for sx in 0..2 {
                    let c = brand_pixel(
                        (px as f32 + (sx as f32 + 0.5) * 0.5) * step,
                        (py as f32 + (sy as f32 + 0.5) * 0.5) * step,
                    );
                    let a = c[3] as u32;
                    acc_r += c[0] as u32 * a;
                    acc_g += c[1] as u32 * a;
                    acc_b += c[2] as u32 * a;
                    acc_a += a;
                }
            }
            let i = (py * size + px) * 4;
            let div = acc_a.max(1);
            out[i] = (acc_r / div) as u8;
            out[i + 1] = (acc_g / div) as u8;
            out[i + 2] = (acc_b / div) as u8;
            out[i + 3] = (acc_a / 4) as u8;
        }
    }
    out
}

/// Nombre del icono instalado; vacío si no se instaló y hay que usar el pixmap.
fn installed_icon_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let mut bases: Vec<PathBuf> = Vec::new();
        if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
            bases.push(PathBuf::from(dir));
        }
        if let Some(home) = std::env::var_os("HOME") {
            bases.push(PathBuf::from(home).join(".local/share"));
        }
        // `PREFIX=/usr/local` instala el icono en /usr/local/share: se recorre
        // XDG_DATA_DIRS (Docker/Flatpak lo usan) además de /usr/share.
        let dirs = std::env::var_os("XDG_DATA_DIRS")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        bases.extend(std::env::split_paths(&dirs));
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let installed = bases.iter().filter(|b| seen.insert((*b).clone())).any(|b| {
            b.join("icons/hicolor/scalable/apps/cinnamon-docker.svg")
                .is_file()
        });
        if installed {
            "cinnamon-docker".to_string()
        } else {
            String::new()
        }
    })
    .clone()
}

fn tray_icon() -> ksni::Icon {
    static ICON: LazyLock<ksni::Icon> = LazyLock::new(|| {
        let rgba = brand_rgba(128);
        // ksni espera ARGB, no RGBA.
        let data = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, a]| [a, r, g, b])
            .collect();
        ksni::Icon {
            width: 128,
            height: 128,
            data,
        }
    });
    ICON.clone()
}

fn app_icon() -> egui::IconData {
    egui::IconData {
        rgba: brand_rgba(128),
        width: 128,
        height: 128,
    }
}

// ── app ────────────────────────────────────────────────────────────────────

struct App {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    theme: ThemeMode,
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
    /// Proyecto compose sobre el que actúan los botones del header; "" =
    /// «Sin proyecto».
    selected_project: String,
    tray_handle: Option<ksni::blocking::Handle<CinnamonTray>>,
    tray_state: Arc<Mutex<TrayState>>,
    tray_ok: bool,
    tray_ctx_set: bool,
    allow_close: bool,
    show_requested: bool,
    quit_requested: bool,
}

/// Botones del motor, reutilizados en la fila 1 (ancho) o la fila 2 (estrecho).
/// Devuelve (iniciar, detener, refrescar).
fn engine_buttons(
    ui: &mut egui::Ui,
    active: Option<bool>,
    socket: Option<bool>,
    busy: bool,
) -> (bool, bool, bool) {
    let mut start = false;
    let mut stop = false;
    let mut refresh = false;
    if ui.button("Refrescar").clicked() {
        refresh = true;
    }
    let idle = !busy;
    if ui
        .add_enabled(
            idle && (active == Some(true) || socket == Some(true)),
            egui::Button::new("Detener motor"),
        )
        .clicked()
    {
        stop = true;
    }
    if ui
        .add_enabled(
            idle && active != Some(true),
            egui::Button::new("Iniciar motor"),
        )
        .clicked()
    {
        start = true;
    }
    if busy {
        ui.label(
            egui::RichText::new("esperando polkit…")
                .size(11.0)
                .color(pal().muted),
        );
        ui.spinner();
    }
    (start, stop, refresh)
}

impl App {
    fn new(tx: Sender<Msg>, rx: Receiver<Msg>) -> Self {
        let theme = load_theme();
        set_current_mode(theme);
        let tray_state = Arc::new(Mutex::new(TrayState::default()));
        let tray = CinnamonTray {
            tx: tx.clone(),
            state: tray_state.clone(),
            ctx: None,
        };
        let (tray_handle, tray_ok, tray_err) = match tray.spawn() {
            Ok(h) => (Some(h), true, None),
            Err(e) => (None, false, Some(e.to_string())),
        };
        let mut app = Self {
            tx,
            rx,
            theme,
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
            selected_project: CINNAMON_PROJECT.to_string(),
            tray_handle,
            tray_state,
            tray_ok,
            tray_ctx_set: false,
            allow_close: false,
            show_requested: false,
            quit_requested: false,
        };
        spawn_refresh(app.tx.clone());
        app.log_push("Panel listo. Refresco automático cada 2s.");
        match tray_err {
            Some(e) => app.log_push(format!("Bandeja no disponible: {e}")),
            None => app.log_push("Bandeja: icono registrado (SNI)."),
        }
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

    fn set_theme(&mut self, ctx: &egui::Context, mode: ThemeMode) {
        if mode == self.theme {
            return;
        }
        self.theme = mode;
        set_current_mode(mode);
        apply_theme(ctx, mode);
        save_theme(mode);
        self.log_push(format!("Tema: {}", mode.label()));
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
                    // Si el proyecto elegido desapareció, cae en cinnamon (o en
                    // el primer proyecto con nombre); si sigue, no se toca.
                    let projects = self.project_list();
                    if !projects.contains(&self.selected_project) {
                        self.selected_project = if projects.iter().any(|p| p == CINNAMON_PROJECT) {
                            CINNAMON_PROJECT.to_string()
                        } else {
                            projects
                                .iter()
                                .find(|p| !p.is_empty())
                                .cloned()
                                .unwrap_or_default()
                        };
                    }
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
                Msg::Tray(action) => match action {
                    TrayAction::Show => self.show_requested = true,
                    TrayAction::EngineStart => {
                        if !self.engine_busy && self.engine_active != Some(true) {
                            self.engine_busy = true;
                            engine_action(self.tx.clone(), true);
                        }
                    }
                    TrayAction::EngineStop => {
                        // El modal de confirmación solo se pinta en `ui`.
                        self.show_requested = true;
                        self.confirm_stop_engine = true;
                    }
                    TrayAction::Refresh => self.request_refresh(),
                    TrayAction::StartProject(project) => self.start_project(&project),
                    TrayAction::StopProject(project) => self.stop_project(&project),
                    TrayAction::Quit => self.quit_requested = true,
                },
            }
        }
    }

    /// Publica en la bandeja solo lo que cambió: evita repintar el icono cada
    /// ciclo de 500 ms.
    fn sync_tray(&mut self) {
        let Some(handle) = &self.tray_handle else {
            return;
        };
        let items = self.selected_items();
        let selected_running = items.iter().filter(|c| c.is_up()).count();
        let next = TrayState {
            engine_active: self.engine_active,
            socket_active: self.socket_active,
            busy: self.engine_busy || self.global_busy,
            selected_project: self.selected_project.clone(),
            selected_running,
            selected_stopped: items.len() - selected_running,
            selected_can_start: can_start_project(&self.selected_project, &items),
        };
        let changed = self.tray_state.lock().map(|s| *s != next).unwrap_or(true);
        if !changed {
            return;
        }
        if let Ok(mut s) = self.tray_state.lock() {
            *s = next;
        }
        handle.update(|_| ());
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

    /// Proyectos presentes, cinnamon primero, resto alfabético, "" al final.
    /// Cinnamon se añade siempre: sin contenedores (motor apagado o stack nunca
    /// levantado) el selector debe poder lanzar el compose desde cero.
    fn project_list(&self) -> Vec<String> {
        let mut keys: Vec<String> = std::iter::once(CINNAMON_PROJECT.to_string())
            .chain(self.containers.iter().map(|c| c.project.clone()))
            .collect();
        keys.sort_by_key(|k| project_rank(k));
        keys.dedup();
        keys
    }

    fn selected_items(&self) -> Vec<Container> {
        self.containers
            .iter()
            .filter(|c| c.project == self.selected_project)
            .cloned()
            .collect()
    }

    fn selected_label(&self) -> String {
        group_key(&self.selected_project)
    }

    fn start_selected(&mut self) {
        let project = self.selected_project.clone();
        self.start_project(&project);
    }

    fn start_project(&mut self, project: &str) {
        if self.global_busy {
            return;
        }
        let items: Vec<Container> = self
            .containers
            .iter()
            .filter(|c| c.project == project)
            .cloned()
            .collect();
        if project != CINNAMON_PROJECT && items.is_empty() {
            return;
        }
        self.global_busy = true;
        if project == CINNAMON_PROJECT {
            start_all(self.tx.clone());
        } else {
            group_start(self.tx.clone(), project.to_string(), items);
        }
    }

    fn stop_selected(&mut self) {
        let project = self.selected_project.clone();
        self.stop_project(&project);
    }

    fn stop_project(&mut self, project: &str) {
        if self.global_busy {
            return;
        }
        let items: Vec<Container> = self
            .containers
            .iter()
            .filter(|c| c.project == project)
            .cloned()
            .collect();
        if project != CINNAMON_PROJECT && items.is_empty() {
            return;
        }
        self.global_busy = true;
        if project == CINNAMON_PROJECT {
            // Cinnamon se apaga con compose stop (conserva la red y el orden).
            stop_all(self.tx.clone());
        } else {
            group_stop(self.tx.clone(), project.to_string(), items);
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let mut do_start = false;
        let mut ask_stop = false;
        let mut do_refresh = false;
        let mut do_start_selected = false;
        let mut do_stop_selected = false;
        let mut theme_req = None;
        let narrow = ui.available_width() < 1100.0;
        let (active, socket, engine_busy) =
            (self.engine_active, self.socket_active, self.engine_busy);
        let theme = self.theme;

        ui.horizontal(|ui| {
            brand_mark(ui, 34.0);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(
                    egui::RichText::new("Docker Cinnamon")
                        .size(26.0)
                        .strong()
                        .color(pal().text),
                );
                let version = if self.docker_version.is_empty() {
                    "Docker —".to_string()
                } else {
                    format!(
                        "Docker {} · panel de contenedores",
                        short_docker_version(&self.docker_version)
                    )
                };
                ui.label(egui::RichText::new(version).size(12.0).color(pal().muted));
            });
            ui.add_space(10.0);
            match self.engine_active {
                Some(true) => dot_pill(ui, pal().ok, "Motor activo"),
                Some(false) => dot_pill(ui, pal().err, "Motor detenido"),
                None => dot_pill(ui, pal().muted, "Motor desconocido"),
            };
            match (self.socket_active, self.engine_active) {
                (Some(true), Some(false)) => pill(ui, "socket activo", pal().warn),
                (Some(true), _) => pill(ui, "socket activo", pal().muted),
                (Some(false), _) => pill(ui, "socket detenido", pal().muted),
                (None, _) => pill(ui, "socket —", pal().muted),
            };
            let up = self.containers.iter().filter(|c| c.is_up()).count();
            pill(
                ui,
                format!("{up}/{} arriba", self.containers.len()),
                pal().muted,
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme_req = theme_switch(ui, theme);
                if !narrow {
                    let (s, p, r) = engine_buttons(ui, active, socket, engine_busy);
                    do_start |= s;
                    ask_stop |= p;
                    do_refresh |= r;
                    // En estrecho los totales ya se ven en las tarjetas resumen.
                    match self.stats_total() {
                        Some((cpu, used, limit, pct)) => {
                            pill(
                                ui,
                                format!(
                                    "RAM {} / {} ({pct:.1}%)",
                                    human_bytes(used),
                                    human_bytes(limit)
                                ),
                                pal().muted,
                            );
                            pill(ui, format!("CPU {cpu:.1}%"), pal().muted);
                        }
                        None => {
                            pill(ui, "RAM —", pal().muted);
                            pill(ui, "CPU —", pal().muted);
                        }
                    }
                }
            });
        });

        if let Some(mode) = theme_req {
            self.set_theme(ui.ctx(), mode);
        }

        // Bloque 2: selector de proyecto y acciones sobre el grupo elegido.
        ui.add_space(10.0);
        let projects = self.project_list();
        let mut selected = self.selected_project.clone();
        let sel_items = self.selected_items();
        let idle = !self.global_busy;
        // Cinnamon siempre se puede lanzar: el compose crea los contenedores, así
        // que 0 arriba es un caso válido (solo se deshabilita con los 8 en marcha).
        let can_start = idle && can_start_project(&selected, &sel_items);
        let can_stop = idle && sel_items.iter().any(|c| c.is_up());
        let total = self.containers.len();
        let global_busy = self.global_busy;

        egui::Frame::new()
            .fill(pal().card)
            .corner_radius(16)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .stroke(egui::Stroke::new(1.0, pal().border))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("project_selector")
                        .selected_text(self.selected_label())
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            for p in &projects {
                                ui.selectable_value(&mut selected, p.clone(), group_key(p));
                            }
                        });
                    let primary = ui
                        .scope(|ui| {
                            ui.spacing_mut().button_padding = egui::vec2(16.0, 8.0);
                            ui.add_enabled(
                                can_start,
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "Levantar {}",
                                        group_key(&selected)
                                    ))
                                    .size(12.5)
                                    .color(pal().on_accent)
                                    .strong(),
                                )
                                .fill(pal().accent)
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(12),
                            )
                        })
                        .inner;
                    if primary
                        .on_hover_text(if selected == CINNAMON_PROJECT {
                            "Arranque en serie con espera healthy"
                        } else {
                            "docker start secuencial de los contenedores parados del proyecto"
                        })
                        .clicked()
                    {
                        do_start_selected = true;
                    }
                    if ui
                        .add_enabled(
                            can_stop,
                            egui::Button::new(
                                egui::RichText::new(format!("Apagar {}", group_key(&selected)))
                                    .size(12.5)
                                    .color(if can_stop { pal().err } else { pal().muted }),
                            )
                            .fill(tint(pal().err, if can_stop { 18 } else { 8 }))
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(12),
                        )
                        .on_hover_text("Detiene los contenedores en marcha de este proyecto")
                        .clicked()
                    {
                        do_stop_selected = true;
                    }
                    let count = total;
                    ui.label(
                        egui::RichText::new(format!(
                            "{count} contenedor{}",
                            if count == 1 { "" } else { "es" }
                        ))
                        .size(11.0)
                        .color(pal().muted),
                    );
                    if global_busy {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("operación en curso…")
                                .size(11.0)
                                .color(pal().muted),
                        );
                    }
                    if narrow {
                        let (s, p, r) = engine_buttons(ui, active, socket, engine_busy);
                        do_start |= s;
                        ask_stop |= p;
                        do_refresh |= r;
                    }
                });
            });

        if selected != self.selected_project {
            self.selected_project = selected;
        }

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
        if do_start_selected {
            self.start_selected();
        }
        if do_stop_selected {
            self.stop_selected();
        }
    }

    /// Modal de confirmación de "Detener motor". Devuelve true si se confirmó.
    fn confirm_stop_modal(&mut self, ctx: &egui::Context) -> bool {
        if !self.confirm_stop_engine {
            return false;
        }
        let mut choice: Option<bool> = None;
        let frame = egui::Frame::new()
            .fill(pal().surface)
            .corner_radius(20)
            .inner_margin(egui::Margin::same(18))
            .stroke(egui::Stroke::new(1.0, pal().border))
            .shadow(egui::Shadow {
                offset: [0, 10],
                blur: 30,
                spread: 0,
                color: pal().card_shadow,
            });
        egui::Modal::new(egui::Id::unique("confirm_stop_engine"))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_max_width(520.0);
                ui.label(
                    egui::RichText::new("¿Detener el motor Docker?")
                        .size(18.0)
                        .strong()
                        .color(pal().text),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "Se detendrá el motor y se apagarán TODOS los contenedores de TODOS \
                         los proyectos (no solo Cinnamon). Los datos en volúmenes se conservan.",
                    )
                    .size(13.0)
                    .color(pal().text),
                );
                ui.add_space(10.0);
                egui::Frame::new()
                    .fill(tint(pal().muted, 10))
                    .corner_radius(12)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(engine_command(false))
                                .monospace()
                                .size(12.0)
                                .color(pal().muted),
                        );
                    });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if soft_button(ui, "Sí, detener todo", pal().err, true).clicked() {
                        choice = Some(true);
                    }
                    if soft_button(ui, "Cancelar", pal().muted, true).clicked() {
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
            let color = if notice.error { pal().err } else { pal().ok };
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(14, 10))
                .corner_radius(16)
                .fill(tint(color, 14))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(&notice.text)
                                .size(12.0)
                                .monospace()
                                .color(color),
                        );
                        if ui.small_button("ocultar").clicked() {
                            self.notice = None;
                        }
                    });
                    if let Some(cmd) = notice.command.clone() {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&cmd)
                                    .size(12.0)
                                    .monospace()
                                    .strong()
                                    .color(pal().text),
                            );
                            if ui.small_button("Copiar comando").clicked() {
                                ui.ctx().copy_text(cmd.clone());
                                self.log_push(format!("copiado al portapapeles: {cmd}"));
                            }
                        });
                        ui.label(
                            egui::RichText::new(
                                "Pégalo en una terminal (Konsole) si polkit no muestra el diálogo.",
                            )
                            .size(11.0)
                            .color(pal().muted),
                        );
                    }
                });
            ui.add_space(10.0);
        }
        if let Some(err) = self.compose_error.clone() {
            egui::Frame::new()
                .fill(tint(pal().err, 12))
                .corner_radius(14)
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("docker ps: {err}"))
                            .size(12.0)
                            .color(pal().err),
                    );
                });
            ui.add_space(10.0);
        }

        let up = self.containers.iter().filter(|c| c.is_up()).count();
        let total = self.containers.len();
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.columns(3, |cols| {
                match self.stats_total() {
                    Some((cpu, used, limit, pct)) => {
                        summary_card(
                            &mut cols[0],
                            "CPU total",
                            &format!("{cpu:.1}%"),
                            "suma de contenedores",
                        );
                        summary_card(
                            &mut cols[1],
                            "RAM total",
                            &human_bytes(used),
                            &format!("de {} · {pct:.1}%", human_bytes(limit)),
                        );
                    }
                    None => {
                        summary_card(&mut cols[0], "CPU total", "—", "sin datos aún");
                        summary_card(&mut cols[1], "RAM total", "—", "sin datos aún");
                    }
                }
                summary_card(
                    &mut cols[2],
                    "Contenedores",
                    &format!("{up}/{total}"),
                    "arriba",
                );
            });
        });
        ui.add_space(16.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // ── todos los contenedores del daemon ──────────────────────
                ui.label(
                    egui::RichText::new("Contenedores")
                        .strong()
                        .size(20.0)
                        .color(pal().text),
                );
                ui.label(
                    egui::RichText::new(
                        "Todos los proyectos del daemon, agrupados por proyecto compose.",
                    )
                    .size(12.0)
                    .color(pal().muted),
                );
                ui.add_space(4.0);
                if containers.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        egui::Frame::new()
                            .fill(pal().card)
                            .stroke(egui::Stroke::new(1.0, pal().border))
                            .corner_radius(18)
                            .inner_margin(egui::Margin::symmetric(12, 16))
                            .show(ui, |ui| {
                                ui.set_width(360.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        egui::RichText::new("Sin contenedores (o motor detenido).")
                                            .size(14.0)
                                            .color(pal().text),
                                    );
                                    ui.label(
                                        egui::RichText::new(
                                            "Arranca el motor o levanta el stack para ver las tarjetas.",
                                        )
                                        .size(11.0)
                                        .color(pal().muted),
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
                keys.sort_by_key(|k| project_rank(k));

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
                    let wide = ui.available_width() > 720.0;
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&label)
                                .strong()
                                .size(16.0)
                                .color(if key == CINNAMON_PROJECT {
                                    pal().info
                                } else {
                                    pal().text
                                }),
                        );
                        pill(
                            ui,
                            format!("{up}/{}", items.len()),
                            if up > 0 { pal().ok } else { pal().muted },
                        );
                        if soft_button(ui, "Arrancar todos", pal().accent, can_start)
                            .on_hover_text(if key == CINNAMON_PROJECT {
                                "Serie con espera healthy: postgres → mongo → apps"
                            } else {
                                "docker start de los contenedores parados"
                            })
                            .clicked()
                        {
                            req_group_start = Some((key.clone(), items.clone()));
                        }
                        if soft_button(ui, "Detener todos", pal().err, can_stop).clicked() {
                            req_group_stop = Some((key.clone(), items.clone()));
                        }
                        if key == CINNAMON_PROJECT && wide {
                            ui.label(
                                egui::RichText::new("orden serie + healthy")
                                    .size(11.0)
                                    .color(pal().muted),
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
                                .color(pal().muted),
                            );
                        }
                    });
                    ui.add_space(10.0);

                    for c in &items {
                        let busy = self.busy_containers.contains(&c.name);
                        let blocked = busy || self.global_busy;
                        let stats = self.stats_for(c);
                        // En tarjetas estrechas la pill de servicio no cabe sin
                        // solaparse con las acciones: el nombre ya la contiene.
                        let compact = ui.available_width() < 620.0;
                        egui::Frame::new()
                            .fill(pal().card)
                            .stroke(egui::Stroke::new(1.0, pal().border))
                            .corner_radius(18)
                            .inner_margin(egui::Margin::symmetric(16, 12))
                            .shadow(egui::Shadow {
                                offset: [0, 1],
                                blur: 10,
                                spread: 0,
                                color: pal().card_shadow,
                            })
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.x = 6.0;
                                // Línea 1: estado, nombre, chips y acciones.
                                ui.horizontal(|ui| {
                                    let max_name = if compact { 130.0 } else { 220.0 };
                                    let name_w =
                                        text_width(ui, &c.name, 14.0, true).min(max_name);
                                    status_dot(ui, container_state_color(c));
                                    clipped_label(
                                        ui,
                                        egui::RichText::new(&c.name)
                                            .monospace()
                                            .size(14.0)
                                            .strong()
                                            .color(pal().text),
                                        name_w,
                                        17.0,
                                    )
                                    .on_hover_text(&c.name);
                                    if !compact {
                                        pill(
                                            ui,
                                            if c.service.is_empty() {
                                                "—"
                                            } else {
                                                c.service.as_str()
                                            },
                                            pal().info,
                                        )
                                        .on_hover_text("servicio compose");
                                    }
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
                                            if soft_button(ui, "Logs", pal().muted, !blocked)
                                                .clicked()
                                            {
                                                container_action_req =
                                                    Some((c.clone(), ContainerOp::Logs));
                                            }
                                            if c.is_up() {
                                                if soft_button(
                                                    ui,
                                                    "Reiniciar",
                                                    pal().muted,
                                                    !blocked,
                                                )
                                                .clicked()
                                                {
                                                    container_action_req =
                                                        Some((c.clone(), ContainerOp::Restart));
                                                }
                                                if soft_button(
                                                    ui,
                                                    "Detener",
                                                    pal().err,
                                                    !blocked,
                                                )
                                                .clicked()
                                                {
                                                    container_action_req =
                                                        Some((c.clone(), ContainerOp::Stop));
                                                }
                                            } else if soft_button(
                                                ui,
                                                "Iniciar",
                                                pal().accent,
                                                !blocked,
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
                                            .color(pal().muted),
                                        img_w,
                                        14.0,
                                    )
                                    .on_hover_text(&c.image);
                                    ui.label(
                                        egui::RichText::new("·").size(11.0).color(pal().muted),
                                    );
                                    clipped_label(
                                        ui,
                                        egui::RichText::new(&c.status)
                                            .size(11.0)
                                            .color(pal().muted),
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
                                                            .color(pal().muted),
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
                                                        pal().accent,
                                                    );
                                                    stat_text(
                                                        ui,
                                                        "CPU",
                                                        format!("{:.1}%", st.cpu),
                                                        Some(st.cpu / 100.0),
                                                        pal().accent,
                                                    );
                                                }
                                                None => {
                                                    stat_text(
                                                        ui,
                                                        "RAM",
                                                        "—".to_string(),
                                                        None,
                                                        pal().muted,
                                                    );
                                                    stat_text(
                                                        ui,
                                                        "CPU",
                                                        "—".to_string(),
                                                        None,
                                                        pal().muted,
                                                    );
                                                }
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(10.0);
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
            ui.label(
                egui::RichText::new("Actividad")
                    .size(15.0)
                    .strong()
                    .color(pal().text),
            );
            ui.label(
                egui::RichText::new(format!("{} / {LOG_CAP}", self.log.len()))
                    .size(11.0)
                    .color(pal().muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if soft_button(ui, "Salir", pal().muted, true)
                    .on_hover_text("Cierra la aplicación (también puedes usar Salir en la bandeja)")
                    .clicked()
                {
                    self.allow_close = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if soft_button(ui, "Copiar", pal().muted, true)
                    .on_hover_text("Copia todo el log al portapapeles")
                    .clicked()
                {
                    ui.ctx()
                        .copy_text(self.log.iter().cloned().collect::<Vec<_>>().join("\n"));
                }
                if soft_button(ui, "Limpiar", pal().muted, true).clicked() {
                    self.log.clear();
                }
            });
        });
        ui.add_space(10.0);
        let rest = egui::vec2(ui.available_width(), ui.available_height());
        ui.allocate_ui_with_layout(rest, egui::Layout::top_down(egui::Align::Min), |ui| {
            egui::Frame::new()
                .fill(tint(pal().muted, 8))
                .corner_radius(14)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_min_height(ui.available_height());
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            for line in &self.log {
                                ui.label(
                                    egui::RichText::new(line)
                                        .monospace()
                                        .size(11.5)
                                        .color(pal().text),
                                );
                            }
                        });
                });
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
        "running" => pal().ok,
        "exited" | "dead" => pal().err,
        "restarting" | "paused" | "created" => pal().warn,
        _ => pal().muted,
    }
}

fn health_color_str(health: &str) -> egui::Color32 {
    match health {
        "healthy" => pal().ok,
        "unhealthy" => pal().err,
        "starting" => pal().warn,
        _ => pal().muted,
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain();

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
        ctx.request_repaint_after(Duration::from_millis(500));

        if !self.tray_ctx_set {
            if let Some(h) = &self.tray_handle {
                h.update(|t| t.ctx = Some(ctx.clone()));
            }
            self.tray_ctx_set = true;
        }
        if self.quit_requested {
            self.quit_requested = false;
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.show_requested {
            self.show_requested = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.tray_ok {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.log_push("Ventana oculta en la bandeja. Usa «Salir» en el icono para terminar.");
        }
        self.sync_tray();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        paint_window_background(ui.ctx());
        self.confirm_stop_modal(ui.ctx());

        let header_frame = egui::Frame::new()
            .fill(pal().surface)
            .inner_margin(egui::Margin::symmetric(18, 12));
        egui::Panel::top("header")
            .frame(header_frame)
            .show(ui, |ui| self.header(ui));

        let max_log = ui.ctx().content_rect().width() * 0.55;
        let log_frame = egui::Frame::new()
            .fill(pal().surface)
            .inner_margin(egui::Margin::same(16))
            .corner_radius(18)
            .stroke(egui::Stroke::new(1.0, pal().border))
            .outer_margin(egui::Margin {
                left: 0,
                right: 12,
                top: 12,
                bottom: 12,
            });
        egui::Panel::right("log")
            .resizable(true)
            .default_size(380.0)
            .min_size(260.0)
            .max_size(max_log)
            .frame(log_frame)
            .show(ui, |ui| self.log_panel(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(18, 16)),
            )
            .show(ui, |ui| self.body(ui));
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
    println!("Compose: {}", compose_file());
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
    let theme = app.theme;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Docker Cinnamon")
            .with_inner_size([1320.0, 840.0])
            .with_min_inner_size([980.0, 560.0])
            .with_icon(std::sync::Arc::new(app_icon())),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "Docker Cinnamon",
        options,
        Box::new(move |cc| {
            apply_theme(&cc.egui_ctx, theme);
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel_luminance(c: egui::Color32) -> f32 {
        fn ch(v: u8) -> f32 {
            let s = v as f32 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    fn contrast_ratio(a: egui::Color32, b: egui::Color32) -> f32 {
        let (la, lb) = (rel_luminance(a), rel_luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn parsea_modo_de_tema() {
        for (s, mode) in [
            ("light", ThemeMode::Light),
            (" claro ", ThemeMode::Light),
            ("DARK", ThemeMode::Dark),
            ("Oscuro", ThemeMode::Dark),
        ] {
            assert_eq!(ThemeMode::parse(s), Some(mode), "entrada: {s:?}");
        }
        for s in ["", "  ", "auto", "claro2", "gris"] {
            assert_eq!(ThemeMode::parse(s), None, "entrada: {s:?}");
        }
    }

    #[test]
    fn key_hace_roundtrip() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            assert_eq!(ThemeMode::parse(mode.key()), Some(mode));
        }
    }

    #[test]
    fn contraste_wcag_aa_en_ambos_temas() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let p = palette_for(mode);
            for (nombre, color) in [("text", p.text), ("muted", p.muted)] {
                let ratio = contrast_ratio(color, p.card);
                assert!(
                    ratio >= 4.5,
                    "{nombre} sobre card en {mode:?}: {ratio:.2} < 4.5"
                );
            }
        }
    }

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
    fn icono_de_bandeja_128() {
        let icon = tray_icon();
        assert_eq!(icon.width, 128);
        assert_eq!(icon.height, 128);
        assert_eq!(icon.data.len(), 128 * 128 * 4);
        // ARGB: esquina superior izquierda transparente; centro opaco.
        assert_eq!(icon.data[0], 0);
        let center = (64 * 128 + 64) * 4;
        assert_eq!(icon.data[center], 255);
        // Punto naranja arriba a la derecha.
        let orange = (25 * 128 + 102) * 4;
        assert_eq!(icon.data[orange], 255);
        assert!(icon.data[orange + 1] > 150, "rojo del punto naranja");
        assert_eq!(brand_rgba(128).len(), 65536);
    }

    #[test]
    fn icono_de_ventana_128() {
        let icon = app_icon();
        assert_eq!((icon.width, icon.height), (128, 128));
        assert_eq!(icon.rgba.len(), 65536);
    }

    #[test]
    fn compose_file_resuelve_entorno() {
        for vacio in [None, Some(""), Some("  ")] {
            assert_eq!(resolve_compose_file(vacio), DEFAULT_COMPOSE_FILE);
        }
        assert_eq!(resolve_compose_file(Some("/tmp/x.yml")), "/tmp/x.yml");
        assert_eq!(resolve_compose_file(Some(" /tmp/x.yml ")), "/tmp/x.yml");
    }

    #[test]
    fn bytes_humanos() {
        assert_eq!(human_bytes(583_113_113), "556 MiB");
        assert_eq!(human_bytes(16_707_422_781), "15.6 GiB");
    }

    #[test]
    fn can_start_cinnamon_por_servicios() {
        fn c(name: &str, state: &str) -> Container {
            Container {
                name: name.to_string(),
                service: name.to_string(),
                project: CINNAMON_PROJECT.to_string(),
                state: state.to_string(),
                ..Default::default()
            }
        }
        assert!(can_start_project(CINNAMON_PROJECT, &[]));
        let full: Vec<Container> = SERVICES.iter().map(|s| c(s, "running")).collect();
        assert!(!can_start_project(CINNAMON_PROJECT, &full));
        let mut stopped = full.clone();
        stopped[0].state = "exited".to_string();
        assert!(can_start_project(CINNAMON_PROJECT, &stopped));
        let mut missing = full;
        missing.pop();
        assert!(can_start_project(CINNAMON_PROJECT, &missing));
        assert!(!can_start_project("otro", &[]));
    }
}
