//! Страница «Устройства»: секция `[sync]` и живой статус синхронизации.
//!
//! Единственная страница с явной парой «Применить / Отменить»: ошибка
//! здесь стоит дорого — не та папка означает второй журнал, не тот адрес —
//! молчаливое отсутствие синка.

use driftling_core::{SyncMode, SyncPatch};
use driftling_ipc::Request;
use eframe::egui::{self, RichText, TextEdit};

use crate::app::SettingsApp;
use crate::i18n::fl;
use crate::theme::{
    self, card, card_title, hint, outline_button, primary_button, row_sep, segmented, setting_row,
    AMBER, DANGER, MUTED, SUCCESS,
};

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    theme::page_title(ui, &fl!("nav-devices"));
    ui.add_space(4.0);
    form(app, ui);
    status(app, ui);
}

fn form(app: &mut SettingsApp, ui: &mut egui::Ui) {
    card(ui, |ui| {
        card_title(ui, &fl!("section-where-lives"));
        let modes = [SyncMode::Off, SyncMode::Server, SyncMode::Folder];
        let labels: Vec<String> = vec![
            fl!("sync-mode-off"),
            fl!("sync-mode-server"),
            fl!("sync-mode-folder"),
        ];
        let selected = modes.iter().position(|m| *m == app.sync_form.mode);
        setting_row(ui, &fl!("sync-mode"), Some(&fl!("sync-mode-hint")), |ui| {
            if let Some(i) = segmented(ui, &labels, selected, app.accent) {
                app.sync_form.mode = modes[i];
            }
        });

        let mut problem: Option<String> = None;
        match app.sync_form.mode {
            SyncMode::Server => {
                row_sep(ui);
                setting_row(ui, &fl!("sync-address"), None, |ui| {
                    ui.add(
                        TextEdit::singleline(&mut app.sync_form.address)
                            .hint_text("http://192.168.1.10:8787")
                            .desired_width(220.0),
                    );
                });
                let addr = app.sync_form.address.trim();
                if addr.is_empty() {
                    problem = Some(fl!("sync-address-empty"));
                } else if !addr.starts_with("http://") && !addr.starts_with("https://") {
                    problem = Some(fl!("sync-address-scheme"));
                }
                setting_row(
                    ui,
                    &fl!("sync-token"),
                    Some(&fl!("sync-token-hint")),
                    |ui| {
                        ui.add(
                            TextEdit::singleline(&mut app.sync_form.token)
                                .password(true)
                                .hint_text(if app.sync_form.token_set {
                                    fl!("sync-token-kept")
                                } else {
                                    fl!("sync-token-empty")
                                })
                                .desired_width(220.0),
                        );
                    },
                );
            }
            SyncMode::Folder => {
                row_sep(ui);
                setting_row(
                    ui,
                    &fl!("sync-folder"),
                    Some(&fl!("sync-folder-hint")),
                    |ui| {
                        if outline_button(ui, &fl!("btn-open"), MUTED, true).clicked() {
                            crate::system::open_path(&app.sync_form.folder);
                        }
                        ui.add(
                            TextEdit::singleline(&mut app.sync_form.folder)
                                .hint_text("~/Yandex.Disk/driftling")
                                .desired_width(200.0),
                        );
                    },
                );
                let folder = crate::system::expand_tilde(app.sync_form.folder.trim());
                if app.sync_form.folder.trim().is_empty() {
                    problem = Some(fl!("sync-folder-empty"));
                } else if !folder.exists() {
                    problem = Some(fl!("sync-folder-missing"));
                } else if !folder.is_dir() {
                    problem = Some(fl!("sync-folder-not-dir"));
                }
            }
            SyncMode::Off => {
                hint(ui, &fl!("sync-off-hint"));
            }
        }
        if let Some(text) = &problem {
            ui.add_space(2.0);
            ui.label(RichText::new(text).size(12.5).color(DANGER));
        }

        // Панель применения поднимается, только когда форма разошлась с тем,
        // что реально стоит у демона.
        let saved = app.cfg.sync.clone();
        let dirty = saved.mode != app.sync_form.mode
            || saved.address != app.sync_form.address.trim()
            || saved.folder != app.sync_form.folder.trim()
            || !app.sync_form.token.is_empty();
        if dirty {
            row_sep(ui);
            ui.horizontal(|ui| {
                let ok = problem.is_none() && !app.busy();
                if primary_button(ui, &fl!("btn-apply"), ok, app.accent).clicked() {
                    let patch = driftling_core::ConfigPatch {
                        sync: Some(SyncPatch {
                            mode: Some(app.sync_form.mode),
                            address: Some(app.sync_form.address.trim().to_string()),
                            // Пустое поле — «не менять»: маска из GetConfig
                            // не должна затирать настоящий токен.
                            token: (!app.sync_form.token.is_empty())
                                .then(|| app.sync_form.token.clone()),
                            folder: Some(app.sync_form.folder.trim().to_string()),
                        }),
                        ..Default::default()
                    };
                    app.cfg.sync.mode = app.sync_form.mode;
                    app.cfg.sync.address = app.sync_form.address.trim().to_string();
                    app.cfg.sync.folder = app.sync_form.folder.trim().to_string();
                    if !app.sync_form.token.is_empty() {
                        app.sync_form.token_set = true;
                        app.sync_form.token.clear();
                    }
                    app.apply_patch(ui, patch);
                }
                if outline_button(ui, &fl!("btn-discard"), MUTED, true).clicked() {
                    app.sync_form.mode = saved.mode;
                    app.sync_form.address = saved.address.clone();
                    app.sync_form.folder = saved.folder.clone();
                    app.sync_form.token.clear();
                }
            });
        }
    });
}

