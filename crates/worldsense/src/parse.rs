//! Разбор и фильтрация JSON-снапшотов от KWin-скрипта.
//!
//! Чистые функции без D-Bus и KDE — юнит-тесты гоняются где угодно.
//! Формат JSON задаёт `assets/kwin/driftling-sense.qml`; лишние поля
//! игнорируются, недостающие берут значения по умолчанию — чтобы скрипт
//! и демон могли обновляться не в ногу.

use driftling_core::Rect;
use serde::Deserialize;

use crate::{WindowPlatform, WorldSnapshot};

/// Кромка короче этого — не платформа (стоять негде): отсекает служебные
/// окна-точки вроде xwaylandvideobridge (1×1 px).
const MIN_PLATFORM_SIZE: f32 = 16.0;

/// Окно из снапшота скрипта. Порядок в массиве — bottom-to-top (стекинг KWin).
#[derive(Debug, Deserialize)]
pub(crate) struct RawWindow {
    /// KWin `internalId` (QUuid строкой) — источник стабильного id.
    #[serde(default)]
    pub iid: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub minimized: bool,
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default, rename = "skipTaskbar")]
    pub skip_taskbar: bool,
    /// resourceClass (app id) — по нему отсекаем собственные окна driftling.
    #[serde(default)]
    pub cls: String,
}

/// Рабочая область активного экрана (workspace.clientArea с учётом панелей).
/// Из прямоугольника пока нужен только низ — верх нижней панели; остальное
/// добавим в D6 (мультимонитор).
#[derive(Debug, Deserialize)]
pub(crate) struct RawWorkArea {
    pub y: f32,
    pub h: f32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawSnapshot {
    #[serde(default)]
    pub windows: Vec<RawWindow>,
    #[serde(default, rename = "workArea")]
    pub work_area: Option<RawWorkArea>,
    #[serde(default, rename = "anyFullscreen")]
    pub any_fullscreen: bool,
}

/// JSON от скрипта → готовый снапшот. `None` — мусор на входе.
pub(crate) fn parse_and_filter(json: &str) -> Option<WorldSnapshot> {
    let raw: RawSnapshot = serde_json::from_str(json).ok()?;
    Some(filter(&raw))
}

/// Смысловая фильтрация: типовую (панели/попапы/чужие рабочие столы) уже
/// сделал скрипт, здесь — свёрнутые, свои окна, невидимые хелперы и мелочь.
pub(crate) fn filter(raw: &RawSnapshot) -> WorldSnapshot {
    let mut platforms: Vec<WindowPlatform> = raw
        .windows
        .iter()
        .filter(|w| !w.minimized)
        .filter(|w| !w.skip_taskbar)
        .filter(|w| !is_own_window(&w.cls))
        .filter(|w| w.w >= MIN_PLATFORM_SIZE && w.h >= MIN_PLATFORM_SIZE)
        .map(|w| WindowPlatform {
            rect: Rect::new(w.x, w.y, w.w, w.h),
            id: fnv1a64(&w.iid),
        })
        .collect();
    // Скрипт шлёт снизу вверх, наружу отдаём сверху вниз.
    platforms.reverse();
    WorldSnapshot {
        platforms,
        workspace_bottom: raw.work_area.as_ref().map(|wa| wa.y + wa.h),
        // Флагу скрипта доверяем, но на всякий случай дублируем по окнам.
        fullscreen_active: raw.any_fullscreen
            || raw.windows.iter().any(|w| w.fullscreen && !w.minimized),
    }
}

/// Собственные окна питомца (оверлей, настройки) — не платформы.
fn is_own_window(cls: &str) -> bool {
    cls.to_ascii_lowercase().contains("driftling")
}

/// FNV-1a 64 бит: стабильный id окна из строки QUuid. Ручная реализация,
/// чтобы не тянуть зависимость ради десяти строк.
pub(crate) fn fnv1a64(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Снапшот, близкий к реальному дампу с живой сессии KWin 6.3.6.
    const SAMPLE: &str = r#"{
        "windows": [
            {"iid": "{aaaa}", "x": 46, "y": 99, "w": 1, "h": 1,
             "minimized": false, "fullscreen": false, "skipTaskbar": true,
             "cls": "xwaylandvideobridge"},
            {"iid": "{bbbb}", "x": 2399, "y": 98, "w": 961, "h": 884,
             "minimized": true, "fullscreen": false, "skipTaskbar": false,
             "cls": "org.kde.discover"},
            {"iid": "{cccc}", "x": 1920, "y": 0, "w": 1874, "h": 1080,
             "minimized": false, "fullscreen": false, "skipTaskbar": false,
             "cls": "google-chrome"},
            {"iid": "{dddd}", "x": 100, "y": 50, "w": 800, "h": 600,
             "minimized": false, "fullscreen": false, "skipTaskbar": false,
             "cls": "Driftling-settings"},
            {"iid": "{eeee}", "x": 348, "y": 112, "w": 1270, "h": 855,
             "minimized": false, "fullscreen": false, "skipTaskbar": false,
             "cls": "code"}
        ],
        "workArea": {"x": 46, "y": 0, "w": 1874, "h": 1080},
        "anyFullscreen": false,
        "seq": 7
    }"#;

