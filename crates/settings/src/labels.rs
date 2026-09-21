//! Человеческие подписи: состояние питомца, стадия, пресеты цвета,
//! длительности. Всё локализуется здесь, чтобы страницы оставались про
//! раскладку, а не про строки.

use driftling_core::Stage;
use eframe::egui::Color32;

use crate::i18n::fl;
use crate::theme::{AMBER, DANGER, MUTED, SLEEP_BLUE, SUCCESS};

pub fn state_label(state: Option<&str>) -> String {
    match state {
        None => fl!("state-absent"),
        Some("Idle") => fl!("state-idle"),
        Some("Walk") => fl!("state-walk"),
        Some("Sleep") => fl!("state-sleep"),
        Some("Falling") => fl!("state-falling"),
        Some("Dragged") => fl!("state-dragged"),
        Some("Landing") => fl!("state-landing"),
        Some("Climb") => fl!("state-climb"),
        Some("Bonk") => fl!("state-bonk"),
        Some("Roll") => fl!("state-roll"),
        Some(_) => fl!("state-unknown"),
    }
}

/// Цвет бейджа состояния; `accent_light` — светлый тон цвета питомца.
pub fn state_color(state: Option<&str>, accent_light: Color32) -> Color32 {
    match state {
        None => MUTED,
        Some("Idle") => SUCCESS,
        Some("Walk") => accent_light,
        Some("Sleep") => SLEEP_BLUE,
        Some("Climb") => accent_light,
        Some("Falling" | "Dragged" | "Landing" | "Bonk" | "Roll") => AMBER,
        Some(_) => MUTED,
    }
}

/// Локализованная подпись пресета цвета по машинному имени из
/// [`PET_PRESETS`]; незнакомое имя показывается как есть (не падаем).
pub fn preset_label(name: &str) -> String {
    match name {
        "greige" => fl!("color-greige"),
        "amber" => fl!("color-amber"),
        "mint" => fl!("color-mint"),
        "sky" => fl!("color-sky"),
        "rose" => fl!("color-rose"),
        "slate" => fl!("color-slate"),
        "sand" => fl!("color-sand"),
        "violet" => fl!("color-violet"),
        other => other.to_string(),
    }
}

/// Ключ Fluent подписи стадии роста — чистое отображение, тесты сверяют
/// его с fl!-рендером каждого ключа (compile-time проверка ключей).
pub fn stage_key(stage: Stage) -> &'static str {
    match stage {
        Stage::Egg => "stage-egg",
        Stage::Baby => "stage-baby",
        Stage::Child => "stage-child",
        Stage::Teen => "stage-teen",
        Stage::Adult => "stage-adult",
    }
}

/// Локализованная подпись стадии роста для чипа на карточке питомца.
pub fn stage_label(stage: Stage) -> String {
    crate::i18n::loader().get(stage_key(stage))
}

/// Цвет бара стата ухода (0..=100): выше 60 — зелёный, 30..=60 — янтарный,
/// ниже 30 — красный (пора ухаживать).
pub fn stat_bar_color(v: f32) -> Color32 {
    if v > 60.0 {
        SUCCESS
    } else if v >= 30.0 {
        AMBER
    } else {
        DANGER
    }
}

/// Клиентская валидация нового имени: непустое после trim (демон делает
/// то же самое, но мы не хотим гонять заведомо пустой запрос).
pub fn valid_pet_name(name: &str) -> Option<&str> {
    let trimmed = name.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Аптайм демона в человекочитаемом виде: плюральные формы — по правилам
/// CLDR через Fluent («1 минута / 2 минуты / 5 минут», ТД-30).
pub fn format_uptime(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!(
            "{} {}",
            fl!("uptime-hours", hours = h),
            fl!("uptime-minutes", minutes = m)
        )
    } else if m > 0 {
        format!(
            "{} {}",
            fl!("uptime-minutes", minutes = m),
            fl!("uptime-seconds", seconds = s)
        )
    } else {
        fl!("uptime-seconds", seconds = s)
    }
}

/// Человеческое имя вещи мира по машинному (швабра, лежанка, таракан…).
/// Неизвестное имя (демон новее окна) показывается как есть — это лучше,
/// чем «нет перевода».
pub fn prop_label(kind: &str) -> String {
    match kind {
        "mop" => fl!("prop-mop"),
        "bed" => fl!("prop-bed"),
        "ball" => fl!("prop-ball"),
        "dustball" => fl!("prop-dustball"),
        "roach" => fl!("prop-roach"),
        "bug" => fl!("prop-bug"),
        other => other.to_string(),
    }
}

/// Доля 0..1 для шкалы: значение вне диапазона прижимается к краю, а не
/// уезжает за него.
pub fn norm(v: f32, min: f32, max: f32) -> f32 {
    if max <= min {
        return 0.0;
    }
    ((v - min) / (max - min)).clamp(0.0, 1.0)
}
