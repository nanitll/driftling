//! Растеризация текста для оверлея (фаза B3/B6): пункты контекстного меню,
//! речевые пузыри питомца («Привет!» при вылуплении). fontdue + вендоренный
//! DejaVu Sans (кириллица покрыта, лицензия — assets/fonts/DejaVu-LICENSE),
//! выход — альфа-битмапы в формате sprite::Frame.
//!
//! Про альфу и premultiply. Вейланд-рендер (crates/platform/src/wayland.rs,
//! `fn blit`) НЕ смешивает пиксели: альфа 0 пропускается, всё остальное
//! копируется как есть, а буфер компоситор трактует как premultiplied ARGB.
//! Отсюда две договорённости:
//! - `menu_frame`/`bubble_frame` пекут сглаженные глифы прямо на непрозрачный
//!   фон карточки — внутри карточки альфа 255, блендинг на блите не нужен;
//! - `text_frame` (голый текст) хранит PREMULTIPLIED-альфу: поверх пустого
//!   (прозрачного) холста компоситор смешает кромки корректно. Ограничение:
//!   при наложении поверх других спрайтов полупрозрачные кромки глифов
//!   перезапишут пиксели под собой — текущий blit не умеет src-over.

use crate::geometry::Rect;
use crate::sprite::Frame;
use std::sync::OnceLock;

static FONT_DATA: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");

// Язык дизайна settings-окна: тёмная карточка, светлый текст; акцент
// приходит параметром (menu_frame) — он следует за цветом питомца.
const CARD_BG: u32 = 0xff_23_23_2e;
const CARD_STROKE: u32 = 0xff_2f_2f_3d;
const TEXT_COLOR: u32 = 0xff_e8_e8_f0;
/// Скругление карточек, px (как в настройках).
const CARD_RADIUS: f32 = 10.0;
/// Сила подсветки строки меню под курсором.
const HOVER_STRENGTH: f32 = 0.35;

fn font() -> &'static fontdue::Font {
    static FONT: OnceLock<fontdue::Font> = OnceLock::new();
    FONT.get_or_init(|| {
        fontdue::Font::from_bytes(FONT_DATA, fontdue::FontSettings::default())
            .expect("вендоренный DejaVuSans.ttf обязан парситься")
    })
}

/// Позиции глифов текста (fontdue Layout, Y вниз, поддерживает '\n').
fn layout_glyphs(text: &str, px: f32) -> Vec<fontdue::layout::GlyphPosition> {
    use fontdue::layout::{CoordinateSystem, Layout, TextStyle};
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.append(&[font()], &TextStyle::new(text, px, 0));
    layout.glyphs().clone()
}

/// Высота строки шрифта в пикселях (для вертикального выравнивания).
fn line_height(px: f32) -> f32 {
    font()
        .horizontal_line_metrics(px)
        .map(|m| m.new_line_size)
        .unwrap_or(px * 1.2)
}

/// Габариты текста: ширина по крайнему глифу, высота по числу строк.
pub(crate) fn measure(text: &str, px: f32) -> (f32, f32) {
    let w = layout_glyphs(text, px)
        .iter()
        .map(|g| g.x + g.width as f32)
        .fold(0.0f32, f32::max);
    let lines = text.split('\n').count().max(1);
    (w, line_height(px) * lines as f32)
}

/// Смешать пиксель `color` с покрытием `cov` (0..=1) поверх кадра.
/// Кадр хранится в premultiplied ARGB; источник премультиплицируется здесь.
/// На непрозрачном фоне (карточки) результат остаётся непрозрачным.
pub(crate) fn blend_px(frame: &mut Frame, x: i32, y: i32, color: u32, cov: f32) {
    if x < 0 || y < 0 || x >= frame.w as i32 || y >= frame.h as i32 {
        return;
    }
    let sa = cov.clamp(0.0, 1.0) * (((color >> 24) & 0xff) as f32 / 255.0);
    if sa <= 0.0 {
        return;
    }
    let i = (y as u32 * frame.w + x as u32) as usize;
    let dst = frame.argb[i];
    let inv = 1.0 - sa;
    let ch = |sh: u32| -> u32 {
        let sc = ((color >> sh) & 0xff) as f32 * sa; // premultiply источника
        let dc = ((dst >> sh) & 0xff) as f32; // приёмник уже premultiplied
        ((sc + dc * inv).round() as u32).min(255)
    };
    let da = ((dst >> 24) & 0xff) as f32;
    let oa = ((sa * 255.0 + da * inv).round() as u32).min(255);
    frame.argb[i] = (oa << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0);
}

/// Нарисовать глифы текста в кадр со смещением (`ox`, `oy`).
pub(crate) fn draw_text(frame: &mut Frame, text: &str, px: f32, color: u32, ox: f32, oy: f32) {
    for g in layout_glyphs(text, px) {
        if g.width == 0 || g.height == 0 {
            continue;
        }
        let (_, coverage) = font().rasterize_config(g.key);
        for row in 0..g.height {
            for col in 0..g.width {
                let a = coverage[row * g.width + col] as f32 / 255.0;
                if a > 0.0 {
                    blend_px(
                        frame,
                        (g.x + ox).round() as i32 + col as i32,
                        (g.y + oy).round() as i32 + row as i32,
                        color,
                        a,
                    );
                }
            }
        }
    }
}

