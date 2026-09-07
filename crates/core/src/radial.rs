//! Радиальное меню ПКМ (фаза G4): кольцо круглых кнопок вокруг питомца.
//!
//! Замена вертикальному текстовому списку. Принципы:
//! - **не мешает**: центр кольца пуст — питомец остаётся виден и продолжает
//!   жить, кнопки висят вокруг него на расстоянии; подпись появляется
//!   только у кнопки под курсором;
//! - **игровое**: иконки, а не строки; кольцо вырастает из центра при
//!   открытии (`grow` 0..1), наведённая кнопка чуть крупнее и в цвете
//!   питомца;
//! - **полезное**: над кольцом — три мини-шкалы (сытость/энергия/
//!   настроение), чтобы решение «покормить или уложить» принималось глядя
//!   на них, без похода в настройки.
//!
//! Модуль рисует в [`Frame`] с premultiplied-альфой поверх прозрачного
//! холста (как речевой пузырь) — композитор смешает края сам. Иконки
//! рисуются примитивами (диски, кольца, линии) через SDF, а не сетками:
//! так они масштабируются под размер питомца без «лесенки».

use crate::geometry::{Rect, Vec2};
use crate::sprite::Frame;
use crate::text::{blend_px, draw_text, measure, transparent};

/// Что нарисовано на кнопке. Порядок действий задаёт демон; иконка — только
/// картинка, семантику она не знает.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Печенье — покормить.
    Cookie,
    /// Конфета — вкусняшка.
    Candy,
    /// Мячик — поиграть.
    Ball,
    /// Месяц — уложить спать.
    Moon,
    /// Шестерёнка — настройки.
    Gear,
    /// Крестик — убрать с экрана.
    Cross,
}

/// Кнопка меню: иконка + локализованная подпись (показывается при наведении).
#[derive(Debug, Clone)]
pub struct RadialItem {
    pub icon: Icon,
    pub label: String,
}

/// Геометрия кольца в координатах кадра меню.
#[derive(Debug, Clone, PartialEq)]
pub struct RadialLayout {
    /// Сторона квадратного кадра, px.
    pub side: u32,
    /// Центр кольца (он же центр питомца) в координатах кадра.
    pub center: Vec2,
    /// Радиус кольца до центров кнопок.
    pub ring_r: f32,
    /// Радиус кнопки.
    pub petal_r: f32,
    /// Центры кнопок (полностью выросшего кольца).
    pub petals: Vec<Vec2>,
}

/// Отступ под подписи и шкалы вокруг кольца, px.
const MARGIN: f32 = 34.0;
/// Зазор между питомцем и кнопками, px.
const GAP: f32 = 10.0;

// Палитра в языке дизайна настроек (см. text.rs) + цвета иконок.
const PETAL_BG: u32 = 0xff_23_23_2e;
const PETAL_STROKE: u32 = 0xff_3a_3a_4a;
const TEXT: u32 = 0xff_e8_e8_f0;
const COOKIE: u32 = 0xff_d9_a0_66;
const COOKIE_DOT: u32 = 0xff_5a_3a_22;
const CANDY: u32 = 0xff_f0_8a_b8;
const BALL: u32 = 0xff_f4_f4_f8;
const MOON: u32 = 0xff_f2_d4_7a;
const GEAR: u32 = 0xff_b8_b8_c8;
const CROSS: u32 = 0xff_e0_6a_6a;
const BAR_TRACK: u32 = 0xff_3a_3a_4a;
const BAR_SATIETY: u32 = 0xff_d9_a0_66;
const BAR_ENERGY: u32 = 0xff_f2_d4_7a;
const BAR_MOOD: u32 = 0xff_f0_8a_b8;

/// Разложить `n` кнопок кольцом вокруг питомца размера `pet_size`.
/// Кольцо начинается сверху и идёт по часовой — первая кнопка над головой.
pub fn radial_layout(n: usize, pet_size: f32) -> RadialLayout {
    let petal_r = (pet_size * 0.26).clamp(15.0, 24.0);
    let ring_r = pet_size * 0.72 + GAP + petal_r;
    let half = ring_r + petal_r + MARGIN;
    let side = (half * 2.0).ceil() as u32;
    let center = Vec2::new(half, half);
    let petals = (0..n.max(1))
        .map(|i| {
            let a =
                -core::f32::consts::FRAC_PI_2 + i as f32 * core::f32::consts::TAU / n.max(1) as f32;
            Vec2::new(center.x + ring_r * a.cos(), center.y + ring_r * a.sin())
        })
        .collect();
    RadialLayout {
        side,
        center,
        ring_r,
        petal_r,
        petals,
    }
}

