//! Окно настроек Driftling (ТЗ §3.3, редизайн M0.5).
//!
//! Современное двухпанельное окно: тёмный сайдбар с навигацией и страницы
//! «Питомец» (карточка питомца с живым портретом, характеристики только
//! для чтения), «Приложение» (автозапуск, демон, о программе) и скрытая
//! «Отладка» (только с флагом `--debug` — прямое редактирование
//! характеристик через IPC `SetAttributes`).
//!
//! Характеристики принадлежат питомцу (pet.json на стороне демона),
//! пользователь их не редактирует — в M2+ они будут расти через уход.
//! Все IPC-вызовы — в фоновых потоках, UI-поток никогда не блокируется.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod i18n;
use i18n::fl;

use driftling_core::sprite::{placeholder, Frame};
use driftling_core::PetAttributes;
use driftling_ipc::{call, Request, Response};
use eframe::egui::{
    self, Align2, Button, CollapsingHeader, Color32, CornerRadius, DragValue, FontData,
    FontDefinitions, FontFamily, FontId, Label, Layout, Margin, RichText, ScrollArea, Sense,
    Slider, Stroke, StrokeKind, TextStyle, TextureHandle, TextureOptions,
};

// ---------------------------------------------------------------------------
// Палитра дизайн-системы
// ---------------------------------------------------------------------------

const BG: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x22);
const SIDEBAR_BG: Color32 = Color32::from_rgb(0x15, 0x15, 0x1c);
const CARD: Color32 = Color32::from_rgb(0x23, 0x23, 0x2e);
const CARD_STROKE: Color32 = Color32::from_rgb(0x2f, 0x2f, 0x3d);
const TEXT: Color32 = Color32::from_rgb(0xe8, 0xe8, 0xf0);
const MUTED: Color32 = Color32::from_rgb(0x9a, 0x9a, 0xac);
const ACCENT: Color32 = Color32::from_rgb(0x8a, 0x63, 0xd2);
const ACCENT_LIGHT: Color32 = Color32::from_rgb(0xb2, 0x96, 0xe8);
const SUCCESS: Color32 = Color32::from_rgb(0x6c, 0xcb, 0x5f);
const DANGER: Color32 = Color32::from_rgb(0xe0, 0x5f, 0x5f);
const SLEEP_BLUE: Color32 = Color32::from_rgb(0x6b, 0x8f, 0xd2);
const AMBER: Color32 = Color32::from_rgb(0xe0, 0xa8, 0x4f);
const TRACK: Color32 = Color32::from_rgb(0x2f, 0x2f, 0x3d);
const PORTRAIT_BG: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x27);

/// Полупрозрачная версия цвета — подложки бейджей и выделений.
fn tinted(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

// ---------------------------------------------------------------------------
// Чистая логика (юнит-тесты внизу файла)
// ---------------------------------------------------------------------------

/// Локализованная подпись состояния питомца для бейджа (ТД-30).
fn state_label(state: Option<&str>) -> String {
    match state {
        None => fl!("state-absent"),
        Some("Idle") => fl!("state-idle"),
        Some("Walk") => fl!("state-walk"),
        Some("Sleep") => fl!("state-sleep"),
        Some("Falling") => fl!("state-falling"),
        Some("Dragged") => fl!("state-dragged"),
        Some("Landing") => fl!("state-landing"),
        Some(_) => fl!("state-unknown"),
    }
}

/// Цвет бейджа состояния.
fn state_color(state: Option<&str>) -> Color32 {
    match state {
        None => MUTED,
        Some("Idle") => SUCCESS,
        Some("Walk") => ACCENT_LIGHT,
        Some("Sleep") => SLEEP_BLUE,
        Some("Falling" | "Dragged" | "Landing") => AMBER,
        Some(_) => MUTED,
    }
}

/// Доля значения на шкале min..max (для прогресс-баров характеристик).
fn norm(v: f32, min: f32, max: f32) -> f32 {
    if max <= min {
        return 0.0;
    }
    ((v - min) / (max - min)).clamp(0.0, 1.0)
}

/// Аптайм демона в человекочитаемом виде: плюральные формы — по правилам
/// CLDR через Fluent («1 минута / 2 минуты / 5 минут», ТД-30).
fn format_uptime(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!(
            "{} {}",
            fl!("uptime-hours", hours = h),
            fl!("uptime-minutes", minutes = m)
        )
    } else if m > 0 {
        format!(
            "{} {}",
            fl!("uptime-minutes", minutes = m),
            fl!("uptime-seconds", seconds = s)
        )
    } else {
        fl!("uptime-seconds", seconds = s)
    }
}

/// ARGB8888-пиксель спрайта -> Color32.
fn argb_to_color32(p: u32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        ((p >> 16) & 0xff) as u8,
        ((p >> 8) & 0xff) as u8,
        (p & 0xff) as u8,
        (p >> 24) as u8,
    )
}

