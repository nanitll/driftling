//! Живая проверка KWin-провайдера на настоящей сессии KDE Plasma 6.
//!
//! По умолчанию тест выключен (юнит-тесты крейта не требуют KDE); запуск
//! руками на живой сессии:
//!
//! ```text
//! cargo test -p driftling-worldsense --test live -- --ignored --nocapture
//! ```
//!
//! Тест read-only по отношению к рабочему столу: грузит сенсорный скрипт,
//! читает снапшоты, печатает статистику и выгружает скрипт (Drop провайдера).

use std::time::{Duration, Instant};

#[test]
#[ignore = "требует живую сессию KDE Plasma 6 (грузит скрипт в KWin)"]
fn live_kwin_snapshot_stats() {
    let t0 = Instant::now();
    let mut sense = driftling_worldsense::detect();

    // Ждём первый снапшот (загрузка скрипта + стартовый push).
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut first: Option<driftling_worldsense::WorldSnapshot> = None;
    while Instant::now() < deadline {
        if let Some(s) = sense.latest() {
            first = Some(s);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let ttfs = t0.elapsed();
    let snap = first.expect(
        "снапшот не пришёл за 10 с — это точно живая сессия KDE Plasma 6 \
         и driftling не запущен вторым экземпляром?",
    );

    println!("live: первый снапшот через {ttfs:?}");
    println!(
        "live: платформ={} workspace_bottom={:?} fullscreen_active={}",
        snap.platforms.len(),
        snap.workspace_bottom,
        snap.fullscreen_active
    );
    for (i, p) in snap.platforms.iter().enumerate() {
        println!(
            "live:   [{i}] id={:016x} rect={:.0},{:.0} {:.0}x{:.0}",
            p.id, p.rect.x, p.rect.y, p.rect.w, p.rect.h
        );
    }

    // Наблюдаем поток обновлений: heartbeat идёт раз в 5 с, значит за 12 с
    // содержимое может и не поменяться, но провайдер обязан оставаться живым.
    let mut content_changes = 0u32;
    let mut last = snap.clone();
    let t1 = Instant::now();
    while t1.elapsed() < Duration::from_secs(12) {
        match sense.latest() {
            Some(s) => {
                if s != last {
                    content_changes += 1;
                    println!(
                        "live: изменение #{content_changes} через {:?} (платформ={})",
                        t1.elapsed(),
                        s.platforms.len()
                    );
                    last = s;
                }
            }
            None => panic!("провайдер умер посреди наблюдения"),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("live: изменений содержимого за 12 с: {content_changes}");
    // Drop провайдера выгружает скрипт из KWin.
}