    #[test]
    fn otbor_i_poryadok() {
        let snap = parse_and_filter(SAMPLE).expect("валидный JSON");
        // Осталось два окна: chrome и code (1×1-хелпер, свёрнутое, своё — вон).
        assert_eq!(snap.platforms.len(), 2);
        // Скрипт слал bottom-to-top (chrome ниже code) — наружу top-to-bottom.
        assert_eq!(snap.platforms[0].id, fnv1a64("{eeee}"));
        assert_eq!(snap.platforms[1].id, fnv1a64("{cccc}"));
        // Прямоугольник переносится целиком.
        let top = &snap.platforms[0];
        assert_eq!(
            (top.rect.x, top.rect.y, top.rect.w, top.rect.h),
            (348.0, 112.0, 1270.0, 855.0)
        );
    }

    #[test]
    fn pol_iz_work_area() {
        let snap = parse_and_filter(SAMPLE).unwrap();
        // Верх нижней панели: y + h рабочей области.
        assert_eq!(snap.workspace_bottom, Some(1080.0));
        assert!(!snap.fullscreen_active);
    }

    #[test]
    fn bez_work_area_pol_neizvesten() {
        let snap = parse_and_filter(r#"{"windows": []}"#).unwrap();
        assert_eq!(snap.workspace_bottom, None);
        assert!(snap.platforms.is_empty());
        assert!(!snap.fullscreen_active);
    }

    #[test]
    fn fullscreen_iz_flaga_i_iz_okon() {
        // Флаг скрипта.
        let s = r#"{"windows": [], "anyFullscreen": true}"#;
        assert!(parse_and_filter(s).unwrap().fullscreen_active);
        // Дубль по окнам, даже если флаг забыт.
        let s = r#"{"windows": [
            {"iid": "{a}", "x": 0, "y": 0, "w": 1920, "h": 1080, "fullscreen": true}
        ]}"#;
        assert!(parse_and_filter(s).unwrap().fullscreen_active);
        // Свёрнутое полноэкранное — не считается.
        let s = r#"{"windows": [
            {"iid": "{a}", "x": 0, "y": 0, "w": 1920, "h": 1080,
             "fullscreen": true, "minimized": true}
        ]}"#;
        assert!(!parse_and_filter(s).unwrap().fullscreen_active);
    }

    #[test]
    fn svoi_okna_otsekayutsya_bez_ucheta_registra() {
        assert!(is_own_window("driftling"));
        assert!(is_own_window("Driftling-settings"));
        assert!(is_own_window("io.github.nanitll.Driftling"));
        assert!(!is_own_window("google-chrome"));
    }

    #[test]
    fn musor_na_vhode_daet_none() {
        assert!(parse_and_filter("").is_none());
        assert!(parse_and_filter("не json").is_none());
        assert!(parse_and_filter(r#"{"windows": 42}"#).is_none());
    }

    #[test]
    fn lishnie_i_nedostayushchie_polya_ne_lomayut() {
        // Скрипт новее демона: неизвестные поля игнорируются.
        let s = r#"{"windows": [
            {"iid": "{a}", "x": 0, "y": 0, "w": 100, "h": 100, "newField": "x"}
        ], "futureTopLevel": {"a": 1}}"#;
        let snap = parse_and_filter(s).unwrap();
        assert_eq!(snap.platforms.len(), 1);
        // Демон новее скрипта: отсутствие флагов = значения по умолчанию.
        assert_eq!(snap.platforms[0].id, fnv1a64("{a}"));
    }

    #[test]
    fn fnv1a64_kontrolnye_vektory() {
        // Классические контрольные значения FNV-1a 64.
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64("foobar"), 0x85944171f73967e8);
        // Стабильность: одинаковый вход — одинаковый id.
        assert_eq!(fnv1a64("{uuid}"), fnv1a64("{uuid}"));
        assert_ne!(fnv1a64("{uuid1}"), fnv1a64("{uuid2}"));
    }
}
