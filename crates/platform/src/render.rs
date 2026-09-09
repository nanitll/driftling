//! CPU-рендер сцены, общий для бэкендов: границы, dirty-check, блит.
//! Пиксельный формат — ARGB как u32, в байтах — little-endian (B, G, R, A):
//! это и wl_shm ARGB8888, и 32-битный ZPixmap на LSB-first X-серверах.

use driftling_core::sprite::Frame;
use driftling_core::{Deform, Orient, Rect};

use crate::Scene;

/// Объединённый прямоугольник всех непустых спрайтов сцены в логических
/// пикселях: (x, y, w, h). None — рисовать нечего.
pub(crate) fn scene_bounds(scene: &Scene<'_>) -> Option<(i32, i32, u32, u32)> {
    let mut acc: Option<(i32, i32, i32, i32)> = None;
    for s in &scene.sprites {
        if s.invisible() {
            continue;
        }
        let r = s.screen_rect();
        let x0 = r.x.round() as i32;
        let y0 = r.y.round() as i32;
        let x1 = x0 + r.w as i32;
        let y1 = y0 + r.h as i32;
        acc = Some(match acc {
            None => (x0, y0, x1, y1),
            Some((ax0, ay0, ax1, ay1)) => (ax0.min(x0), ay0.min(y0), ax1.max(x1), ay1.max(y1)),
        });
    }
    acc.map(|(x0, y0, x1, y1)| (x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Кластер сцены: группа спрайтов, которая рисуется в один буфер.
///
/// Зачем: один буфер на всю сцену означал бы буфер размером с экран, как
/// только на нём появляются вещи по разным углам (питомец слева, домик
/// справа). Это и память (8 МБ на кадр вместо десятков килобайт), и CPU —
/// каждый перерисованный кадр чистит и заполняет весь экран. Разбиение по
/// сгусткам возвращает буферы к размеру самих объектов.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cluster {
    /// Границы в логических экранных координатах.
    pub bounds: (i32, i32, u32, u32),
    /// Индексы спрайтов сцены, попавших в кластер (порядок сохранён —
    /// это порядок блита).
    pub sprites: Vec<usize>,
}

/// Разбить сцену на кластеры: спрайты, чьи прямоугольники пересекаются
/// (с запасом `gap`), рисуются вместе. Кластеров не больше `max`: лишние
/// сливаются с ближайшим — лучше один буфер побольше, чем десяток
/// поверхностей, которые композитор будет складывать по одной.
pub(crate) fn cluster_scene(scene: &Scene<'_>, gap: i32, max: usize) -> Vec<Cluster> {
    let mut out: Vec<Cluster> = Vec::new();
    for (i, s) in scene.sprites.iter().enumerate() {
        if s.invisible() {
            continue;
        }
        let r = s.screen_rect();
        let b = (
            r.x.round() as i32,
            r.y.round() as i32,
            r.w.max(1.0) as u32,
            r.h.max(1.0) as u32,
        );
        // Ищем кластер, с которым спрайт соприкасается, и вливаем в него;
        // после слияния кластер мог дотянуться до других — сливаем и их.
        let mut hit: Option<usize> = None;
        let mut j = 0;
        while j < out.len() {
            if touches(out[j].bounds, b, gap) {
                match hit {
                    None => {
                        out[j].bounds = union(out[j].bounds, b);
                        out[j].sprites.push(i);
                        hit = Some(j);
                    }
                    Some(h) => {
                        let merged = out.remove(j);
                        out[h].bounds = union(out[h].bounds, merged.bounds);
                        out[h].sprites.extend(merged.sprites);
                        out[h].sprites.sort_unstable();
                        continue;
                    }
                }
            }
            j += 1;
        }
        if hit.is_none() {
            out.push(Cluster {
                bounds: b,
                sprites: vec![i],
            });
        }
    }
    // Слишком много сгустков — сливаем самые близкие, пока не уместимся.
    while out.len() > max.max(1) {
        let mut best = (0usize, 1usize, i64::MAX);
        for a in 0..out.len() {
            for b in (a + 1)..out.len() {
                let cost = union_area(out[a].bounds, out[b].bounds)
                    - area(out[a].bounds)
                    - area(out[b].bounds);
                if cost < best.2 {
                    best = (a, b, cost);
                }
            }
        }
        let merged = out.remove(best.1);
        out[best.0].bounds = union(out[best.0].bounds, merged.bounds);
        out[best.0].sprites.extend(merged.sprites);
        out[best.0].sprites.sort_unstable();
    }
    out
}

fn union(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
    let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
    let x1 = (a.0 + a.2 as i32).max(b.0 + b.2 as i32);
    let y1 = (a.1 + a.3 as i32).max(b.1 + b.3 as i32);
    (x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
}

fn area(a: (i32, i32, u32, u32)) -> i64 {
    a.2 as i64 * a.3 as i64
}

fn union_area(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> i64 {
    area(union(a, b))
}

fn touches(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32), gap: i32) -> bool {
    let ax1 = a.0 + a.2 as i32 + gap;
    let ay1 = a.1 + a.3 as i32 + gap;
    let bx1 = b.0 + b.2 as i32;
    let by1 = b.1 + b.3 as i32;
    a.0 - gap < bx1 && b.0 < ax1 && a.1 - gap < by1 && b.1 < ay1
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
    orient: Orient,
    /// Деформация и прозрачность огрубляются до сотых: дрожание последнего
    /// знака не должно заставлять перерисовывать кадр каждый тик.
    deform: (i32, i32, i32),
    alpha: i32,
}

pub(crate) fn content_key(
    scene: &Scene<'_>,
    sprites: &[usize],
    origin: (i32, i32),
    scale: u32,
) -> ContentKey {
    ContentKey {
        scale,
        sprites: sprites
            .iter()
            .filter_map(|i| scene.sprites.get(*i))
            .filter(|s| !s.invisible())
            .map(|s| SpriteKey {
                argb: s.frame.argb.as_ptr() as usize,
                w: s.frame.w,
                h: s.frame.h,
                rel: {
                    let r = s.screen_rect();
                    (r.x.round() as i32 - origin.0, r.y.round() as i32 - origin.1)
                },
                orient: s.orient,
                deform: (
                    (s.deform.scale_x * 100.0).round() as i32,
                    (s.deform.scale_y * 100.0).round() as i32,
                    (s.deform.lean * 10.0).round() as i32,
                ),
                alpha: (s.alpha * 100.0).round() as i32,
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
    sprites: &[usize],
    origin: (i32, i32),
    scale: u32,
) {
    canvas.fill(0); // 0x00000000 — полностью прозрачно
    for s in sprites.iter().filter_map(|i| scene.sprites.get(*i)) {
        if s.invisible() {
            continue;
        }
        let r = s.screen_rect();
        let rel = (r.x.round() as i32 - origin.0, r.y.round() as i32 - origin.1);
        blit(
            canvas, size, s.frame, rel, s.orient, s.deform, s.alpha, scale,
        );
    }
}

/// Индекс исходного пикселя кадра для точки (dx, dy) в системе координат
/// вывода. Сама математика поворота живёт в ядре ([`Orient::source_pixel`]),
/// чтобы у оверлея и дев-инструментов она была общая.
fn source_index(frame: &Frame, orient: Orient, dx: u32, dy: u32) -> usize {
    let (sx, sy) = orient.source_pixel(frame.w, frame.h, dx, dy);
    (sy * frame.w + sx) as usize
}

/// Блит спрайта на холст: nearest-neighbour масштаб, ориентация кадра
/// (зеркала + поворот на четверти), пропуск пикселей с альфой 0, клип по
/// краям. `origin` — логические координаты левого верхнего угла спрайта.
#[allow(clippy::too_many_arguments)]
fn blit(
    canvas: &mut [u8],
    (cw, ch): (u32, u32),
    frame: &Frame,
    (ox, oy): (i32, i32),
    orient: Orient,
    deform: Deform,
    alpha: f32,
    scale: u32,
) {
    if frame.w == 0 || frame.h == 0 || alpha <= 0.004 {
        return;
    }
    let (ow, oh) = orient.output_size(frame.w, frame.h);
    let (out_w, out_h) = deform.output_size(ow, oh);
    let plain = deform.is_identity();
    let base_x = ox * scale as i32;
    let base_y = oy * scale as i32;
    for dy in 0..out_h * scale {
        let cy = base_y + dy as i32;
        if cy < 0 || cy >= ch as i32 {
            continue;
        }
        for dx in 0..out_w * scale {
            let cx = base_x + dx as i32;
            if cx < 0 || cx >= cw as i32 {
                continue;
            }
            // Без деформации выборка прямая — быстрый путь обычного кадра.
            let (sx, sy) = if plain {
                (dx / scale, dy / scale)
            } else {
                let (fx, fy) =
                    deform.source_point((ow, oh), (out_w, out_h), dx / scale, dy / scale);
                if fx < 0.0 || fy < 0.0 {
                    continue;
                }
                let (sx, sy) = (fx as u32, fy as u32);
                if sx >= ow || sy >= oh {
                    continue;
                }
                (sx, sy)
            };
            let px = frame.argb[source_index(frame, orient, sx, sy)];
            if px >> 24 == 0 {
                continue; // прозрачный пиксель спрайта
            }
            let px = fade(px, alpha);
            if px >> 24 == 0 {
                continue;
            }
            let off = ((cy as u32 * cw + cx as u32) * 4) as usize;
            // ARGB8888 little-endian: байты B, G, R, A.
            if px >> 24 == 0xff {
                canvas[off..off + 4].copy_from_slice(&px.to_le_bytes());
            } else {
                // Полупрозрачный пиксель НАКЛАДЫВАЕТСЯ на уже нарисованное
                // (src-over для premultiplied), а не заменяет его: раньше
                // копирование пробивало питомца насквозь — сквозь зелень
                // укачивания и облачка пыли просвечивал рабочий стол.
                let dst =
                    u32::from_le_bytes(canvas[off..off + 4].try_into().expect("4 байта пикселя"));
                canvas[off..off + 4].copy_from_slice(&over(px, dst).to_le_bytes());
            }
        }
    }
}

/// Наложить premultiplied-источник на приёмник: `dst = src + dst·(1−α)`.
fn over(src: u32, dst: u32) -> u32 {
    let inv = 255 - ((src >> 24) & 0xff);
    let ch = |sh: u32| -> u32 {
        let s = (src >> sh) & 0xff;
        let d = (dst >> sh) & 0xff;
        (s + (d * inv + 127) / 255).min(255)
    };
    (ch(24) << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Умножить premultiplied-пиксель на прозрачность: и альфа, и каналы.
fn fade(px: u32, alpha: f32) -> u32 {
    if alpha >= 0.996 {
        return px;
    }
    let k = alpha.clamp(0.0, 1.0);
    let ch = |sh: u32| ((((px >> sh) & 0xff) as f32 * k).round() as u32).min(255);
    (ch(24) << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

#[cfg(test)]
mod tests {
    /// Все спрайты сцены — как их видит бэкенд без кластеров (X11).
    fn all_of(scene: &Scene<'_>) -> Vec<usize> {
        (0..scene.sprites.len()).collect()
    }

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
                orient: Orient::mirrored(mirror),
                deform: Deform::NONE,
                alpha: 1.0,
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
        blit(
            &mut canvas,
            (4, 4),
            &f,
            (1, 1),
            Orient::IDENTITY,
            Deform::NONE,
            1.0,
            1,
        );
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
        blit(
            &mut canvas,
            (2, 2),
            &f,
            (0, 0),
            Orient::mirrored(true),
            Deform::NONE,
            1.0,
            1,
        );
        assert_eq!(pixel(&canvas, 2, 0, 0), 0xff_00_22_00); // B слева
        assert_eq!(pixel(&canvas, 2, 1, 0), 0xff_11_00_00); // A справа
        assert_eq!(pixel(&canvas, 2, 0, 1), 0); // прозрачный (зеркало C)
        assert_eq!(pixel(&canvas, 2, 1, 1), 0xff_00_00_33);
    }

    /// Фаза G: поворот на четверть (питомец на стене) — низ кадра
    /// уходит влево при 1 четверти и вправо при 3.
    #[test]
    fn blit_quarter_turns_rotate_the_frame() {
        let f = frame_2x2(); // A B / C .
        let cw = Orient {
            flip_x: false,
            flip_y: false,
            quarter_turns: 1,
        };
        let mut canvas = vec![0u8; 2 * 2 * 4];
        blit(&mut canvas, (2, 2), &f, (0, 0), cw, Deform::NONE, 1.0, 1);
        // Поворот по часовой: A уходит вправо-вверх, C — влево-вверх.
        assert_eq!(pixel(&canvas, 2, 1, 0), 0xff_11_00_00);
        assert_eq!(pixel(&canvas, 2, 0, 0), 0xff_00_00_33);
        assert_eq!(pixel(&canvas, 2, 1, 1), 0xff_00_22_00);

        let ccw = Orient {
            quarter_turns: 3,
            ..cw
        };
        let mut canvas = vec![0u8; 2 * 2 * 4];
        blit(&mut canvas, (2, 2), &f, (0, 0), ccw, Deform::NONE, 1.0, 1);
        assert_eq!(pixel(&canvas, 2, 0, 1), 0xff_11_00_00);
        assert_eq!(pixel(&canvas, 2, 0, 0), 0xff_00_22_00);
    }

    /// Вертикальное отражение (питомец под потолком) переворачивает ряды.
    #[test]
    fn blit_flip_y_swaps_rows() {
        let f = frame_2x2();
        let orient = Orient {
            flip_x: false,
            flip_y: true,
            quarter_turns: 0,
        };
        let mut canvas = vec![0u8; 2 * 2 * 4];
        blit(
            &mut canvas,
            (2, 2),
            &f,
            (0, 0),
            orient,
            Deform::NONE,
            1.0,
            1,
        );
        assert_eq!(pixel(&canvas, 2, 0, 0), 0xff_00_00_33, "нижний ряд наверху");
        assert_eq!(pixel(&canvas, 2, 0, 1), 0xff_11_00_00);
    }

    /// Фаза G6: сплющивание тянет кадр по X и жмёт по Y, низ остаётся на
    /// месте (якорь), рисунок продолжает попадать в холст.
    #[test]
    fn blit_deform_squashes_around_the_anchor() {
        let f = frame_2x2();
        let squash = Deform {
            scale_x: 2.0,
            scale_y: 1.0,
            ..Deform::NONE
        };
        let mut canvas = vec![0u8; 4 * 2 * 4];
        blit(
            &mut canvas,
            (4, 2),
            &f,
            (0, 0),
            Orient::IDENTITY,
            squash,
            1.0,
            1,
        );
        // Левый пиксель растянут на две колонки.
        assert_eq!(pixel(&canvas, 4, 0, 0), 0xff_11_00_00);
        assert_eq!(pixel(&canvas, 4, 1, 0), 0xff_11_00_00);
        assert_eq!(pixel(&canvas, 4, 2, 0), 0xff_00_22_00);
    }

    /// Полупрозрачный слой НАКЛАДЫВАЕТСЯ на непрозрачный, а не пробивает
    /// его: иначе питомец под зеленью укачивания просвечивал насквозь.
    #[test]
    fn blit_blends_translucent_over_opaque() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 2 * 2 * 4];
        // Сначала непрозрачный слой.
        blit(
            &mut canvas,
            (2, 2),
            &f,
            (0, 0),
            Orient::IDENTITY,
            Deform::NONE,
            1.0,
            1,
        );
        // Поверх — тот же кадр наполовину прозрачным.
        blit(
            &mut canvas,
            (2, 2),
            &f,
            (0, 0),
            Orient::IDENTITY,
            Deform::NONE,
            0.5,
            1,
        );
        let px = pixel(&canvas, 2, 0, 0);
        assert_eq!(
            px >> 24,
            0xff,
            "пиксель обязан остаться непрозрачным: {px:08x}"
        );
    }

    /// Прозрачность гасит premultiplied-пиксель целиком: и каналы, и альфу.
    #[test]
    fn blit_alpha_fades_the_sprite() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 2 * 2 * 4];
        blit(
            &mut canvas,
            (2, 2),
            &f,
            (0, 0),
            Orient::IDENTITY,
            Deform::NONE,
            0.5,
            1,
        );
        let px = pixel(&canvas, 2, 0, 0);
        assert_eq!(px >> 24, 0x80, "альфа вдвое: {px:08x}");
        assert_eq!((px >> 16) & 0xff, 0x09, "канал тоже вдвое: {px:08x}");
        // Полностью прозрачный спрайт не рисуется вовсе.
        let mut canvas = vec![0u8; 2 * 2 * 4];
        blit(
            &mut canvas,
            (2, 2),
            &f,
            (0, 0),
            Orient::IDENTITY,
            Deform::NONE,
            0.0,
            1,
        );
        assert_eq!(pixel(&canvas, 2, 0, 0), 0);
    }

    #[test]
    fn blit_scale2_nearest_neighbour() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 4 * 4 * 4];
        blit(
            &mut canvas,
            (4, 4),
            &f,
            (0, 0),
            Orient::IDENTITY,
            Deform::NONE,
            1.0,
            2,
        );
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
        blit(
            &mut canvas,
            (2, 2),
            &f,
            (-1, -1),
            Orient::IDENTITY,
            Deform::NONE,
            1.0,
            1,
        );
        assert_eq!(pixel(&canvas, 2, 0, 0), 0); // прозрачный угол спрайта
                                                // За правым нижним краем — тоже без паники.
        blit(
            &mut canvas,
            (2, 2),
            &f,
            (1, 1),
            Orient::IDENTITY,
            Deform::NONE,
            1.0,
            1,
        );
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
        compose(&mut canvas, (bw, bh), &scene, &all_of(&scene), (bx, by), 1);
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

    /// Далеко разнесённые вещи (питомец у одной стены, домик у другой)
    /// живут в РАЗНЫХ буферах: иначе буфер был бы размером с экран.
    #[test]
    fn clusters_split_far_apart_sprites() {
        let f = frame_2x2();
        let at = |x: f32, y: f32| SpriteInstance {
            frame: &f,
            origin: Vec2::new(x, y),
            orient: Orient::IDENTITY,
            deform: Deform::NONE,
            alpha: 1.0,
        };
        let scene = Scene {
            sprites: vec![at(0.0, 0.0), at(3.0, 0.0), at(900.0, 500.0)],
            input_rects: Vec::new(),
        };
        let clusters = cluster_scene(&scene, 4, 6);
        assert_eq!(clusters.len(), 2, "два сгустка: {clusters:?}");
        assert_eq!(clusters[0].sprites, vec![0, 1], "соседи вместе");
        assert_eq!(clusters[0].bounds, (0, 0, 5, 2));
        assert_eq!(clusters[1].sprites, vec![2]);
        assert_eq!(clusters[1].bounds, (900, 500, 2, 2));
    }

    /// Спрайт-перемычка сливает уже созданные сгустки в один.
    #[test]
    fn clusters_merge_through_a_bridge() {
        let f = frame_2x2();
        let at = |x: f32| SpriteInstance {
            frame: &f,
            origin: Vec2::new(x, 0.0),
            orient: Orient::IDENTITY,
            deform: Deform::NONE,
            alpha: 1.0,
        };
        let scene = Scene {
            sprites: vec![at(0.0), at(20.0), at(10.0)],
            input_rects: Vec::new(),
        };
        let clusters = cluster_scene(&scene, 9, 6);
        assert_eq!(clusters.len(), 1, "перемычка склеила: {clusters:?}");
        assert_eq!(clusters[0].sprites, vec![0, 1, 2]);
    }

    /// Сгустков не больше предела: лишние сливаются с ближайшими, а не
    /// плодят поверхности.
    #[test]
    fn clusters_are_capped() {
        let f = frame_2x2();
        let scene = Scene {
            sprites: (0..8)
                .map(|i| SpriteInstance {
                    frame: &f,
                    origin: Vec2::new(i as f32 * 100.0, 0.0),
                    orient: Orient::IDENTITY,
                    deform: Deform::NONE,
                    alpha: 1.0,
                })
                .collect(),
            input_rects: Vec::new(),
        };
        let clusters = cluster_scene(&scene, 4, 3);
        assert_eq!(clusters.len(), 3);
        let total: usize = clusters.iter().map(|c| c.sprites.len()).sum();
        assert_eq!(total, 8, "ни один спрайт не потерян");
    }

    /// Невидимые спрайты (спрятанный питомец) не создают сгустков.
    #[test]
    fn clusters_skip_invisible() {
        let empty = Frame {
            w: 0,
            h: 0,
            argb: Vec::new(),
        };
        let scene = Scene {
            sprites: vec![SpriteInstance {
                frame: &empty,
                origin: Vec2::default(),
                orient: Orient::IDENTITY,
                deform: Deform::NONE,
                alpha: 1.0,
            }],
            input_rects: Vec::new(),
        };
        assert!(cluster_scene(&scene, 4, 6).is_empty());
    }

    #[test]
    fn scene_bounds_unions_sprites() {
        let f = frame_2x2();
        let scene = Scene {
            sprites: vec![
                SpriteInstance {
                    frame: &f,
                    origin: Vec2::new(0.0, 0.0),
                    orient: Orient::IDENTITY,
                    deform: Deform::NONE,
                    alpha: 1.0,
                },
                SpriteInstance {
                    frame: &f,
                    origin: Vec2::new(10.0, 4.0),
                    orient: Orient::IDENTITY,
                    deform: Deform::NONE,
                    alpha: 1.0,
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
        let ka = content_key(&a, &all_of(&a), (10, 20), 1);
        let kb = content_key(&b, &all_of(&b), (300, 400), 1);
        // Кадр тот же, позиция другая → перерисовка не нужна.
        assert_eq!(ka, kb);
    }

    #[test]
    fn content_key_detects_frame_change() {
        let f1 = frame_2x2();
        let f2 = frame_2x2(); // другой Vec → другой адрес пикселей
        let a = scene_one(&f1, Vec2::new(0.0, 0.0), false);
        let b = scene_one(&f2, Vec2::new(0.0, 0.0), false);
        assert_ne!(
            content_key(&a, &all_of(&a), (0, 0), 1),
            content_key(&b, &all_of(&b), (0, 0), 1)
        );
    }

    #[test]
    fn content_key_detects_mirror_and_scale() {
        let f = frame_2x2();
        let plain = scene_one(&f, Vec2::new(0.0, 0.0), false);
        let mirrored = scene_one(&f, Vec2::new(0.0, 0.0), true);
        assert_ne!(
            content_key(&plain, &all_of(&plain), (0, 0), 1),
            content_key(&mirrored, &all_of(&mirrored), (0, 0), 1)
        );
        assert_ne!(
            content_key(&plain, &all_of(&plain), (0, 0), 1),
            content_key(&plain, &all_of(&plain), (0, 0), 2)
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