/// Плавное появление: ease-out, чтобы кольцо «выстреливало» и мягко
/// садилось на место.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

/// Положение и радиус кнопки `i` на фазе роста `grow` (0..1).
fn petal_at(layout: &RadialLayout, i: usize, grow: f32) -> (Vec2, f32) {
    let k = ease(grow);
    let p = layout.petals[i];
    let c = layout.center;
    (
        Vec2::new(c.x + (p.x - c.x) * k, c.y + (p.y - c.y) * k),
        layout.petal_r * (0.4 + 0.6 * k),
    )
}

/// Индекс кнопки под точкой `local` (координаты кадра) у выросшего кольца.
/// Захват чуть шире самой кнопки — попадать пальцем/мышью должно быть легко.
pub fn radial_hit(layout: &RadialLayout, local: Vec2) -> Option<usize> {
    let reach = layout.petal_r * 1.35;
    layout
        .petals
        .iter()
        .enumerate()
        .map(|(i, p)| (i, (p.x - local.x).hypot(p.y - local.y)))
        .filter(|(_, d)| *d <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

// ---- Примитивы -----------------------------------------------------------

fn disc(frame: &mut Frame, c: Vec2, r: f32, color: u32, alpha: f32) {
    let (x0, x1) = (
        (c.x - r - 1.0).floor() as i32,
        (c.x + r + 1.0).ceil() as i32,
    );
    let (y0, y1) = (
        (c.y - r - 1.0).floor() as i32,
        (c.y + r + 1.0).ceil() as i32,
    );
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f32 + 0.5 - c.x).hypot(y as f32 + 0.5 - c.y)) - r;
            let cov = (0.5 - d).clamp(0.0, 1.0) * alpha;
            if cov > 0.0 {
                blend_px(frame, x, y, color, cov);
            }
        }
    }
}

fn ring(frame: &mut Frame, c: Vec2, r: f32, thickness: f32, color: u32, alpha: f32) {
    let (x0, x1) = (
        (c.x - r - 1.0).floor() as i32,
        (c.x + r + 1.0).ceil() as i32,
    );
    let (y0, y1) = (
        (c.y - r - 1.0).floor() as i32,
        (c.y + r + 1.0).ceil() as i32,
    );
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d =
                ((x as f32 + 0.5 - c.x).hypot(y as f32 + 0.5 - c.y) - r).abs() - thickness / 2.0;
            let cov = (0.5 - d).clamp(0.0, 1.0) * alpha;
            if cov > 0.0 {
                blend_px(frame, x, y, color, cov);
            }
        }
    }
}

/// Отрезок толщиной `t` с круглыми концами.
fn line(frame: &mut Frame, a: Vec2, b: Vec2, t: f32, color: u32) {
    let (x0, x1) = (a.x.min(b.x) - t, a.x.max(b.x) + t);
    let (y0, y1) = (a.y.min(b.y) - t, a.y.max(b.y) + t);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    for y in y0.floor() as i32..=y1.ceil() as i32 {
        for x in x0.floor() as i32..=x1.ceil() as i32 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let h = (((px - a.x) * dx + (py - a.y) * dy) / len2).clamp(0.0, 1.0);
            let d = (px - (a.x + dx * h)).hypot(py - (a.y + dy * h)) - t / 2.0;
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_px(frame, x, y, color, cov);
            }
        }
    }
}

/// Кольцо `(rc, rr, thickness)`, обрезанное диском-маской `(mc, mr)`:
/// шов на мяче не вылезает за мяч.
fn arc_in_disc(
    frame: &mut Frame,
    rc: Vec2,
    rr: f32,
    thickness: f32,
    mc: Vec2,
    mr: f32,
    color: u32,
) {
    let (x0, x1) = (
        (mc.x - mr - 1.0).floor() as i32,
        (mc.x + mr + 1.0).ceil() as i32,
    );
    let (y0, y1) = (
        (mc.y - mr - 1.0).floor() as i32,
        (mc.y + mr + 1.0).ceil() as i32,
    );
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let d_ring = ((px - rc.x).hypot(py - rc.y) - rr).abs() - thickness / 2.0;
            let d_mask = (px - mc.x).hypot(py - mc.y) - mr;
            let d = d_ring.max(d_mask);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_px(frame, x, y, color, cov);
            }
        }
    }
}

