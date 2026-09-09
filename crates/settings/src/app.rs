//! Само окно: состояние, сайдбар, применение настроек, цикл кадра.
//!
//! Страницы живут в [`crate::pages`] и получают приложение целиком —
//! immediate-mode UI не терпит промежуточных «моделей страницы», зато
//! терпит один честный объект состояния.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use driftling_core::{Config, ConfigPatch, PetAttributes, DEFAULT_PET_COLOR};
use driftling_ipc::Request;
use eframe::egui::{self, Key, Layout, Margin, RichText, ScrollArea, TextureOptions};

use crate::i18n::fl;
use crate::pages;
use crate::state::{
    self, ActionOutcome, PollState, Want, FOCUS_BIT, WANT_CONFIG, WANT_SYNC, WANT_WORLD,
};
use crate::system;
use crate::theme::{self, Toast, ToastTone};
use crate::uiprefs::UiPrefs;

/// Страницы окна. Порядок = порядок в сайдбаре и горячие клавиши Ctrl+1..5.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Page {
    Pet,
    World,
    Devices,
    App,
    Advanced,
}

impl Page {
    pub fn key(self) -> &'static str {
        match self {
            Page::Pet => "pet",
            Page::World => "world",
            Page::Devices => "devices",
            Page::App => "app",
            Page::Advanced => "advanced",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        [Page::Pet, Page::World, Page::Devices, Page::App]
            .into_iter()
            .find(|p| p.key() == key)
    }

    pub fn title(self) -> String {
        match self {
            Page::Pet => fl!("nav-pet"),
            Page::World => fl!("nav-world"),
            Page::Devices => fl!("nav-devices"),
            Page::App => fl!("nav-app"),
            Page::Advanced => fl!("nav-advanced"),
        }
    }

    /// Что этой странице нужно опрашивать у демона.
    fn wants(self) -> u8 {
        match self {
            Page::World => WANT_WORLD | WANT_CONFIG,
            Page::Devices => WANT_SYNC | WANT_CONFIG,
            Page::Advanced => WANT_WORLD | WANT_CONFIG,
            _ => 0,
        }
    }
}

/// Форма секции `[sync]`: правится и применяется явно, с валидацией.
pub struct SyncForm {
    pub mode: driftling_core::SyncMode,
    pub address: String,
    /// Пустая строка = «не менять» (демон токен не отдаёт).
    pub token: String,
    pub folder: String,
    pub token_set: bool,
}

pub struct SettingsApp {
    pub page: Page,
    last_page: Option<Page>,
    /// Страница «Продвинутые» видна (настройка окна либо флаг --debug).
    pub advanced: bool,
    /// Окно открыто с --debug: открываемся сразу на «Продвинутых».
    pub debug_flag: bool,
    pub prefs: UiPrefs,

    pub poll: Arc<Mutex<PollState>>,
    pub want: Want,
    /// Результат последней команды (общий слот на всё окно).
    pub action: Arc<Mutex<Option<ActionOutcome>>>,
    /// Сколько команд сейчас в полёте — контролы на это время гаснут.
    pub inflight: Arc<Mutex<usize>>,
    pub toasts: Vec<Toast>,

    /// Живая форма настроек мира: правится и применяется сразу.
    pub cfg: Config,
    /// С чем сравнивать, чтобы понять, что человек тронул.
    pub cfg_seen: Option<Config>,
    pub sync_form: SyncForm,
    /// Форма характеристик питомца (страница «Питомец» → «Характер»).
    pub attrs: PetAttributes,
    pub attrs_touched: bool,

    pub renaming: bool,
    pub rename_buf: String,
    pub rename_focus: bool,
    pub custom_rgb: [u8; 3],
    pub stop_armed_at: Option<f64>,
    pub autostart: bool,
    pub doctor: Arc<Mutex<Option<String>>>,
    pub doctor_running: bool,

    pub portrait: theme::Portrait,
    pub logo: Option<egui::TextureHandle>,
    pub accent_argb: u32,
    pub accent: egui::Color32,
    pub accent_light: egui::Color32,
}

impl SettingsApp {
    pub fn new(cc: &eframe::CreationContext<'_>, debug_flag: bool, start: Option<Page>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let (accent, accent_light) = theme::accent_pair(DEFAULT_PET_COLOR);
        theme::apply_style(&cc.egui_ctx, accent, accent_light);

        let prefs = UiPrefs::load();
        let advanced = prefs.advanced || debug_flag;
        let poll = Arc::new(Mutex::new(PollState::default()));
        let want: Want = Arc::new(AtomicU8::new(FOCUS_BIT | WANT_CONFIG));
        state::spawn_poller(Arc::clone(&poll), Arc::clone(&want), cc.egui_ctx.clone());

        // Стартовое состояние форм — из файла: окно должно быть полезным
        // ещё до первого ответа демона.
        let cfg = crate::config_io::load_or_default();
        let page = start
            .or_else(|| debug_flag.then_some(Page::Advanced))
            .or_else(|| Page::from_key(&prefs.start_page))
            .unwrap_or(Page::Pet);

        Self {
            page,
            last_page: None,
            advanced,
            debug_flag,
            prefs,
            poll,
            want,
            action: Arc::new(Mutex::new(None)),
            inflight: Arc::new(Mutex::new(0)),
            toasts: Vec::new(),
            sync_form: SyncForm {
                mode: cfg.sync.mode,
                address: cfg.sync.address.clone(),
                token: String::new(),
                folder: cfg.sync.folder.clone(),
                token_set: !cfg.sync.token.is_empty(),
            },
            cfg_seen: None,
            cfg,
            attrs: PetAttributes::default(),
            attrs_touched: false,
            renaming: false,
            rename_buf: String::new(),
            rename_focus: false,
            custom_rgb: theme::argb_to_rgb(DEFAULT_PET_COLOR),
            stop_armed_at: None,
            autostart: system::autostart_enabled(),
            doctor: Arc::new(Mutex::new(None)),
            doctor_running: false,
            portrait: theme::Portrait::default(),
            logo: None,
            accent_argb: DEFAULT_PET_COLOR,
            accent,
            accent_light,
        }
    }

