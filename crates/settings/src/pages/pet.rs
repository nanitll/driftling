//! Страница «Питомец»: кто это существо, как оно себя чувствует и что с
//! ним можно сделать прямо сейчас.
//!
//! Всё на этой странице меняет САМОГО питомца, то есть пишется событиями
//! журнала и уезжает на другие устройства — об этом сказано подписью внизу.

use driftling_core::{PetAttributes, Stage, DEFAULT_PET_COLOR, PET_PRESETS};
use driftling_ipc::Request;
use eframe::egui::{
    self, Button, Color32, CornerRadius, Key, Margin, RichText, Stroke, TextEdit, TextStyle,
};

use crate::app::SettingsApp;
use crate::i18n::fl;
use crate::labels::{
    format_uptime, preset_label, stage_label, stat_bar_color, state_color, state_label,
    valid_pet_name,
};
use crate::state::PetSnapshot;
use crate::theme::{
    self, badge, card, card_title, hint, meter, outline_button, primary_button, row_sep, segmented,
    setting_row, swatch, ToastTone, CARD_STROKE, DANGER, MUTED, PORTRAIT_BG, TEXT,
};

/// Размеры питомца, между которыми выбирают: спрайт и масштаб мира.
const SIZES: [(u32, &str); 4] = [(64, "S"), (96, "M"), (128, "L"), (192, "XL")];

/// Наборы характера явными числами: множители поверх весов из ста сделали
/// бы сон или прогулку недостижимыми.
const TEMPERS: [(&str, f32, u32, u32); 3] = [
    ("temper-calm", 25.0, 18, 26),
    ("temper-normal", 38.0, 32, 14),
    ("temper-lively", 70.0, 60, 8),
];

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let (checked, up, info) = {
        let st = app.poll.lock().unwrap();
        (st.checked, st.up, st.info.clone())
    };
    theme::page_title(ui, &fl!("nav-pet"));
    ui.add_space(4.0);

    if checked && !up {
        // Одно честное действие вместо россыпи выключенных контролов.
        let start = theme::empty_state(
            ui,
            &fl!("daemon-not-running"),
            &fl!("daemon-down-hint"),
            Some((&fl!("btn-start-daemon"), app.accent)),
        );
        if start {
            let msg = crate::system::start_daemon();
            app.toast(ui, msg, ToastTone::Warn);
        }
        return;
    }
    hero(app, ui, checked, up, info.as_ref());
    if !up {
        return;
    }
    care(app, ui, info.as_ref());
    appearance(app, ui, info.as_ref());
    temperament(app, ui, info.as_ref());
    ui.add_space(2.0);
    hint(ui, &fl!("pet-journal-note"));
}

/// Портрет, имя, состояние и две кнопки присутствия.
fn hero(
    app: &mut SettingsApp,
    ui: &mut egui::Ui,
    checked: bool,
    up: bool,
    info: Option<&PetSnapshot>,
) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            let size = info
                .map(|i| i.attributes.size)
                .unwrap_or_else(|| PetAttributes::default().size);
            let color = info.map(|i| i.color).unwrap_or(DEFAULT_PET_COLOR);
            let stage = info.map(|i| i.stage).unwrap_or(Stage::Adult);
            // Текстура берётся под размер картинки, а не под размер
            // питомца: при size = 192 в карточку 112 px грузился бы
            // впятеро больший набор кадров.
            app.portrait.ensure(ui.ctx(), size.min(112), color, stage);
            let t = ui.input(|i| i.time);
            egui::Frame::new()
                .fill(PORTRAIT_BG)
                .stroke(Stroke::new(1.0, CARD_STROKE))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    if let Some(tex) = app.portrait.current(t) {
                        let present = info.is_some_and(|i| i.state.is_some());
                        let img = egui::Image::new((tex.id(), egui::vec2(112.0, 112.0)));
                        ui.add(if present {
                            img
                        } else {
                            img.tint(Color32::from_gray(120))
                        });
                    }
                });
            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.add_space(4.0);
                let name = info
                    .map(|i| i.name.clone())
                    .unwrap_or_else(|| fl!("default-pet-name"));
                name_row(app, ui, up && info.is_some(), &name);
                ui.add_space(2.0);
                ui.horizontal(|ui| match (checked, up, info) {
                    (false, ..) => badge(ui, &fl!("badge-checking"), MUTED),
                    (true, false, _) => badge(ui, &fl!("daemon-not-running"), DANGER),
                    (true, true, Some(i)) => {
                        let s = i.state.as_deref();
                        badge(ui, &state_label(s), state_color(s, app.accent_light));
                        badge(ui, &stage_label(i.stage), MUTED);
                    }
                    (true, true, None) => badge(ui, &fl!("badge-no-data"), MUTED),
                });
                if let (true, Some(i)) = (up, info) {
                    ui.label(
                        RichText::new(fl!("online-for", uptime = format_uptime(i.uptime_secs)))
                            .size(12.5)
                            .color(MUTED),
                    );
                }
                ui.add_space(6.0);

                if checked && !up {
                    // Единственное честное действие при лежащем демоне.
                    hint(ui, &fl!("daemon-down-hint"));
                    ui.add_space(4.0);
                    if primary_button(ui, &fl!("btn-start-daemon"), true, app.accent).clicked() {
                        let msg = crate::system::start_daemon();
                        app.toast(ui, msg, ToastTone::Warn);
                    }
                    return;
                }

                let present = info.is_some_and(|i| i.state.is_some());
                let busy = app.busy();
                ui.horizontal(|ui| {
                    if primary_button(ui, &fl!("btn-summon"), up && !present && !busy, app.accent)
                        .clicked()
                    {
                        app.command(ui, Request::Summon, fl!("msg-pet-summoned"));
                    }
                    if outline_button(ui, &fl!("btn-dismiss"), MUTED, up && present && !busy)
                        .on_hover_text(fl!("btn-dismiss-hint"))
                        .clicked()
                    {
                        app.command(ui, Request::DismissWithWave, fl!("msg-pet-dismissed"));
                    }
                });
            });
        });
    });
}