/// Экранировать аргумент для ключа Exec по Desktop Entry spec: значение
/// проходит два разбора — общий unescape строки файла, затем разбор
/// аргументов с кавычками. Простые пути не трогаем; всё остальное берём
/// в двойные кавычки, а `"` `` ` `` `$` `\` экранируем с учётом обоих
/// уровней (`\` в файле пишется как `\\`).
fn exec_quote(arg: &str) -> String {
    let simple = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | ':' | '+'));
    if simple {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        match c {
            // Уровень Exec-разбора требует `\"`; чтобы `\` пережил общий
            // unescape, в файле это `\\"`.
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            // Литеральный бэкслеш: `\\` на уровне Exec = `\\\\` в файле.
            '\\' => out.push_str("\\\\\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Содержимое autostart-файла для KDE: запускаем демона после панели.
fn desktop_file_content(exec: &str) -> String {
    let exec = exec_quote(exec);
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Driftling\n\
         Exec={exec}\n\
         X-KDE-autostart-after=panel\n"
    )
}

/// Путь autostart-файла внутри каталога конфигов.
fn autostart_path_in(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join("driftling.desktop")
}

/// `$XDG_CONFIG_HOME` либо `~/.config` — как в driftling_core::config.
fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn autostart_path() -> PathBuf {
    autostart_path_in(&config_home())
}

/// Exec для autostart: бинарь демона рядом с текущим бинарём настроек,
/// если он там есть; иначе полагаемся на `driftling` в PATH.
fn daemon_exec_in(settings_dir: Option<&Path>) -> String {
    settings_dir
        .map(|d| d.join("driftling"))
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "driftling".to_string())
}

fn daemon_exec() -> String {
    let exe = std::env::current_exe().ok();
    daemon_exec_in(exe.as_deref().and_then(Path::parent))
}

/// Каким механизмом управлять автозапуском (ТД-28): предпочитаем systemd
/// user unit (Restart=on-failure, journald), .desktop — фолбэк.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutostartBackend {
    /// Установлен dist/driftling.service — рулим через systemctl --user.
    SystemdUnit,
    /// Юнита нет (или нет systemd) — XDG autostart .desktop-файл.
    DesktopFile,
}

/// Определить бэкенд: юнит считается установленным, если systemd его видит.
fn autostart_backend() -> AutostartBackend {
    let installed = std::process::Command::new("systemctl")
        .args(["--user", "list-unit-files", "driftling.service"])
        .output()
        .map(|o| {
            o.status.success() && String::from_utf8_lossy(&o.stdout).contains("driftling.service")
        })
        .unwrap_or(false);
    if installed {
        AutostartBackend::SystemdUnit
    } else {
        AutostartBackend::DesktopFile
    }
}

/// systemctl --user enable/disable driftling.service.
fn set_autostart_systemd(enable: bool) -> Result<(), String> {
    let action = if enable { "enable" } else { "disable" };
    let out = std::process::Command::new("systemctl")
        .args(["--user", action, "driftling.service"])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Текущее фактическое состояние автозапуска (для тумблера).
fn autostart_enabled() -> bool {
    match autostart_backend() {
        AutostartBackend::SystemdUnit => std::process::Command::new("systemctl")
            .args(["--user", "is-enabled", "--quiet", "driftling.service"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        AutostartBackend::DesktopFile => autostart_path().exists(),
    }
}

/// Включить/выключить автозапуск выбранным бэкендом.
fn set_autostart(enable: bool) -> Result<(), String> {
    match autostart_backend() {
        AutostartBackend::SystemdUnit => set_autostart_systemd(enable),
        AutostartBackend::DesktopFile => {
            set_autostart_at(&autostart_path(), enable, &daemon_exec())
        }
    }
}

/// Та же логика с явными путями — для юнит-тестов.
fn set_autostart_at(path: &Path, enable: bool, exec: &str) -> Result<(), String> {
    if enable {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, desktop_file_content(exec)).map_err(|e| e.to_string())
    } else {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Фоновый опрос демона
// ---------------------------------------------------------------------------

/// Снимок PetInfo от демона.
#[derive(Clone)]
struct PetSnapshot {
    name: String,
    state: Option<String>,
    attributes: PetAttributes,
    uptime_secs: u64,
}

/// Последнее известное состояние демона (пишут фоновые потоки, читает UI).
#[derive(Default)]
struct PollState {
    /// Хотя бы один опрос уже завершился.
    checked: bool,
    /// Демон ответил на последний запрос.
    up: bool,
    info: Option<PetSnapshot>,
    /// Сырой PetInfo (pretty JSON) для дебаг-панели.
    raw: Option<String>,
}

/// Один запрос PetInfo -> общий слот. Вызывается из фоновых потоков.
fn poll_once(slot: &Arc<Mutex<PollState>>) {
    let mut next = PollState {
        checked: true,
        ..Default::default()
    };
    if let Ok(Response::PetInfo {
        name,
        state,
        attributes,
        uptime_secs,
    }) = call(&Request::PetInfo)
    {
        next.up = true;
        next.raw = Some(
            serde_json::to_string_pretty(&serde_json::json!({
                "name": name,
                "state": state,
                "attributes": serde_json::to_value(attributes).unwrap_or_default(),
                "uptime_secs": uptime_secs,
            }))
            .unwrap_or_default(),
        );
        next.info = Some(PetSnapshot {
            name,
            state,
            attributes,
            uptime_secs,
        });
    }
    *slot.lock().unwrap() = next;
}

/// Вечный поток опроса раз в секунду — блокирующий IPC не в UI-потоке.
fn spawn_poller(slot: Arc<Mutex<PollState>>, ctx: egui::Context) {
    std::thread::spawn(move || loop {
        poll_once(&slot);
        ctx.request_repaint();
        std::thread::sleep(Duration::from_secs(1));
    });
}

/// Разовая IPC-команда в короткоживущем потоке; результат — строкой в слот,
/// после команды сразу дёргаем свежий PetInfo, чтобы UI не ждал секунду.
fn spawn_action(
    req: Request,
    ok_text: String,
    result: Arc<Mutex<Option<String>>>,
    poll: Arc<Mutex<PollState>>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let text = match call(&req) {
            Ok(Response::Error(e)) => fl!("daemon-error", error = e),
            Ok(_) => ok_text,
            Err(e) => fl!("generic-error", error = format!("{e:#}")),
        };
        *result.lock().unwrap() = Some(text);
        poll_once(&poll);
        ctx.request_repaint();
    });
}

// ---------------------------------------------------------------------------
// Шрифты и стиль
// ---------------------------------------------------------------------------

const FONT_REGULAR: &[&str] = &[
    "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
    "/usr/share/fonts/truetype/cantarell/Cantarell-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];
const FONT_SEMIBOLD: &[&str] = &[
    "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf",
    "/usr/share/fonts/truetype/cantarell/Cantarell-Bold.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
];

/// Имя полужирной семьи (заголовки); наполняется в `install_fonts`.
fn semibold_family() -> FontFamily {
    FontFamily::Name("ui-semibold".into())
}

/// Первый существующий файл из списка.
fn first_existing(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

/// Системный шрифт с кириллицей первым в Proportional + отдельная
/// полужирная семья для заголовков. Ничего не нашлось — остаёмся на
/// встроенных шрифтах egui (кириллица там частичная, но окно живёт).
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    if let Some(bytes) = first_existing(FONT_REGULAR) {
        fonts
            .font_data
            .insert("ui-regular".into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "ui-regular".into());
        // Кириллица и в моноширинном фолбэке (сырые JSON с именем питомца).
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .push("ui-regular".into());
    } else {
        log::warn!("системный шрифт с кириллицей не найден, остаёмся на встроенных");
    }

    // Полужирная семья: свой файл либо фолбэк на Proportional.
    let mut semibold_list = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if let Some(bytes) = first_existing(FONT_SEMIBOLD) {
        fonts
            .font_data
            .insert("ui-semibold".into(), Arc::new(FontData::from_owned(bytes)));
        semibold_list.insert(0, "ui-semibold".into());
    }
    fonts.families.insert(semibold_family(), semibold_list);

    ctx.set_fonts(fonts);
}

/// Тёмная тема дизайн-системы поверх egui-дефолтов.
fn apply_style(ctx: &egui::Context) {
    ctx.all_styles_mut(style_mut);
}

fn style_mut(style: &mut egui::Style) {
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.slider_width = 240.0;
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(22.0)),
        (TextStyle::Body, FontId::proportional(15.0)),
        (TextStyle::Button, FontId::proportional(15.0)),
        (TextStyle::Small, FontId::proportional(12.5)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.window_fill = BG;
    v.panel_fill = BG;
    v.extreme_bg_color = SIDEBAR_BG;
    v.faint_bg_color = Color32::from_rgb(0x27, 0x27, 0x33);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, ACCENT_LIGHT);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;

    let r = CornerRadius::same(10);
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.weak_bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, CARD_STROKE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.noninteractive.corner_radius = r;

    v.widgets.inactive.bg_fill = Color32::from_rgb(0x2b, 0x2b, 0x3a);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x2b, 0x2b, 0x3a);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3a, 0x4b));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = r;

    v.widgets.hovered.bg_fill = Color32::from_rgb(0x33, 0x33, 0x44);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x33, 0x33, 0x44);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x4a, 0x4a, 0x60));
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.corner_radius = r;

    v.widgets.active.bg_fill = Color32::from_rgb(0x3b, 0x3b, 0x50);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x3b, 0x3b, 0x50);
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.active.corner_radius = r;

    v.widgets.open.bg_fill = CARD;
    v.widgets.open.weak_bg_fill = CARD;
    v.widgets.open.corner_radius = r;
}

