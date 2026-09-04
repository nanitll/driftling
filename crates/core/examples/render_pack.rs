//! Дев-инструмент фазы C: рендер встроенного пака в PNG для визуальной
//! проверки арта и генерация иконки приложения.
//!
//! ```text
//! cargo run -p driftling-core --example render_pack -- sheets <каталог>
//! cargo run -p driftling-core --example render_pack -- icons  <каталог>
//! ```
//!
//! `sheets`: обзорный лист на стадию (все семейства построчно, 4x, светлый
//! и тёмный фон), фильмстрип на каждое семейство (4x) и «линейка» стадий
//! рядом со старым процедурным блобом. `icons`: иконка приложения
//! 128/64/48/32 на тёмной карточке (нейтральный greige — иконка не следует
//! пользовательскому цвету).

use std::fs;
use std::path::Path;

use driftling_core::growth::Stage;
use driftling_core::pack::{default_pack, Pack, BODY_ANIMS, EGG_ANIMS, EXTRA_ANIMS};
use driftling_core::palette::DEFAULT_PET_COLOR;
use driftling_core::sprite::{placeholder_colored, Frame};
use driftling_core::{Direction, Orient, Surface};

const LIGHT_BG: u32 = 0xff_e9_e9_ef;
const DARK_BG: u32 = 0xff_34_36_3f;
const PAD: u32 = 4;
const SCALE: u32 = 4;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (mode, out) = match args.as_slice() {
        [_, m, o] if m == "sheets" || m == "icons" || m == "surfaces" => (m.as_str(), Path::new(o)),
        _ => {
            eprintln!("использование: render_pack (sheets|icons|surfaces) <каталог>");
            std::process::exit(2);
        }
    };
    fs::create_dir_all(out).expect("каталог вывода");
    let pack = match default_pack() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("встроенный пак битый: {e}");
            std::process::exit(1);
        }
    };
    match mode {
        "sheets" => sheets(pack, out),
        "surfaces" => surfaces(pack, out),
        _ => icons(pack, out),
    }
}

/// Пиксельный холст ARGB с альфой 0/255 поверх заданного фона.
struct Canvas {
    w: u32,
    h: u32,
    px: Vec<u32>,
}

impl Canvas {
    fn new(w: u32, h: u32, bg: u32) -> Self {
        Canvas {
            w,
            h,
            px: vec![bg; (w * h) as usize],
        }
    }

    /// Блит с ориентацией — той же математикой поворота, что и в оверлее
    /// ([`Orient::source_pixel`]); дев-пруф показывает ровно то, что
    /// увидит композитор на стене и под потолком.
    fn blit_oriented(&mut self, f: &Frame, x0: u32, y0: u32, orient: Orient) {
        let (ow, oh) = orient.output_size(f.w, f.h);
        for dy in 0..oh {
            for dx in 0..ow {
                let (sx, sy) = orient.source_pixel(f.w, f.h, dx, dy);
                let p = f.argb[(sy * f.w + sx) as usize];
                if p >> 24 == 0 {
                    continue;
                }
                let (cx, cy) = (x0 + dx, y0 + dy);
                if cx < self.w && cy < self.h {
                    self.px[(cy * self.w + cx) as usize] = p;
                }
            }
        }
    }

    fn blit(&mut self, f: &Frame, x0: u32, y0: u32) {
        for y in 0..f.h {
            for x in 0..f.w {
                let p = f.argb[(y * f.w + x) as usize];
                if p >> 24 == 0 {
                    continue;
                }
                let (dx, dy) = (x0 + x, y0 + y);
                if dx < self.w && dy < self.h {
                    self.px[(dy * self.w + dx) as usize] = p;
                }
            }
        }
    }

    fn save(&self, path: &Path) {
        write_png(path, self.w, self.h, &self.px);
    }
}

