//! Процедурный плейсхолдер-спрайт M0: круглый «дрифтлинг» с глазами и лапками.
//! Настоящие паки (.driftpack) придут в M4; интерфейс кадров уже финальный.

use crate::behavior::PetState;
use crate::pet::Direction;

/// Кадр: ARGB8888 (premultiplied не требуется — альфа 0 или 255).
#[derive(Debug, Clone)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub argb: Vec<u32>,
}

/// Набор кадров под каждое состояние.
#[derive(Debug, Clone)]
pub struct SpriteSet {
    pub size: u32,
    pub idle: Vec<Frame>,
    pub walk: Vec<Frame>,
    pub sleep: Vec<Frame>,
    pub falling: Vec<Frame>,
    pub dragged: Vec<Frame>,
    pub landing: Vec<Frame>,
}

impl SpriteSet {
    /// Кадр для состояния в момент `t` секунд с входа в состояние.
    pub fn frame(&self, state: PetState, t: f32, facing: Direction) -> &Frame {
        let (frames, fps) = match state {
            PetState::Idle => (&self.idle, 2.0),
            PetState::Walk => (&self.walk, 6.0),
            PetState::Sleep => (&self.sleep, 1.0),
            PetState::Falling => (&self.falling, 8.0),
            PetState::Dragged => (&self.dragged, 4.0),
            PetState::Landing => (&self.landing, 6.0),
        };
        let idx = ((t * fps) as usize) % frames.len().max(1);
        let _ = facing; // зеркалирование делает рендер по флагу facing
        &frames[idx]
    }
}

const BODY: u32 = 0xff_8a_63_d2; // сиреневый корпус
const BODY_DARK: u32 = 0xff_6b_47_ad;
const EYE: u32 = 0xff_1e_1e_2e;
const EYE_SHINE: u32 = 0xff_ff_ff_ff;
const CHEEK: u32 = 0xff_e8_9a_c7;

/// Сгенерировать набор кадров размером `size` px (квадрат).
pub fn placeholder(size: u32) -> SpriteSet {
    SpriteSet {
        size,
        idle: vec![blob(size, 0.0, true, false), blob(size, 0.0, false, false)],
        walk: vec![
            blob_shift(size, 0.06, true, 0),
            blob_shift(size, 0.0, true, 1),
            blob_shift(size, 0.06, true, 2),
            blob_shift(size, 0.0, true, 3),
        ],
        sleep: vec![blob(size, 0.12, false, true), blob(size, 0.16, false, true)],
        falling: vec![blob(size, -0.1, true, false)],
        dragged: vec![blob(size, -0.05, true, false)],
        landing: vec![blob(size, 0.22, true, false)],
    }
}

fn blob(size: u32, squash: f32, eyes_open: bool, zzz: bool) -> Frame {
    blob_impl(size, squash, eyes_open, zzz, None)
}

fn blob_shift(size: u32, squash: f32, eyes_open: bool, step: u8) -> Frame {
    blob_impl(size, squash, eyes_open, false, Some(step))
}

/// Рисуем эллипс-тело с прижатием `squash`, глаза, щёки и лапки.
fn blob_impl(size: u32, squash: f32, eyes_open: bool, zzz: bool, walk_step: Option<u8>) -> Frame {
    let s = size as f32;
    let mut argb = vec![0u32; (size * size) as usize];

    let cx = s / 2.0;
    let rx = s * 0.38;
    let ry = s * 0.34 * (1.0 - squash);
    // Тело стоит на нижней кромке кадра (минус место под лапки).
    let foot_h = s * 0.06;
    let cy = s - foot_h - ry;

    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - cx) / rx;
            let dy = (y as f32 - cy) / ry;
            let d = dx * dx + dy * dy;
            if d <= 1.0 {
                let i = (y * size + x) as usize;
                argb[i] = if d > 0.82 { BODY_DARK } else { BODY };
            }
        }
    }

    // Лапки: две полукруглые ножки; при ходьбе шагают в противофазе.
    let (lift_l, lift_r) = match walk_step {
        Some(0) => (foot_h * 0.9, 0.0),
        Some(2) => (0.0, foot_h * 0.9),
        _ => (0.0, 0.0),
    };
    for (fx, lift) in [(cx - rx * 0.45, lift_l), (cx + rx * 0.45, lift_r)] {
        fill_circle(&mut argb, size, fx, s - foot_h / 2.0 - lift, foot_h * 0.9, BODY_DARK);
    }

    // Глаза.
    let ey = cy - ry * 0.15;
    for ex in [cx - rx * 0.38, cx + rx * 0.38] {
        if eyes_open {
            fill_circle(&mut argb, size, ex, ey, s * 0.045, EYE);
            fill_circle(&mut argb, size, ex + s * 0.012, ey - s * 0.012, s * 0.015, EYE_SHINE);
        } else {
            // Закрытый глаз — короткая дуга.
            for dx in -3i32..=3 {
                put(&mut argb, size, (ex + dx as f32) as i32, ey as i32, EYE);
            }
        }
    }

    // Щёки.
    for ex in [cx - rx * 0.6, cx + rx * 0.6] {
        fill_circle(&mut argb, size, ex, ey + ry * 0.35, s * 0.03, CHEEK);
    }

    // «Z z» над головой у спящего.
    if zzz {
        let zx = (cx + rx * 0.7) as i32;
        let zy = (cy - ry - s * 0.08) as i32;
        draw_z(&mut argb, size, zx, zy, 5, EYE);
        draw_z(&mut argb, size, zx + 7, zy - 8, 3, EYE);
    }

    Frame { w: size, h: size, argb }
}

fn put(argb: &mut [u32], size: u32, x: i32, y: i32, c: u32) {
    if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
        argb[(y as u32 * size + x as u32) as usize] = c;
    }
}

fn fill_circle(argb: &mut [u32], size: u32, cx: f32, cy: f32, r: f32, c: u32) {
    let (x0, x1) = ((cx - r) as i32, (cx + r) as i32);
    let (y0, y1) = ((cy - r) as i32, (cy + r) as i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy <= r * r {
                put(argb, size, x, y, c);
            }
        }
    }
}

/// Буква Z из трёх штрихов размера `n`.
fn draw_z(argb: &mut [u32], size: u32, x: i32, y: i32, n: i32, c: u32) {
    for i in 0..n {
        put(argb, size, x + i, y, c);
        put(argb, size, x + i, y + n - 1, c);
        put(argb, size, x + (n - 1 - i), y + i, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_have_pixels() {
        let set = placeholder(96);
        for frames in [&set.idle, &set.walk, &set.sleep, &set.falling, &set.dragged, &set.landing] {
            assert!(!frames.is_empty());
            for f in frames.iter() {
                assert_eq!((f.w, f.h), (96, 96));
                assert!(f.argb.iter().any(|&p| p >> 24 != 0), "кадр не должен быть пустым");
            }
        }
    }

    #[test]
    fn frame_selection_cycles() {
        let set = placeholder(64);
        let a = set.frame(PetState::Walk, 0.0, Direction::Right) as *const _;
        let b = set.frame(PetState::Walk, 0.5, Direction::Right) as *const _;
        assert_ne!(a, b);
    }
}