/// SDF скруглённого прямоугольника (координаты относительно левого верха).
fn rounded_rect_sdf(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let px = (x - w / 2.0).abs() - (w / 2.0 - r);
    let py = (y - h / 2.0).abs() - (h / 2.0 - r);
    let ax = px.max(0.0);
    let ay = py.max(0.0);
    (ax * ax + ay * ay).sqrt() + px.max(py).min(0.0) - r
}

/// Карточка: скруглённый прямоугольник CARD_BG с обводкой CARD_STROKE,
/// сглаженный край через SDF-покрытие.
fn draw_card(frame: &mut Frame, x0: f32, y0: f32, w: f32, h: f32, r: f32) {
    for y in y0.floor() as i32..(y0 + h).ceil() as i32 {
        for x in x0.floor() as i32..(x0 + w).ceil() as i32 {
            let d = rounded_rect_sdf(x as f32 - x0 + 0.5, y as f32 - y0 + 0.5, w, h, r);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                let color = if d > -1.5 { CARD_STROKE } else { CARD_BG };
                blend_px(frame, x, y, color, cov);
            }
        }
    }
}

/// Полупрозрачная заливка скруглённого прямоугольника (подсветка строки).
fn fill_rounded(frame: &mut Frame, area: Rect, r: f32, color: u32, k: f32) {
    let Rect { x: x0, y: y0, w, h } = area;
    for y in y0.floor() as i32..(y0 + h).ceil() as i32 {
        for x in x0.floor() as i32..(x0 + w).ceil() as i32 {
            let d = rounded_rect_sdf(x as f32 - x0 + 0.5, y as f32 - y0 + 0.5, w, h, r);
            let cov = (0.5 - d).clamp(0.0, 1.0) * k;
            if cov > 0.0 {
                blend_px(frame, x, y, color, cov);
            }
        }
    }
}

pub(crate) fn transparent(w: u32, h: u32) -> Frame {
    Frame {
        w,
        h,
        argb: vec![0; (w * h) as usize],
    }
}