// ---------------------------------------------------------------------------
// Мелкие виджеты дизайн-системы
// ---------------------------------------------------------------------------

/// Карточка: содержимое никогда не лежит на голом фоне.
fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_STROKE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Секционная подпись: 13px, приглушённая, ВЕРХНИЙ РЕГИСТР.
fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(13.0)
            .color(MUTED)
            .family(semibold_family()),
    );
}

/// Заголовок страницы (22, полужирный).
fn page_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(22.0)
            .family(semibold_family())
            .color(TEXT),
    );
}

/// Бейдж-чип состояния: скруглённая подложка в тон текста.
fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(tinted(color, 36))
        .corner_radius(CornerRadius::same(99))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(13.0).color(color));
        });
}

/// Акцентная (заливка) кнопка; выключенная — заметно приглушена.
fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let (fill, fg) = if enabled {
        (ACCENT, Color32::WHITE)
    } else {
        (
            Color32::from_rgb(0x2c, 0x2a, 0x38),
            Color32::from_rgb(0x6e, 0x6e, 0x80),
        )
    };
    ui.add_enabled(
        enabled,
        Button::new(RichText::new(text).color(fg))
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(8)),
    )
}

/// Контурная кнопка (нейтральная либо «опасная»); выключенная — приглушена.
fn outline_button(ui: &mut egui::Ui, text: &str, color: Color32, enabled: bool) -> egui::Response {
    let fg = if enabled {
        color
    } else {
        Color32::from_rgb(0x6e, 0x6e, 0x80)
    };
    ui.add_enabled(
        enabled,
        Button::new(RichText::new(text).color(fg))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, tinted(fg, 140)))
            .corner_radius(CornerRadius::same(8)),
    )
}

/// Кастомный тумблер-переключатель с анимированной ручкой (по мотивам
/// канонического toggle из egui demo).
fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(44.0, 24.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let bg = Color32::from_rgb(
            (TRACK.r() as f32 + (ACCENT.r() as f32 - TRACK.r() as f32) * t) as u8,
            (TRACK.g() as f32 + (ACCENT.g() as f32 - TRACK.g() as f32) * t) as u8,
            (TRACK.b() as f32 + (ACCENT.b() as f32 - TRACK.b() as f32) * t) as u8,
        );
        let p = ui.painter();
        p.rect(
            rect,
            CornerRadius::same(12),
            bg,
            Stroke::new(1.0, tinted(Color32::WHITE, 14)),
            StrokeKind::Inside,
        );
        let x = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), t);
        p.circle_filled(
            egui::pos2(x, rect.center().y),
            8.5,
            Color32::from_rgb(0xf2, 0xf2, 0xf8),
        );
    }
    response
}

