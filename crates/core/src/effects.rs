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
