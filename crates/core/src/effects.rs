//! Мелочи, которые делают картинку живой (фаза G6): тень под питомцем,
//! пыль от приземления, лужица после укачивания.
//!
//! Всё рисуется примитивами в [`Frame`] с premultiplied-альфой: кадры
//! маленькие, пекутся один раз на набор спрайтов, а размер и прозрачность
//! на экране подбирает рендер (`Deform` + `alpha`). Поэтому тень может
//! плавно сжиматься по мере подъёма питомца, а пыль — раздуваться и таять,
//! не требуя ни одного лишнего кадра.

use crate::geometry::Vec2;
use crate::sprite::Frame;

/// Мягкий эллипс с растушёванным краем — заготовка тени.
/// `w` — ширина в пикселях; высота втрое меньше.
pub fn shadow_frame(w: u32) -> Frame {
    let w = w.max(6);
    let h = (w / 3).max(3);
    let mut frame = blank(w, h);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let (rx, ry) = (w as f32 / 2.0, h as f32 / 2.0);
    for y in 0..h {
        for x in 0..w {
            let nx = (x as f32 + 0.5 - cx) / rx;
            let ny = (y as f32 + 0.5 - cy) / ry;
            let d = (nx * nx + ny * ny).sqrt();
            if d >= 1.0 {
                continue;
            }
            // К краю тень мягко тает — резкая кромка выдаёт «наклейку».
            let cov = ((1.0 - d) * 1.6).min(1.0);
            put(&mut frame, x, y, 0x00_00_00_00, cov * 0.55);
        }
    }
    frame
}

/// Облачко пыли: три перекрывающихся кружка, светлые и почти прозрачные.
pub fn puff_frame(size: u32, color: u32) -> Frame {
    let s = size.max(4);
    let mut frame = blank(s, s);
    let f = s as f32;
    for (cx, cy, r, a) in [
        (f * 0.5, f * 0.55, f * 0.42, 0.55),
        (f * 0.3, f * 0.62, f * 0.3, 0.45),
        (f * 0.72, f * 0.6, f * 0.28, 0.45),
    ] {
        disc(&mut frame, Vec2::new(cx, cy), r, color, a);
    }
    frame
}

/// Лужица: плоская клякса с бликом (после укачивания).
pub fn puddle_frame(w: u32, color: u32) -> Frame {
    let w = w.max(8);
    let h = (w / 3).max(4);
    let mut frame = blank(w, h);
    let (fw, fh) = (w as f32, h as f32);
    for (cx, cy, r) in [
        (fw * 0.5, fh * 0.55, fh * 0.45),
        (fw * 0.3, fh * 0.6, fh * 0.38),
        (fw * 0.72, fh * 0.58, fh * 0.4),
    ] {
        disc(&mut frame, Vec2::new(cx, cy), r, color, 0.9);
    }
    disc(
        &mut frame,
        Vec2::new(fw * 0.42, fh * 0.42),
        fh * 0.12,
        0xff_ff_ff_ff,
        0.35,
    );
    frame
}

/// Швабра: палка с мочалкой на конце. Питомец достаёт её, чтобы убрать
/// за собой лужу (фаза H0 — первый настоящий предмет в мире питомца).
/// `h` — высота в пикселях (примерно рост питомца).
pub fn mop_frame(h: u32, handle: u32, head: u32) -> Frame {
    let h = h.max(10);
    let w = (h / 3).max(5);
    let mut frame = blank(w, h);
    let fw = w as f32;
    let fh = h as f32;
    // Черенок: тонкая палка сверху вниз, чуть тоньше к верху.
    let stick_x = fw * 0.5;
    for y in 0..(fh * 0.72) as u32 {
        let t = y as f32 / fh;
        let half = (fw * (0.09 + 0.03 * t)).max(0.6);
        for x in ((stick_x - half) as u32)..=((stick_x + half) as u32).min(w - 1) {
            put(&mut frame, x, y, handle, 1.0);
        }
    }
    // Мочалка: трапеция из вертикальных прядей.
    let top = fh * 0.68;
    for y in top as u32..h {
        let t = (y as f32 - top) / (fh - top);
        let half = fw * (0.22 + 0.28 * t);
        let (x0, x1) = ((stick_x - half) as i32, (stick_x + half) as i32);
        for x in x0.max(0)..=x1.min(w as i32 - 1) {
            // Пряди: через одну чуть темнее — видно, что это мочалка.
            let shade = if (x as u32 + y).is_multiple_of(3) {
                0.75
            } else {
                1.0
            };
            put(&mut frame, x as u32, y, head, shade);
        }
    }
    frame
}

