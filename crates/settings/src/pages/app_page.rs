//! Страница «Приложение»: программа, а не питомец. Автозапуск, демон,
//! окно и служебное. Работает всегда — настройки окна лежат в своём файле
//! и от демона не зависят.

use driftling_ipc::Request;
use eframe::egui::{self, RichText};

use crate::app::SettingsApp;
use crate::i18n::fl;
use crate::labels::format_uptime;
use crate::theme::{
    self, card, card_title, hint, outline_button, primary_button, row_sep, segmented, setting_row,
    toggle_switch, ToastTone, DANGER, MUTED,
};
use crate::uiprefs::Language;

/// Сколько секунд кнопка остановки стоит «на взводе» (подтверждение).
const STOP_ARM_SECS: f64 = 3.0;

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    theme::page_title(ui, &fl!("nav-app"));
    ui.add_space(4.0);
    startup(app, ui);
    window(app, ui);
    service(app, ui);
}

fn startup(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let (up, uptime) = {
        let st = app.poll.lock().unwrap();
        (st.up, st.info.as_ref().map(|i| i.uptime_secs))
    };
    card(ui, |ui| {
        card_title(ui, &fl!("section-launch"));
        let mut autostart = app.autostart;
        setting_row(
            ui,
            &fl!("autostart-title"),
            Some(&fl!("autostart-desc")),
            |ui| {
                if toggle_switch(ui, &mut autostart, app.accent).changed() {
                    app.autostart = autostart;
                    match crate::system::set_autostart(autostart) {
                        Ok(()) => app.toast(
                            ui,
                            if autostart {
                                fl!("msg-autostart-on")
                            } else {
                                fl!("msg-autostart-off")
                            },
                            ToastTone::Good,
                        ),
                        Err(e) => {
                            app.autostart = crate::system::autostart_enabled();
                            app.toast(ui, fl!("generic-error", error = e), ToastTone::Bad);
                        }
                    }
                }
            },
        );
        row_sep(ui);
        setting_row(ui, &fl!("section-daemon"), None, |ui| {
            let busy = app.busy();
            if up {
                let armed = app
                    .stop_armed_at
                    .is_some_and(|t| ui.input(|i| i.time) - t < STOP_ARM_SECS);
                let label = if armed {
                    fl!("btn-stop-confirm")
                } else {
                    fl!("btn-stop-daemon")
                };
                if outline_button(ui, &label, DANGER, !busy).clicked() {
                    if armed {
                        app.stop_armed_at = None;
                        app.command(ui, Request::Quit, fl!("msg-daemon-stopping"));
                    } else {
                        app.stop_armed_at = Some(ui.input(|i| i.time));
                    }
                }
            } else if primary_button(ui, &fl!("btn-start-daemon"), !busy, app.accent).clicked() {
                let msg = crate::system::start_daemon();
                app.toast(ui, msg, ToastTone::Warn);
            }
            let text = match (up, uptime) {
                (true, Some(secs)) => fl!("online-for", uptime = format_uptime(secs)),
                (true, None) => fl!("daemon-running"),
                _ => fl!("daemon-not-running"),
            };
            ui.label(RichText::new(text).size(13.0).color(MUTED));
        });
    });
}

fn window(app: &mut SettingsApp, ui: &mut egui::Ui) {
    card(ui, |ui| {
        card_title(ui, &fl!("section-window"));
        let langs = Language::ALL;
        let labels: Vec<String> = vec![fl!("lang-system"), fl!("lang-ru"), fl!("lang-en")];
        let selected = langs.iter().position(|l| *l == app.prefs.language);
        setting_row(
            ui,
            &fl!("ui-language"),
            Some(&fl!("ui-language-hint")),
            |ui| {
                if let Some(i) = segmented(ui, &labels, selected, app.accent) {
                    app.prefs.language = langs[i];
                    save_prefs(app, ui);
                }
            },
        );
        row_sep(ui);
        let pages = app.nav_pages();
        let page_labels: Vec<String> = pages.iter().map(|p| p.title()).collect();
        let selected = pages.iter().position(|p| p.key() == app.prefs.start_page);
        setting_row(ui, &fl!("ui-start-page"), None, |ui| {
            if let Some(i) = segmented(ui, &page_labels, selected, app.accent) {
                app.prefs.start_page = pages[i].key().to_string();
                save_prefs(app, ui);
            }
        });
        row_sep(ui);
        let mut advanced = app.prefs.advanced;
        setting_row(
            ui,
            &fl!("ui-advanced"),
            Some(&fl!("ui-advanced-hint")),
            |ui| {
                if toggle_switch(ui, &mut advanced, app.accent).changed() {
                    app.prefs.advanced = advanced;
                    app.advanced = advanced || app.debug_flag;
                    save_prefs(app, ui);
                }
            },
        );
        row_sep(ui);
        let mut scale = app.cfg.comfort.text_scale;
        setting_row(
            ui,
            &fl!("ui-text-scale"),
            Some(&fl!("ui-text-scale-hint")),
            |ui| {
                let r = ui.add(egui::Slider::new(&mut scale, 0.8..=2.0).step_by(0.05));
                if r.drag_stopped() || r.lost_focus() {
                    app.cfg.comfort.text_scale = scale;
                    let patch = app.world_patch();
                    app.apply_patch(ui, patch);
                } else if r.changed() {
                    app.cfg.comfort.text_scale = scale;
                }
            },
        );
    });
}

fn service(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let cfg_path = {
        let st = app.poll.lock().unwrap();
        st.config
            .as_ref()
            .map(|c| c.path.clone())
            .unwrap_or_else(|| driftling_core::config::path().display().to_string())
    };
    card(ui, |ui| {
        card_title(ui, &fl!("section-service"));
        setting_row(ui, &fl!("service-config"), Some(&cfg_path), |ui| {
            if outline_button(ui, &fl!("btn-open"), MUTED, true).clicked() {
                crate::system::open_path(&cfg_path);
            }
        });
        row_sep(ui);
        setting_row(
            ui,
            &fl!("service-doctor"),
            Some(&fl!("service-doctor-hint")),
            |ui| {
                if outline_button(ui, &fl!("btn-run"), MUTED, !app.doctor_running).clicked() {
                    app.doctor_running = true;
                    crate::system::run_doctor(std::sync::Arc::clone(&app.doctor), ui.ctx().clone());
                }
            },
        );
        let report = app.doctor.lock().unwrap().clone();
        if let Some(text) = report {
            app.doctor_running = false;
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .id_salt("doctor")
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(text)
                            .monospace()
                            .size(12.0)
                            .color(theme::TEXT),
                    );
                });
        } else if app.doctor_running {
            ui.add_space(4.0);
            hint(ui, &fl!("service-doctor-running"));
        }
        row_sep(ui);
        setting_row(ui, &fl!("about-version"), None, |ui| {
            ui.label(
                RichText::new(format!(
                    "{} · v{}",
                    env!("CARGO_PKG_VERSION"),
                    driftling_ipc::PROTOCOL_VERSION
                ))
                .size(13.0)
                .color(MUTED),
            );
        });
    });
}

fn save_prefs(app: &mut SettingsApp, ui: &egui::Ui) {
    if let Err(e) = app.prefs.save() {
        app.toast(ui, fl!("generic-error", error = e), ToastTone::Bad);
    } else {
        app.toast(ui, fl!("msg-ui-saved"), ToastTone::Good);
    }
}