/// Строка характеристики: подпись, значение, опциональный тонкий бар.
fn stat_row(ui: &mut egui::Ui, label: &str, value: &str, frac: Option<f32>) {
    let h = 26.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter();
    let cy = rect.center().y;
    p.text(
        egui::pos2(rect.left(), cy),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        MUTED,
    );
    p.text(
        egui::pos2(rect.left() + 160.0, cy),
        Align2::LEFT_CENTER,
        value,
        FontId::proportional(15.0),
        TEXT,
    );
    if let Some(f) = frac {
        let bar = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 280.0, cy - 3.0),
            egui::pos2(rect.right(), cy + 3.0),
        );
        if bar.width() > 20.0 {
            p.rect_filled(bar, 3.0, TRACK);
            let w = bar.width() * f.clamp(0.0, 1.0);
            if w >= 2.0 {
                p.rect_filled(
                    egui::Rect::from_min_size(bar.min, egui::vec2(w, bar.height())),
                    3.0,
                    ACCENT,
                );
            }
        }
    }
}

/// Точка-индикатор статуса демона + подпись.
fn daemon_dot(ui: &mut egui::Ui, up: bool, checked: bool) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
        let color = if !checked {
            MUTED
        } else if up {
            SUCCESS
        } else {
            DANGER
        };
        ui.painter().circle_filled(rect.center(), 4.0, color);
        let text = if !checked {
            fl!("badge-checking")
        } else if up {
            fl!("daemon-running")
        } else {
            fl!("daemon-not-running")
        };
        ui.label(RichText::new(text).size(12.5).color(MUTED));
    });
}

// ---------------------------------------------------------------------------
// Портрет питомца (кадры плейсхолдера -> текстуры egui)
// ---------------------------------------------------------------------------

fn frame_to_image(f: &Frame) -> egui::ColorImage {
    egui::ColorImage::new(
        [f.w as usize, f.h as usize],
        f.argb.iter().map(|&p| argb_to_color32(p)).collect(),
    )
}

/// Кэш idle-кадров под текущий размер питомца.
#[derive(Default)]
struct Portrait {
    size: u32,
    frames: Vec<TextureHandle>,
}

impl Portrait {
    fn ensure(&mut self, ctx: &egui::Context, size: u32) {
        if self.size == size && !self.frames.is_empty() {
            return;
        }
        let set = placeholder(size.clamp(32, 256));
        self.frames = set
            .idle
            .iter()
            .enumerate()
            .map(|(i, f)| {
                ctx.load_texture(
                    format!("pet-idle-{i}"),
                    frame_to_image(f),
                    TextureOptions::NEAREST,
                )
            })
            .collect();
        self.size = size;
    }

    /// Кадр на момент времени `t` (2 fps, как idle в спрайт-движке).
    fn current(&self, t: f64) -> Option<&TextureHandle> {
        (!self.frames.is_empty()).then(|| {
            let idx = (t * 2.0) as usize % self.frames.len();
            &self.frames[idx]
        })
    }
}

// ---------------------------------------------------------------------------
// Приложение
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Page {
    Pet,
    App,
    Debug,
}

struct SettingsApp {
    page: Page,
    /// Страница прошлого кадра — при смене скроллим наверх.
    last_page: Option<Page>,
    debug_enabled: bool,
    poll: Arc<Mutex<PollState>>,
    /// Результат последней команды на странице питомца.
    pet_action: Arc<Mutex<Option<String>>>,
    /// Результат остановки демона (страница «Приложение»).
    daemon_action: Arc<Mutex<Option<String>>>,
    /// Результат применения характеристик (дебаг).
    debug_action: Arc<Mutex<Option<String>>>,
    autostart: bool,
    autostart_result: Option<String>,
    /// Время постановки кнопки остановки «на взвод» (подтверждение).
    stop_armed_at: Option<f64>,
    /// Форма дебаг-панели; один раз синкается с живым PetInfo.
    form: PetAttributes,
    form_synced: bool,
    portrait: Portrait,
    logo: Option<TextureHandle>,
}

impl SettingsApp {
    fn new(cc: &eframe::CreationContext<'_>, debug_enabled: bool) -> Self {
        install_fonts(&cc.egui_ctx);
        apply_style(&cc.egui_ctx);

        let poll = Arc::new(Mutex::new(PollState::default()));
        spawn_poller(Arc::clone(&poll), cc.egui_ctx.clone());

        Self {
            // С флагом --debug открываемся сразу на админ-панели.
            page: if debug_enabled {
                Page::Debug
            } else {
                Page::Pet
            },
            last_page: None,
            debug_enabled,
            poll,
            pet_action: Arc::new(Mutex::new(None)),
            daemon_action: Arc::new(Mutex::new(None)),
            debug_action: Arc::new(Mutex::new(None)),
            autostart: autostart_enabled(),
            autostart_result: None,
            stop_armed_at: None,
            form: PetAttributes::default(),
            form_synced: false,
            portrait: Portrait::default(),
            logo: None,
        }
    }

    fn action(&self, ui: &egui::Ui, req: Request, ok: String, slot: &Arc<Mutex<Option<String>>>) {
        spawn_action(
            req,
            ok,
            Arc::clone(slot),
            Arc::clone(&self.poll),
            ui.ctx().clone(),
        );
    }

    // -- Сайдбар ------------------------------------------------------------

