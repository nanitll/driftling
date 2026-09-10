//! Страница «Мир»: правила, по которым живёт мир вещей.
//!
//! Всё здесь — локальные ключи `config.toml` (секции `[game]`, `[world]`,
//! `[comfort]`): они НЕ уезжают на другие устройства. Каждая ручка
//! применяется сразу — «Сохранить» нет.
//!
//! На виду — то, что человек трогает: шесть переключателей и три кнопки.
//! Частоты, списки видов и мелкая уборка — под «Подробнее»: настройка,
//! которую крутят раз в жизни, не должна занимать место каждый день.

use driftling_core::PropKind;
use driftling_ipc::Request;
use eframe::egui::{self, RichText};

use crate::app::SettingsApp;
use crate::i18n::fl;
use crate::labels::prop_label;
use crate::theme::{
    self, card, card_title, chip, details, hint, outline_button, primary_button, row_sep,
    setting_row, setting_row_sub, toggle_switch, MUTED,
};

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    theme::page_title(ui, &fl!("nav-world"));
    ui.add_space(4.0);
    if !app.daemon_up() {
        hint(ui, &fl!("world-offline-note"));
        ui.add_space(6.0);
    }

    peace(app, ui);
    fun(app, ui);
    things(app, ui);
    ui.add_space(2.0);
    hint(ui, &fl!("world-local-note"));
}

/// Покой: как питомец уживается с работой.
fn peace(app: &mut SettingsApp, ui: &mut egui::Ui) {
    card(ui, |ui| {
        card_title(ui, &fl!("section-peace"));
        let mut changed = false;

        let mut quiet = app.cfg.comfort.quiet;
        setting_row_sub(ui, &fl!("world-quiet"), &fl!("world-quiet-hint"), |ui| {
            if toggle_switch(ui, &mut quiet, app.accent).changed() {
                app.cfg.comfort.quiet = quiet;
                changed = true;
            }
        });
        let mut hide = app.cfg.comfort.hide_on_fullscreen;
        setting_row(
            ui,
            &fl!("world-hide"),
            Some(&fl!("world-hide-hint")),
            |ui| {
                if toggle_switch(ui, &mut hide, app.accent).changed() {
                    app.cfg.comfort.hide_on_fullscreen = hide;
                    changed = true;
                }
            },
        );

        details(ui, "peace-more", |ui| {
            let mut sick = app.cfg.comfort.motion_sickness;
            setting_row(
                ui,
                &fl!("world-sick"),
                Some(&fl!("world-sick-hint")),
                |ui| {
                    if toggle_switch(ui, &mut sick, app.accent).changed() {
                        app.cfg.comfort.motion_sickness = sick;
                        changed = true;
                    }
                },
            );
            if hide {
                let mut grace = app.cfg.comfort.fullscreen_grace_secs;
                setting_row(
                    ui,
                    &fl!("world-grace"),
                    Some(&fl!("world-grace-hint")),
                    |ui| {
                        changed |= slider(ui, &mut grace, 0.2..=5.0, 0.0, &fl!("unit-secs"));
                        app.cfg.comfort.fullscreen_grace_secs = grace;
                    },
                );
            }
            let mut surprises = app.cfg.comfort.surprises;
            setting_row(
                ui,
                &fl!("world-surprises"),
                Some(&fl!("world-surprises-hint")),
                |ui| {
                    changed |= slider(ui, &mut surprises, 0.0..=2.0, 0.05, "");
                    app.cfg.comfort.surprises = surprises;
                },
            );
        });

        if changed {
            let patch = app.world_patch();
            app.apply_patch(ui, patch);
        }
    });
}

