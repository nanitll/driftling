//! Язык интерфейса окна: палитра, шрифты, стиль egui и мелкие виджеты,
//! из которых собраны все страницы. Здесь нет ни IPC, ни бизнес-логики —
//! только то, как оно выглядит.

use std::sync::Arc;

use crate::i18n::fl;
use driftling_core::sprite::{placeholder_colored, Frame};
use driftling_core::{palette, Stage};
use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Layout, Margin, RichText, Sense, Stroke, StrokeKind, TextStyle, TextureHandle, TextureOptions,
};

pub const BG: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x22);
pub const SIDEBAR_BG: Color32 = Color32::from_rgb(0x15, 0x15, 0x1c);
pub const CARD: Color32 = Color32::from_rgb(0x23, 0x23, 0x2e);
pub const CARD_STROKE: Color32 = Color32::from_rgb(0x2f, 0x2f, 0x3d);
pub const TEXT: Color32 = Color32::from_rgb(0xe8, 0xe8, 0xf0);
pub const MUTED: Color32 = Color32::from_rgb(0x9a, 0x9a, 0xac);
pub const SUCCESS: Color32 = Color32::from_rgb(0x6c, 0xcb, 0x5f);
pub const DANGER: Color32 = Color32::from_rgb(0xe0, 0x5f, 0x5f);
pub const SLEEP_BLUE: Color32 = Color32::from_rgb(0x6b, 0x8f, 0xd2);
pub const AMBER: Color32 = Color32::from_rgb(0xe0, 0xa8, 0x4f);
pub const TRACK: Color32 = Color32::from_rgb(0x2f, 0x2f, 0x3d);
pub const PORTRAIT_BG: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x27);

// Акцент интерфейса — НЕ константа: он следует за цветом питомца из
// PetInfo (fallback DEFAULT_PET_COLOR, когда демон лежит), см. accent_pair
// и SettingsApp::{accent, accent_light}.

pub fn tinted(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Акцентная пара из цвета питомца (ARGB): сам цвет для заливок и его
/// осветлённый тон для текста/линий на тёмном фоне.
pub fn accent_pair(argb: u32) -> (Color32, Color32) {
    (
        argb_to_color32(0xff00_0000 | (argb & 0x00ff_ffff)),
        argb_to_color32(palette::lighten(0xff00_0000 | (argb & 0x00ff_ffff), 1.35)),
    )
}

/// Упаковать RGB-триплет цветового пикера в непрозрачный ARGB.
pub fn rgb_to_argb([r, g, b]: [u8; 3]) -> u32 {
    0xff00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// Распаковать ARGB в RGB-триплет для цветового пикера.
pub fn argb_to_rgb(argb: u32) -> [u8; 3] {
    [
        ((argb >> 16) & 0xff) as u8,
        ((argb >> 8) & 0xff) as u8,
        (argb & 0xff) as u8,
    ]
}

// ---------------------------------------------------------------------------
// Чистая логика (юнит-тесты внизу файла)
// ---------------------------------------------------------------------------

pub fn argb_to_color32(p: u32) -> Color32 {
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
/// Системные шрифты с кириллицей — берём первый существующий.
pub const FONT_REGULAR: &[&str] = &[
    "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
    "/usr/share/fonts/truetype/cantarell/Cantarell-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];
pub const FONT_SEMIBOLD: &[&str] = &[
    "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf",
    "/usr/share/fonts/truetype/cantarell/Cantarell-Bold.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
];

/// Имя полужирной семьи (заголовки); наполняется в `install_fonts`.
pub fn semibold_family() -> FontFamily {
    FontFamily::Name("ui-semibold".into())
}

/// Первый существующий файл из списка.
pub fn first_existing(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

/// Системный шрифт с кириллицей первым в Proportional + отдельная
/// полужирная семья для заголовков. Ничего не нашлось — остаёмся на
/// встроенных шрифтах egui (кириллица там частичная, но окно живёт).
pub fn install_fonts(ctx: &egui::Context) {
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

/// Тёмная тема дизайн-системы поверх egui-дефолтов; акцент — цвет питомца.
/// Повторный вызов при перекраске идемпотентен.
pub fn apply_style(ctx: &egui::Context, accent: Color32, accent_light: Color32) {
    ctx.all_styles_mut(|style| style_mut(style, accent, accent_light));
}

pub fn style_mut(style: &mut egui::Style, accent: Color32, accent_light: Color32) {
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
    v.selection.bg_fill = accent;
    v.selection.stroke = Stroke::new(1.0, accent_light);
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
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
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
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
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

/// Секционная подпись мелкой группы: 13px, приглушённая, верхний регистр.
/// Заголовки карточек рисует [`card_title`] — заглавные плохо читаются
/// на кириллице.
#[allow(dead_code)]
pub fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(13.0)
            .color(MUTED)
            .family(semibold_family()),
    );
}

/// Заголовок страницы (22, полужирный).
pub fn page_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(22.0)
            .family(semibold_family())
            .color(TEXT),
    );
}

/// Бейдж-чип состояния: скруглённая подложка в тон текста.
pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(tinted(color, 36))
        .corner_radius(CornerRadius::same(99))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(13.0).color(color));
        });
}