/// Дев-пруф фазы G: «комната» с питомцем на всех поверхностях —
/// пол, обе стены и потолок, каждый со своим кадром и ориентацией.
fn surfaces(pack: &Pack, out: &Path) {
    const ROOM_W: u32 = 420;
    const ROOM_H: u32 = 300;
    const PET: u32 = 64;
    let frames = |anim: &str| pack.frames(Stage::Adult, anim, PET, DEFAULT_PET_COLOR);
    let walk = frames("walk");
    let climb = frames("climb");
    let cling = frames("cling");
    let idle = frames("idle");

    for (name, bg) in [("surfaces_light", LIGHT_BG), ("surfaces_dark", DARK_BG)] {
        let mut c = Canvas::new(ROOM_W, ROOM_H, bg);
        // Пол: обычная ориентация, мордой вправо и влево.
        c.blit_oriented(
            &walk[0],
            40,
            ROOM_H - PET,
            Surface::Floor.orient(Direction::Right),
        );
        c.blit_oriented(
            &idle[0],
            150,
            ROOM_H - PET,
            Surface::Floor.orient(Direction::Left),
        );
        // Левая стена: ползёт вверх (facing Left = вверх).
        c.blit_oriented(&climb[0], 0, 90, Surface::WallLeft.orient(Direction::Left));
        // Правая стена: висит, держась (facing Right = вверх).
        c.blit_oriented(
            &cling[0],
            ROOM_W - PET,
            120,
            Surface::WallRight.orient(Direction::Right),
        );
        // Потолок: висит вниз головой и ползёт вправо.
        c.blit_oriented(&climb[1], 200, 0, Surface::Ceiling.orient(Direction::Right));
        c.save(&out.join(format!("{name}.png")));
    }
    println!("пруф поверхностей записан в {}", out.display());
}

/// Все семейства стадии в порядке манифест-констант.
fn stage_anims(stage: Stage) -> Vec<&'static str> {
    if stage == Stage::Egg {
        EGG_ANIMS.to_vec()
    } else {
        // Семейства фазы G — следом за базовыми, тем же порядком, что в
        // константах: лист-обзор должен показывать весь арт стадии.
        BODY_ANIMS
            .iter()
            .chain(EXTRA_ANIMS.iter())
            .copied()
            .collect()
    }
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Egg => "egg",
        Stage::Baby => "baby",
        Stage::Child => "child",
        Stage::Teen => "teen",
        Stage::Adult => "adult",
    }
}

const ALL_STAGES: [Stage; 5] = [
    Stage::Egg,
    Stage::Baby,
    Stage::Child,
    Stage::Teen,
    Stage::Adult,
];

fn sheets(pack: &Pack, out: &Path) {
    for stage in ALL_STAGES {
        let native = pack.native(stage);
        let cell = native * SCALE + PAD;
        let anims = stage_anims(stage);
        let cols = anims
            .iter()
            .map(|a| {
                pack.frames(stage, a, native * SCALE, DEFAULT_PET_COLOR)
                    .len()
            })
            .max()
            .unwrap_or(1) as u32;
        for (bg, tag) in [(LIGHT_BG, "light"), (DARK_BG, "dark")] {
            let mut c = Canvas::new(cols * cell + PAD, anims.len() as u32 * cell + PAD, bg);
            for (row, anim) in anims.iter().enumerate() {
                let frames = pack.frames(stage, anim, native * SCALE, DEFAULT_PET_COLOR);
                for (col, f) in frames.iter().enumerate() {
                    c.blit(f, PAD + col as u32 * cell, PAD + row as u32 * cell);
                }
            }
            c.save(&out.join(format!("sheet_{}_{tag}.png", stage_name(stage))));
        }
        // Фильмстрип на семейство (тёмный фон).
        for anim in anims {
            let frames = pack.frames(stage, anim, native * SCALE, DEFAULT_PET_COLOR);
            let mut c = Canvas::new(frames.len() as u32 * cell + PAD, cell + PAD, DARK_BG);
            for (col, f) in frames.iter().enumerate() {
                c.blit(f, PAD + col as u32 * cell, PAD);
            }
            c.save(&out.join(format!("strip_{}_{anim}.png", stage_name(stage))));
        }
    }
    lineup(pack, out);
    println!("листы записаны в {}", out.display());
}