/// Диск с вырезанным диском (месяц): рисуем покрытие как разность SDF.
fn crescent(frame: &mut Frame, c: Vec2, r: f32, bite: Vec2, bite_r: f32, color: u32) {
    let (x0, x1) = (
        (c.x - r - 1.0).floor() as i32,
        (c.x + r + 1.0).ceil() as i32,
    );
    let (y0, y1) = (
        (c.y - r - 1.0).floor() as i32,
        (c.y + r + 1.0).ceil() as i32,
    );
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let d_outer = (px - c.x).hypot(py - c.y) - r;
            let d_bite = (px - bite.x).hypot(py - bite.y) - bite_r;
            // Внутри внешнего и снаружи выкуса.
            let d = d_outer.max(-d_bite);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_px(frame, x, y, color, cov);
            }
        }
    }
}

/// Иконка размером `s` (половина стороны) в точке `c`.
fn draw_icon(frame: &mut Frame, icon: Icon, c: Vec2, s: f32, accent: u32) {
    match icon {
        Icon::Cookie => {
            disc(frame, c, s, COOKIE, 1.0);
            // Откушенный край: выкус справа сверху.
            disc(
                frame,
                Vec2::new(c.x + s * 0.85, c.y - s * 0.75),
                s * 0.45,
                PETAL_BG,
                1.0,
            );
            for (dx, dy) in [(-0.35, -0.3), (0.2, -0.05), (-0.15, 0.4), (0.4, 0.45)] {
                disc(
                    frame,
                    Vec2::new(c.x + s * dx, c.y + s * dy),
                    s * 0.17,
                    COOKIE_DOT,
                    1.0,
                );
            }
        }
        Icon::Candy => {
            // Круглая карамелька в фантике: два бантика-ушка по бокам.
            for sign in [-1.0f32, 1.0] {
                let base = Vec2::new(c.x + sign * s * 0.62, c.y);
                let tip_top = Vec2::new(c.x + sign * s * 1.15, c.y - s * 0.5);
                let tip_bot = Vec2::new(c.x + sign * s * 1.15, c.y + s * 0.5);
                line(frame, base, tip_top, s * 0.24, CANDY);
                line(frame, base, tip_bot, s * 0.24, CANDY);
            }
            disc(frame, c, s * 0.66, CANDY, 1.0);
            // Косые полоски — в цвете питомца.
            for dx in [-0.28f32, 0.22] {
                line(
                    frame,
                    Vec2::new(c.x + s * (dx - 0.18), c.y - s * 0.5),
                    Vec2::new(c.x + s * (dx + 0.18), c.y + s * 0.5),
                    s * 0.16,
                    accent,
                );
            }
        }
        Icon::Ball => {
            disc(frame, c, s, BALL, 1.0);
            // Шов теннисного мяча: две дуги — кольца, сдвинутые вбок, но
            // обрезанные по диску мяча (рисуем только внутри него).
            for sign in [-1.0f32, 1.0] {
                let seam = Vec2::new(c.x + sign * s * 1.25, c.y);
                arc_in_disc(frame, seam, s * 1.05, s * 0.22, c, s * 0.92, accent);
            }
            ring(frame, c, s, s * 0.12, PETAL_STROKE, 1.0);
        }
        Icon::Moon => {
            crescent(
                frame,
                c,
                s,
                Vec2::new(c.x + s * 0.55, c.y - s * 0.35),
                s * 0.85,
                MOON,
            );
            // Две звёздочки-точки.
            disc(
                frame,
                Vec2::new(c.x + s * 0.7, c.y + s * 0.55),
                s * 0.13,
                MOON,
                1.0,
            );
            disc(
                frame,
                Vec2::new(c.x + s * 0.95, c.y - s * 0.1),
                s * 0.1,
                MOON,
                1.0,
            );
        }
        Icon::Gear => {
            for i in 0..8 {
                let a = i as f32 * core::f32::consts::TAU / 8.0;
                let tip = Vec2::new(c.x + a.cos() * s, c.y + a.sin() * s);
                line(frame, c, tip, s * 0.42, GEAR);
            }
            disc(frame, c, s * 0.68, GEAR, 1.0);
            disc(frame, c, s * 0.28, PETAL_BG, 1.0);
        }
        Icon::Cross => {
            let k = s * 0.7;
            line(
                frame,
                Vec2::new(c.x - k, c.y - k),
                Vec2::new(c.x + k, c.y + k),
                s * 0.34,
                CROSS,
            );
            line(
                frame,
                Vec2::new(c.x - k, c.y + k),
                Vec2::new(c.x + k, c.y - k),
                s * 0.34,
                CROSS,
            );
        }
    }
}