/// Акцентная (заливка) кнопка; выключенная — заметно приглушена.
pub fn primary_button(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    accent: Color32,
) -> egui::Response {
    let (fill, fg) = if enabled {
        (accent, Color32::WHITE)
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
pub fn outline_button(
    ui: &mut egui::Ui,
    text: &str,
    color: Color32,
    enabled: bool,
) -> egui::Response {
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
/// канонического toggle из egui demo); включённый трек — в акценте.
pub fn toggle_switch(ui: &mut egui::Ui, on: &mut bool, accent: Color32) -> egui::Response {
    let size = egui::vec2(44.0, 24.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let bg = Color32::from_rgb(
            (TRACK.r() as f32 + (accent.r() as f32 - TRACK.r() as f32) * t) as u8,
            (TRACK.g() as f32 + (accent.g() as f32 - TRACK.g() as f32) * t) as u8,
            (TRACK.b() as f32 + (accent.b() as f32 - TRACK.b() as f32) * t) as u8,
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

/// Квадратик-пресет цвета питомца; текущий обведён кольцом.
pub fn swatch(ui: &mut egui::Ui, argb: u32, selected: bool, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect.shrink(3.0), 6.0, argb_to_color32(0xff00_0000 | argb));
        if selected {
            p.rect_stroke(rect, 8.0, Stroke::new(2.0, TEXT), StrokeKind::Inside);
        } else if response.hovered() {
            p.rect_stroke(
                rect,
                8.0,
                Stroke::new(1.0, tinted(TEXT, 90)),
                StrokeKind::Inside,
            );
        }
    }
    response.on_hover_text(tooltip)
}

/// Точка-индикатор статуса демона + подпись.
pub fn daemon_dot(ui: &mut egui::Ui, up: bool, checked: bool) {
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

pub fn frame_to_image(f: &Frame) -> egui::ColorImage {
    egui::ColorImage::new(
        [f.w as usize, f.h as usize],
        f.argb.iter().map(|&p| argb_to_color32(p)).collect(),
    )
}

/// «Дежурные» кадры стадии из встроенного арт-пака (фаза C): idle тела,
/// для яйца — семейство `egg` (у яйца нет idle). Колоризация — тот же
/// путь, что у демона (pack::colorize цветом питомца). Битый пак — не
/// смерть: фолбэк на процедурный блоб с логом, как в демоне.
pub fn pack_idle_frames(stage: Stage, target_px: u32, color: u32) -> Vec<Frame> {
    match driftling_core::pack::default_pack() {
        Ok(p) => {
            let anim = if stage == Stage::Egg { "egg" } else { "idle" };
            p.frames(stage, anim, target_px, color)
        }
        Err(e) => {
            log::warn!("арт-пак не загрузился ({e}) — процедурный фолбэк");
            placeholder_colored(target_px, stage, color).idle
        }
    }
}

/// Кэш дежурных кадров под текущие размер, цвет и стадию питомца.
#[derive(Default)]
pub struct Portrait {
    size: u32,
    color: u32,
    /// Стадия роста из PetInfo (None до первого ensure) — яйцо в карточке
    /// выглядит яйцом, а не взрослым.
    stage: Option<Stage>,
    frames: Vec<TextureHandle>,
}

impl Portrait {
    pub fn ensure(&mut self, ctx: &egui::Context, size: u32, color: u32, stage: Stage) {
        if self.size == size
            && self.color == color
            && self.stage == Some(stage)
            && !self.frames.is_empty()
        {
            return;
        }
        let frames = pack_idle_frames(stage, size.clamp(32, 256), color);
        self.frames = frames
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
        self.color = color;
        self.stage = Some(stage);
    }

    /// Кадр на момент времени `t` (2 fps, как idle в спрайт-движке).
    pub fn current(&self, t: f64) -> Option<&TextureHandle> {
        (!self.frames.is_empty()).then(|| {
            let idx = (t * 2.0) as usize % self.frames.len();
            &self.frames[idx]
        })
    }
}

// ---------------------------------------------------------------------------
// Приложение
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Виджеты подробных настроек (редизайн M0.6)
// ---------------------------------------------------------------------------

/// Тонкая линия-разделитель между строками внутри карточки.
pub const HAIRLINE: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x36);

/// Строка настройки: подпись слева, контрол справа, пояснение под подписью.
///
/// Настоящий виджет, а не рисование по фиксированным офсетам: длинная
/// русская подпись усекается многоточием и не наезжает на контрол, а сам
/// контрол не уезжает за край на узком окне.
pub fn setting_row<R>(
    ui: &mut egui::Ui,
    label: &str,
    hint: Option<&str>,
    control: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let mut out = None;
    ui.horizontal(|ui| {
        let control_w = 210.0_f32.min(ui.available_width() * 0.5);
        let text_w = (ui.available_width() - control_w - 12.0).max(80.0);
        ui.vertical(|ui| {
            ui.set_width(text_w);
            ui.add(egui::Label::new(RichText::new(label).size(14.5).color(TEXT)).truncate());
            if let Some(h) = hint {
                ui.add(
                    egui::Label::new(RichText::new(h).size(12.0).color(MUTED))
                        .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            out = Some(control(ui));
        });
    });
    out.expect("контрол строки настройки отрисован")
}

/// Разделитель между строками настроек внутри одной карточки.
pub fn row_sep(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, HAIRLINE);
    ui.add_space(6.0);
}

/// Шкала-значение: полоска с числом. В отличие от старой рисованной строки
/// не пропадает на узком окне — там остаётся честное число.
pub fn meter(ui: &mut egui::Ui, value: f32, color: Color32) {
    let w = ui.available_width().clamp(60.0, 180.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(w, 10.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, 5.0, TRACK);
        let k = crate::labels::norm(value, 0.0, 100.0);
        if k > 0.0 {
            let mut fill = rect;
            fill.set_width((rect.width() * k).max(6.0));
            p.rect_filled(fill, 5.0, color);
        }
    }
    response.on_hover_text(format!("{value:.0} / 100"));
}

/// Чип-переключатель для множественного выбора (виды транспорта, гости).
pub fn chip(ui: &mut egui::Ui, label: &str, on: bool, accent: Color32) -> egui::Response {
    let text = RichText::new(label)
        .size(13.0)
        .color(if on { Color32::WHITE } else { MUTED });
    ui.add(
        Button::new(text)
            .fill(if on { accent } else { Color32::TRANSPARENT })
            .stroke(Stroke::new(1.0, if on { accent } else { CARD_STROKE }))
            .corner_radius(CornerRadius::same(99)),
    )
}

/// Пустое состояние: почему тут пусто и что с этим сделать.
pub fn empty_state(
    ui: &mut egui::Ui,
    title: &str,
    hint: &str,
    action: Option<(&str, Color32)>,
) -> bool {
    let mut clicked = false;
    ui.vertical_centered(|ui| {
        ui.add_space(18.0);
        ui.label(
            RichText::new(title)
                .size(16.0)
                .family(semibold_family())
                .color(TEXT),
        );
        ui.add_space(4.0);
        ui.label(RichText::new(hint).size(13.0).color(MUTED));
        if let Some((label, accent)) = action {
            ui.add_space(10.0);
            clicked = primary_button(ui, label, true, accent).clicked();
        }
        ui.add_space(18.0);
    });
    clicked
}

/// Уведомление: что случилось после команды. Живёт секунды и исчезает —
/// в отличие от бессмертных строк-результатов, которые висели до следующего
/// действия и через минуту уже врали.
#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub tone: ToastTone,
    /// Момент появления (время egui).
    pub born: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastTone {
    /// Сделано.
    Good,
    /// Сделано наполовину: нужен перезапуск, применилось не всё.
    Warn,
    /// Не вышло.
    Bad,
}

impl ToastTone {
    pub fn color(self) -> Color32 {
        match self {
            ToastTone::Good => SUCCESS,
            ToastTone::Warn => AMBER,
            ToastTone::Bad => DANGER,
        }
    }
}

/// Сколько живёт уведомление, сек.
pub const TOAST_TTL: f64 = 4.5;

/// Показать очередь уведомлений в нижнем углу и выкинуть истёкшие.
pub fn toasts(ui: &mut egui::Ui, queue: &mut Vec<Toast>, now: f64) {
    queue.retain(|t| now - t.born < TOAST_TTL);
    if queue.is_empty() {
        return;
    }
    // Больше трёх на экране — это уже не уведомления, а лента.
    while queue.len() > 3 {
        queue.remove(0);
    }
    let area = egui::Area::new("toasts".into())
        .anchor(Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
        .order(egui::Order::Foreground);
    area.show(ui.ctx(), |ui| {
        ui.vertical(|ui| {
            for t in queue.iter() {
                let left = TOAST_TTL - (now - t.born);
                let alpha = (left / 0.6).clamp(0.0, 1.0) as f32;
                let color = t.tone.color();
                egui::Frame::new()
                    .fill(tinted(CARD, (240.0 * alpha) as u8))
                    .stroke(Stroke::new(1.0, tinted(color, (170.0 * alpha) as u8)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let dot = ui
                                .allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover())
                                .0;
                            ui.painter().circle_filled(
                                dot.center(),
                                4.0,
                                tinted(color, (255.0 * alpha) as u8),
                            );
                            ui.label(
                                RichText::new(&t.text)
                                    .size(13.0)
                                    .color(tinted(TEXT, (255.0 * alpha) as u8)),
                            );
                        });
                    });
                ui.add_space(6.0);
            }
        });
    });
}

/// Сегментированный переключатель: выбор одного из нескольких (размер
/// питомца, темперамент, режим синка). Возвращает новый индекс, если
/// человек переключил.
pub fn segmented(
    ui: &mut egui::Ui,
    options: &[String],
    selected: Option<usize>,
    accent: Color32,
) -> Option<usize> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        for (i, label) in options.iter().enumerate() {
            if chip(ui, label, selected == Some(i), accent).clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}

/// Заголовок карточки: обычный регистр, полужирный. Заглавные плохо
/// читаются на кириллице и рвут длинные русские подписи.
pub fn card_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(15.5)
            .family(semibold_family())
            .color(TEXT),
    );
    ui.add_space(2.0);
}

/// Пояснение под заголовком карточки или под группой строк.
pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(MUTED));
}