/// Развлечения: гости и транспорт — два тумблера и кнопки «сейчас».
fn fun(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let riding = {
        let st = app.poll.lock().unwrap();
        st.world.as_ref().and_then(|w| w.ride.clone())
    };
    card(ui, |ui| {
        card_title(ui, &fl!("section-fun"));
        let mut changed = false;
        let quiet = app.cfg.comfort.quiet;

        let mut war = app.cfg.game.war_mode;
        setting_row(ui, &fl!("world-war"), Some(&fl!("world-war-hint")), |ui| {
            if toggle_switch(ui, &mut war, app.accent).changed() {
                app.cfg.game.war_mode = war;
                changed = true;
            }
        });
        let mut auto = app.cfg.game.rides;
        setting_row(
            ui,
            &fl!("world-rides"),
            Some(&fl!("world-rides-hint")),
            |ui| {
                if toggle_switch(ui, &mut auto, app.accent).changed() {
                    app.cfg.game.rides = auto;
                    changed = true;
                }
            },
        );
        if quiet && (war || auto) {
            hint(ui, &fl!("world-muted-by-quiet"));
        }

        row_sep(ui);
        let alive = app.daemon_up() && !app.busy();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            if primary_button(
                ui,
                &fl!("btn-ride-now"),
                alive && riding.is_none(),
                app.accent,
            )
            .clicked()
            {
                app.command(ui, Request::Ride { kind: None }, fl!("msg-ride-called"));
            }
            if outline_button(ui, &fl!("btn-stop-ride"), MUTED, alive && riding.is_some()).clicked()
            {
                app.command(ui, Request::StopRide, fl!("msg-ride-stopped"));
            }
            if outline_button(ui, &fl!("btn-summon-guest"), app.accent_light, alive).clicked() {
                app.command(ui, Request::Mob { kind: None }, fl!("msg-guest-called"));
            }
        });
        if let Some(kind) = &riding {
            hint(ui, &fl!("world-riding-now", kind = prop_label(kind)));
        }

        details(ui, "fun-more", |ui| {
            let mut every = app.cfg.game.ride_every_mins;
            setting_row(ui, &fl!("world-ride-every"), None, |ui| {
                changed |= slider(ui, &mut every, 2.0..=120.0, 0.0, &fl!("unit-mins"));
                app.cfg.game.ride_every_mins = every;
            });
            changed |= kinds_row(
                app,
                ui,
                &fl!("world-ride-kinds"),
                &PropKind::VEHICLES,
                Kind::Rides,
            );
            let mut mob_every = app.cfg.game.mob_every_mins;
            setting_row(ui, &fl!("world-mob-every"), None, |ui| {
                changed |= slider(ui, &mut mob_every, 2.0..=120.0, 0.0, &fl!("unit-mins"));
                app.cfg.game.mob_every_mins = mob_every;
            });
            changed |= kinds_row(
                app,
                ui,
                &fl!("world-mob-kinds"),
                &PropKind::MOBS,
                Kind::Mobs,
            );
        });

        if changed {
            let patch = app.world_patch();
            app.apply_patch(ui, patch);
        }
    });
}

/// Вещи: домик и мяч на виду, остальное — под «Подробнее».
fn things(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let (props, has_ball, has_house) = {
        let st = app.poll.lock().unwrap();
        let props = st
            .world
            .as_ref()
            .map(|w| w.props.clone())
            .unwrap_or_default();
        let ball = props.iter().any(|p| p.kind == "ball");
        let house = props.iter().any(|p| p.kind == "house");
        (props, ball, house)
    };
    card(ui, |ui| {
        card_title(ui, &fl!("section-things"));
        let mut changed = false;

        let mut house = app.cfg.world.auto_house;
        setting_row(
            ui,
            &fl!("world-house"),
            Some(&fl!("world-house-hint")),
            |ui| {
                if toggle_switch(ui, &mut house, app.accent).changed() {
                    app.cfg.world.auto_house = house;
                    changed = true;
                    // Флаг и вещь — разные вещи: выключенный флаг не даёт домику
                    // вернуться при свёртке журнала, а со сцены его убирает
                    // отдельная команда.
                    if app.daemon_up() && (house || has_house) {
                        let kind = "house".to_string();
                        let req = if house {
                            Request::PlaceProp { kind }
                        } else {
                            Request::TakeProp { kind }
                        };
                        app.command(ui, req, fl!("msg-settings-applied"));
                    }
                }
            },
        );
        let mut ball = has_ball;
        setting_row_sub(ui, &fl!("world-ball"), &fl!("world-ball-hint"), |ui| {
            let alive = app.daemon_up() && !app.busy();
            ui.add_enabled_ui(alive, |ui| {
                if toggle_switch(ui, &mut ball, app.accent).changed() {
                    app.command(
                        ui,
                        Request::Toy { show: Some(ball) },
                        if ball {
                            fl!("msg-ball-out")
                        } else {
                            fl!("msg-ball-away")
                        },
                    );
                }
            });
        });

        details(ui, "things-more", |ui| {
            let mut bowl = app.cfg.world.auto_bowl;
            setting_row(
                ui,
                &fl!("world-bowl"),
                Some(&fl!("world-bowl-hint")),
                |ui| {
                    if toggle_switch(ui, &mut bowl, app.accent).changed() {
                        app.cfg.world.auto_bowl = bowl;
                        changed = true;
                    }
                },
            );
            let mut bed = app.cfg.world.auto_bed;
            setting_row(ui, &fl!("world-bed"), Some(&fl!("world-bed-hint")), |ui| {
                if toggle_switch(ui, &mut bed, app.accent).changed() {
                    app.cfg.world.auto_bed = bed;
                    changed = true;
                }
            });
            if house {
                let mut days = app.cfg.world.house_age_days;
                setting_row(ui, &fl!("world-house-age"), None, |ui| {
                    changed |= slider(ui, &mut days, 0.0..=14.0, 0.5, &fl!("unit-days"));
                    app.cfg.world.house_age_days = days;
                });
            }
            row_sep(ui);
            for (label, hint_key, which) in [
                (fl!("world-puddles"), fl!("world-puddles-hint"), 0),
                (fl!("world-sneezes"), fl!("world-sneezes-hint"), 1),
                (fl!("world-litter"), fl!("world-litter-hint"), 2),
                (fl!("world-shadow"), fl!("world-shadow-hint"), 3),
            ] {
                let mut on = match which {
                    0 => app.cfg.world.puddles,
                    1 => app.cfg.world.sneezes,
                    2 => app.cfg.world.litter,
                    _ => app.cfg.world.shadow,
                };
                setting_row(ui, &label, Some(&hint_key), |ui| {
                    if toggle_switch(ui, &mut on, app.accent).changed() {
                        match which {
                            0 => app.cfg.world.puddles = on,
                            1 => app.cfg.world.sneezes = on,
                            2 => app.cfg.world.litter = on,
                            _ => app.cfg.world.shadow = on,
                        }
                        changed = true;
                    }
                });
            }
            let mut delay = app.cfg.world.chore_delay_secs;
            setting_row(
                ui,
                &fl!("world-chore"),
                Some(&fl!("world-chore-hint")),
                |ui| {
                    changed |= slider(ui, &mut delay, 1.0..=60.0, 0.0, &fl!("unit-secs"));
                    app.cfg.world.chore_delay_secs = delay;
                },
            );
            row_sep(ui);
            ui.label(RichText::new(fl!("world-scene")).size(13.0).color(MUTED));
            if props.is_empty() {
                hint(ui, &fl!("world-scene-empty"));
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                    for p in &props {
                        let tag = if p.mob {
                            fl!("world-tag-guest")
                        } else if p.persistent {
                            fl!("world-tag-kept")
                        } else {
                            fl!("world-tag-temporary")
                        };
                        ui.label(RichText::new(prop_label(&p.kind)).size(13.0))
                            .on_hover_text(tag);
                    }
                });
            }
        });

        if changed {
            let patch = app.world_patch();
            app.apply_patch(ui, patch);
        }
    });
}