/// Пилюля с текстом: тёмная капсула, текст по центру. Возвращает её рамку.
fn label_pill(frame: &mut Frame, text: &str, px: f32, anchor: Vec2, below: bool) -> Rect {
    let (tw, th) = measure(text, px);
    let pad_x = (px * 0.6).round();
    let pad_y = (px * 0.3).round();
    let w = tw + 2.0 * pad_x;
    let h = th + 2.0 * pad_y;
    let x = (anchor.x - w / 2.0).clamp(1.0, (frame.w as f32 - w - 1.0).max(1.0));
    let y =
        if below { anchor.y } else { anchor.y - h }.clamp(1.0, (frame.h as f32 - h - 1.0).max(1.0));
    // Капсула: два диска + линия между ними.
    let r = h / 2.0;
    line(
        frame,
        Vec2::new(x + r, y + r),
        Vec2::new((x + w - r).max(x + r), y + r),
        h,
        PETAL_BG,
    );
    draw_text(frame, text, px, TEXT, x + pad_x, y + pad_y);
    Rect::new(x, y, w, h)
}

/// Мини-шкалы над кольцом: три полоски в капсуле.
fn stats_pill(frame: &mut Frame, layout: &RadialLayout, stats: [f32; 3], accent: u32) {
    let bar_w = (layout.petal_r * 2.8).round();
    let bar_h = 4.0;
    let gap = 3.0;
    let w = bar_w + 12.0;
    let h = 3.0 * bar_h + 2.0 * gap + 10.0;
    let x = layout.center.x - w / 2.0;
    let y = (layout.center.y - layout.ring_r - layout.petal_r - h - 6.0).max(1.0);
    let r = 6.0;
    line(
        frame,
        Vec2::new(x + r, y + h / 2.0),
        Vec2::new(x + w - r, y + h / 2.0),
        h,
        PETAL_BG,
    );
    let colors = [BAR_SATIETY, BAR_ENERGY, BAR_MOOD];
    for (i, (value, color)) in stats.iter().zip(colors).enumerate() {
        let by = y + 5.0 + i as f32 * (bar_h + gap);
        let bx = x + 6.0;
        line(
            frame,
            Vec2::new(bx + bar_h / 2.0, by + bar_h / 2.0),
            Vec2::new(bx + bar_w - bar_h / 2.0, by + bar_h / 2.0),
            bar_h,
            BAR_TRACK,
        );
        let fill = bar_w * (value.clamp(0.0, 100.0) / 100.0);
        if fill >= bar_h {
            // Настроение — в цвете самого питомца (осветлённом ради контраста).
            let col = if i == 2 {
                crate::palette::lighten(accent, 1.25)
            } else {
                color
            };
            line(
                frame,
                Vec2::new(bx + bar_h / 2.0, by + bar_h / 2.0),
                Vec2::new(bx + fill - bar_h / 2.0, by + bar_h / 2.0),
                bar_h,
                col,
            );
        }
    }
}

