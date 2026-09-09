//! Страница «Мир»: правила, по которым живёт мир вещей.
//!
//! Всё здесь — локальные ключи `config.toml` (секции `[game]`, `[world]`,
//! `[comfort]`): они НЕ уезжают на другие устройства, о чём сказано
//! подписью. Каждая ручка применяется сразу — «Сохранить» нет.

use driftling_core::PropKind;
use driftling_ipc::Request;
use eframe::egui::{self, RichText};

use crate::app::SettingsApp;
use crate::i18n::fl;
use crate::labels::prop_label;
use crate::theme::{
    self, card, card_title, chip, hint, outline_button, primary_button, row_sep, setting_row,
    toggle_switch, MUTED,
};

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    theme::page_title(ui, &fl!("nav-world"));
    ui.add_space(4.0);
    if !app.daemon_up() {
        hint(ui, &fl!("world-offline-note"));
        ui.add_space(6.0);
    }

    peace(app, ui);
    guests(app, ui);
    rides(app, ui);
    things(app, ui);
    mess(app, ui);
    ui.add_space(2.0);
    hint(ui, &fl!("world-local-note"));
}

/// Покой: тихий час и вежливость к полноэкранным окнам.
fn peace(app: &mut SettingsApp, ui: &mut egui::Ui) {
    card(ui, |ui| {
        card_title(ui, &fl!("section-peace"));
        let mut changed = false;
        let mut quiet = app.cfg.comfort.quiet;
        setting_row(
            ui,
            &fl!("world-quiet"),
            Some(&fl!("world-quiet-hint")),
            |ui| {
                if toggle_switch(ui, &mut quiet, app.accent).changed() {
                    app.cfg.comfort.quiet = quiet;
                    changed = true;
                }
            },
        );
        row_sep(ui);
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
        if hide {
            let mut grace = app.cfg.comfort.fullscreen_grace_secs;
            setting_row(
                ui,
                &fl!("world-grace"),
                Some(&fl!("world-grace-hint")),
                |ui| {
                    let r = ui.add(egui::Slider::new(&mut grace, 0.2..=5.0).suffix(" с"));
                    if r.drag_stopped() || r.lost_focus() {
                        app.cfg.comfort.fullscreen_grace_secs = grace;
                        changed = true;
                    } else if r.changed() {
                        app.cfg.comfort.fullscreen_grace_secs = grace;
                    }
                },
            );
        }
        row_sep(ui);
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
        row_sep(ui);
        let mut surprises = app.cfg.comfort.surprises;
        setting_row(
            ui,
            &fl!("world-surprises"),
            Some(&fl!("world-surprises-hint")),
            |ui| {
                let r = ui.add(egui::Slider::new(&mut surprises, 0.0..=2.0).step_by(0.05));
                if r.drag_stopped() || r.lost_focus() {
                    app.cfg.comfort.surprises = surprises;
                    changed = true;
                } else if r.changed() {
                    app.cfg.comfort.surprises = surprises;
                }
            },
        );
        if changed {
            let patch = app.world_patch();
            app.apply_patch(ui, patch);
        }
    });
}

/// Гости: режим войны.
fn guests(app: &mut SettingsApp, ui: &mut egui::Ui) {
    card(ui, |ui| {
        card_title(ui, &fl!("section-guests"));
        let quiet = app.cfg.comfort.quiet;
        let mut changed = false;
        let mut war = app.cfg.game.war_mode;
        setting_row(ui, &fl!("world-war"), Some(&fl!("world-war-hint")), |ui| {
            if toggle_switch(ui, &mut war, app.accent).changed() {
                app.cfg.game.war_mode = war;
                changed = true;
            }
        });
        if quiet && war {
            hint(ui, &fl!("world-muted-by-quiet"));
        }
        if war {
            let mut every = app.cfg.game.mob_every_mins;
            setting_row(ui, &fl!("world-mob-every"), None, |ui| {
                let r = ui.add(egui::Slider::new(&mut every, 2.0..=120.0).suffix(" мин"));
                if r.drag_stopped() || r.lost_focus() {
                    app.cfg.game.mob_every_mins = every;
                    changed = true;
                } else if r.changed() {
                    app.cfg.game.mob_every_mins = every;
                }
            });
            changed |= kinds_row(
                app,
                ui,
                &fl!("world-mob-kinds"),
                &PropKind::MOBS,
                Kind::Mobs,
            );
        }
        row_sep(ui);
        ui.horizontal(|ui| {
            let alive = app.daemon_up() && !app.busy();
            if outline_button(ui, &fl!("btn-summon-guest"), app.accent_light, alive).clicked() {
                app.command(ui, Request::Mob { kind: None }, fl!("msg-guest-called"));
            }
            hint(ui, &fl!("world-guest-manual-hint"));
        });
        if changed {
            let patch = app.world_patch();
            app.apply_patch(ui, patch);
        }
    });
}

