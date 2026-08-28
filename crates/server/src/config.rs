//! Конфиг сервера: один TOML-файл (модель atuin/PocketBase, ТЗ §3.5).
//!
//! TLS здесь нет намеренно — сервер слушает плоский HTTP, шифрование
//! вешает reverse-proxy (Caddy/nginx), см. dist/server/README.md.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

/// Содержимое конфиг-файла. Все поля имеют дефолты — пустой файл валиден.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Адрес прослушивания, `ip:порт`. Дефолт — только localhost:
    /// наружу сервер выставляет reverse-proxy, не мы.
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Путь к базе SQLite (WAL). Относительный путь разрешается
    /// от каталога конфиг-файла, а не от cwd процесса.
    #[serde(default = "default_db")]
    pub db: PathBuf,
}

fn default_listen() -> String {
    "127.0.0.1:8787".to_string()
}

fn default_db() -> PathBuf {
    PathBuf::from("driftling-server.sqlite3")
}

impl Config {
    /// Прочитать конфиг; отсутствующий файл — ошибка (опечатка в пути
    /// не должна молча приводить к дефолтам и пустой базе не там).
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("не удалось прочитать конфиг {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text)
            .with_context(|| format!("не удалось разобрать конфиг {}", path.display()))?;
        if cfg.db.is_relative() {
            if let Some(dir) = path.parent() {
                cfg.db = dir.join(&cfg.db);
            }
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_gets_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.listen, "127.0.0.1:8787");
        assert_eq!(cfg.db, PathBuf::from("driftling-server.sqlite3"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        // Опечатка в ключе — ошибка, а не молчаливый дефолт.
        assert!(toml::from_str::<Config>("listne = \"1.2.3.4:1\"").is_err());
    }
}
