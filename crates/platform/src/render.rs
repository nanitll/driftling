//! CPU-рендер сцены, общий для бэкендов: границы, dirty-check, блит.
//! Пиксельный формат — ARGB как u32, в байтах — little-endian (B, G, R, A):
//! это и wl_shm ARGB8888, и 32-битный ZPixmap на LSB-first X-серверах.

use driftling_core::sprite::Frame;
use driftling_core::Rect;

use crate::Scene;

/// Объединённый прямоугольник всех непустых спрайтов сцены в логических
/// пикселях: (x, y, w, h). None — рисовать нечего.
pub(crate) fn scene_bounds(scene: &Scene<'_>) -> Option<(i32, i32, u32, u32)> {
    let mut acc: Option<(i32, i32, i32, i32)> = None;
    for s in &scene.sprites {
        if s.frame.w == 0 || s.frame.h == 0 {
            continue;
        }
        let x0 = s.origin.x.round() as i32;
        let y0 = s.origin.y.round() as i32;
        let x1 = x0 + s.frame.w as i32;
        let y1 = y0 + s.frame.h as i32;
        acc = Some(match acc {
            None => (x0, y0, x1, y1),
            Some((ax0, ay0, ax1, ay1)) => (ax0.min(x0), ay0.min(y0), ax1.max(x1), ay1.max(y1)),
        });
    }
    acc.map(|(x0, y0, x1, y1)| (x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Ключ содержимого буфера для dirty-check. Позиция сцены на экране в ключ
/// НЕ входит: чистое перемещение не требует перерисовки, только сдвиг окна.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct ContentKey {
    scale: u32,
    sprites: Vec<SpriteKey>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct SpriteKey {
    /// Идентичность пикселей — адрес массива кадра. Кадры живут в SpriteSet
    /// приложения; новый кадр = другой Vec = другой адрес. Теоретическая
    /// коллизия (новая аллокация по старому адресу при том же размере кадра)
    /// дала бы один устаревший кадр; в текущем коде набор пересоздаётся только
    /// со сменой размера — там меняются и w/h.
    argb: usize,
    w: u32,
    h: u32,
    /// Положение спрайта внутри буфера (относительно объединённых границ).
    rel: (i32, i32),
    mirror: bool,
}

pub(crate) fn content_key(scene: &Scene<'_>, origin: (i32, i32), scale: u32) -> ContentKey {
    ContentKey {
        scale,
        sprites: scene
            .sprites
            .iter()
            .filter(|s| s.frame.w > 0 && s.frame.h > 0)
            .map(|s| SpriteKey {
                argb: s.frame.argb.as_ptr() as usize,
                w: s.frame.w,
                h: s.frame.h,
                rel: (
                    s.origin.x.round() as i32 - origin.0,
                    s.origin.y.round() as i32 - origin.1,
                ),
                mirror: s.mirror,
            })
            .collect(),
    }
}

/// Input-прямоугольники сцены (логические экранные) → локальные координаты
/// поверхности питомца, с округлением наружу. Пустые/вырожденные выбрасываются.
pub(crate) fn local_input_rects(rects: &[Rect], origin: (i32, i32)) -> Vec<(i32, i32, i32, i32)> {
    rects
        .iter()
        .filter(|r| r.w > 0.0 && r.h > 0.0)
        .map(|r| {
            let x0 = r.x.floor() as i32 - origin.0;
            let y0 = r.y.floor() as i32 - origin.1;
            let x1 = (r.x + r.w).ceil() as i32 - origin.0;
            let y1 = (r.y + r.h).ceil() as i32 - origin.1;
            (x0, y0, x1 - x0, y1 - y0)
        })
        .collect()
}

/// Собрать буфер питомца: прозрачный фон + спрайты относительно `origin`
/// (левый верх объединённых границ). `size` — физический размер (уже * scale).
pub(crate) fn compose(
    canvas: &mut [u8],
    size: (u32, u32),
    scene: &Scene<'_>,
    origin: (i32, i32),
    scale: u32,
) {
    canvas.fill(0); // 0x00000000 — полностью прозрачно
    for s in &scene.sprites {
        let rel = (
            s.origin.x.round() as i32 - origin.0,
            s.origin.y.round() as i32 - origin.1,
        );
        blit(canvas, size, s.frame, rel, s.mirror, scale);
    }
}

/// Блит спрайта на холст: nearest-neighbour масштаб, опциональное
/// горизонтальное зеркало, пропуск пикселей с альфой 0, клип по краям.
/// `origin` — логические координаты левого верхнего угла спрайта на холсте.
fn blit(
    canvas: &mut [u8],
    (cw, ch): (u32, u32),
    frame: &Frame,
    (ox, oy): (i32, i32),
    mirror: bool,
    scale: u32,
) {
    if frame.w == 0 || frame.h == 0 {
        return;
    }
    let base_x = ox * scale as i32;
    let base_y = oy * scale as i32;
    for dy in 0..frame.h * scale {
        let cy = base_y + dy as i32;
        if cy < 0 || cy >= ch as i32 {
            continue;
        }
        let sy = dy / scale;
        for dx in 0..frame.w * scale {
            let cx = base_x + dx as i32;
            if cx < 0 || cx >= cw as i32 {
                continue;
            }
            let sx = if mirror {
                frame.w - 1 - dx / scale
            } else {
                dx / scale
            };
            let px = frame.argb[(sy * frame.w + sx) as usize];
            if px >> 24 == 0 {
                continue; // прозрачный пиксель спрайта
            }
            let off = ((cy as u32 * cw + cx as u32) * 4) as usize;
            // ARGB8888 little-endian: байты B, G, R, A.
            canvas[off..off + 4].copy_from_slice(&px.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpriteInstance;
    use driftling_core::Vec2;

    /// Холст w*h, читаем пиксель обратно как ARGB u32.
    fn pixel(canvas: &[u8], w: u32, x: u32, y: u32) -> u32 {
        let off = ((y * w + x) * 4) as usize;
        u32::from_le_bytes(canvas[off..off + 4].try_into().unwrap())
    }

    fn frame_2x2() -> Frame {
        // A B
        // C .   (правый нижний прозрачный)
        Frame {
            w: 2,
            h: 2,
            argb: vec![0xff_11_00_00, 0xff_00_22_00, 0xff_00_00_33, 0x00_00_00_00],
        }
    }

    fn scene_one(frame: &Frame, origin: Vec2, mirror: bool) -> Scene<'_> {
        Scene {
            sprites: vec![SpriteInstance {
                frame,
                origin,
                mirror,
            }],
            input_rects: vec![Rect::new(
                origin.x,
                origin.y,
                frame.w as f32,
                frame.h as f32,
            )],
        }
    }

    // --- blit -------------------------------------------------------------

    #[test]
    fn blit_scale1_pixel_perfect() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 4 * 4 * 4];
        blit(&mut canvas, (4, 4), &f, (1, 1), false, 1);
        assert_eq!(pixel(&canvas, 4, 1, 1), 0xff_11_00_00);
        assert_eq!(pixel(&canvas, 4, 2, 1), 0xff_00_22_00);
        assert_eq!(pixel(&canvas, 4, 1, 2), 0xff_00_00_33);
        // Прозрачный пиксель спрайта не затирает фон.
        assert_eq!(pixel(&canvas, 4, 2, 2), 0);
        // Вокруг — прозрачно.
        assert_eq!(pixel(&canvas, 4, 0, 0), 0);
        assert_eq!(pixel(&canvas, 4, 3, 3), 0);
    }

    #[test]
    fn blit_mirror_swaps_columns() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 2 * 2 * 4];
        blit(&mut canvas, (2, 2), &f, (0, 0), true, 1);
        assert_eq!(pixel(&canvas, 2, 0, 0), 0xff_00_22_00); // B слева
        assert_eq!(pixel(&canvas, 2, 1, 0), 0xff_11_00_00); // A справа
        assert_eq!(pixel(&canvas, 2, 0, 1), 0); // прозрачный (зеркало C)
        assert_eq!(pixel(&canvas, 2, 1, 1), 0xff_00_00_33);
    }

    #[test]
    fn blit_scale2_nearest_neighbour() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 4 * 4 * 4];
        blit(&mut canvas, (4, 4), &f, (0, 0), false, 2);
        // Каждый исходный пиксель — блок 2x2.
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(pixel(&canvas, 4, x, y), 0xff_11_00_00);
        }
        for (x, y) in [(2, 0), (3, 1)] {
            assert_eq!(pixel(&canvas, 4, x, y), 0xff_00_22_00);
        }
        for (x, y) in [(2, 2), (3, 3)] {
            assert_eq!(pixel(&canvas, 4, x, y), 0); // прозрачный блок
        }
    }

    #[test]
    fn blit_clips_out_of_bounds() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 2 * 2 * 4];
        // Наполовину за левым верхним углом: не паникует, видимая часть верна.
        blit(&mut canvas, (2, 2), &f, (-1, -1), false, 1);
        assert_eq!(pixel(&canvas, 2, 0, 0), 0); // прозрачный угол спрайта
                                                // За правым нижним краем — тоже без паники.
        blit(&mut canvas, (2, 2), &f, (1, 1), false, 1);
        assert_eq!(pixel(&canvas, 2, 1, 1), 0xff_11_00_00);
    }

    // --- compose: спрайт рисуется относительно границ буфера ---------------

    #[test]
    fn compose_draws_relative_to_bounds() {
        let f = frame_2x2();
        let scene = scene_one(&f, Vec2::new(100.0, 200.0), false);
        let (bx, by, bw, bh) = scene_bounds(&scene).unwrap();
        assert_eq!((bx, by, bw, bh), (100, 200, 2, 2));
        let mut canvas = vec![0u8; (bw * bh * 4) as usize];
        compose(&mut canvas, (bw, bh), &scene, (bx, by), 1);
        // Спрайт лёг в (0,0) буфера, а не в экранные (100,200).
        assert_eq!(pixel(&canvas, bw, 0, 0), 0xff_11_00_00);
        assert_eq!(pixel(&canvas, bw, 1, 1), 0);
    }

    // --- границы сцены ------------------------------------------------------

    #[test]
    fn scene_bounds_rounds_origin() {
        let f = frame_2x2();
        let scene = scene_one(&f, Vec2::new(10.6, -3.4), false);
        assert_eq!(scene_bounds(&scene), Some((11, -3, 2, 2)));
    }

    #[test]
    fn scene_bounds_unions_sprites() {
        let f = frame_2x2();
        let scene = Scene {
            sprites: vec![
                SpriteInstance {
                    frame: &f,
                    origin: Vec2::new(0.0, 0.0),
                    mirror: false,
                },
                SpriteInstance {
                    frame: &f,
                    origin: Vec2::new(10.0, 4.0),
                    mirror: false,
                },
            ],
            input_rects: vec![],
        };
        assert_eq!(scene_bounds(&scene), Some((0, 0, 12, 6)));
    }

    #[test]
    fn scene_bounds_skips_empty_frames_and_empty_scene() {
        let empty = Frame {
            w: 0,
            h: 0,
            argb: vec![],
        };
        let scene = scene_one(&empty, Vec2::new(5.0, 5.0), false);
        assert_eq!(scene_bounds(&scene), None);
        let none = Scene {
            sprites: vec![],
            input_rects: vec![],
        };
        assert_eq!(scene_bounds(&none), None);
    }

    // --- dirty-check содержимого --------------------------------------------

    #[test]
    fn content_key_ignores_pure_movement() {
        let f = frame_2x2();
        let a = scene_one(&f, Vec2::new(10.0, 20.0), false);
        let b = scene_one(&f, Vec2::new(300.0, 400.0), false);
        let ka = content_key(&a, (10, 20), 1);
        let kb = content_key(&b, (300, 400), 1);
        // Кадр тот же, позиция другая → перерисовка не нужна.
        assert_eq!(ka, kb);
    }

    #[test]
    fn content_key_detects_frame_change() {
        let f1 = frame_2x2();
        let f2 = frame_2x2(); // другой Vec → другой адрес пикселей
        let a = scene_one(&f1, Vec2::new(0.0, 0.0), false);
        let b = scene_one(&f2, Vec2::new(0.0, 0.0), false);
        assert_ne!(content_key(&a, (0, 0), 1), content_key(&b, (0, 0), 1));
    }

    #[test]
    fn content_key_detects_mirror_and_scale() {
        let f = frame_2x2();
        let plain = scene_one(&f, Vec2::new(0.0, 0.0), false);
        let mirrored = scene_one(&f, Vec2::new(0.0, 0.0), true);
        assert_ne!(
            content_key(&plain, (0, 0), 1),
            content_key(&mirrored, (0, 0), 1)
        );
        assert_ne!(
            content_key(&plain, (0, 0), 1),
            content_key(&plain, (0, 0), 2)
        );
    }

    // --- input region в локальных координатах --------------------------------

    #[test]
    fn local_input_rects_translate_and_round_outward() {
        let rects = [Rect::new(10.2, 20.7, 3.5, 1.1)];
        // origin границ буфера = (10, 20)
        let local = local_input_rects(&rects, (10, 20));
        // floor(10.2)-10=0, floor(20.7)-20=0, ceil(13.7)-10-0=4, ceil(21.8)-20-0=2
        assert_eq!(local, vec![(0, 0, 4, 2)]);
    }

    #[test]
    fn local_input_rects_skip_degenerate() {
        let rects = [Rect::new(0.0, 0.0, 0.0, 5.0), Rect::new(1.0, 1.0, 2.0, 2.0)];
        assert_eq!(local_input_rects(&rects, (0, 0)), vec![(1, 1, 2, 2)]);
    }
}
