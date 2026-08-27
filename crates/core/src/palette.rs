//! Палитра питомца: базовый цвет тела — пользовательская настройка
//! (журнальное событие `Recolored`), все производные тона (тёмная кромка,
//! скорлупа яйца, акцент интерфейса) выводятся из него хелперами ниже.
//!
//! Формат цвета — ARGB8888, как у кадров [`crate::sprite::Frame`]; альфа
//! значимых цветов всегда 0xff (fold и демон нормализуют её сами).

/// Цвет тела по умолчанию: тёплый серо-бежевый.
pub const DEFAULT_PET_COLOR: u32 = 0xff_b0_a2_94;

/// Пресеты цвета с машинными именами (имя — ключ локализации в UI,
/// в журнал пишется только сам цвет). Первый пресет — дефолт; прежний
/// фирменный сиреневый остался обычным пресетом в хвосте.
pub const PET_PRESETS: &[(u32, &str)] = &[
    (DEFAULT_PET_COLOR, "greige"),
    (0xff_e8_94_4a, "amber"),
    (0xff_5f_bf_8f, "mint"),
    (0xff_6b_8f_d2, "sky"),
    (0xff_d2_7a_9e, "rose"),
    (0xff_8a_94_a6, "slate"),
    (0xff_d4_b8_6a, "sand"),
    (0xff_8a_63_d2, "violet"),
];

/// Затемнить непрозрачный ARGB-цвет: каналы умножаются на `k` (0..=1).
pub fn darken(argb: u32, k: f32) -> u32 {
    scale(argb, k)
}

/// Осветлить ARGB-цвет: каналы умножаются на `k` (>= 1) с потолком 255.
pub fn lighten(argb: u32, k: f32) -> u32 {
    scale(argb, k)
}

/// Общая механика darken/lighten: масштаб каналов, альфа не трогается.
fn scale(argb: u32, k: f32) -> u32 {
    let ch = |sh: u32| {
        let c = ((argb >> sh) & 0xff) as f32;
        (c * k.max(0.0)).round().min(255.0) as u32
    };
    (argb & 0xff00_0000) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Относительная яркость 0..=1 (веса Rec. 709) — гард контраста деталей
/// (щёки на тёмном теле, см. sprite.rs).
pub fn luminance(argb: u32) -> f32 {
    let ch = |sh: u32| ((argb >> sh) & 0xff) as f32 / 255.0;
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_eight_unique_opaque_colors_with_default_first() {
        assert_eq!(PET_PRESETS.len(), 8);
        assert_eq!(PET_PRESETS[0].0, DEFAULT_PET_COLOR);
        assert_eq!(PET_PRESETS[0].1, "greige");
        // Сиреневый — больше не дефолт, но жив как пресет.
        assert!(PET_PRESETS.contains(&(0xff_8a_63_d2, "violet")));
        for (argb, name) in PET_PRESETS {
            assert_eq!(argb >> 24, 0xff, "{name}: пресет обязан быть непрозрачным");
            assert!(!name.is_empty());
        }
        let mut colors: Vec<u32> = PET_PRESETS.iter().map(|(c, _)| *c).collect();
        colors.sort_unstable();
        colors.dedup();
        assert_eq!(colors.len(), 8, "цвета пресетов не повторяются");
        let mut names: Vec<&str> = PET_PRESETS.iter().map(|(_, n)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 8, "имена пресетов не повторяются");
    }

    #[test]
    fn darken_scales_channels_down_and_keeps_alpha() {
        let d = darken(0xff_80_40_20, 0.5);
        assert_eq!(d, 0xff_40_20_10);
        // Отрицательный k клампится в 0, альфа живёт.
        assert_eq!(darken(0xff_80_40_20, -1.0), 0xff_00_00_00);
        assert_eq!(darken(0x80_ff_ff_ff, 0.5) >> 24, 0x80, "альфа не трогается");
    }

    #[test]
    fn lighten_scales_up_with_255_ceiling() {
        let l = lighten(0xff_80_40_20, 1.5);
        assert_eq!(l, 0xff_c0_60_30);
        assert_eq!(lighten(0xff_c0_c0_c0, 2.0), 0xff_ff_ff_ff, "потолок 255");
        // k = 1 — идентичность.
        assert_eq!(lighten(DEFAULT_PET_COLOR, 1.0), DEFAULT_PET_COLOR);
    }

    #[test]
    fn luminance_spans_black_to_white() {
        assert_eq!(luminance(0xff_00_00_00), 0.0);
        assert!((luminance(0xff_ff_ff_ff) - 1.0).abs() < 1e-6);
        // Зелёный ярче синего при равных каналах (веса Rec. 709).
        assert!(luminance(0xff_00_ff_00) > luminance(0xff_00_00_ff));
        // Все пресеты — «светлые» тела: розовые щёки читаются.
        for (argb, name) in PET_PRESETS {
            assert!(luminance(*argb) >= 0.35, "{name}: пресет слишком тёмный");
        }
    }
}
