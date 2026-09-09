//! Запись настроек: обычный путь — через демона, запасной — прямо в файл.
//!
//! Демон предпочтительный, но НЕ единственный писатель `config.toml`: окно
//! чаще всего открывают как раз тогда, когда питомца на экране нет, и
//! настройки в этот момент должны редактироваться, а не гаснуть.

use driftling_core::{Config, ConfigPatch};

/// Итог записи в файл мимо демона.
pub struct OfflineSave {
    /// Какие секции реально изменились (машинные имена) — попадают в
    /// подпись «применится при запуске».
    #[allow(dead_code)]
    pub applied: Vec<String>,
    /// Битый конфиг переименован сюда, чтобы правки человека не пропали
    /// молча под дефолтами.
    pub rescued: Option<String>,
}

/// Прочитать-изменить-записать: патч ложится поверх того, что в файле, а
/// не поверх дефолтов. Непарсящийся файл не затирается, а отодвигается в
/// сторону с понятным именем.
pub fn save_offline(patch: &ConfigPatch) -> Result<OfflineSave, String> {
    save_offline_at(&driftling_core::config::path(), patch)
}

/// То же с явным файлом — тестам не нужно трогать окружение процесса.
pub fn save_offline_at(path: &std::path::Path, patch: &ConfigPatch) -> Result<OfflineSave, String> {
    let path = path.to_path_buf();
    let (mut cfg, rescued) = match Config::load_at(&path) {
        Ok(c) => (c, None),
        Err(_) if path.exists() => {
            let backup = path.with_extension("toml.broken");
            std::fs::rename(&path, &backup).map_err(|e| e.to_string())?;
            (
                Config::default(),
                Some(
                    backup
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                ),
            )
        }
        Err(e) => return Err(e),
    };
    let applied = patch.apply_to(&mut cfg);
    cfg.save_at(&path)?;
    Ok(OfflineSave { applied, rescued })
}

/// Прочитать настройки из файла — стартовое состояние форм, пока демон не
/// ответил (и единственный источник, если он не ответит вовсе).
pub fn load_or_default() -> Config {
    Config::load().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use driftling_core::{GameConfig, SyncPatch};

    /// Свой файл на тест: никаких переменных окружения, значит никаких
    /// гонок между тестами в одном процессе.
    fn sandbox(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("driftling-cfgio-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    /// Патч ложится поверх ФАЙЛА, а не поверх дефолтов: соседние секции
    /// и токен синка переживают правку одной галочки.
    #[test]
    fn offline_patch_keeps_the_rest_of_the_file() {
        let path = sandbox("keep");
        let mut seed = Config::default();
        seed.sync.token = "секрет".into();
        seed.physics.pet_height_cm = 42.0;
        seed.save_at(&path).unwrap();

        let patch = ConfigPatch {
            game: Some(GameConfig {
                war_mode: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let saved = save_offline_at(&path, &patch).unwrap();
        assert_eq!(saved.applied, vec!["game".to_string()]);
        assert!(saved.rescued.is_none());

        let on_disk = Config::load_at(&path).unwrap();
        assert!(on_disk.game.war_mode);
        assert_eq!(on_disk.sync.token, "секрет", "токен цел");
        assert_eq!(on_disk.physics.pet_height_cm, 42.0, "физика цела");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Пустой токен в патче не затирает настоящий: `None` значит «не
    /// менять» — иначе первое же сохранение чего угодно убило бы синк.
    #[test]
    fn empty_token_means_keep() {
        let path = sandbox("token");
        let mut seed = Config::default();
        seed.sync.token = "живой".into();
        seed.save_at(&path).unwrap();

        let patch = ConfigPatch {
            sync: Some(SyncPatch {
                address: Some("http://example".into()),
                token: None,
                ..Default::default()
            }),
            ..Default::default()
        };
        save_offline_at(&path, &patch).unwrap();
        let on_disk = Config::load_at(&path).unwrap();
        assert_eq!(on_disk.sync.address, "http://example");
        assert_eq!(on_disk.sync.token, "живой");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Непарсящийся конфиг не затирается дефолтами, а отодвигается в
    /// сторону: правки человека не должны пропадать молча.
    #[test]
    fn broken_config_is_rescued_not_overwritten() {
        let path = sandbox("broken");
        std::fs::write(&path, "это не toml [[[").unwrap();

        let saved = save_offline_at(&path, &ConfigPatch::default()).unwrap();
        let rescued = saved.rescued.expect("битый файл сохранён");
        assert!(rescued.contains("broken"), "имя копии: {rescued}");
        let backup = path.with_file_name(&rescued);
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "это не toml [[[");
        assert!(Config::load_at(&path).is_ok(), "новый файл читается");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