/// Миска: половина эллипса с ободком и едой внутри, если `filled`.
pub fn bowl_frame(w: u32, body: u32, food: u32, filled: bool) -> Frame {
    let w = w.max(10);
    let h = (w * 2 / 3).max(6);
    let mut frame = blank(w, h);
    let (fw, fh) = (w as f32, h as f32);
    let rim = crate::palette::darken(body, 0.7);
    // Чаша: нижняя половина эллипса.
    for y in 0..h {
        for x in 0..w {
            let nx = (x as f32 + 0.5 - fw / 2.0) / (fw / 2.0);
            let ny = (y as f32 + 0.5 - fh * 0.35) / (fh * 0.65);
            if ny < 0.0 {
                continue;
            }
            let d = (nx * nx + ny * ny).sqrt();
            if d < 1.0 {
                let color = if d > 0.82 { rim } else { body };
                put(&mut frame, x, y, color, 1.0);
            }
        }
    }
    // Ободок и еда горкой.
    for x in 0..w {
        put(&mut frame, x, (fh * 0.33) as u32, rim, 0.9);
    }
    if filled {
        for y in (fh * 0.16) as u32..(fh * 0.4) as u32 {
            let t = (y as f32 - fh * 0.16) / (fh * 0.24);
            let half = fw * 0.36 * t.max(0.15);
            for x in ((fw / 2.0 - half) as u32)..=((fw / 2.0 + half) as u32).min(w - 1) {
                put(&mut frame, x, y, food, 1.0);
            }
        }
    }
    frame
}

/// Лежанка: мягкий валик с углублением — на неё можно забраться.
pub fn bed_frame(w: u32, body: u32) -> Frame {
    let w = w.max(12);
    let h = (w * 2 / 5).max(8);
    let mut frame = blank(w, h);
    let (fw, fh) = (w as f32, h as f32);
    let edge = crate::palette::darken(body, 0.72);
    let inner = crate::palette::lighten(body, 1.18);
    for y in 0..h {
        for x in 0..w {
            let nx = (x as f32 + 0.5 - fw / 2.0) / (fw / 2.0);
            let ny = (y as f32 + 0.5 - fh * 0.45) / (fh * 0.55);
            if ny < -0.2 {
                continue;
            }
            if nx * nx + ny * ny < 1.0 {
                // Углубление в середине — там питомец и лежит.
                let dip = nx.abs() < 0.62 && (y as f32) < fh * 0.55;
                put(&mut frame, x, y, if dip { inner } else { edge }, 1.0);
            }
        }
    }
    frame
}

/// Мячик: шар с бликом и полосой.
pub fn ball_frame(d: u32, body: u32, stripe: u32) -> Frame {
    let d = d.max(6);
    let mut frame = blank(d, d);
    let r = d as f32 / 2.0;
    disc(&mut frame, Vec2::new(r, r), r - 0.5, body, 1.0);
    // Полоса поперёк и блик.
    for y in 0..d {
        let ny = (y as f32 + 0.5 - r) / r;
        if ny.abs() < 0.18 {
            for x in 0..d {
                let nx = (x as f32 + 0.5 - r) / r;
                if nx * nx + ny * ny < 0.92 {
                    put(&mut frame, x, y, stripe, 1.0);
                }
            }
        }
    }
    disc(
        &mut frame,
        Vec2::new(r * 0.68, r * 0.62),
        r * 0.22,
        0xff_ff_ff_ff,
        0.5,
    );
    frame
}

fn blank(w: u32, h: u32) -> Frame {
    Frame {
        w,
        h,
        argb: vec![0; (w * h) as usize],
    }
}