fn status(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let (up, sync) = {
        let st = app.poll.lock().unwrap();
        (st.up, st.sync.clone())
    };
    card(ui, |ui| {
        card_title(ui, &fl!("section-sync-status"));
        let Some(s) = sync else {
            if up {
                hint(ui, &fl!("sync-status-unavailable"));
            } else {
                hint(ui, &fl!("daemon-down-hint"));
            }
            return;
        };
        // Форма разошлась с тем, что реально работает у демона.
        let live_mode = s.mode.as_str();
        if live_mode != app.sync_form.mode.as_str() {
            ui.label(
                RichText::new(fl!("sync-not-applied"))
                    .size(12.5)
                    .color(AMBER),
            );
            ui.add_space(4.0);
        }
        let ago = |secs: Option<u64>| match secs {
            Some(v) => fl!("sync-seconds-ago", secs = v),
            None => fl!("sync-never"),
        };
        setting_row(ui, &fl!("sync-last-push"), None, |ui| {
            ui.label(RichText::new(ago(s.last_push_secs)).size(13.0).color(MUTED));
        });
        setting_row(ui, &fl!("sync-last-pull"), None, |ui| {
            ui.label(RichText::new(ago(s.last_pull_secs)).size(13.0).color(MUTED));
        });
        setting_row(ui, &fl!("sync-events"), None, |ui| {
            ui.label(
                RichText::new(format!("{} / {}", s.events, s.devices))
                    .size(13.0)
                    .color(MUTED),
            );
        });
        if live_mode == "server" {
            setting_row(ui, &fl!("sync-presence"), None, |ui| {
                let (text, color) = if s.holding {
                    (fl!("sync-here"), SUCCESS)
                } else {
                    (
                        s.holder
                            .clone()
                            .map(|h| fl!("sync-elsewhere", device = h))
                            .unwrap_or_else(|| fl!("sync-nobody")),
                        MUTED,
                    )
                };
                ui.label(RichText::new(text).size(13.0).color(color));
            });
        }
        if let Some(err) = &s.last_error {
            ui.add_space(4.0);
            ui.label(RichText::new(err).size(12.5).color(DANGER));
        }
        row_sep(ui);
        ui.horizontal(|ui| {
            if outline_button(ui, &fl!("btn-reload"), MUTED, up && !app.busy()).clicked() {
                app.command(ui, Request::Reload, fl!("msg-settings-applied"));
            }
            hint(ui, &fl!("sync-reload-hint"));
        });
    });
}
