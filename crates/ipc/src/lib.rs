//! Управляющий сокет: `driftling ctl …` разговаривает с демоном
//! JSON-строками (одна строка = одно сообщение) через unix-сокет
//! в `$XDG_RUNTIME_DIR/driftling.sock`.

use anyhow::{Context, Result};
use driftling_core::PetAttributes;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Summon,
    Dismiss,
    Status,
    /// Полная карточка питомца для настроек: имя, характеристики, состояние.
    PetInfo,
    /// ТОЛЬКО дебаг-панель (ТЗ §3.3): напрямую задать характеристики.
    /// Демон клампит значения, применяет к живому питомцу и персистит.
    SetAttributes(PetAttributes),
    /// Перечитать config.toml (настройки приложения).
    Reload,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Status {
        pets: u32,
        state: String,
        uptime_secs: u64,
    },
    PetInfo {
        name: String,
        /// None = питомец сейчас убран с экрана (dismiss).
        state: Option<String>,
        attributes: PetAttributes,
        uptime_secs: u64,
    },
    Error(String),
}

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("driftling.sock")
}

/// Клиентский вызов: соединиться, отправить запрос, дождаться ответа.
pub fn call(req: &Request) -> Result<Response> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("демон не запущен? нет сокета {}", path.display()))?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    reader.read_line(&mut buf)?;
    Ok(serde_json::from_str(buf.trim())?)
}

/// Слушатель на стороне демона. Блокирующий `accept` — запускать в своём
/// потоке; обработчик получает запрос и возвращает ответ.
pub struct Server {
    listener: UnixListener,
}

impl Server {
    pub fn bind() -> Result<Self> {
        let path = socket_path();
        // Убираем протухший сокет от прошлого запуска.
        if path.exists() && UnixStream::connect(&path).is_err() {
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("не удалось занять сокет {}", path.display()))?;
        Ok(Self { listener })
    }

    /// Обслуживать по одному соединению за раз (для M0 достаточно).
    pub fn serve<F: FnMut(Request) -> Response>(&self, mut handler: F) -> Result<()> {
        for stream in self.listener.incoming() {
            let stream = stream?;
            let mut reader = BufReader::new(&stream);
            let mut buf = String::new();
            if reader.read_line(&mut buf).is_err() {
                continue;
            }
            let resp = match serde_json::from_str::<Request>(buf.trim()) {
                Ok(req) => {
                    let quit = req == Request::Quit;
                    let resp = handler(req);
                    if quit {
                        write_resp(&stream, &resp)?;
                        return Ok(());
                    }
                    resp
                }
                Err(e) => Response::Error(format!("bad request: {e}")),
            };
            write_resp(&stream, &resp)?;
        }
        Ok(())
    }
}

fn write_resp(mut stream: &UnixStream, resp: &Response) -> Result<()> {
    let mut line = serde_json::to_string(resp)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_over_real_socket() {
        // Изолируем путь сокета от реального демона.
        let dir = std::env::temp_dir().join(format!("driftling-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &dir);

        let server = Server::bind().unwrap();
        let t = std::thread::spawn(move || {
            server
                .serve(|req| match req {
                    Request::Status => Response::Status {
                        pets: 1,
                        state: "Idle".into(),
                        uptime_secs: 5,
                    },
                    Request::Quit => Response::Ok,
                    _ => Response::Ok,
                })
                .unwrap();
        });

        match call(&Request::Status).unwrap() {
            Response::Status { pets, .. } => assert_eq!(pets, 1),
            other => panic!("unexpected: {other:?}"),
        }
        assert_eq!(call(&Request::Quit).unwrap(), Response::Ok);
        t.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