/// Транспорт.
fn rides(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let riding = {
        let st = app.poll.lock().unwrap();
        st.world.as_ref().and_then(|w| w.ride.clone())
    };
    card(ui, |ui| {
        card_title(ui, &fl!("section-rides"));
        let mut changed = false;
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
        if app.cfg.comfort.quiet && auto {
            hint(ui, &fl!("world-muted-by-quiet"));
        }
        if auto {
            let mut every = app.cfg.game.ride_every_mins;
            setting_row(ui, &fl!("world-ride-every"), None, |ui| {
                let r = ui.add(egui::Slider::new(&mut every, 2.0..=120.0).suffix(" мин"));
                if r.drag_stopped() || r.lost_focus() {
                    app.cfg.game.ride_every_mins = every;
                    changed = true;
                } else if r.changed() {
                    app.cfg.game.ride_every_mins = every;
                }
            });
        }
        changed |= kinds_row(
            app,
            ui,
            &fl!("world-ride-kinds"),
            &PropKind::VEHICLES,
            Kind::Rides,
        );
        row_sep(ui);
        ui.horizontal(|ui| {
            let alive = app.daemon_up() && !app.busy();
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
            if let Some(kind) = &riding {
                hint(ui, &fl!("world-riding-now", kind = prop_label(kind)));
            }
        });
        if changed {
            let patch = app.world_patch();
            app.apply_patch(ui, patch);
        }
    });
}

/// Вещи: что заводится само, домик и мяч, список сцены.
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
        row_sep(ui);
        // Домик — и настройка, и команда: выключенный флаг не даёт ему
        // вернуться при ближайшей свёртке журнала, а вещь со сцены надо
        // убрать отдельно.
        let mut house = app.cfg.world.auto_house;
        setting_row(
            ui,
            &fl!("world-house"),
            Some(&fl!("world-house-hint")),
            |ui| {
                if toggle_switch(ui, &mut house, app.accent).changed() {
                    app.cfg.world.auto_house = house;
                    changed = true;
                    if app.daemon_up() {
                        let req = if house {
                            Request::PlaceProp {
                                kind: "house".into(),
                            }
                        } else {
                            Request::TakeProp {
                                kind: "house".into(),
                            }
                        };
                        if house || has_house {
                            app.command(ui, req, fl!("msg-settings-applied"));
                        }
                    }
                }
            },
        );
        if house {
            let mut days = app.cfg.world.house_age_days;
            setting_row(ui, &fl!("world-house-age"), None, |ui| {
                let r = ui.add(
                    egui::Slider::new(&mut days, 0.0..=14.0)
                        .step_by(0.5)
                        .suffix(" сут"),
                );
                if r.drag_stopped() || r.lost_focus() {
                    app.cfg.world.house_age_days = days;
                    changed = true;
                } else if r.changed() {
                    app.cfg.world.house_age_days = days;
                }
            });
        }
        row_sep(ui);
        let mut ball = has_ball;
        setting_row(
            ui,
            &fl!("world-ball"),
            Some(&fl!("world-ball-hint")),
            |ui| {
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
            },
        );
        row_sep(ui);
        ui.label(RichText::new(fl!("world-scene")).size(13.5).color(MUTED));
        ui.add_space(2.0);
        if props.is_empty() {
            hint(ui, &fl!("world-scene-empty"));
        } else {
            for p in &props {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(prop_label(&p.kind)).size(13.0));
                    let tag = if p.mob {
                        fl!("world-tag-guest")
                    } else if p.persistent {
                        fl!("world-tag-kept")
                    } else {
                        fl!("world-tag-temporary")
                    };
                    ui.label(RichText::new(tag).size(12.0).color(MUTED));
                });
            }
        }
        if changed {
            let patch = app.world_patch();
            app.apply_patch(ui, patch);
        }
    });
}

/// Беспорядок: лужи, чихи, мусор, тень.
fn mess(app: &mut SettingsApp, ui: &mut egui::Ui) {
    card(ui, |ui| {
        card_title(ui, &fl!("section-mess"));
        let mut changed = false;
        for (label, hint_key, value) in [
            (fl!("world-puddles"), fl!("world-puddles-hint"), 0),
            (fl!("world-sneezes"), fl!("world-sneezes-hint"), 1),
            (fl!("world-litter"), fl!("world-litter-hint"), 2),
            (fl!("world-shadow"), fl!("world-shadow-hint"), 3),
        ] {
            let mut on = match value {
                0 => app.cfg.world.puddles,
                1 => app.cfg.world.sneezes,
                2 => app.cfg.world.litter,
                _ => app.cfg.world.shadow,
            };
            setting_row(ui, &label, Some(&hint_key), |ui| {
                if toggle_switch(ui, &mut on, app.accent).changed() {
                    match value {
                        0 => app.cfg.world.puddles = on,
                        1 => app.cfg.world.sneezes = on,
                        2 => app.cfg.world.litter = on,
                        _ => app.cfg.world.shadow = on,
                    }
                    changed = true;
                }
            });
        }
        row_sep(ui);
        let mut delay = app.cfg.world.chore_delay_secs;
        setting_row(
            ui,
            &fl!("world-chore"),
            Some(&fl!("world-chore-hint")),
            |ui| {
                let r = ui.add(egui::Slider::new(&mut delay, 1.0..=60.0).suffix(" с"));
                if r.drag_stopped() || r.lost_focus() {
                    app.cfg.world.chore_delay_secs = delay;
                    changed = true;
                } else if r.changed() {
                    app.cfg.world.chore_delay_secs = delay;
                }
            },
        );
        if changed {
            let patch = app.world_patch();
            app.apply_patch(ui, patch);
        }
    });
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