/// Слайдер, который применяет значение по отпусканию, а не на каждом
/// пикселе: иначе одно перетаскивание — сотня записей конфига.
fn slider(
    ui: &mut egui::Ui,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    step: f64,
    unit: &str,
) -> bool {
    let mut s = egui::Slider::new(value, range);
    if step > 0.0 {
        s = s.step_by(step);
    }
    if !unit.is_empty() {
        s = s.suffix(format!(" {unit}"));
    }
    let r = ui.add(s);
    r.drag_stopped() || r.lost_focus()
}

/// Какой список видов правим.
#[derive(Clone, Copy)]
enum Kind {
    Rides,
    Mobs,
}

/// Строка чипов «какие виды допущены»: пустой список = все.
fn kinds_row(
    app: &mut SettingsApp,
    ui: &mut egui::Ui,
    label: &str,
    all: &[PropKind],
    which: Kind,
) -> bool {
    let mut changed = false;
    let list = match which {
        Kind::Rides => app.cfg.game.ride_kinds.clone(),
        Kind::Mobs => app.cfg.game.mob_kinds.clone(),
    };
    let empty = list.is_empty();
    setting_row(ui, label, Some(&fl!("world-kinds-hint")), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
            for kind in all {
                let name = kind.as_str();
                // Пустой список означает «все», поэтому все чипы горят.
                let on = empty || list.iter().any(|n| n == name);
                if chip(ui, &prop_label(name), on, app.accent).clicked() {
                    let mut next: Vec<String> = if empty {
                        all.iter().map(|k| k.as_str().to_string()).collect()
                    } else {
                        list.clone()
                    };
                    if on {
                        next.retain(|n| n != name);
                    } else {
                        next.push(name.to_string());
                    }
                    // Сняли все — это снова «все», а не «ни одного»: иначе
                    // случайный выбор остался бы без вариантов.
                    if next.len() == all.len() || next.is_empty() {
                        next.clear();
                    }
                    match which {
                        Kind::Rides => app.cfg.game.ride_kinds = next,
                        Kind::Mobs => app.cfg.game.mob_kinds = next,
                    }
                    changed = true;
                }
            }
        });
    });
    changed
}
