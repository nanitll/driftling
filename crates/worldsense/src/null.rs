//! Null-провайдер: данных о мире нет, питомец живёт на нижней кромке экрана.
//! Это штатная деградация для неподдержанных окружений, не ошибка.

use crate::{WorldSense, WorldSnapshot};

pub(crate) struct NullSense;

impl WorldSense for NullSense {
    fn latest(&mut self) -> Option<WorldSnapshot> {
        None
    }
}