    // -- Команды и настройки --------------------------------------------------

    /// Отправить команду демону и показать результат уведомлением.
    pub fn command(&self, ui: &egui::Ui, req: Request, ok: String) {
        *self.inflight.lock().unwrap() += 1;
        state::spawn_action(
            req,
            ok,
            Arc::clone(&self.action),
            Arc::clone(&self.poll),
            Arc::clone(&self.want),
            Arc::clone(&self.inflight),
            ui.ctx().clone(),
        );
    }

    /// Идут ли сейчас команды: пока идут, кнопки не принимают повторных
    /// нажатий — в append-only журнале двойной клик стоит двух событий.
    pub fn busy(&self) -> bool {
        *self.inflight.lock().unwrap() > 0
    }

    /// Демон отвечает.
    pub fn daemon_up(&self) -> bool {
        self.poll.lock().unwrap().up
    }

    /// Применить патч настроек: живому демону — по IPC, лежащему — прямо в
    /// файл, с честной подписью «применится при запуске».
    pub fn apply_patch(&mut self, ui: &egui::Ui, patch: ConfigPatch) {
        if self.daemon_up() {
            self.command(
                ui,
                Request::SetConfig {
                    patch: Box::new(patch),
                },
                fl!("msg-settings-applied"),
            );
            return;
        }
        match crate::config_io::save_offline(&patch) {
            Ok(saved) => {
                if let Some(name) = saved.rescued {
                    self.toast(ui, fl!("msg-config-rescued", file = name), ToastTone::Warn);
                }
                self.toast(ui, fl!("msg-settings-saved-offline"), ToastTone::Warn);
            }
            Err(e) => self.toast(ui, fl!("generic-error", error = e), ToastTone::Bad),
        }
    }

    /// Патч из текущей формы настроек мира (игровые секции целиком).
    pub fn world_patch(&self) -> ConfigPatch {
        ConfigPatch {
            game: Some(self.cfg.game.clone()),
            world: Some(self.cfg.world),
            comfort: Some(self.cfg.comfort),
            ..Default::default()
        }
    }

    /// Показать уведомление.
    pub fn toast(&mut self, ui: &egui::Ui, text: String, tone: ToastTone) {
        self.toasts.push(Toast {
            text,
            tone,
            born: ui.input(|i| i.time),
        });
        ui.ctx().request_repaint_after(Duration::from_millis(300));
    }

    // -- Кадр -----------------------------------------------------------------

    /// Акцент окна следует за цветом питомца (демон лежит — дефолт).
    fn sync_accent(&mut self, ctx: &egui::Context) {
        let pet_color = {
            let st = self.poll.lock().unwrap();
            st.info
                .as_ref()
                .map(|i| i.color)
                .unwrap_or(DEFAULT_PET_COLOR)
        };
        if pet_color == self.accent_argb {
            return;
        }
        self.accent_argb = pet_color;
        (self.accent, self.accent_light) = theme::accent_pair(pet_color);
        theme::apply_style(ctx, self.accent, self.accent_light);
        self.logo = None; // перекрасится лениво в сайдбаре
    }

    /// Догнать формы до того, что реально у демона, пока человек их не
    /// трогал. Раньше форма синкалась ровно один раз и через час врала.
    fn sync_forms(&mut self) {
        let (config, attrs) = {
            let st = self.poll.lock().unwrap();
            (
                st.config.as_ref().map(|c| (c.config.clone(), c.token_set)),
                st.info.as_ref().map(|i| i.attributes),
            )
        };
        if let Some((cfg, token_set)) = config {
            let fresh = self.cfg_seen.as_ref() != Some(&cfg);
            let untouched = self.cfg_seen.as_ref() == Some(&self.cfg);
            if fresh && (untouched || self.cfg_seen.is_none()) {
                self.sync_form.mode = cfg.sync.mode;
                self.sync_form.address = cfg.sync.address.clone();
                self.sync_form.folder = cfg.sync.folder.clone();
                self.sync_form.token_set = token_set;
                self.cfg = cfg.clone();
            }
            self.cfg_seen = Some(cfg);
        }
        if let (Some(a), false) = (attrs, self.attrs_touched) {
            self.attrs = a;
        }
    }

