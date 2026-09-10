//! Страница «Продвинутые»: единственное место с техническими терминами и
//! сырыми данными. Открывается настройкой окна или флагом `--debug`.

use driftling_core::PropKind;
use driftling_ipc::Request;
use eframe::egui::{self, RichText};

use crate::app::SettingsApp;
use crate::i18n::fl;
use crate::labels::{prop_label, stat_bar_color};
use crate::theme::{
    self, card, card_title, hint, meter, outline_button, segmented, setting_row, AMBER, MUTED,
};

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    theme::page_title(ui, &fl!("nav-advanced"));
    ui.add_space(2.0);
    ui.label(
        RichText::new(fl!("advanced-warning"))
            .size(12.5)
            .color(AMBER),
    );
    ui.add_space(6.0);
    physics(app, ui);
    raw(app, ui);
    dangerous(app, ui);
}

/// Масштаб мира: рост питомца «в жизни».
fn physics(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let room = {
        let st = app.poll.lock().unwrap();
        st.world.as_ref().and_then(|w| w.screen)
    };
    card(ui, |ui| {
        card_title(ui, &fl!("section-physics"));
        let mut cm = app.cfg.physics.pet_height_cm;
        setting_row(
            ui,
            &fl!("physics-height"),
            Some(&fl!("physics-height-hint")),
            |ui| {
                let r = ui.add(egui::Slider::new(&mut cm, 5.0..=100.0).suffix(" см"));
                if r.drag_stopped() || r.lost_focus() {
                    app.cfg.physics.pet_height_cm = cm;
                    let patch = driftling_core::ConfigPatch {
                        physics: Some(app.cfg.physics),
                        ..Default::default()
                    };
                    app.apply_patch(ui, patch);
                } else if r.changed() {
                    app.cfg.physics.pet_height_cm = cm;
                }
            },
        );
        // Пиксели в метре считаются из размера питомца и его роста «в
        // жизни» — та же формула, что в ядре.
        if let Some((_, _, w, h)) = room {
            let size = {
                let st = app.poll.lock().unwrap();
                st.info
                    .as_ref()
                    .map(|i| i.attributes.size as f32)
                    .unwrap_or(96.0)
            };
            let px_per_m = size / (cm.max(1.0) as f32 / 100.0);
            hint(
                ui,
                &fl!(
                    "physics-room",
                    w = format!("{:.1}", w / px_per_m),
                    h = format!("{:.1}", h / px_per_m)
                ),
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if outline_button(ui, &fl!("btn-reset-default"), MUTED, true).clicked() {
                app.cfg.physics = driftling_core::PhysicsConfig::default();
                let patch = driftling_core::ConfigPatch {
                    physics: Some(app.cfg.physics),
                    ..Default::default()
                };
                app.apply_patch(ui, patch);
            }
            if outline_button(ui, &fl!("btn-reload"), MUTED, app.daemon_up()).clicked() {
                app.command(ui, Request::Reload, fl!("msg-settings-applied"));
            }
        });
    });
}

/// Сырые данные: скрытый стат и ответы демона как есть.
fn raw(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let (health, raw, world) = {
        let st = app.poll.lock().unwrap();
        (
            st.info.as_ref().map(|i| i.stats.health),
            st.raw.clone(),
            st.world.clone(),
        )
    };
    card(ui, |ui| {
        card_title(ui, &fl!("section-raw"));
        if let Some(h) = health {
            setting_row(
                ui,
                &fl!("stat-health"),
                Some(&fl!("stat-health-hint")),
                |ui| {
                    ui.label(RichText::new(format!("{h:.0}")).size(13.0).color(MUTED));
                    meter(ui, h, stat_bar_color(h));
                },
            );
        }
        if let Some(w) = &world {
            setting_row(ui, &fl!("world-screen"), None, |ui| {
                let text = match w.screen {
                    Some((x, y, sw, sh)) => format!("{sw:.0}×{sh:.0} @ ({x:.0}, {y:.0})"),
                    None => fl!("world-screen-unknown"),
                };
                ui.label(RichText::new(text).size(13.0).color(MUTED));
            });
            if w.fullscreen_hidden {
                hint(ui, &fl!("world-hidden-now"));
            }
        }
        if let Some(text) = raw {
            egui::CollapsingHeader::new(RichText::new(fl!("raw-petinfo")).size(13.0).color(MUTED))
                .id_salt("raw-petinfo")
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(text)
                            .monospace()
                            .size(11.5)
                            .color(theme::TEXT),
                    );
                });
        }
    });
}

/// Опасное: точечные вызовы в обход настроек.
fn dangerous(app: &mut SettingsApp, ui: &mut egui::Ui) {
    card(ui, |ui| {
        card_title(ui, &fl!("section-dangerous"));
        let alive = app.daemon_up() && !app.busy();
        let rides: Vec<String> = PropKind::VEHICLES
            .iter()
            .map(|k| prop_label(k.as_str()))
            .collect();
        setting_row(
            ui,
            &fl!("dangerous-ride"),
            Some(&fl!("dangerous-ride-hint")),
            |ui| {
                ui.add_enabled_ui(alive, |ui| {
                    if let Some(i) = segmented(ui, &rides, None, app.accent) {
                        let kind = PropKind::VEHICLES[i].as_str().to_string();
                        app.command(
                            ui,
                            Request::Ride { kind: Some(kind) },
                            fl!("msg-ride-called"),
                        );
                    }
                });
            },
        );
        let mobs: Vec<String> = PropKind::MOBS
            .iter()
            .map(|k| prop_label(k.as_str()))
            .collect();
        setting_row(
            ui,
            &fl!("dangerous-mob"),
            Some(&fl!("dangerous-mob-hint")),
            |ui| {
                ui.add_enabled_ui(alive, |ui| {
                    if let Some(i) = segmented(ui, &mobs, None, app.accent) {
                        let kind = PropKind::MOBS[i].as_str().to_string();
                        app.command(
                            ui,
                            Request::Mob { kind: Some(kind) },
                            fl!("msg-guest-called"),
                        );
                    }
                });
            },
        );
    });
}