/// Имя: подпись с карандашом либо инлайн-редактор.
fn name_row(app: &mut SettingsApp, ui: &mut egui::Ui, can_rename: bool, name: &str) {
    if !app.renaming {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(name)
                    .size(20.0)
                    .family(theme::semibold_family())
                    .color(TEXT),
            );
            if can_rename {
                let pencil = ui
                    .add(
                        Button::new(RichText::new("✏").size(14.0).color(MUTED))
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE),
                    )
                    .on_hover_text(fl!("rename-hint"));
                if pencil.clicked() {
                    app.renaming = true;
                    app.rename_focus = true;
                    app.rename_buf = name.to_string();
                }
            }
        });
        return;
    }
    if !can_rename {
        app.renaming = false;
        return;
    }
    ui.horizontal(|ui| {
        let resp = ui.add(
            TextEdit::singleline(&mut app.rename_buf)
                .desired_width(200.0)
                .font(TextStyle::Body),
        );
        if app.rename_focus {
            resp.request_focus();
            app.rename_focus = false;
        }
        let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let applied = primary_button(ui, &fl!("btn-apply"), true, app.accent).clicked() || entered;
        // Esc читается из фокуса поля, а не глобально: иначе он закрывал бы
        // редактор, даже когда человек печатает в другом месте окна.
        if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            app.renaming = false;
        } else if applied {
            match valid_pet_name(&app.rename_buf) {
                Some(new_name) => {
                    app.renaming = false;
                    app.command(
                        ui,
                        Request::Rename(new_name.to_string()),
                        fl!("msg-renamed"),
                    );
                }
                None => app.toast(ui, fl!("msg-rename-empty"), ToastTone::Bad),
            }
        }
    });
}

/// Уход: видимые статы и четыре действия.
fn care(app: &mut SettingsApp, ui: &mut egui::Ui, info: Option<&PetSnapshot>) {
    card(ui, |ui| {
        card_title(ui, &fl!("section-condition"));
        match info {
            Some(i) => {
                let s = i.stats;
                for (label, v) in [
                    (fl!("stat-satiety"), s.satiety),
                    (fl!("stat-energy"), s.energy),
                    (fl!("stat-mood"), s.mood),
                ] {
                    setting_row(ui, &label, None, |ui| {
                        ui.label(RichText::new(format!("{v:.0}")).size(13.0).color(MUTED));
                        meter(ui, v, stat_bar_color(v));
                    });
                }
            }
            None => hint(ui, &fl!("condition-unavailable")),
        }
        row_sep(ui);
        let busy = app.busy();
        let alive = info.is_some_and(|i| i.state.is_some()) && !busy;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            if primary_button(ui, &fl!("btn-feed"), alive, app.accent).clicked() {
                app.command(ui, Request::Feed { treat: false }, fl!("msg-fed"));
            }
            if outline_button(ui, &fl!("btn-treat"), app.accent_light, alive).clicked() {
                app.command(ui, Request::Feed { treat: true }, fl!("msg-treated"));
            }
            if outline_button(ui, &fl!("btn-play"), app.accent_light, alive).clicked() {
                app.command(ui, Request::Play, fl!("msg-played"));
            }
            if outline_button(ui, &fl!("btn-sleep"), app.accent_light, alive).clicked() {
                app.command(ui, Request::PutToSleep, fl!("msg-sleeping"));
            }
        });
    });
}

