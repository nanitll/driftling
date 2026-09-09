//! Локализация крейта (ТД-30): Fluent через i18n-embed.
//!
//! Ассеты вшиты в бинарь (`i18n/{en,ru}/driftling.ftl`), язык берётся
//! из локали ОС (DesktopLanguageRequester), фолбэк-цепочка ru -> en;
//! en обязан быть полным. Ключи проверяются на этапе компиляции
//! макросом `fl!` (i18n-embed-fl) по en-файлу.
//!
//! Локализуется только UI-вывод: ctl/doctor и IPC-ошибки демона.
//! Строки журнала демона остаются русскими — логи не UI.

use i18n_embed::fluent::{fluent_language_loader, FluentLanguageLoader};
use i18n_embed::{DesktopLanguageRequester, LanguageLoader};
use rust_embed::RustEmbed;
use std::sync::LazyLock;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

static LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader = fluent_language_loader!();
    let requested = DesktopLanguageRequester::requested_languages();
    // select() сам грузит фолбэк-язык последним в цепочке; ошибка выбора
    // (пустая/кривая локаль) не должна ронять процесс — остаёмся на en.
    if i18n_embed::select(&loader, &Localizations, &requested).is_err() {
        let _ = i18n_embed::select(
            &loader,
            &Localizations,
            &[loader.fallback_language().clone()],
        );
    }
    // Без Unicode-изоляторов вокруг подстановок: вывод идёт в терминал,
    // U+2068/U+2069 там только мешают.
    loader.set_use_isolating(false);
    loader
});

/// Статический загрузчик переводов крейта.
pub(crate) fn loader() -> &'static FluentLanguageLoader {
    &LOADER
}

/// `fl!("ключ", арг = знач, ...)` — локализованная строка с compile-time
/// проверкой ключа и аргументов по `i18n/en/driftling.ftl`.
macro_rules! fl {
    ($message_id:literal $(, $($rest:tt)*)?) => {
        i18n_embed_fl::fl!(crate::i18n::loader(), $message_id $(, $($rest)*)?)
    };
}
pub(crate) use fl;

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
        let en = keys("en/driftling.ftl");
        let ru = keys("ru/driftling.ftl");
        let missing: Vec<&String> = en.difference(&ru).collect();
        assert!(missing.is_empty(), "нет русских строк для: {missing:?}");
        let extra: Vec<&String> = ru.difference(&en).collect();
        assert!(extra.is_empty(), "лишние русские строки: {extra:?}");
    }
}
