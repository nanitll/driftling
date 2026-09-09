//! Локализация окна настроек (ТД-30): Fluent через i18n-embed.
//!
//! Ассеты вшиты в бинарь (`i18n/{en,ru}/driftling-settings.ftl`), язык
//! берётся из локали ОС (DesktopLanguageRequester), фолбэк-цепочка
//! ru -> en; en обязан быть полным. Ключи проверяются на этапе
//! компиляции макросом `fl!` (i18n-embed-fl) по en-файлу.

use i18n_embed::fluent::{fluent_language_loader, FluentLanguageLoader};
use i18n_embed::{DesktopLanguageRequester, LanguageLoader};
use rust_embed::RustEmbed;
use std::sync::LazyLock;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

static LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader = fluent_language_loader!();
    // Язык окна выбирается в настройках; «как в системе» — обычная
    // цепочка локалей ОС. Читается один раз: Fluent-бандл выбирается на
    // старте, поэтому смена языка честно просит перезапустить окно.
    let requested = match crate::uiprefs::UiPrefs::load().language.code() {
        Some(code) => code
            .parse::<i18n_embed::unic_langid::LanguageIdentifier>()
            .map(|li| vec![li])
            .unwrap_or_else(|_| DesktopLanguageRequester::requested_languages()),
        None => DesktopLanguageRequester::requested_languages(),
    };
    // select() сам грузит фолбэк-язык последним в цепочке; ошибка выбора
    // (пустая/кривая локаль) не должна ронять процесс — остаёмся на en.
    if i18n_embed::select(&loader, &Localizations, &requested).is_err() {
        let _ = i18n_embed::select(
            &loader,
            &Localizations,
            &[loader.fallback_language().clone()],
        );
    }
    // Без Unicode-изоляторов вокруг подстановок — в egui они рисуются
    // как пустые глифы.
    loader.set_use_isolating(false);
    loader
});

/// Статический загрузчик переводов крейта.
pub(crate) fn loader() -> &'static FluentLanguageLoader {
    &LOADER
}

/// `fl!("ключ", арг = знач, ...)` — локализованная строка с compile-time
/// проверкой ключа и аргументов по `i18n/en/driftling-settings.ftl`.
macro_rules! fl {
    ($message_id:literal $(, $($rest:tt)*)?) => {
        i18n_embed_fl::fl!(crate::i18n::loader(), $message_id $(, $($rest)*)?)
    };
}
pub(crate) use fl;

/// Загрузчик конкретного языка — детерминированные проверки плюралов
/// в тестах независимо от локали машины.
#[cfg(test)]
pub(crate) fn loader_for(lang: &str) -> FluentLanguageLoader {
    let li: i18n_embed::unic_langid::LanguageIdentifier = lang.parse().expect("валидный код языка");
    let loader = fluent_language_loader!();
    i18n_embed::select(&loader, &Localizations, &[li]).expect("язык есть в ассетах");
    loader.set_use_isolating(false);
    loader
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ключи файла локали: строки вида `ключ = значение` верхнего уровня.
    fn keys(file: &str) -> std::collections::BTreeSet<String> {
        let asset = Localizations::get(file).expect("ассет локали вшит в бинарь");
        let text = String::from_utf8_lossy(&asset.data);
        text.lines()
            .filter(|l| !l.starts_with([' ', '#', '.', '*', '[']))
            .filter_map(|l| l.split_once(" = "))
            .map(|(k, _)| k.trim().to_string())
            .collect()
    }

    /// Паритет локалей: у каждого ключа из en есть русская строка. Fluent
    /// молча подставляет английскую, и «почти переведённое» окно замечаешь
    /// только на скриншоте от пользователя.
    #[test]
    fn ru_translates_every_en_key() {
        let en = keys("en/driftling-settings.ftl");
        let ru = keys("ru/driftling-settings.ftl");
        let missing: Vec<&String> = en.difference(&ru).collect();
        assert!(missing.is_empty(), "нет русских строк для: {missing:?}");
        let extra: Vec<&String> = ru.difference(&en).collect();
        assert!(extra.is_empty(), "лишние русские строки: {extra:?}");
    }
}