    /// Забрать готовый результат команды в очередь уведомлений.
    fn drain_action(&mut self, ui: &egui::Ui) {
        let outcome = self.action.lock().unwrap().take();
        if let Some(o) = outcome {
            let tone = if !o.good {
                if o.partial {
                    ToastTone::Warn
                } else {
                    ToastTone::Bad
                }
            } else {
                ToastTone::Good
            };
            self.toast(ui, o.text, tone);
        }
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, page: Page, index: usize) {
        let selected = self.page == page;
        let text = RichText::new(page.title()).size(15.0).color(if selected {
            self.accent_light
        } else {
            theme::MUTED
        });
        // Настоящая кнопка, а не рисунок painter'ом: её видит скринридер и
        // достаёт клавиатура.
        let button = egui::Button::new(text)
            .fill(if selected {
                theme::tinted(self.accent, 42)
            } else {
                egui::Color32::TRANSPARENT
            })
            .stroke(egui::Stroke::NONE)
            .corner_radius(egui::CornerRadius::same(8))
            .min_size(egui::vec2(ui.available_width(), 34.0));
        let r = ui
            .add(button)
            .on_hover_text(fl!("nav-hotkey", n = index.to_string()));
        if r.clicked() {
            self.page = page;
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            if self.logo.is_none() {
                let frames =
                    theme::pack_idle_frames(driftling_core::Stage::Adult, 64, self.accent_argb);
                self.logo = Some(ui.ctx().load_texture(
                    "logo",
                    theme::frame_to_image(&frames[0]),
                    TextureOptions::NEAREST,
                ));
            }
            if let Some(tex) = &self.logo {
                ui.add(egui::Image::new((tex.id(), egui::vec2(26.0, 26.0))));
            }
            ui.label(
                RichText::new("Driftling")
                    .size(17.0)
                    .family(theme::semibold_family())
                    .color(theme::TEXT),
            );
        });
        ui.add_space(14.0);

        for (i, page) in self.nav_pages().into_iter().enumerate() {
            self.nav_item(ui, page, i + 1);
            ui.add_space(4.0);
        }

        let (checked, up) = {
            let st = self.poll.lock().unwrap();
            (st.checked, st.up)
        };
        ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(4.0);
            theme::daemon_dot(ui, up, checked);
        });
    }

    /// Страницы сайдбара в порядке показа.
    pub fn nav_pages(&self) -> Vec<Page> {
        let mut pages = vec![Page::Pet, Page::World, Page::Devices, Page::App];
        if self.advanced {
            pages.push(Page::Advanced);
        }
        pages
    }

    /// Ctrl+1..5 — на страницу по номеру.
    fn hotkeys(&mut self, ctx: &egui::Context) {
        let keys = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5];
        let pages = self.nav_pages();
        for (i, key) in keys.into_iter().enumerate() {
            if ctx.input(|inp| inp.modifiers.ctrl && inp.key_pressed(key)) {
                if let Some(p) = pages.get(i) {
                    self.page = *p;
                }
            }
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.sync_accent(&ctx);
        self.sync_forms();
        self.drain_action(ui);
        self.hotkeys(&ctx);

        // Опрашиваем ровно то, что нужно открытой странице, и реже, когда
        // на окно не смотрят: PetInfo на стороне демона сворачивает журнал.
        let focused = ctx.input(|i| i.focused);
        let want = self.page.wants() | if focused { FOCUS_BIT } else { 0 };
        self.want.store(want, Ordering::Relaxed);

        egui::Panel::left("sidebar")
            .exact_size(196.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::SIDEBAR_BG)
                    .inner_margin(Margin::symmetric(10, 14)),
            )
            .show(ui, |ui| self.sidebar(ui));

        let page_changed = self.last_page != Some(self.page);
        self.last_page = Some(self.page);

        egui::CentralPanel::default_margins()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(Margin::same(20)),
            )
            .show(ui, |ui| {
                let mut scroll = ScrollArea::vertical().auto_shrink(false).id_salt(self.page);
                if page_changed {
                    scroll = scroll.vertical_scroll_offset(0.0);
                }
                scroll.show(ui, |ui| {
                    // Одна колонка фиксированной ширины по центру: карточки
                    // перестают растягиваться пустотой на широком окне.
                    let width = ui.available_width().min(660.0);
                    let pad = ((ui.available_width() - width) / 2.0).max(0.0);
                    ui.horizontal(|ui| {
                        ui.add_space(pad);
                        ui.vertical(|ui| {
                            ui.set_width(width);
                            ui.spacing_mut().item_spacing = egui::vec2(10.0, 12.0);
                            match self.page {
                                Page::Pet => pages::pet::show(self, ui),
                                Page::World => pages::world::show(self, ui),
                                Page::Devices => pages::devices::show(self, ui),
                                Page::App => pages::app_page::show(self, ui),
                                Page::Advanced => pages::advanced::show(self, ui),
                            }
                        });
                    });
                });
            });

        let now = ui.input(|i| i.time);
        theme::toasts(ui, &mut self.toasts, now);
    }
}