fn disc(frame: &mut Frame, c: Vec2, r: f32, color: u32, alpha: f32) {
    let (x0, x1) = ((c.x - r - 1.0).max(0.0) as u32, (c.x + r + 1.0) as u32);
    let (y0, y1) = ((c.y - r - 1.0).max(0.0) as u32, (c.y + r + 1.0) as u32);
    for y in y0..=y1.min(frame.h.saturating_sub(1)) {
        for x in x0..=x1.min(frame.w.saturating_sub(1)) {
            let d = (x as f32 + 0.5 - c.x).hypot(y as f32 + 0.5 - c.y) - r;
            let cov = (0.5 - d).clamp(0.0, 1.0) * alpha;
            if cov > 0.0 {
                put(frame, x, y, color, cov);
            }
        }
    }
}

/// Положить пиксель поверх накопленного (src-over, premultiplied).
/// Альфа канала `color` игнорируется — плотность задаёт `cov`: так тень
/// (чёрный с нулевой альфой) и цветные облачка рисуются одним кодом.
fn put(frame: &mut Frame, x: u32, y: u32, color: u32, cov: f32) {
    let sa = cov.clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let i = (y * frame.w + x) as usize;
    let dst = frame.argb[i];
    let inv = 1.0 - sa;
    let ch = |sh: u32| -> u32 {
        let sc = ((color >> sh) & 0xff) as f32 * sa;
        let dc = ((dst >> sh) & 0xff) as f32;
        ((sc + dc * inv).round() as u32).min(255)
    };
    let da = ((dst >> 24) & 0xff) as f32;
    let oa = ((sa * 255.0 + da * inv).round() as u32).min(255);
    frame.argb[i] = (oa << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(frame: &Frame) -> usize {
        frame.argb.iter().filter(|&&p| p >> 24 != 0).count()
    }

    /// Тень — плоский эллипс: шире, чем выше, полупрозрачный в центре и
    /// растушёванный к краю.
    #[test]
    fn shadow_is_a_soft_flat_ellipse() {
        let f = shadow_frame(60);
        assert!(f.w > f.h * 2, "тень должна быть плоской");
        assert!(ink(&f) > 100);
        let center = f.argb[((f.h / 2) * f.w + f.w / 2) as usize];
        let edge = f.argb[((f.h / 2) * f.w + 1) as usize];
        assert!(center >> 24 > edge >> 24, "край мягче центра");
        assert!(center >> 24 < 0xf0, "тень не глухая");
        // Углы пусты — это эллипс, а не прямоугольник.
        assert_eq!(f.argb[0] >> 24, 0);
    }

    /// Швабра: палка сверху, мочалка снизу, всё внутри кадра.
    #[test]
    fn mop_has_a_handle_and_a_head() {
        let f = mop_frame(60, 0xff_8a_6a_44, 0xff_d8_d8_e0);
        assert!(f.h > f.w * 2, "швабра вытянута вверх");
        let row_ink = |y: u32| {
            (0..f.w)
                .filter(|&x| f.argb[(y * f.w + x) as usize] >> 24 != 0)
                .count()
        };
        assert!(row_ink(2) >= 1, "черенок наверху");
        assert!(row_ink(f.h - 2) > row_ink(2) * 2, "мочалка внизу шире");
    }

    /// Предметы рисуются, не пустые и в своих пропорциях.
    #[test]
    fn props_have_shape() {
        let bowl = bowl_frame(40, 0xff_b0_a2_94, 0xff_d9_a0_66, true);
        let empty = bowl_frame(40, 0xff_b0_a2_94, 0xff_d9_a0_66, false);
        assert!(ink(&bowl) > ink(&empty), "в полной миске видно еду");
        assert!(bowl.w > bowl.h, "миска шире, чем выше");
        let bed = bed_frame(60, 0xff_b0_a2_94);
        assert!(ink(&bed) > 100 && bed.w > bed.h);
        let ball = ball_frame(30, 0xff_e8_94_4a, 0xff_f4_f4_f8);
        assert!(ink(&ball) > 300, "мяч круглый и плотный");
        assert_eq!(ball.w, ball.h);
    }

    /// Пыль и лужица рисуются и остаются premultiplied.
    #[test]
    fn puff_and_puddle_are_premultiplied() {
        for f in [
            puff_frame(24, 0xff_e8_e8_f0),
            puddle_frame(40, 0xff_7d_c4_7d),
        ] {
            assert!(ink(&f) > 20);
            for &p in &f.argb {
                let a = p >> 24;
                for sh in [16, 8, 0] {
                    assert!((p >> sh) & 0xff <= a, "канал больше альфы: {p:08x}");
                }
            }
        }
    }
}