/// Внешний вид: цвет и размер.
fn appearance(app: &mut SettingsApp, ui: &mut egui::Ui, info: Option<&PetSnapshot>) {
    let current = info.map(|i| i.color).unwrap_or(app.accent_argb);
    let size = info
        .map(|i| i.attributes.size)
        .unwrap_or_else(|| PetAttributes::default().size);
    card(ui, |ui| {
        card_title(ui, &fl!("section-appearance"));
        let busy = app.busy();
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                for &(argb, name) in PET_PRESETS {
                    let selected = (argb ^ current) & 0x00ff_ffff == 0;
                    if swatch(ui, argb, selected, &preset_label(name)).clicked() && !selected {
                        app.command(ui, Request::Recolor(argb), fl!("msg-recolored"));
                    }
                }
            });
            ui.add_space(4.0);
            setting_row(ui, &fl!("color-custom"), None, |ui| {
                // Кнопка обязательна: у пикера нет «отпустили мышь», а
                // событие в append-only журнал на каждый кадр недопустимо.
                if primary_button(ui, &fl!("btn-apply"), !busy, app.accent).clicked() {
                    let argb = theme::rgb_to_argb(app.custom_rgb);
                    app.command(ui, Request::Recolor(argb), fl!("msg-recolored"));
                }
                ui.color_edit_button_srgb(&mut app.custom_rgb);
            });
            row_sep(ui);
            let labels: Vec<String> = SIZES
                .iter()
                .map(|(px, name)| format!("{name} · {px}"))
                .collect();
            let selected = SIZES.iter().position(|(px, _)| *px == size);
            setting_row(ui, &fl!("pet-size"), Some(&fl!("pet-size-hint")), |ui| {
                if let Some(i) = theme::segmented(ui, &labels, selected, app.accent) {
                    let mut attrs = info.map(|i| i.attributes).unwrap_or_default();
                    attrs.size = SIZES[i].0;
                    app.command(ui, Request::SetAttributes(attrs), fl!("msg-attrs-applied"));
                }
            });
        });
    });
}

/// Характер: наборы и тонкая настройка.
fn temperament(app: &mut SettingsApp, ui: &mut egui::Ui, info: Option<&PetSnapshot>) {
    card(ui, |ui| {
        card_title(ui, &fl!("section-temper"));
        let live = info.map(|i| i.attributes);
        let labels: Vec<String> = TEMPERS
            .iter()
            .map(|(key, ..)| match *key {
                "temper-calm" => fl!("temper-calm"),
                "temper-lively" => fl!("temper-lively"),
                _ => fl!("temper-normal"),
            })
            .chain(std::iter::once(fl!("temper-custom")))
            .collect();
        let selected = live.and_then(|a| {
            TEMPERS
                .iter()
                .position(|(_, w, c, s)| {
                    (a.walk_speed - w).abs() < 0.5 && a.curiosity == *c && a.sleepiness == *s
                })
                .or(Some(TEMPERS.len()))
        });
        setting_row(ui, &fl!("temper-preset"), Some(&fl!("temper-hint")), |ui| {
            if let Some(i) = segmented(ui, &labels, selected, app.accent) {
                if let Some((_, walk, curiosity, sleepiness)) = TEMPERS.get(i) {
                    let mut attrs = live.unwrap_or_default();
                    attrs.walk_speed = *walk;
                    attrs.curiosity = *curiosity;
                    attrs.sleepiness = *sleepiness;
                    app.attrs = attrs;
                    app.attrs_touched = false;
                    app.command(ui, Request::SetAttributes(attrs), fl!("msg-attrs-applied"));
                }
            }
        });
        row_sep(ui);
        egui::CollapsingHeader::new(RichText::new(fl!("temper-fine")).size(13.5).color(MUTED))
            .id_salt("fine-tuning")
            .show(ui, |ui| {
                let mut touched = false;
                setting_row(ui, &fl!("attr-walk-speed"), None, |ui| {
                    touched |= ui
                        .add(
                            egui::Slider::new(&mut app.attrs.walk_speed, 5.0..=160.0)
                                .suffix(" px/s"),
                        )
                        .changed();
                });
                setting_row(ui, &fl!("attr-curiosity"), None, |ui| {
                    touched |= ui
                        .add(egui::Slider::new(&mut app.attrs.curiosity, 0..=100))
                        .changed();
                });
                let sleep_max = 100u32.saturating_sub(app.attrs.curiosity);
                setting_row(ui, &fl!("attr-sleepiness"), None, |ui| {
                    touched |= ui
                        .add(egui::Slider::new(&mut app.attrs.sleepiness, 0..=sleep_max))
                        .changed();
                });
                setting_row(ui, &fl!("attr-sleep-range"), None, |ui| {
                    touched |= ui
                        .add(egui::DragValue::new(&mut app.attrs.sleep_max).range(1.0..=7200.0))
                        .changed();
                    ui.label(RichText::new("…").color(MUTED));
                    touched |= ui
                        .add(egui::DragValue::new(&mut app.attrs.sleep_min).range(1.0..=3600.0))
                        .changed();
                });
                if touched {
                    app.attrs_touched = true;
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let dirty = app.attrs_touched;
                    if primary_button(ui, &fl!("btn-apply"), dirty && !app.busy(), app.accent)
                        .clicked()
                    {
                        let attrs = app.attrs;
                        app.attrs_touched = false;
                        app.command(ui, Request::SetAttributes(attrs), fl!("msg-attrs-applied"));
                    }
                    // Форма протухает: питомец растёт, и «Применить» без
                    // пересинка откатывал бы его назад.
                    if outline_button(ui, &fl!("btn-take-current"), MUTED, dirty).clicked() {
                        if let Some(a) = live {
                            app.attrs = a;
                        }
                        app.attrs_touched = false;
                    }
                });
            });
    });
}
