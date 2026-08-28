//! Supervision-цикл, общий для бэкендов (ТД-2): одна «попытка» = живая
//! сессия у дисплей-сервера; её смерть не убивает демон, а доставляет
//! приложению [`Event::OutputLost`] и пересоздаёт сессию по лестнице бэкоффа.
//! Если ПЕРВАЯ попытка не смогла даже подняться (нет дисплея/протоколов) —
//! возвращается ошибка: ретраи тут бессмысленны, а внешний рестарт при гонке
//! старта сессии — забота systemd-юнита (Restart=on-failure).

use std::time::{Duration, Instant};

use crate::{App, Event, Pace};

pub(crate) const ACTIVE_TICK: Duration = Duration::from_millis(33);
pub(crate) const CALM_TICK: Duration = Duration::from_millis(200);
pub(crate) const DROWSY_TICK: Duration = Duration::from_millis(1000);

/// Лестница пауз перед переподключением (ТД-2).
const BACKOFF_STEPS: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];
/// Сессия, прожившая дольше этого, считается стабильной — лестница заново.
const BACKOFF_STABLE: Duration = Duration::from_secs(60);
/// Шаг дренажа очередей приложения во время паузы переподключения:
/// `ctl status`/`ctl quit` остаются отзывчивыми, пока дисплея нет.
const LOST_TICK: Duration = Duration::from_millis(250);

/// Период тика для темпа приложения (энергобюджет ТЗ §7, ТД-3).
pub(crate) fn pace_delay(pace: Pace) -> Duration {
    match pace {
        Pace::Active => ACTIVE_TICK,
        Pace::Calm => CALM_TICK,
        Pace::Drowsy => DROWSY_TICK,
    }
}

/// Итог одной попытки.
pub(crate) enum Outcome {
    /// Приложение попросило выход — завершаемся по-настоящему.
    Exit,
    /// Живая сессия умерла (ошибка соединения / слой закрыт) — переподключаться.
    Lost(anyhow::Error),
    /// До event loop не дошли (нет соединения/глобалов).
    SetupFailed(anyhow::Error),
}

/// Лестница бэкоффа: 1с → 2с → 5с → 10с → 30с (потолок); попытка, прожившая
/// дольше [`BACKOFF_STABLE`], возвращает лестницу к началу.
pub(crate) struct Backoff {
    step: usize,
}

impl Backoff {
    pub(crate) fn new() -> Self {
        Self { step: 0 }
    }

    /// Пауза после попытки, длившейся `ran`.
    pub(crate) fn after_attempt(&mut self, ran: Duration) -> Duration {
        if ran >= BACKOFF_STABLE {
            self.step = 0;
        }
        let delay = BACKOFF_STEPS[self.step];
        self.step = (self.step + 1).min(BACKOFF_STEPS.len() - 1);
        delay
    }
}

/// Пауза между попытками. Спим ломтиками и продолжаем тикать приложение,
/// чтобы его внешние очереди (IPC) не зависали на всё время бэкоффа.
/// `false` — приложение попросило выход.
fn wait_lost(app: &mut dyn App, clock: Instant, delay: Duration) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        if app.wants_exit() {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(LOST_TICK));
        // Сцена не нужна — тик здесь только ради дренажа очередей.
        let _ = app.tick(clock.elapsed().as_secs_f64());
    }
}

/// Запустить supervision-цикл над `attempt`; возвращается после запроса
/// выхода приложением. `label` — имя бэкенда для логов.
///
/// `attempt` получает приложение и общие монотонные часы, обязана вернуть
/// приложение назад (оно переживает все попытки) вместе с итогом.
pub(crate) fn run(
    app: impl App + 'static,
    label: &str,
    mut attempt: impl FnMut(Box<dyn App>, Instant) -> (Box<dyn App>, Outcome),
) -> anyhow::Result<()> {
    let mut app: Box<dyn App> = Box::new(app);
    // Одни монотонные часы на все попытки: `now` приложения не прыгает
    // и не обнуляется при переподключении.
    let clock = Instant::now();
    let mut backoff = Backoff::new();
    let mut attempt_no: u64 = 0;

    loop {
        attempt_no += 1;
        let attempt_start = Instant::now();
        let (returned, outcome) = attempt(app, clock);
        app = returned;

        let reason = match outcome {
            Outcome::Exit => return Ok(()),
            Outcome::Lost(e) => {
                // Живая сессия оборвалась — приложение ставит симуляцию на
                // паузу (и само решает, что делать с миром).
                let now = clock.elapsed().as_secs_f64();
                if !app.event(Event::OutputLost, now) || app.wants_exit() {
                    return Ok(());
                }
                e
            }
            Outcome::SetupFailed(e) if attempt_no == 1 => return Err(e),
            // Повторный запуск не поднялся (дисплей-сервер ещё стартует) —
            // мы всё ещё в потерянном состоянии, OutputLost уже доставлен.
            Outcome::SetupFailed(e) => e,
        };

        let delay = backoff.after_attempt(attempt_start.elapsed());
        log::warn!(
            "{label}-сессия потеряна (попытка {attempt_no}): {reason:#}; переподключение через {delay:?}"
        );
        if !wait_lost(app.as_mut(), clock, delay) {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pace_delay_matches_budget() {
        assert_eq!(pace_delay(Pace::Active), Duration::from_millis(33));
        assert_eq!(pace_delay(Pace::Calm), Duration::from_millis(200));
        assert_eq!(pace_delay(Pace::Drowsy), Duration::from_millis(1000));
    }

    #[test]
    fn backoff_ladder_caps_and_resets() {
        let mut b = Backoff::new();
        let quick = Duration::from_secs(1); // попытка умерла быстро
        let secs: Vec<u64> = (0..7).map(|_| b.after_attempt(quick).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 5, 10, 30, 30, 30]);
        // Стабильная сессия возвращает лестницу к началу.
        assert_eq!(b.after_attempt(Duration::from_secs(61)).as_secs(), 1);
        assert_eq!(b.after_attempt(quick).as_secs(), 2);
    }
}