/// Линейка стадий (3x, по базовой линии) + старый процедурный блоб для
/// сравнения «до/после».
fn lineup(pack: &Pack, out: &Path) {
    let k = 3;
    let mut frames: Vec<Frame> = Vec::new();
    for stage in ALL_STAGES {
        let native = pack.native(stage);
        let anim = if stage == Stage::Egg { "egg" } else { "idle" };
        frames.push(
            pack.frames(stage, anim, native * k, DEFAULT_PET_COLOR)
                .remove(0),
        );
    }
    // Блоб рисуется сразу в целевом размере (он процедурный).
    frames.push(placeholder_colored(96, Stage::Adult, DEFAULT_PET_COLOR).idle[0].clone());
    let h = frames.iter().map(|f| f.h).max().unwrap() + PAD * 2;
    let w: u32 = frames.iter().map(|f| f.w + PAD).sum::<u32>() + PAD;
    for (bg, tag) in [(LIGHT_BG, "light"), (DARK_BG, "dark")] {
        let mut c = Canvas::new(w, h, bg);
        let mut x = PAD;
        for f in &frames {
            c.blit(f, x, h - PAD - f.h); // базовая линия — низ
            x += f.w + PAD;
        }
        c.save(&out.join(format!("lineup_{tag}.png")));
    }
}

// ---- Иконка приложения ----

/// Карточка: тёмный скруглённый квадрат с лёгким вертикальным градиентом.
fn icon_card(size: u32) -> Canvas {
    let mut c = Canvas::new(size, size, 0);
    let r = size as f32 * 0.22;
    let (top, bot) = (0xff_33_2f_40u32, 0xff_25_22_2eu32);
    for y in 0..size {
        let t = y as f32 / size as f32;
        let lerp = |sh: u32| {
            let a = ((top >> sh) & 0xff) as f32;
            let b = ((bot >> sh) & 0xff) as f32;
            ((a + (b - a) * t) as u32) & 0xff
        };
        let row = 0xff00_0000 | (lerp(16) << 16) | (lerp(8) << 8) | lerp(0);
        for x in 0..size {
            // SDF скруглённого квадрата.
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let half = size as f32 / 2.0;
            let (dx, dy) = (
                (fx - half).abs() - (half - r),
                (fy - half).abs() - (half - r),
            );
            let dist = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
            if dist <= r {
                c.px[(y * size + x) as usize] = row;
            }
        }
    }
    c
}

fn icons(pack: &Pack, out: &Path) {
    // Взрослый идл на карточке 128, спрайт 96 (3x) по центру.
    let sprite = pack
        .frames(Stage::Adult, "idle", 96, DEFAULT_PET_COLOR)
        .remove(0);
    let mut card = icon_card(128);
    card.blit(&sprite, (128 - sprite.w) / 2, (128 - sprite.h) / 2);
    card.save(&out.join("io.github.nanitll.driftling.png"));
    // Мелкие размеры собираются заново в целых множителях спрайта
    // (64 -> 2x, 48/32 -> 1x): area-даунскейл пиксель-арта мылится.
    let two = pack
        .frames(Stage::Adult, "idle", 64, DEFAULT_PET_COLOR)
        .remove(0);
    let mut c64 = icon_card(64);
    c64.blit(&two, 0, 0);
    c64.save(&out.join("io.github.nanitll.driftling-64.png"));
    let one = pack
        .frames(Stage::Adult, "idle", 32, DEFAULT_PET_COLOR)
        .remove(0);
    for side in [48u32, 32] {
        let mut c = icon_card(side);
        c.blit(&one, (side - one.w) / 2, (side - one.h) / 2);
        c.save(&out.join(format!("io.github.nanitll.driftling-{side}.png")));
    }
    println!("иконки записаны в {}", out.display());
}

// ---- Минимальный PNG-энкодер (RGBA8, deflate stored-блоками) ----

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, t) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *t = c;
    }
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_buf = kind.to_vec();
    crc_buf.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_buf).to_be_bytes());
}

fn write_png(path: &Path, w: u32, h: u32, argb: &[u32]) {
    // Скан-строки: фильтр 0 + RGBA8.
    let mut raw = Vec::with_capacity((h * (1 + w * 4)) as usize);
    for y in 0..h {
        raw.push(0u8);
        for x in 0..w {
            let p = argb[(y * w + x) as usize];
            raw.extend_from_slice(&[
                (p >> 16 & 0xff) as u8,
                (p >> 8 & 0xff) as u8,
                (p & 0xff) as u8,
                (p >> 24 & 0xff) as u8,
            ]);
        }
    }
    // zlib: stored-блоки по 65535 байт.
    let mut z = vec![0x78u8, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png: Vec<u8> = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // RGBA8
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    fs::write(path, png).expect("запись PNG");
}