/// Голый текст: плотный битмап, сглаженные глифы на прозрачном фоне,
/// premultiplied ARGB (см. модульный doc-комментарий про ограничение blit).
pub fn text_frame(text: &str, px: f32, color: u32) -> Frame {
    let glyphs = layout_glyphs(text, px);
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for g in glyphs.iter().filter(|g| g.width > 0 && g.height > 0) {
        x0 = x0.min(g.x.round() as i32);
        y0 = y0.min(g.y.round() as i32);
        x1 = x1.max(g.x.round() as i32 + g.width as i32);
        y1 = y1.max(g.y.round() as i32 + g.height as i32);
    }
    if x0 > x1 {
        return transparent(1, 1);
    }
    let mut frame = transparent((x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32);
    draw_text(&mut frame, text, px, color, -x0 as f32, -y0 as f32);
    frame
}

/// Речевой пузырь: тёмная карточка с обводкой, текст по центру и маленький
/// хвостик снизу по центру. Глифы запечены на непрозрачный фон карточки.
pub fn bubble_frame(text: &str, px: f32) -> Frame {
    let (tw, th) = measure(text, px);
    let pad_x = (px * 0.7).round().max(6.0);
    let pad_y = (px * 0.45).round().max(4.0);
    let tail_h = (px * 0.5).round().max(5.0);
    let cw = (tw + 2.0 * pad_x).ceil().max(2.0 * CARD_RADIUS + 2.0);
    let ch = (th + 2.0 * pad_y).ceil().max(2.0 * CARD_RADIUS + 2.0);
    let mut frame = transparent(cw as u32, (ch + tail_h) as u32);

    draw_card(&mut frame, 0.0, 0.0, cw, ch, CARD_RADIUS);

    // Хвостик: сужающийся книзу треугольник, стартует чуть выше нижней
    // кромки, чтобы перекрыть шов обводки.
    let tail_w = (px * 0.9).round().max(6.0);
    let cx = cw / 2.0;
    for j in 0..tail_h as i32 {
        let half = tail_w / 2.0 * (1.0 - j as f32 / tail_h);
        let y = ch as i32 - 2 + j;
        let (l, r) = ((cx - half).round() as i32, (cx + half).round() as i32);
        for x in l..=r {
            let color = if x == l || x == r {
                CARD_STROKE
            } else {
                CARD_BG
            };
            blend_px(&mut frame, x, y, color, 1.0);
        }
    }

    draw_text(
        &mut frame,
        text,
        px,
        TEXT_COLOR,
        (cw - tw) / 2.0,
        (ch - th) / 2.0,
    );
    frame
}

/// Вертикальное меню в языке дизайна настроек: карточка, скругление 10px,
/// строка под курсором подсвечена цветом `accent` (ARGB; демон передаёт
/// осветлённый цвет питомца). Возвращает кадр и хит-области строк в
/// координатах кадра (для маппинга курсора в индекс строки).
pub fn menu_frame(
    rows: &[&str],
    hovered: Option<usize>,
    px: f32,
    accent: u32,
) -> (Frame, Vec<Rect>) {
    let pad = (px * 0.4).round().max(4.0);
    let row_pad_x = (px * 0.8).round().max(8.0);
    let row_h = (px * 1.8).round();
    let max_tw = rows.iter().map(|r| measure(r, px).0).fold(0.0f32, f32::max);
    let w = (max_tw + 2.0 * row_pad_x + 2.0 * pad)
        .ceil()
        .max(2.0 * CARD_RADIUS + 2.0);
    let h = (rows.len() as f32 * row_h + 2.0 * pad)
        .ceil()
        .max(2.0 * CARD_RADIUS + 2.0);
    let mut frame = transparent(w as u32, h as u32);
    draw_card(&mut frame, 0.0, 0.0, w, h, CARD_RADIUS);

    let line_h = line_height(px);
    let mut rects = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let top = pad + i as f32 * row_h;
        let row_rect = Rect::new(pad, top, w - 2.0 * pad, row_h);
        if hovered == Some(i) {
            fill_rounded(&mut frame, row_rect, 6.0, accent, HOVER_STRENGTH);
        }
        draw_text(
            &mut frame,
            row,
            px,
            TEXT_COLOR,
            pad + row_pad_x,
            top + (row_h - line_h) / 2.0,
        );
        rects.push(row_rect);
    }
    (frame, rects)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(frame: &Frame) -> usize {
        frame.argb.iter().filter(|&&p| p >> 24 != 0).count()
    }

    #[test]
    fn cyrillic_and_latin_have_glyphs() {
        for s in ["Привет", "Feed"] {
            let f = text_frame(s, 16.0, 0xff_ff_ff_ff);
            assert!(f.w > 4 && f.h > 4, "{s}: битмап слишком мал");
            assert!(ink(&f) > 10, "{s}: глифы не растеризовались");
        }
    }

    #[test]
    fn empty_text_gives_empty_frame() {
        let f = text_frame("", 16.0, 0xff_ff_ff_ff);
        assert_eq!(ink(&f), 0);
    }

    #[test]
    fn text_frame_is_premultiplied() {
        // У premultiplied-пикселя канал не превышает альфу.
        let f = text_frame("Привет", 16.0, 0xff_e8_e8_f0);
        for &p in f.argb.iter() {
            let a = p >> 24;
            for sh in [16, 8, 0] {
                assert!((p >> sh) & 0xff <= a, "канал больше альфы: {p:08x}");
            }
        }
    }

    #[test]
    fn bubble_has_opaque_card() {
        let f = bubble_frame("Привет!", 14.0);
        assert!(ink(&f) > 0);
        // Центр карточки непрозрачен (глифы запечены на фон).
        let (cx, cy) = (f.w / 2, f.h / 3);
        assert_eq!(f.argb[(cy * f.w + cx) as usize] >> 24, 0xff);
    }

    /// Акцент для тестов меню — любой непрозрачный цвет.
    const TEST_ACCENT: u32 = 0xff_b0_a2_94;

    #[test]
    fn menu_hit_rects_match_rows() {
        let rows = ["Покормить", "Играть", "Спать"];
        let (frame, rects) = menu_frame(&rows, None, 14.0, TEST_ACCENT);
        assert_eq!(rects.len(), rows.len());
        assert!(ink(&frame) > 0);
        // Хит-области внутри кадра и не пересекаются по вертикали.
        for w in rects.windows(2) {
            assert!((w[0].bottom() - w[1].y).abs() < 0.01);
        }
        for r in &rects {
            assert!(r.right() <= frame.w as f32 && r.bottom() <= frame.h as f32);
        }
    }

    #[test]
    fn hover_changes_pixels() {
        let rows = ["Feed", "Play"];
        let (plain, _) = menu_frame(&rows, None, 14.0, TEST_ACCENT);
        let (hovered, _) = menu_frame(&rows, Some(1), 14.0, TEST_ACCENT);
        assert_eq!((plain.w, plain.h), (hovered.w, hovered.h));
        assert_ne!(plain.argb, hovered.argb);
    }

    /// Акцент виден только на подсвеченной строке и реально красит её.
    #[test]
    fn hover_accent_recolors_highlight() {
        let rows = ["Feed", "Play"];
        let (a, _) = menu_frame(&rows, Some(0), 14.0, 0xff_e8_94_4a);
        let (b, _) = menu_frame(&rows, Some(0), 14.0, 0xff_5f_bf_8f);
        assert_ne!(a.argb, b.argb, "разный акцент — разная подсветка");
        let (p1, _) = menu_frame(&rows, None, 14.0, 0xff_e8_94_4a);
        let (p2, _) = menu_frame(&rows, None, 14.0, 0xff_5f_bf_8f);
        assert_eq!(p1.argb, p2.argb, "без ховера акцент не участвует");
    }
}