    fn nav_item(&mut self, ui: &mut egui::Ui, page: Page, label: &str) {
        let selected = self.page == page;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), Sense::click());
        if response.clicked() {
            self.page = page;
        }
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, 8.0, tinted(ACCENT, 42));
        } else if response.hovered() {
            p.rect_filled(rect, 8.0, tinted(Color32::WHITE, 10));
        }
        let color = if selected { ACCENT_LIGHT } else { MUTED };
        p.text(
            egui::pos2(rect.left() + 12.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(15.0),
            color,
        );
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        // Логотип: живой кадр питомца + словомарка.
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            if self.logo.is_none() {
                let set = placeholder(64);
                self.logo = Some(ui.ctx().load_texture(
                    "logo",
                    frame_to_image(&set.idle[0]),
                    TextureOptions::NEAREST,
                ));
            }
            if let Some(tex) = &self.logo {
                ui.add(egui::Image::new((tex.id(), egui::vec2(26.0, 26.0))));
            }
            ui.label(
                RichText::new("Driftling")
                    .size(17.0)
                    .family(semibold_family())
                    .color(TEXT),
            );
        });
        ui.add_space(14.0);

        self.nav_item(ui, Page::Pet, &fl!("nav-pet"));
        ui.add_space(4.0);
        self.nav_item(ui, Page::App, &fl!("nav-app"));
        if self.debug_enabled {
            ui.add_space(4.0);
            self.nav_item(ui, Page::Debug, &fl!("nav-debug"));
        }

        let (checked, up) = {
            let st = self.poll.lock().unwrap();
            (st.checked, st.up)
        };
        ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(4.0);
            daemon_dot(ui, up, checked);
        });
    }

    // -- Страница «Питомец» -------------------------------------------------

    fn hero_card(&mut self, ui: &mut egui::Ui, st: &PollState) {
        let info = st.info.clone();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                // Портрет: рамка-подложка, пиксели чёткие (nearest).
                let size = info
                    .as_ref()
                    .map(|i| i.attributes.size)
                    .unwrap_or_else(|| PetAttributes::default().size);
                self.portrait.ensure(ui.ctx(), size);
                let t = ui.input(|i| i.time);
                egui::Frame::new()
                    .fill(PORTRAIT_BG)
                    .stroke(Stroke::new(1.0, CARD_STROKE))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        if let Some(tex) = self.portrait.current(t) {
                            let present = info.as_ref().is_some_and(|i| i.state.is_some());
                            let img = egui::Image::new((tex.id(), egui::vec2(112.0, 112.0)));
                            let img = if present {
                                img
                            } else {
                                // Убранный питомец — приглушённый портрет.
                                img.tint(Color32::from_gray(120))
                            };
                            ui.add(img);
                        }
                    });
                ui.add_space(8.0);

                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    let name = info
                        .as_ref()
                        .map(|i| i.name.clone())
                        .unwrap_or_else(|| fl!("default-pet-name"));
                    ui.label(
                        RichText::new(name)
                            .size(20.0)
                            .family(semibold_family())
                            .color(TEXT),
                    );
                    ui.add_space(2.0);
                    ui.horizontal(|ui| match (&st.checked, &st.up, &info) {
                        (false, ..) => badge(ui, &fl!("badge-checking"), MUTED),
                        (true, false, _) => badge(ui, &fl!("daemon-not-running"), DANGER),
                        (true, true, Some(i)) => {
                            let s = i.state.as_deref();
                            badge(ui, &state_label(s), state_color(s));
                        }
                        (true, true, None) => badge(ui, &fl!("badge-no-data"), MUTED),
                    });
                    if st.up {
                        if let Some(i) = &info {
                            ui.label(
                                RichText::new(fl!(
                                    "online-for",
                                    uptime = format_uptime(i.uptime_secs)
                                ))
                                .size(12.5)
                                .color(MUTED),
                            );
                        }
                    }
                    ui.add_space(6.0);

                    let present = info.as_ref().is_some_and(|i| i.state.is_some());
                    ui.horizontal(|ui| {
                        if primary_button(ui, &fl!("btn-summon"), st.up && !present).clicked() {
                            self.action(
                                ui,
                                Request::Summon,
                                fl!("msg-pet-summoned"),
                                &self.pet_action,
                            );
                        }
                        if outline_button(ui, &fl!("btn-dismiss"), TEXT, st.up && present).clicked()
                        {
                            self.action(
                                ui,
                                Request::Dismiss,
                                fl!("msg-pet-dismissed"),
                                &self.pet_action,
                            );
                        }
                    });
                    if let Some(text) = &*self.pet_action.lock().unwrap() {
                        ui.label(RichText::new(text).size(12.5).color(MUTED));
                    }
                });
            });
        });
        // Живой портрет: 2 fps, будим отрисовку заранее.
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }

    fn stats_card(&mut self, ui: &mut egui::Ui, st: &PollState) {
        card(ui, |ui| {
            section_label(ui, &fl!("section-stats"));
            ui.add_space(2.0);
            match &st.info {
                Some(i) => {
                    let a = i.attributes;
                    // Плотный ритм строк: межстрочный зазор 3px, не 12.
                    ui.spacing_mut().item_spacing.y = 3.0;
                    stat_row(
                        ui,
                        &fl!("stat-speed"),
                        &fl!("stat-speed-value", value = format!("{:.0}", a.walk_speed)),
                        Some(norm(a.walk_speed, 5.0, 400.0)),
                    );
                    stat_row(
                        ui,
                        &fl!("stat-curiosity"),
                        &format!("{}/100", a.curiosity),
                        Some(a.curiosity as f32 / 100.0),
                    );
                    stat_row(
                        ui,
                        &fl!("stat-sleepiness"),
                        &format!("{}/100", a.sleepiness),
                        Some(a.sleepiness as f32 / 100.0),
                    );
                    stat_row(
                        ui,
                        &fl!("stat-size"),
                        &fl!("stat-size-value", value = a.size.to_string()),
                        None,
                    );
                    stat_row(
                        ui,
                        &fl!("stat-sleep"),
                        &fl!(
                            "stat-sleep-value",
                            min = format!("{:.0}", a.sleep_min),
                            max = format!("{:.0}", a.sleep_max)
                        ),
                        None,
                    );
                    ui.spacing_mut().item_spacing.y = 10.0;
                }
                None => {
                    ui.label(RichText::new(fl!("stats-unavailable")).color(MUTED));
                }
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(fl!("stats-grow-note"))
                    .size(12.5)
                    .color(MUTED)
                    .italics(),
            );
        });
    }

    fn page_pet(&mut self, ui: &mut egui::Ui) {
        let st = {
            let guard = self.poll.lock().unwrap();
            PollState {
                checked: guard.checked,
                up: guard.up,
                info: guard.info.clone(),
                raw: None,
            }
        };
        page_title(ui, &fl!("nav-pet"));
        ui.add_space(4.0);
        self.hero_card(ui, &st);
        ui.add_space(2.0);
        self.stats_card(ui, &st);
    }

    // -- Страница «Приложение» ----------------------------------------------

    fn page_app(&mut self, ui: &mut egui::Ui) {
        let (checked, up, uptime) = {
            let st = self.poll.lock().unwrap();
            (st.checked, st.up, st.info.as_ref().map(|i| i.uptime_secs))
        };

        page_title(ui, &fl!("nav-app"));
        ui.add_space(4.0);

        card(ui, |ui| {
            section_label(ui, &fl!("section-launch"));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(fl!("autostart-title"));
                    ui.label(RichText::new(fl!("autostart-desc")).size(12.5).color(MUTED));
                });
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    if toggle_switch(ui, &mut self.autostart).changed() {
                        self.autostart_result = Some(match set_autostart(self.autostart) {
                            Ok(()) if self.autostart => fl!("msg-autostart-on"),
                            Ok(()) => fl!("msg-autostart-off"),
                            Err(e) => {
                                // Не вышло — галка возвращается к факту.
                                self.autostart = autostart_enabled();
                                fl!("generic-error", error = e)
                            }
                        });
                    }
                });
            });
            if let Some(text) = &self.autostart_result {
                ui.label(RichText::new(text).size(12.5).color(MUTED));
            }
        });
        ui.add_space(2.0);

        card(ui, |ui| {
            section_label(ui, &fl!("section-daemon"));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                daemon_dot(ui, up, checked);
                if let (true, Some(secs)) = (up, uptime) {
                    ui.label(
                        RichText::new(fl!("uptime-label", uptime = format_uptime(secs)))
                            .size(12.5)
                            .color(MUTED),
                    );
                }
            });
            ui.add_space(2.0);

            // Подтверждение вторым кликом; взвод сбрасывается через 3 с.
            let now = ui.input(|i| i.time);
            if let Some(t) = self.stop_armed_at {
                if now - t > 3.0 {
                    self.stop_armed_at = None;
                }
            }
            let armed = self.stop_armed_at.is_some();
            let label = if armed {
                fl!("btn-stop-confirm")
            } else {
                fl!("btn-stop-daemon")
            };
            if outline_button(ui, &label, DANGER, up).clicked() {
                if armed {
                    self.stop_armed_at = None;
                    self.action(
                        ui,
                        Request::Quit,
                        fl!("msg-daemon-stopped"),
                        &self.daemon_action,
                    );
                } else {
                    self.stop_armed_at = Some(now);
                    ui.ctx().request_repaint_after(Duration::from_secs(3));
                }
            }
            if let Some(text) = &*self.daemon_action.lock().unwrap() {
                ui.label(RichText::new(text).size(12.5).color(MUTED));
            }
        });
        ui.add_space(2.0);

        card(ui, |ui| {
            section_label(ui, &fl!("section-about"));
            ui.add_space(2.0);
            ui.label(format!("Driftling {}", env!("CARGO_PKG_VERSION")));
            ui.label(RichText::new(fl!("about-desc")).size(12.5).color(MUTED));
        });
    }

    // -- Страница «Отладка» ---------------------------------------------------

    fn page_debug(&mut self, ui: &mut egui::Ui) {
        page_title(ui, &fl!("nav-debug"));
        ui.add_space(4.0);

        // Предупреждение: это админка, а не игровой путь.
        egui::Frame::new()
            .fill(tinted(AMBER, 26))
            .stroke(Stroke::new(1.0, tinted(AMBER, 120)))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.label(RichText::new(fl!("debug-warning")).size(13.5).color(AMBER));
            });
        ui.add_space(2.0);

        card(ui, |ui| {
            section_label(ui, &fl!("section-pet-attrs"));
            ui.add_space(2.0);

            let f = &mut self.form;
            ui.add(Slider::new(&mut f.size, 32..=256).text(fl!("slider-size")));
            ui.add(Slider::new(&mut f.walk_speed, 5.0..=400.0).text(fl!("slider-speed")));
            ui.add(Slider::new(&mut f.curiosity, 0..=100).text(fl!("slider-curiosity")));
            ui.add(Slider::new(&mut f.sleepiness, 0..=100).text(fl!("slider-sleepiness")));
            ui.horizontal(|ui| {
                ui.label(RichText::new(fl!("sleep-secs-label")).color(MUTED));
                ui.add(
                    DragValue::new(&mut f.sleep_min)
                        .range(1.0..=3600.0)
                        .prefix(format!("{} ", fl!("sleep-from"))),
                );
                ui.add(
                    DragValue::new(&mut f.sleep_max)
                        .range(1.0..=7200.0)
                        .prefix(format!("{} ", fl!("sleep-to"))),
                );
            });
            ui.label(
                RichText::new(fl!("debug-clamp-note"))
                    .size(12.5)
                    .color(MUTED),
            );
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                if primary_button(ui, &fl!("btn-apply"), true).clicked() {
                    self.form = self.form.clamped();
                    self.action(
                        ui,
                        Request::SetAttributes(self.form),
                        fl!("msg-applied"),
                        &self.debug_action,
                    );
                }
                if outline_button(ui, &fl!("btn-reset"), TEXT, true).clicked() {
                    self.form = PetAttributes::default();
                }
            });
            if let Some(text) = &*self.debug_action.lock().unwrap() {
                ui.label(RichText::new(text).size(12.5).color(MUTED));
            }
        });
        ui.add_space(2.0);

        card(ui, |ui| {
            section_label(ui, &fl!("section-raw"));
            let raw = self.poll.lock().unwrap().raw.clone();
            CollapsingHeader::new(RichText::new("PetInfo (JSON)").color(MUTED))
                .default_open(false)
                .show(ui, |ui| match raw {
                    Some(json) => {
                        ui.add(Label::new(RichText::new(json).monospace().color(TEXT)));
                    }
                    None => {
                        ui.label(RichText::new(fl!("raw-none")).color(MUTED));
                    }
                });
        });
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Дебаг-форма один раз синкается с живыми характеристиками.
        if !self.form_synced {
            if let Some(info) = &self.poll.lock().unwrap().info {
                self.form = info.attributes;
                self.form_synced = true;
            }
        }

        egui::Panel::left("sidebar")
            .exact_size(190.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(SIDEBAR_BG)
                    .inner_margin(Margin::symmetric(10, 14)),
            )
            .show(ui, |ui| self.sidebar(ui));

        // У каждой страницы свой скролл; при переключении (и на старте,
        // перекрывая скролл из сохранённой egui-памяти) — всегда наверх.
        let page_changed = self.last_page != Some(self.page);
        self.last_page = Some(self.page);

        egui::CentralPanel::default_margins()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::same(20)))
            .show(ui, |ui| {
                let mut scroll = ScrollArea::vertical().auto_shrink(false).id_salt(self.page);
                if page_changed {
                    scroll = scroll.vertical_scroll_offset(0.0);
                }
                scroll.show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(10.0, 12.0);
                    match self.page {
                        Page::Pet => self.page_pet(ui),
                        Page::App => self.page_app(ui),
                        Page::Debug => self.page_debug(ui),
                    }
                });
            });
    }
}