/// Кадр меню. `grow` — фаза появления 0..1 (1 — полностью раскрыто),
/// `hovered` — кнопка под курсором, `stats` — [сытость, энергия,
/// настроение] для мини-шкал (None — не рисовать), `px` — кегль подписи,
/// `accent` — цвет питомца (подсветка наведённой кнопки и детали иконок).
pub fn radial_frame(
    layout: &RadialLayout,
    items: &[RadialItem],
    hovered: Option<usize>,
    grow: f32,
    stats: Option<[f32; 3]>,
    px: f32,
    accent: u32,
) -> Frame {
    let mut frame = transparent(layout.side, layout.side);
    let grown = grow >= 1.0;
    for (i, item) in items.iter().enumerate().take(layout.petals.len()) {
        let (c, mut r) = petal_at(layout, i, grow);
        let hot = grown && hovered == Some(i);
        if hot {
            r *= 1.15;
        }
        // Мягкая тень, кнопка, обводка (наведённая — в цвете питомца).
        disc(
            &mut frame,
            Vec2::new(c.x, c.y + 1.5),
            r + 1.0,
            0xff_00_00_00,
            0.25,
        );
        disc(
            &mut frame,
            c,
            r,
            if hot { accent } else { PETAL_BG },
            if hot { 0.55 } else { 0.94 },
        );
        if hot {
            disc(&mut frame, c, r, PETAL_BG, 0.35);
        }
        ring(
            &mut frame,
            c,
            r,
            1.2,
            if hot { accent } else { PETAL_STROKE },
            1.0,
        );
        draw_icon(&mut frame, item.icon, c, r * 0.52, accent);
    }
    if grown {
        if let Some(i) = hovered.filter(|&i| i < items.len()) {
            let (c, r) = petal_at(layout, i, 1.0);
            // Подпись снаружи кольца: у верхних кнопок — над, у нижних — под.
            let below = c.y >= layout.center.y;
            let anchor = Vec2::new(c.x, if below { c.y + r + 4.0 } else { c.y - r - 4.0 });
            label_pill(&mut frame, &items[i].label, px, anchor, below);
        }
        if let Some(s) = stats {
            stats_pill(&mut frame, layout, s, accent);
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<RadialItem> {
        [
            (Icon::Cookie, "Покормить"),
            (Icon::Candy, "Вкусняшка"),
            (Icon::Ball, "Поиграть"),
            (Icon::Moon, "Уложить спать"),
            (Icon::Gear, "Настройки"),
            (Icon::Cross, "Убрать"),
        ]
        .into_iter()
        .map(|(icon, l)| RadialItem {
            icon,
            label: l.to_string(),
        })
        .collect()
    }

    fn ink(frame: &Frame) -> usize {
        frame.argb.iter().filter(|&&p| p >> 24 != 0).count()
    }

    /// Кольцо не накрывает питомца: ни одна кнопка не заходит в круг его
    /// размера вокруг центра — центр остаётся свободным.
    #[test]
    fn ring_leaves_the_pet_uncovered() {
        let pet = 96.0;
        let l = radial_layout(6, pet);
        for p in &l.petals {
            let d = (p.x - l.center.x).hypot(p.y - l.center.y);
            assert!(d - l.petal_r >= pet * 0.7, "кнопка налезает на питомца");
        }
        assert_eq!(l.petals.len(), 6);
        // Первая кнопка — строго над головой.
        assert!((l.petals[0].x - l.center.x).abs() < 0.01);
        assert!(l.petals[0].y < l.center.y);
    }

    /// Попадание: точка у центра кнопки — она; между кнопками и в центре
    /// кольца (на питомце) — ничего.
    #[test]
    fn hit_test_picks_nearest_petal_only_nearby() {
        let l = radial_layout(6, 96.0);
        assert_eq!(radial_hit(&l, l.petals[2]), Some(2));
        assert_eq!(radial_hit(&l, l.center), None, "центр — питомец, не кнопка");
        let far = Vec2::new(l.petals[0].x, l.petals[0].y - l.petal_r * 3.0);
        assert_eq!(radial_hit(&l, far), None);
    }

    /// Кадр рисуется, растёт из центра, а подпись есть только у наведённой
    /// кнопки и только у выросшего кольца.
    #[test]
    fn frame_grows_and_labels_only_hovered() {
        let l = radial_layout(6, 96.0);
        let it = items();
        let small = radial_frame(&l, &it, None, 0.2, None, 13.0, 0xff_b0_a2_94);
        let full = radial_frame(&l, &it, None, 1.0, None, 13.0, 0xff_b0_a2_94);
        assert!(ink(&small) > 0 && ink(&full) > ink(&small), "кольцо растёт");
        let hovered = radial_frame(&l, &it, Some(3), 1.0, None, 13.0, 0xff_b0_a2_94);
        assert!(ink(&hovered) > ink(&full), "подпись добавила чернил");
        let hovered_early = radial_frame(&l, &it, Some(3), 0.5, None, 13.0, 0xff_b0_a2_94);
        let plain_early = radial_frame(&l, &it, None, 0.5, None, 13.0, 0xff_b0_a2_94);
        assert_eq!(
            ink(&hovered_early),
            ink(&plain_early),
            "до раскрытия подписи нет"
        );
        // Шкалы добавляют чернил над кольцом.
        let with_stats = radial_frame(
            &l,
            &it,
            None,
            1.0,
            Some([80.0, 20.0, 60.0]),
            13.0,
            0xff_b0_a2_94,
        );
        assert!(ink(&with_stats) > ink(&full));
    }

    /// Кадр premultiplied: канал не превышает альфу (иначе компоситор
    /// покажет грязные кромки).
    #[test]
    fn frame_is_premultiplied() {
        let l = radial_layout(6, 64.0);
        let f = radial_frame(
            &l,
            &items(),
            Some(0),
            1.0,
            Some([50.0; 3]),
            12.0,
            0xff_e8_94_4a,
        );
        for &p in &f.argb {
            let a = p >> 24;
            for sh in [16, 8, 0] {
                assert!((p >> sh) & 0xff <= a, "канал больше альфы: {p:08x}");
            }
        }
    }
}