/// Иконка окна: первый idle-кадр питомца (иначе в заголовке — generic «W»).
fn app_icon() -> egui::IconData {
    let set = placeholder(64);
    let f = &set.idle[0];
    let rgba = f
        .argb
        .iter()
        .flat_map(|&p| {
            [
                ((p >> 16) & 0xff) as u8,
                ((p >> 8) & 0xff) as u8,
                (p & 0xff) as u8,
                (p >> 24) as u8,
            ]
        })
        .collect();
    egui::IconData {
        rgba,
        width: f.w,
        height: f.h,
    }
}

fn main() -> eframe::Result {
    env_logger::init();
    let debug_enabled = std::env::args().any(|a| a == "--debug");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([780.0, 540.0])
            .with_min_inner_size([640.0, 480.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Driftling",
        options,
        Box::new(move |cc| Ok(Box::new(SettingsApp::new(cc, debug_enabled)))),
    )
}

// ---------------------------------------------------------------------------
// Тесты чистой логики
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Отображение всех состояний тотально и идёт через i18n (ТД-30):
    /// сравниваем с fl!-рендером — тест не зависит от локали машины.
    #[test]
    fn state_labels_are_localized_and_total() {
        assert_eq!(state_label(None), fl!("state-absent"));
        assert_eq!(state_label(Some("Idle")), fl!("state-idle"));
        assert_eq!(state_label(Some("Walk")), fl!("state-walk"));
        assert_eq!(state_label(Some("Sleep")), fl!("state-sleep"));
        assert_eq!(state_label(Some("Falling")), fl!("state-falling"));
        assert_eq!(state_label(Some("Dragged")), fl!("state-dragged"));
        assert_eq!(state_label(Some("Landing")), fl!("state-landing"));
        assert_eq!(state_label(Some("???")), fl!("state-unknown"));
        // Известные состояния не проваливаются в «неизвестно».
        assert_ne!(state_label(Some("Idle")), state_label(Some("???")));
    }

    #[test]
    fn state_colors_match_design_system() {
        assert_eq!(state_color(Some("Walk")), ACCENT_LIGHT);
        assert_eq!(state_color(Some("Sleep")), SLEEP_BLUE);
        assert_eq!(state_color(Some("Falling")), AMBER);
        assert_eq!(state_color(Some("Dragged")), AMBER);
        assert_eq!(state_color(None), MUTED);
    }

    #[test]
    fn norm_clamps_to_unit_range() {
        assert_eq!(norm(5.0, 5.0, 400.0), 0.0);
        assert_eq!(norm(400.0, 5.0, 400.0), 1.0);
        assert_eq!(norm(-10.0, 5.0, 400.0), 0.0);
        assert_eq!(norm(999.0, 5.0, 400.0), 1.0);
        // Вырожденная шкала не даёт NaN/inf.
        assert_eq!(norm(1.0, 3.0, 3.0), 0.0);
        let mid = norm(202.5, 5.0, 400.0);
        assert!((mid - 0.5).abs() < 1e-6);
    }

    #[test]
    fn argb_pixel_converts_channelwise() {
        let c = argb_to_color32(0xff_8a_63_d2);
        assert_eq!((c.r(), c.g(), c.b(), c.a()), (0x8a, 0x63, 0xd2, 0xff));
        // Прозрачный пиксель остаётся прозрачным.
        assert_eq!(argb_to_color32(0).a(), 0);
    }

    /// Структура аптайма: часы+минуты / минуты+секунды / секунды.
    /// Сравниваем с fl!-рендером — не зависит от локали машины.
    #[test]
    fn uptime_formatting_structure() {
        assert_eq!(format_uptime(7), fl!("uptime-seconds", seconds = 7));
        assert_eq!(
            format_uptime(75),
            format!(
                "{} {}",
                fl!("uptime-minutes", minutes = 1),
                fl!("uptime-seconds", seconds = 15)
            )
        );
        assert_eq!(
            format_uptime(3700),
            format!(
                "{} {}",
                fl!("uptime-hours", hours = 1),
                fl!("uptime-minutes", minutes = 1)
            )
        );
        assert_eq!(
            format_uptime(7980),
            format!(
                "{} {}",
                fl!("uptime-hours", hours = 2),
                fl!("uptime-minutes", minutes = 13)
            )
        );
    }

    /// Русские плюральные формы CLDR (ТД-30): «1 минута / 2 минуты /
    /// 5 минут» — детерминированно, через явный ru-загрузчик.
    #[test]
    fn uptime_russian_plural_forms() {
        let ru = crate::i18n::loader_for("ru");
        let minutes = |n: u64| ru.get_args("uptime-minutes", HashMap::from([("minutes", n)]));
        assert_eq!(minutes(1), "1 минута");
        assert_eq!(minutes(2), "2 минуты");
        assert_eq!(minutes(5), "5 минут");
        assert_eq!(minutes(21), "21 минута");
        assert_eq!(minutes(64), "64 минуты");
        let hours = |n: u64| ru.get_args("uptime-hours", HashMap::from([("hours", n)]));
        assert_eq!(hours(1), "1 час");
        assert_eq!(hours(3), "3 часа");
        assert_eq!(hours(11), "11 часов");
        let seconds = |n: u64| ru.get_args("uptime-seconds", HashMap::from([("seconds", n)]));
        assert_eq!(seconds(1), "1 секунда");
        assert_eq!(seconds(15), "15 секунд");
    }

    /// Английские плюральные формы (фолбэк-язык обязан быть полным).
    #[test]
    fn uptime_english_plural_forms() {
        let en = crate::i18n::loader_for("en");
        let minutes = |n: u64| en.get_args("uptime-minutes", HashMap::from([("minutes", n)]));
        assert_eq!(minutes(1), "1 minute");
        assert_eq!(minutes(2), "2 minutes");
        let hours = |n: u64| en.get_args("uptime-hours", HashMap::from([("hours", n)]));
        assert_eq!(hours(1), "1 hour");
        assert_eq!(hours(5), "5 hours");
    }

    #[test]
    fn desktop_file_is_well_formed() {
        let text = desktop_file_content("/usr/bin/driftling");
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("Type=Application\n"));
        assert!(text.contains("Name=Driftling\n"));
        assert!(text.contains("Exec=/usr/bin/driftling\n"));
        assert!(text.contains("X-KDE-autostart-after=panel\n"));
        assert!(text.ends_with('\n'));
    }

    /// Exec экранируется по Desktop Entry spec: путь с пробелами/кавычками
    /// переживает оба уровня разбора (общий unescape + разбор аргументов).
    #[test]
    fn exec_quoting_survives_special_paths() {
        // Простой путь остаётся как есть.
        assert_eq!(exec_quote("/usr/bin/driftling"), "/usr/bin/driftling");
        assert_eq!(exec_quote("driftling"), "driftling");

        // Пробел: достаточно кавычек, внутри ничего не экранируется.
        assert_eq!(
            exec_quote("/opt/my apps/driftling"),
            "\"/opt/my apps/driftling\""
        );
        let text = desktop_file_content("/opt/my apps/driftling");
        assert!(text.contains("Exec=\"/opt/my apps/driftling\"\n"));

        // Кавычка: в файле `\\"` (общий unescape -> `\"` для Exec-разбора).
        assert_eq!(exec_quote(r#"/tmp/a"b"#), "\"/tmp/a\\\\\"b\"");
        // Доллар и бэктик — та же схема.
        assert_eq!(exec_quote("/tmp/a$b"), "\"/tmp/a\\\\$b\"");
        assert_eq!(exec_quote("/tmp/a`b"), "\"/tmp/a\\\\`b\"");
        // Литеральный бэкслеш: четыре в файле.
        assert_eq!(exec_quote(r"/tmp/a\b"), "\"/tmp/a\\\\\\\\b\"");
        // Пустая строка не даёт пустого Exec без кавычек.
        assert_eq!(exec_quote(""), "\"\"");
    }

    #[test]
    fn autostart_path_layout() {
        let p = autostart_path_in(Path::new("/home/u/.config"));
        assert_eq!(p, Path::new("/home/u/.config/autostart/driftling.desktop"));
    }

    #[test]
    fn daemon_exec_prefers_sibling_binary() {
        assert_eq!(daemon_exec_in(None), "driftling");
        assert_eq!(
            daemon_exec_in(Some(Path::new("/nonexistent-dir"))),
            "driftling"
        );

        let dir =
            std::env::temp_dir().join(format!("driftling-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("driftling");
        std::fs::write(&bin, b"").unwrap();
        assert_eq!(daemon_exec_in(Some(&dir)), bin.display().to_string());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_autostart_writes_and_removes_desktop_file() {
        let dir =
            std::env::temp_dir().join(format!("driftling-autostart-test-{}", std::process::id()));
        let path = autostart_path_in(&dir);

        set_autostart_at(&path, true, "/usr/bin/driftling").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, desktop_file_content("/usr/bin/driftling"));

        set_autostart_at(&path, false, "/usr/bin/driftling").unwrap();
        assert!(!path.exists());
        set_autostart_at(&path, false, "/usr/bin/driftling").unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }
}
