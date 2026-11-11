use eframe::egui;
use egui::{
    Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Layout, Margin, RichText,
    Stroke, Vec2,
};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::{self, Config};
use crate::launcher;
use crate::mod_manager::{ManagedMod, ModManager, ModPreview};
use crate::scanner::{GameStatus, LoaderStatus};

// ── Obsidian design tokens ──────────────────────────────────────────────
const BACKGROUND: Color32 = Color32::from_rgb(19, 22, 28);
const SURFACE: Color32 = Color32::from_rgb(30, 34, 42);
const ELEVATED_SURFACE: Color32 = Color32::from_rgb(39, 44, 54);
const BORDER: Color32 = Color32::from_rgb(54, 61, 72);
const ACCENT: Color32 = Color32::from_rgb(194, 229, 160);
const PRIMARY_TEXT: Color32 = Color32::from_rgb(229, 233, 226);
const SECONDARY_TEXT: Color32 = Color32::from_rgb(160, 168, 178);
const SUCCESS: Color32 = Color32::from_rgb(155, 195, 132);
const WARNING: Color32 = Color32::from_rgb(210, 174, 105);
const ERROR: Color32 = Color32::from_rgb(203, 125, 125);
const INFORMATION: Color32 = Color32::from_rgb(127, 157, 190);
const DISABLED_SURFACE: Color32 = Color32::from_rgb(54, 62, 73);
const SIDEBAR_WIDTH: f32 = 184.0;
const PANEL_RADIUS: u8 = 6;
const CONTROL_RADIUS: u8 = 5;

const BG_DARK: Color32 = BACKGROUND;
const BG_PANEL: Color32 = SURFACE;
const TEXT_PRIMARY: Color32 = PRIMARY_TEXT;
const TEXT_SECONDARY: Color32 = SECONDARY_TEXT;
const TEXT_DIM: Color32 = SECONDARY_TEXT;
const GREEN_OK: Color32 = SUCCESS;
const RED_ERR: Color32 = ERROR;
const YELLOW_WARN: Color32 = WARNING;
const BUTTON_BG: Color32 = ACCENT;
const BUTTON_HOVER: Color32 = Color32::from_rgb(207, 237, 179);
const BUTTON_DISABLED: Color32 = DISABLED_SURFACE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppSection {
    Library,
    Plugins,
    Diagnostics,
    Settings,
    Log,
}

impl AppSection {
    const ALL: [Self; 5] = [
        Self::Library,
        Self::Plugins,
        Self::Diagnostics,
        Self::Settings,
        Self::Log,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Library => "Biblioteca de mods",
            Self::Plugins => "Plugins",
            Self::Diagnostics => "Diagnóstico",
            Self::Settings => "Ajustes",
            Self::Log => "Registro",
        }
    }

    fn nav_label(self) -> &'static str {
        match self {
            Self::Library => "Biblioteca",
            _ => self.title(),
        }
    }

    fn icon(self) -> NavIcon {
        match self {
            Self::Library => NavIcon::Library,
            Self::Plugins => NavIcon::Plugins,
            Self::Diagnostics => NavIcon::Diagnostics,
            Self::Settings => NavIcon::Settings,
            Self::Log => NavIcon::Log,
        }
    }
}

#[derive(Clone, Copy)]
enum NavIcon {
    Library,
    Plugins,
    Diagnostics,
    Settings,
    Log,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ModFilter {
    #[default]
    All,
    Enabled,
    Disabled,
}

enum ModRowAction {
    Toggle { id: String, enabled: bool },
    Uninstall(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivityLevel {
    Info,
    Error,
}

impl ActivityLevel {
    fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Error => "ERROR",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Self::Info => INFORMATION,
            Self::Error => ERROR,
        }
    }
}

#[derive(Debug, Clone)]
struct ActivityLogEntry {
    timestamp: String,
    level: ActivityLevel,
    message: String,
}

enum ManagerAction {
    Installed { files: usize, skipped: usize },
    Toggled { enabled: bool },
    Uninstalled,
}

enum BackgroundEvent {
    InitialScan(GameStatus),
    Scan(GameStatus),
    Preview(Result<ModPreview, String>),
    SelectedGame(Option<PathBuf>),
    SelectedArchive(Option<PathBuf>),
    Failed(String),
    Manager {
        manager: ModManager,
        result: Result<ManagerAction, String>,
    },
}

// ── App state ──────────────────────────────────────────────────────────
pub struct SekaiHunterApp {
    config: Config,
    status: GameStatus,
    mod_manager: Option<ModManager>,
    mod_manager_error: Option<String>,
    pending_mod: Option<ModPreview>,
    pending_archives: VecDeque<PathBuf>,
    pending_uninstall: Option<String>,
    mod_message: Option<(String, bool)>,
    worker_sender: Sender<BackgroundEvent>,
    worker_receiver: Receiver<BackgroundEvent>,
    worker_busy: bool,
    worker_message: Option<String>,
    activity_log: Vec<ActivityLogEntry>,
    launch_message: Option<(String, bool)>, // (message, is_error)
    selected_section: AppSection,
    copied_launch_opts: bool,
    mod_search: String,
    mod_filter: ModFilter,
}

impl SekaiHunterApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_visuals(&cc.egui_ctx);
        configure_fonts(&cc.egui_ctx);

        let config = Config::load();

        let (mod_manager, mod_manager_error) = match ModManager::load() {
            Ok(manager) => (Some(manager), None),
            Err(error) => {
                log::error!("{error}");
                (None, Some(error))
            }
        };

        let (worker_sender, worker_receiver) = mpsc::channel();
        let mut app = Self {
            config,
            status: GameStatus::empty(),
            mod_manager,
            mod_manager_error,
            pending_mod: None,
            pending_archives: VecDeque::new(),
            pending_uninstall: None,
            mod_message: None,
            worker_sender,
            worker_receiver,
            worker_busy: false,
            worker_message: None,
            activity_log: vec![ActivityLogEntry {
                timestamp: utc_timestamp_now(),
                level: ActivityLevel::Info,
                message: "Sekai Hunter iniciado.".into(),
            }],
            launch_message: None,
            selected_section: AppSection::Library,
            copied_launch_opts: false,
            mod_search: String::new(),
            mod_filter: ModFilter::All,
        };
        app.start_initial_scan(&cc.egui_ctx);
        app
    }

    fn rescan(&mut self, ctx: &egui::Context) {
        if self.worker_busy {
            return;
        }
        self.record("Volviendo a escanear el directorio configurado del juego.");
        if self.config.game_path().is_none() {
            self.status = GameStatus::empty();
            return;
        }
        self.start_scan(ctx);
    }

    fn browse_game_dir(&mut self, ctx: &egui::Context) {
        if self.worker_busy {
            return;
        }
        let sender = self.worker_sender.clone();
        let repaint = ctx.clone();
        self.worker_busy = true;
        self.worker_message = Some("Opening folder picker…".into());
        match thread::Builder::new()
            .name("sekai-folder-picker".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    pollster::block_on(
                        rfd::AsyncFileDialog::new()
                            .set_title("Elige la carpeta de Monster Hunter: World")
                            .pick_folder(),
                    )
                    .map(|file| file.path().to_path_buf())
                }));
                let event = match result {
                    Ok(selected) => BackgroundEvent::SelectedGame(selected),
                    Err(_) => BackgroundEvent::Failed("Folder picker failed unexpectedly.".into()),
                };
                let _ = sender.send(event);
                repaint.request_repaint();
            }) {
            Ok(_) => {}
            Err(error) => {
                self.worker_busy = false;
                self.worker_message = None;
                self.mod_message = Some((format!("Could not open folder picker: {error}"), true));
            }
        }
    }

    fn record(&mut self, message: impl Into<String>) {
        self.record_with_level(message, ActivityLevel::Info);
    }

    fn record_with_level(&mut self, message: impl Into<String>, level: ActivityLevel) {
        let message = message.into();
        match level {
            ActivityLevel::Info => log::info!("{message}"),
            ActivityLevel::Error => log::error!("{message}"),
        }
        self.activity_log.push(ActivityLogEntry {
            timestamp: utc_timestamp_now(),
            level,
            message,
        });
        if self.activity_log.len() > 32 {
            self.activity_log.remove(0);
        }
    }

    fn launch_game(&mut self) {
        self.copied_launch_opts = false;
        self.record("Enviando solicitud de inicio de Monster Hunter: World a Steam.");
        match launcher::launch_via_steam() {
            Ok(()) => {
                let message = match self.status.loader_status() {
                    LoaderStatus::Astera => {
                        "Solicitud enviada a Steam. Astera Loader requiere la opción de inicio que aparece abajo.".into()
                    }
                    LoaderStatus::Stracker => {
                        "Solicitud enviada a Steam. Se detectó Stracker's Loader; verifica su compatibilidad con Proton y los ajustes de Steam.".into()
                    }
                    _ => {
                        "La configuración detectada del loader no está lista para iniciar.".into()
                    }
                };
                self.launch_message = Some((message, false));
            }
            Err(error) => {
                self.record_with_level(
                    format!("Falló el inicio de Steam: {error}"),
                    ActivityLevel::Error,
                );
                self.launch_message = Some((error, true));
            }
        }
    }

    fn queue_archive(&mut self, path: PathBuf) {
        self.pending_archives.push_back(path);
    }

    fn prepare_next_archive(&mut self, ctx: &egui::Context) {
        if self.pending_mod.is_some() || self.worker_busy {
            return;
        }
        let Some(path) = self.pending_archives.pop_front() else {
            return;
        };
        let Some(game_dir) = self.config.game_path() else {
            self.mod_message = Some((
                "Choose the game directory before installing mods.".into(),
                true,
            ));
            return;
        };
        self.worker_busy = true;
        self.worker_message = Some(format!(
            "Reading {}…",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        let sender = self.worker_sender.clone();
        let repaint = ctx.clone();
        match thread::Builder::new()
            .name("sekai-archive-preview".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    ModManager::preview_archive(&path, &game_dir)
                }))
                .unwrap_or_else(|_| Err("Archive preview failed unexpectedly.".into()));
                let _ = sender.send(BackgroundEvent::Preview(result));
                repaint.request_repaint();
            }) {
            Ok(_) => {}
            Err(error) => {
                self.worker_busy = false;
                self.worker_message = None;
                self.mod_message =
                    Some((format!("Could not start archive preview: {error}"), true));
            }
        }
    }

    fn start_initial_scan(&mut self, ctx: &egui::Context) {
        let configured_dir = self.config.game_path();
        let sender = self.worker_sender.clone();
        let repaint = ctx.clone();
        self.worker_busy = true;
        self.worker_message = Some("Checking game installation…".into());
        match thread::Builder::new()
            .name("sekai-startup-scan".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    configured_dir
                        .map(|dir| GameStatus::scan(&dir))
                        .unwrap_or_else(auto_detect_game)
                }));
                let event = match result {
                    Ok(status) => BackgroundEvent::InitialScan(status),
                    Err(_) => BackgroundEvent::Failed("Game detection failed unexpectedly.".into()),
                };
                let _ = sender.send(event);
                repaint.request_repaint();
            }) {
            Ok(_) => {}
            Err(error) => {
                self.worker_busy = false;
                self.worker_message = None;
                log::error!("Could not start installation scan: {error}");
            }
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        let Some(game_dir) = self.config.game_path() else {
            self.status = GameStatus::empty();
            return;
        };
        self.worker_busy = true;
        self.worker_message = Some("Refreshing game and plugin status…".into());
        let sender = self.worker_sender.clone();
        let repaint = ctx.clone();
        match thread::Builder::new()
            .name("sekai-status-scan".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    GameStatus::scan(&game_dir)
                }));
                let event = match result {
                    Ok(status) => BackgroundEvent::Scan(status),
                    Err(_) => {
                        BackgroundEvent::Failed("Game status scan failed unexpectedly.".into())
                    }
                };
                let _ = sender.send(event);
                repaint.request_repaint();
            }) {
            Ok(_) => {}
            Err(error) => {
                self.worker_busy = false;
                self.worker_message = None;
                self.mod_message = Some((format!("Could not start status scan: {error}"), true));
            }
        }
    }

    fn open_archive_picker(&mut self, ctx: &egui::Context) {
        if self.worker_busy {
            return;
        }
        let sender = self.worker_sender.clone();
        let repaint = ctx.clone();
        self.worker_busy = true;
        self.worker_message = Some("Opening archive picker…".into());
        match thread::Builder::new()
            .name("sekai-archive-picker".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    pollster::block_on(
                        rfd::AsyncFileDialog::new()
                            .set_title("Selecciona un archivo ZIP o RAR de mods")
                            .add_filter("Mod archives", &["zip", "rar"])
                            .pick_file(),
                    )
                    .map(|file| file.path().to_path_buf())
                }));
                let event = match result {
                    Ok(selected) => BackgroundEvent::SelectedArchive(selected),
                    Err(_) => BackgroundEvent::Failed("Archive picker failed unexpectedly.".into()),
                };
                let _ = sender.send(event);
                repaint.request_repaint();
            }) {
            Ok(_) => {}
            Err(error) => {
                self.worker_busy = false;
                self.worker_message = None;
                self.mod_message = Some((format!("Could not open archive picker: {error}"), true));
            }
        }
    }

    fn start_manager_job<F>(&mut self, ctx: &egui::Context, label: String, operation: F)
    where
        F: FnOnce(&mut ModManager, &std::path::Path) -> Result<ManagerAction, String>
            + Send
            + 'static,
    {
        if self.worker_busy {
            return;
        }
        let Some(game_dir) = self.config.game_path() else {
            self.mod_message = Some(("Choose the game directory first.".into(), true));
            return;
        };
        let Some(manager) = self.mod_manager.take() else {
            self.mod_message = Some(("The mod registry is unavailable.".into(), true));
            return;
        };
        self.worker_busy = true;
        self.worker_message = Some(label);
        let sender = self.worker_sender.clone();
        let repaint = ctx.clone();
        let manager_holder = Arc::new(Mutex::new(Some(manager)));
        let worker_manager = Arc::clone(&manager_holder);
        match thread::Builder::new()
            .name("sekai-mod-operation".into())
            .spawn(move || {
                let manager = worker_manager
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let Some(mut manager) = manager else {
                    return;
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    operation(&mut manager, &game_dir)
                }))
                .unwrap_or_else(|_| Err("Mod operation failed unexpectedly.".into()));
                let _ = sender.send(BackgroundEvent::Manager { manager, result });
                repaint.request_repaint();
            }) {
            Ok(_) => {}
            Err(error) => {
                self.mod_manager = manager_holder
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                self.worker_busy = false;
                self.worker_message = None;
                self.mod_message = Some((format!("Could not start mod operation: {error}"), true));
            }
        }
    }

    fn handle_background_events(&mut self, ctx: &egui::Context) {
        loop {
            let event = match self.worker_receiver.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.worker_busy = false;
                    self.worker_message = None;
                    self.mod_message =
                        Some(("Background worker stopped unexpectedly.".into(), true));
                    break;
                }
            };
            self.worker_busy = false;
            self.worker_message = None;
            match event {
                BackgroundEvent::InitialScan(status) => {
                    self.status = status;
                    if self.config.game_dir.is_none() {
                        if let Some(dir) = &self.status.game_dir {
                            self.config.game_dir = Some(dir.to_string_lossy().into_owned());
                            self.config.save();
                        }
                    }
                    let message = self
                        .status
                        .game_dir
                        .as_ref()
                        .map(|dir| format!("Game directory detected: {}", dir.display()))
                        .unwrap_or_else(|| {
                            "Monster Hunter: World was not found automatically.".into()
                        });
                    self.record(message);
                    self.prepare_next_archive(ctx);
                }
                BackgroundEvent::Scan(status) => {
                    self.status = status;
                    self.prepare_next_archive(ctx);
                }
                BackgroundEvent::Preview(result) => match result {
                    Ok(preview) => {
                        self.record(format!("Prepared mod archive: {}", preview.archive_name));
                        self.pending_mod = Some(preview);
                    }
                    Err(error) => {
                        self.record_with_level(
                            format!("Could not prepare mod archive: {error}"),
                            ActivityLevel::Error,
                        );
                        self.mod_message = Some((error, true));
                        self.prepare_next_archive(ctx);
                    }
                },
                BackgroundEvent::SelectedGame(selected) => {
                    if let Some(path) = selected {
                        self.config.game_dir = Some(path.to_string_lossy().into_owned());
                        self.config.save();
                        self.record(format!("Selected game directory: {}", path.display()));
                        self.start_scan(ctx);
                    } else {
                        self.prepare_next_archive(ctx);
                    }
                }
                BackgroundEvent::SelectedArchive(selected) => {
                    if let Some(path) = selected {
                        self.queue_archive(path);
                    }
                    self.prepare_next_archive(ctx);
                }
                BackgroundEvent::Failed(message) => {
                    self.record_with_level(message.clone(), ActivityLevel::Error);
                    self.mod_message = Some((message, true));
                    self.prepare_next_archive(ctx);
                }
                BackgroundEvent::Manager { manager, result } => {
                    self.mod_manager = Some(manager);
                    match result {
                        Ok(ManagerAction::Installed { files, skipped }) => {
                            let message = if skipped == 0 {
                                format!("Mod instalado ({files} archivos).")
                            } else {
                                format!(
                                    "Mod instalado ({files} archivos); se omitieron {skipped} conflictos."
                                )
                            };
                            self.record(message.clone());
                            self.mod_message = Some((message, false));
                            self.start_scan(ctx);
                        }
                        Ok(ManagerAction::Toggled { enabled }) => {
                            let message = if enabled {
                                "Mod activado."
                            } else {
                                "Mod desactivado."
                            };
                            self.record(message);
                            self.mod_message = Some((message.into(), false));
                            self.start_scan(ctx);
                        }
                        Ok(ManagerAction::Uninstalled) => {
                            self.record("Mod desinstalado.");
                            self.mod_message = Some(("Mod desinstalado.".into(), false));
                            self.start_scan(ctx);
                        }
                        Err(error) => {
                            self.record_with_level(
                                format!("Mod operation failed: {error}"),
                                ActivityLevel::Error,
                            );
                            self.mod_message = Some((error, true));
                            self.prepare_next_archive(ctx);
                        }
                    }
                }
            }
        }
    }

    fn draw_library_dashboard(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let (marker, _) =
                        ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                    ui.painter().circle_filled(marker.center(), 3.0, ACCENT);
                    ui.label(
                        RichText::new("MONSTER HUNTER: WORLD")
                            .color(SECONDARY_TEXT)
                            .size(11.0)
                            .strong(),
                    );
                });
                ui.add_space(3.0);
                ui.label(
                    RichText::new(library_hero_title(
                        self.status.mhw_exe_found,
                        self.status.loader_status(),
                    ))
                    .font(FontId::new(
                        30.0,
                        FontFamily::Name("Barlow Condensed".into()),
                    ))
                    .color(PRIMARY_TEXT)
                    .strong(),
                );
                ui.label(
                    RichText::new("Administra tus mods y prepara tu próxima cacería.")
                        .color(SECONDARY_TEXT)
                        .size(13.0),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let settings_button = ui.add(
                    egui::Button::new(RichText::new("Ajustes").color(PRIMARY_TEXT).size(12.0))
                        .fill(ELEVATED_SURFACE)
                        .corner_radius(CornerRadius::same(CONTROL_RADIUS))
                        .min_size(Vec2::new(0.0, 34.0))
                        .stroke(Stroke::new(1.0_f32, BORDER)),
                );
                if settings_button.clicked() {
                    self.selected_section = AppSection::Settings;
                }
                draw_status_pill(
                    ui,
                    if self.status.mhw_exe_found {
                        "Juego detectado"
                    } else {
                        "Juego no detectado"
                    },
                    if self.status.mhw_exe_found {
                        SUCCESS
                    } else {
                        WARNING
                    },
                );
                draw_status_pill(
                    ui,
                    loader_status_label(self.status.loader_status()),
                    loader_status_color(self.status.loader_status()),
                );
            });
        });

        ui.add_space(14.0);
        draw_full_width_panel(ui, |ui| {
            let available_width = ui.available_width();
            if available_width >= 650.0 {
                let directory_width = available_width * 0.59;
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(directory_width, 112.0),
                        Layout::top_down(Align::Min),
                        |ui| self.draw_library_directory(ui, ctx),
                    );
                    ui.add_space(16.0);
                    ui.vertical_centered(|ui| self.draw_library_launch_action(ui));
                });
            } else {
                self.draw_library_directory(ui, ctx);
                ui.add_space(12.0);
                ui.vertical_centered(|ui| self.draw_library_launch_action(ui));
            }
        });

        ui.add_space(12.0);
        self.draw_library_launch_options(ui);
        ui.add_space(16.0);
    }

    fn draw_library_directory(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(panel_heading("Directorio del juego"));
        ui.add_space(6.0);
        Frame::new()
            .fill(BACKGROUND)
            .corner_radius(CornerRadius::same(CONTROL_RADIUS))
            .inner_margin(Margin::symmetric(10, 8))
            .stroke(Stroke::new(1.0_f32, BORDER))
            .show(ui, |ui| {
                let path = self.config.game_dir.as_deref().unwrap_or("Sin configurar");
                ui.add(
                    egui::Label::new(
                        RichText::new(path)
                            .monospace()
                            .color(PRIMARY_TEXT)
                            .size(12.0),
                    )
                    .wrap(),
                )
                .on_hover_text(path);
            });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if styled_small_button(ui, "Volver a escanear") {
                self.rescan(ctx);
            }
            if styled_small_button(ui, "Elegir otra carpeta") {
                self.browse_game_dir(ctx);
            }
        });
    }

    fn draw_library_launch_action(&mut self, ui: &mut egui::Ui) {
        let ready = self.status.is_ready();
        let button = draw_launch_button(ui, ready);
        if (button.clicked()
            || (button.has_focus()
                && ui.input(|input| {
                    input.key_pressed(egui::Key::Space) || input.key_pressed(egui::Key::Enter)
                })))
            && ready
        {
            self.launch_game();
        }
        if !ready {
            ui.add_space(5.0);
            ui.label(
                RichText::new(launch_blocked_message(self.status.loader_status()))
                    .color(SECONDARY_TEXT)
                    .small(),
            );
        }
        if let Some((message, is_error)) = &self.launch_message {
            ui.add_space(4.0);
            ui.label(
                RichText::new(message)
                    .color(if *is_error { ERROR } else { SUCCESS })
                    .small(),
            );
        }
    }

    fn draw_library_launch_options(&mut self, ui: &mut egui::Ui) {
        draw_full_width_panel(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Opciones de inicio de Steam")
                        .color(PRIMARY_TEXT)
                        .strong()
                        .size(14.0),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.status.loader_status() == LoaderStatus::Astera
                        && styled_small_button(ui, "Copiar comando")
                    {
                        ui.ctx().copy_text(launcher::STEAM_LAUNCH_OPTION.to_owned());
                        self.copied_launch_opts = true;
                    }
                    if self.copied_launch_opts
                        && self.status.loader_status() == LoaderStatus::Astera
                    {
                        ui.label(RichText::new("Copiado").color(SUCCESS).small());
                    }
                });
            });
            ui.label(
                RichText::new(
                    "En Steam: Biblioteca > Monster Hunter: World > Propiedades > General > Opciones de inicio",
                )
                .color(SECONDARY_TEXT)
                .size(11.5),
            );
            ui.add_space(6.0);
            match self.status.loader_status() {
                LoaderStatus::Astera => {
                    Frame::new()
                        .fill(BACKGROUND)
                        .corner_radius(CornerRadius::same(CONTROL_RADIUS))
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(launcher::STEAM_LAUNCH_OPTION)
                                        .monospace()
                                        .color(ACCENT)
                                        .size(11.5),
                                )
                                .wrap(),
                            );
                        });
                }
                LoaderStatus::Stracker => {
                    ui.label(
                        RichText::new(
                            "Stracker's Loader está detectado. Sekai Hunter iniciará mediante Steam con la configuración actual; no se añadirá un comando de Proton no verificado.",
                        )
                        .color(SECONDARY_TEXT)
                        .size(12.0),
                    );
                }
                LoaderStatus::Conflict => {
                    ui.label(
                        RichText::new(launch_blocked_message(LoaderStatus::Conflict))
                            .color(ERROR)
                            .size(12.0),
                    );
                }
                LoaderStatus::AsteraIncomplete | LoaderStatus::StrackerIncomplete => {
                    ui.label(
                        RichText::new(launch_blocked_message(self.status.loader_status()))
                            .color(WARNING)
                            .size(12.0),
                    );
                }
                LoaderStatus::Missing => {
                    ui.label(
                        RichText::new(launch_blocked_message(LoaderStatus::Missing))
                            .color(SECONDARY_TEXT)
                            .size(12.0),
                    );
                }
            }
        });
    }

    fn draw_mod_library(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let installed_count = self
            .mod_manager
            .as_ref()
            .map_or(0, |manager| manager.mods().len());
        let active_count = self.mod_manager.as_ref().map_or(0, |manager| {
            manager
                .mods()
                .iter()
                .filter(|managed| managed.enabled)
                .count()
        });

        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Biblioteca de mods")
                    .font(FontId::new(
                        22.0,
                        FontFamily::Name("Barlow Condensed".into()),
                    ))
                    .color(PRIMARY_TEXT)
                    .strong(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if library_primary_button(ui, "Añadir mod", !self.worker_busy).clicked() {
                    self.mod_message = None;
                    self.open_archive_picker(ctx);
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("{active_count} activos"))
                        .color(if active_count > 0 {
                            SUCCESS
                        } else {
                            SECONDARY_TEXT
                        })
                        .size(12.0),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("{installed_count} instalados"))
                        .color(SECONDARY_TEXT)
                        .size(12.0),
                );
            });
        });
        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let filter_width = 56.0 + 64.0 + 92.0 + 16.0;
            let search_width = (ui.available_width() - filter_width).clamp(128.0, 320.0);
            Frame::new()
                .fill(BACKGROUND)
                .corner_radius(CornerRadius::same(5))
                .stroke(Stroke::new(1.0_f32, BORDER))
                .show(ui, |ui| {
                    ui.add_sized(
                        Vec2::new(search_width, 32.0),
                        egui::TextEdit::singleline(&mut self.mod_search)
                            .hint_text("Buscar por nombre, archivo o ruta…")
                            .margin(Vec2::new(9.0, 6.0))
                            .frame(false),
                    );
                });
            ui.add_space(8.0);
            library_filter_button(ui, "Todos", ModFilter::All, &mut self.mod_filter);
            library_filter_button(ui, "Activos", ModFilter::Enabled, &mut self.mod_filter);
            library_filter_button(
                ui,
                "Desactivados",
                ModFilter::Disabled,
                &mut self.mod_filter,
            );
        });
        ui.add_space(8.0);

        let is_dragging = ui.input(|input| !input.raw.hovered_files.is_empty());
        Frame::new()
            .fill(if is_dragging {
                ELEVATED_SURFACE
            } else {
                SURFACE
            })
            .corner_radius(CornerRadius::same(5))
            .inner_margin(Margin::symmetric(12, 9))
            .stroke(Stroke::new(
                1.0_f32,
                if is_dragging { ACCENT } else { BORDER },
            ))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(if is_dragging {
                                "Suelta el archivo para revisar el mod"
                            } else {
                                "Suelta aquí un archivo ZIP o RAR"
                            })
                            .color(if is_dragging { ACCENT } else { PRIMARY_TEXT })
                            .strong()
                            .size(12.0),
                        );
                        ui.label(
                            RichText::new("La instalación se previsualiza antes de aplicarse.")
                                .color(SECONDARY_TEXT)
                                .size(11.0),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("RAR: requiere bsdtar, unrar o 7z")
                                .color(SECONDARY_TEXT)
                                .size(10.0),
                        );
                    });
                });
            });

        if self.worker_busy {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new(
                        self.worker_message
                            .as_deref()
                            .map(localized_worker_message)
                            .unwrap_or_else(|| "Procesando…".into()),
                    )
                    .color(SECONDARY_TEXT)
                    .small(),
                );
            });
        }
        if let Some(error) = &self.mod_manager_error {
            ui.add_space(8.0);
            ui.colored_label(ERROR, error);
        } else if let Some((message, is_error)) = &self.mod_message {
            ui.add_space(8.0);
            ui.colored_label(if *is_error { ERROR } else { SUCCESS }, message);
        }

        let mut row_action = None;
        if let Some(manager) = &self.mod_manager {
            let filtered_mods = manager
                .mods()
                .iter()
                .filter(|managed| mod_matches_filter(managed, self.mod_filter))
                .filter(|managed| mod_matches_query(managed, &self.mod_search))
                .collect::<Vec<_>>();

            ui.add_space(8.0);
            if manager.mods().is_empty() {
                ui.label(
                    RichText::new("Aún no hay mods instalados.")
                        .color(SECONDARY_TEXT)
                        .size(13.0),
                );
            } else if filtered_mods.is_empty() {
                ui.label(
                    RichText::new("No hay mods que coincidan con la búsqueda y el filtro.")
                        .color(SECONDARY_TEXT)
                        .size(13.0),
                );
            } else {
                for managed in filtered_mods {
                    if let Some(action) = draw_managed_mod_row(ui, managed) {
                        row_action = Some(action);
                    }
                    ui.add_space(2.0);
                    ui.separator();
                }
            }
        }

        match row_action {
            Some(ModRowAction::Toggle { id, enabled }) => {
                self.mod_message = None;
                let label = if enabled {
                    "Activando mod…"
                } else {
                    "Desactivando mod…"
                };
                self.start_manager_job(ctx, label.into(), move |manager, game_dir| {
                    manager.set_enabled(&id, enabled, game_dir)?;
                    Ok(ManagerAction::Toggled { enabled })
                });
            }
            Some(ModRowAction::Uninstall(id)) => {
                self.pending_uninstall = Some(id);
            }
            None => {}
        }
    }

    fn draw_plugins(&self, ui: &mut egui::Ui) {
        draw_full_width_panel(ui, |ui| {
            ui.label(panel_heading("Directorios de plugins"));
            ui.add_space(8.0);
            draw_plugin_directory_row(ui, "nativePC/plugins", self.status.native_pc_plugins_found);
            ui.separator();
            draw_plugin_directory_row(ui, "AsteraLoader/plugins", self.status.astera_plugins_found);

            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.label(panel_heading("DLL detectadas"));
                ui.label(
                    RichText::new(format!(
                        "{} · {} Stracker · {} Astera",
                        self.status.plugin_count,
                        self.status.stracker_plugin_count,
                        self.status.astera_plugin_count
                    ))
                    .color(SECONDARY_TEXT)
                    .small(),
                );
            });
            ui.add_space(4.0);

            if self.status.plugin_names.is_empty() {
                ui.label(
                    RichText::new("No se encontraron archivos DLL en los directorios compatibles.")
                        .color(SECONDARY_TEXT)
                        .small(),
                );
            } else {
                ui.add_space(4.0);
                for plugin in &self.status.plugin_names {
                    draw_plugin_row(ui, plugin);
                    ui.separator();
                }
            }
        });
    }

    fn draw_diagnostics(&self, ui: &mut egui::Ui) {
        use crate::scanner::DiagLevel;

        let errors = self
            .status
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.level == DiagLevel::Error)
            .collect::<Vec<_>>();
        let warnings = self
            .status
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.level == DiagLevel::Warning)
            .collect::<Vec<_>>();
        let information = self
            .status
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.level == DiagLevel::Info)
            .collect::<Vec<_>>();

        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{} errores", errors.len()))
                    .color(ERROR)
                    .small(),
            );
            ui.label(
                RichText::new(format!("{} avisos", warnings.len()))
                    .color(WARNING)
                    .small(),
            );
            ui.label(
                RichText::new(format!("{} informativos", information.len()))
                    .color(INFORMATION)
                    .small(),
            );
        });
        ui.add_space(12.0);

        if self.status.diagnostics.is_empty() {
            draw_full_width_panel(ui, |ui| {
                ui.label(RichText::new("El escaneo no reportó mensajes.").color(SECONDARY_TEXT));
            });
            return;
        }

        draw_diagnostic_group(ui, "Errores", &errors, ERROR);
        draw_diagnostic_group(ui, "Avisos", &warnings, WARNING);
        draw_diagnostic_group(ui, "Información", &information, INFORMATION);
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        draw_full_width_panel(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(panel_heading("Instalación del juego"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if styled_small_button(ui, "Elegir carpeta") {
                        self.browse_game_dir(ctx);
                    }
                    if styled_small_button(ui, "Volver a escanear") {
                        self.rescan(ctx);
                    }
                });
            });
            ui.add_space(10.0);
            draw_status_cards(ui, &self.status);
            ui.add_space(10.0);
            ui.label(
                RichText::new("Directorio del juego")
                    .color(SECONDARY_TEXT)
                    .small(),
            );
            let dir_text = self.config.game_dir.as_deref().unwrap_or("Sin configurar");
            ui.add(
                egui::Label::new(
                    RichText::new(dir_text)
                        .monospace()
                        .color(PRIMARY_TEXT)
                        .size(12.5),
                )
                .wrap(),
            );
        });

        ui.add_space(14.0);
        draw_full_width_panel(ui, |ui| {
            ui.label(panel_heading("Inicio del juego"));
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                let ready = self.status.is_ready();
                let button = draw_launch_button(ui, ready);
                if (button.clicked()
                    || (button.has_focus()
                        && ui.input(|input| {
                            input.key_pressed(egui::Key::Space)
                                || input.key_pressed(egui::Key::Enter)
                        })))
                    && ready
                {
                    self.launch_game();
                }
                if !ready {
                    ui.label(
                        RichText::new(launch_blocked_message(self.status.loader_status()))
                            .color(SECONDARY_TEXT)
                            .small(),
                    );
                }
                if let Some((message, is_error)) = &self.launch_message {
                    ui.label(
                        RichText::new(message)
                            .color(if *is_error { ERROR } else { SUCCESS })
                            .small(),
                    );
                }
            });

            ui.add_space(12.0);
            match self.status.loader_status() {
                LoaderStatus::Astera => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(panel_heading("Astera Loader · Opciones de inicio de Steam"));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if styled_small_button(ui, "Copiar") {
                                ui.ctx().copy_text(launcher::STEAM_LAUNCH_OPTION.to_owned());
                                self.copied_launch_opts = true;
                            }
                            if self.copied_launch_opts {
                                ui.label(RichText::new("Copiado").color(SUCCESS).small());
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "En Steam: Biblioteca > Monster Hunter: World > Propiedades > General > Opciones de inicio",
                        )
                        .color(SECONDARY_TEXT)
                        .small(),
                    );
                    Frame::new()
                        .fill(BACKGROUND)
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::same(8))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(launcher::STEAM_LAUNCH_OPTION)
                                        .monospace()
                                        .color(SECONDARY_TEXT)
                                        .size(11.5),
                                )
                                .wrap(),
                            );
                        });
                }
                LoaderStatus::Stracker => {
                    ui.label(
                        RichText::new("Stracker's Loader detectado")
                            .color(INFORMATION)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Sekai Hunter inicia el juego mediante Steam con tus ajustes actuales. No se pudo verificar una opción de inicio de Proton específica para Stracker, por lo que la aplicación no añadirá ni copiará un comando no verificado.",
                        )
                        .color(SECONDARY_TEXT)
                        .small(),
                    );
                    ui.hyperlink_to(
                        "Consultar las notas de instalación de Stracker's Loader",
                        "https://www.nexusmods.com/monsterhunterworld/mods/1982",
                    );
                }
                LoaderStatus::Conflict => {
                    ui.label(
                        RichText::new("Se detectó un conflicto entre loaders")
                            .color(ERROR)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(
                            "Desactiva o elimina manualmente uno de los loaders y vuelve a escanear. Los plugins existentes no se modificarán.",
                        )
                        .color(SECONDARY_TEXT)
                        .small(),
                    );
                }
                LoaderStatus::AsteraIncomplete => {
                    ui.label(
                        RichText::new("La instalación de Astera Loader está incompleta.")
                            .color(WARNING)
                            .strong(),
                    );
                }
                LoaderStatus::StrackerIncomplete => {
                    ui.label(
                        RichText::new(
                            "Los archivos de Stracker's Loader están incompletos o son ambiguos.",
                        )
                        .color(WARNING)
                        .strong(),
                    );
                }
                LoaderStatus::Missing => {
                    ui.label(
                        RichText::new(
                            "Instala Astera Loader o Stracker's Loader para configurar el inicio.",
                        )
                        .color(SECONDARY_TEXT),
                    );
                }
            }
        });

        ui.add_space(14.0);
        draw_full_width_panel(ui, |ui| {
            ui.label(panel_heading("Aplicación"));
            ui.add_space(8.0);
            ui.label(
                RichText::new("Ubicación de configuración")
                    .color(SECONDARY_TEXT)
                    .small(),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(config::config_dir().to_string_lossy())
                        .monospace()
                        .color(PRIMARY_TEXT)
                        .size(12.0),
                )
                .wrap(),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "Tamaño de ventana guardado: {:.0} × {:.0}",
                    self.config.window_width, self.config.window_height
                ))
                .color(SECONDARY_TEXT)
                .small(),
            );
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if styled_danger_button(ui, "Borrar directorio guardado") {
                    self.config.game_dir = None;
                    self.config.save();
                    self.status = GameStatus::empty();
                    self.record("Se borró el directorio del juego guardado.");
                }
                ui.label(
                    RichText::new("Solo borra la ruta guardada; no modifica archivos del juego.")
                        .color(SECONDARY_TEXT)
                        .small(),
                );
            });
        });
    }

    fn draw_log(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} entradas", self.activity_log.len()))
                    .color(SECONDARY_TEXT)
                    .small(),
            );
            ui.label(
                RichText::new("Se conservan las 32 más recientes")
                    .color(SECONDARY_TEXT)
                    .small(),
            );
        });
        ui.add_space(10.0);

        draw_full_width_panel(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [176.0, 20.0],
                    egui::Label::new(
                        RichText::new("Fecha y hora")
                            .color(SECONDARY_TEXT)
                            .small()
                            .strong(),
                    ),
                );
                ui.add_sized(
                    [72.0, 20.0],
                    egui::Label::new(
                        RichText::new("Nivel")
                            .color(SECONDARY_TEXT)
                            .small()
                            .strong(),
                    ),
                );
                ui.label(
                    RichText::new("Mensaje")
                        .color(SECONDARY_TEXT)
                        .small()
                        .strong(),
                );
            });
            ui.separator();

            for entry in self.activity_log.iter().rev() {
                ui.horizontal_top(|ui| {
                    ui.add_sized(
                        [176.0, 20.0],
                        egui::Label::new(
                            RichText::new(&entry.timestamp)
                                .monospace()
                                .color(SECONDARY_TEXT)
                                .size(12.0),
                        ),
                    );
                    ui.add_sized(
                        [72.0, 20.0],
                        egui::Label::new(
                            RichText::new(entry.level.label())
                                .monospace()
                                .color(entry.level.color())
                                .size(12.0)
                                .strong(),
                        ),
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(localized_activity_message(&entry.message))
                                .monospace()
                                .color(PRIMARY_TEXT)
                                .size(12.5),
                        )
                        .wrap(),
                    );
                });
                ui.add_space(3.0);
                ui.separator();
                ui.add_space(3.0);
            }
        });
    }

    fn draw_mod_dialogs(&mut self, ctx: &egui::Context) {
        let mut install_action = None;
        let mut cancel_install = false;
        if let Some(preview) = &mut self.pending_mod {
            let is_owned = |path: &str| {
                self.mod_manager.as_ref().is_some_and(|manager| {
                    manager
                        .mods()
                        .iter()
                        .any(|managed| managed.files.iter().any(|file| file.relative_path == path))
                })
            };
            let replaceable_conflicts = preview
                .conflicts
                .iter()
                .filter(|path| !is_owned(path))
                .count();
            let mut shown_conflicts = preview.conflicts.clone();
            if let Some(manager) = &self.mod_manager {
                for file in &preview.files {
                    if manager.mods().iter().any(|managed| {
                        managed
                            .files
                            .iter()
                            .any(|owned| owned.relative_path == file.relative_path)
                    }) && !shown_conflicts.contains(&file.relative_path)
                    {
                        shown_conflicts.push(file.relative_path.clone());
                    }
                }
            }
            let total_conflicts = shown_conflicts.len();
            let safe_file_count = preview
                .files
                .iter()
                .filter(|file| !shown_conflicts.contains(&file.relative_path))
                .count();
            let has_astera_plugins = preview
                .files
                .iter()
                .any(|file| file.relative_path.starts_with("AsteraLoader/plugins/"));
            egui::Window::new("Revisar instalación del mod")
                    .collapsible(false)
                    .resizable(true)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.label(RichText::new(&preview.archive_name).color(PRIMARY_TEXT).strong());
                        ui.horizontal(|ui| {
                            ui.label("Nombre del mod:");
                            ui.text_edit_singleline(&mut preview.name);
                        });
                        ui.label(format!(
                            "Se instalarán {} archivos; se ignoraron {} entradas ajenas o no compatibles.",
                            preview.files.len(),
                            preview.ignored_entries
                        ));
                        if has_astera_plugins
                            && self.status.loader_status() == LoaderStatus::Stracker
                        {
                            ui.colored_label(
                                YELLOW_WARN,
                                "Este archivo incluye plugins de Astera Loader, que Stracker's Loader no carga.",
                            );
                        }
                        ui.label(RichText::new("Rutas de destino detectadas:").color(TEXT_SECONDARY));
                        egui::ScrollArea::vertical()
                            .max_height(150.0)
                            .show(ui, |ui| {
                                for file in preview.files.iter().take(40) {
                                    ui.label(
                                        RichText::new(&file.relative_path)
                                            .monospace()
                                            .size(11.0),
                                    );
                                }
                                if preview.files.len() > 40 {
                                    ui.label(format!("…y {} más", preview.files.len() - 40));
                                }
                            });
                        if !shown_conflicts.is_empty() {
                            ui.colored_label(
                                YELLOW_WARN,
                                format!(
                                    "Hay {} rutas en conflicto. Los mods gestionados no se reemplazan.",
                                    total_conflicts
                                ),
                            );
                            for path in shown_conflicts.iter().take(8) {
                                ui.label(
                                    RichText::new(path)
                                        .monospace()
                                        .color(TEXT_DIM)
                                        .size(10.0),
                                );
                            }
                            if shown_conflicts.len() > 8 {
                                ui.label(format!(
                                    "…y {} conflictos más",
                                    shown_conflicts.len() - 8
                                ));
                            }
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            if shown_conflicts.is_empty() {
                                if ui.button("Instalar mod").clicked() {
                                    install_action = Some(false);
                                }
                            } else {
                                if safe_file_count > 0
                                    && ui                                    .button("Instalar sin reemplazar").clicked()
                                {
                                    install_action = Some(false);
                                }
                                if replaceable_conflicts > 0
                                    && ui
                                        .button("Respaldar y reemplazar archivos existentes")
                                        .clicked()
                                {
                                    install_action = Some(true);
                                }
                            }
                            if ui.button("Cancelar").clicked() {
                                cancel_install = true;
                            }
                        });
                        if replaceable_conflicts > 0 && !shown_conflicts.is_empty() {
                            ui.label(
                                RichText::new("Los archivos existentes se respaldarán y restaurarán al desactivar o desinstalar este mod.")
                                    .color(TEXT_DIM)
                                    .small(),
                            );
                        }
                    });
        }

        if cancel_install {
            self.pending_mod = None;
            self.mod_message = Some(("Instalación del mod cancelada.".into(), false));
            self.prepare_next_archive(ctx);
        } else if let Some(overwrite) = install_action {
            let preview = self.pending_mod.take().expect("preview exists");
            self.start_manager_job(
                ctx,
                format!("Instalando {}…", preview.name),
                move |manager, game_dir| {
                    let (files, skipped) = manager.install(&preview, game_dir, overwrite)?;
                    Ok(ManagerAction::Installed { files, skipped })
                },
            );
        }

        let mut confirm_uninstall = false;
        let mut cancel_uninstall = false;
        if let Some(id) = &self.pending_uninstall {
            let name = self
                .mod_manager
                .as_ref()
                .and_then(|manager| manager.mods().iter().find(|managed| managed.id == *id))
                .map(|managed| managed.name.as_str())
                .unwrap_or("este mod");
            egui::Window::new("¿Desinstalar mod gestionado?")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.label(format!("¿Desinstalar {name}?"));
                        ui.label(
                            RichText::new("Solo se quitarán archivos sin cambios registrados por Sekai Hunter. Los archivos respaldados se restaurarán.")
                                .color(TEXT_SECONDARY)
                                .small(),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Desinstalar").clicked() {
                                confirm_uninstall = true;
                            }
                            if ui.button("Cancelar").clicked() {
                                cancel_uninstall = true;
                            }
                        });
                    });
        }
        if cancel_uninstall {
            self.pending_uninstall = None;
        } else if confirm_uninstall {
            if let Some(id) = self.pending_uninstall.take() {
                self.start_manager_job(
                    ctx,
                    "Desinstalando mod…".into(),
                    move |manager, game_dir| {
                        manager.uninstall(&id, game_dir)?;
                        Ok(ManagerAction::Uninstalled)
                    },
                );
            }
        }
    }
}

fn auto_detect_game() -> GameStatus {
    for candidate in config::common_steam_paths() {
        if candidate.join("MonsterHunterWorld.exe").exists() {
            log::info!("Auto-detected MHW at: {}", candidate.display());
            return GameStatus::scan(&candidate);
        }
    }
    log::info!("MHW not found in common Steam paths.");
    GameStatus::empty()
}

// ── egui visuals ───────────────────────────────────────────────────────
fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = SURFACE;
    visuals.override_text_color = Some(PRIMARY_TEXT);
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, SECONDARY_TEXT);
    visuals.widgets.inactive.bg_fill = SURFACE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, SECONDARY_TEXT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(CONTROL_RADIUS);
    visuals.widgets.hovered.bg_fill = ELEVATED_SURFACE;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, PRIMARY_TEXT);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(CONTROL_RADIUS);
    visuals.widgets.active.bg_fill = ELEVATED_SURFACE;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.active.corner_radius = CornerRadius::same(CONTROL_RADIUS);
    visuals.selection.bg_fill = INFORMATION.gamma_multiply(0.28);
    visuals.selection.stroke = Stroke::new(1.0_f32, INFORMATION);
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.open.bg_fill = SURFACE;
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, SECONDARY_TEXT);
    ctx.set_visuals(visuals);
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Inter-Regular".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/inter/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "Inter-Medium".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/inter/Inter-Medium.ttf"
        ))),
    );
    fonts.font_data.insert(
        "Inter-SemiBold".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/inter/Inter-SemiBold.ttf"
        ))),
    );
    fonts.font_data.insert(
        "Inter-Bold".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/inter/Inter-Bold.ttf"
        ))),
    );
    fonts.font_data.insert(
        "BarlowCondensed-Regular".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/barlow-condensed/BarlowCondensed-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "BarlowCondensed-SemiBold".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/barlow-condensed/BarlowCondensed-SemiBold.ttf"
        ))),
    );
    fonts.families.insert(
        FontFamily::Proportional,
        vec![
            "Inter-Regular".into(),
            "Inter-Medium".into(),
            "Inter-SemiBold".into(),
            "Inter-Bold".into(),
            "Ubuntu-Light".into(),
            "NotoEmoji-Regular".into(),
        ],
    );
    fonts.families.insert(
        FontFamily::Name("Barlow Condensed".into()),
        vec![
            "BarlowCondensed-SemiBold".into(),
            "BarlowCondensed-Regular".into(),
        ],
    );
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(
        egui::TextStyle::Heading,
        FontId::new(22.0, FontFamily::Name("Barlow Condensed".into())),
    );
    style.text_styles.insert(
        egui::TextStyle::Body,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        FontId::new(12.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(13.0));
    style.text_styles.insert(
        egui::TextStyle::Button,
        FontId::new(13.0, FontFamily::Proportional),
    );
    style.spacing.item_spacing = Vec2::new(8.0, 7.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.spacing.scroll = egui::style::ScrollStyle {
        bar_width: 4.0,
        handle_min_length: 24.0,
        bar_inner_margin: 3.0,
        bar_outer_margin: 4.0,
        ..egui::style::ScrollStyle::solid()
    };
    ctx.set_style(style);
}

// ── Main update loop ───────────────────────────────────────────────────
impl eframe::App for SekaiHunterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_background_events(ctx);
        let dropped_files = ctx.input(|input| input.raw.dropped_files.clone());
        for dropped in dropped_files {
            if let Some(path) = dropped.path {
                self.queue_archive(path);
            }
        }
        self.prepare_next_archive(ctx);
        if self.worker_busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if let Some(rect) = ctx.input(|input| input.viewport().inner_rect) {
            self.config.window_width = rect.width();
            self.config.window_height = rect.height();
        }

        egui::SidePanel::left("sekai-navigation")
            .exact_width(SIDEBAR_WIDTH)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(SURFACE)
                    .inner_margin(Margin::symmetric(12, 16)),
            )
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        draw_brand_mark(ui);
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new("SEKAI HUNTER")
                                .font(FontId::new(
                                    22.0,
                                    FontFamily::Name("Barlow Condensed".into()),
                                ))
                                .color(PRIMARY_TEXT)
                                .strong(),
                        );
                        ui.label(
                            RichText::new("MOD MANAGER")
                                .color(SECONDARY_TEXT)
                                .size(10.0),
                        );
                    });
                    ui.add_space(22.0);
                    for section in AppSection::ALL {
                        let count = match section {
                            AppSection::Library => self
                                .mod_manager
                                .as_ref()
                                .map(|manager| manager.mods().len()),
                            AppSection::Plugins => Some(self.status.plugin_count),
                            AppSection::Diagnostics => Some(self.status.diagnostics.len()),
                            AppSection::Settings | AppSection::Log => None,
                        };
                        if draw_nav_item(
                            ui,
                            section.nav_label(),
                            section.icon(),
                            self.selected_section == section,
                            count,
                        ) {
                            self.selected_section = section;
                        }
                    }
                    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("MONSTER HUNTER: WORLD")
                                .color(SECONDARY_TEXT)
                                .size(11.0),
                        );
                        ui.label(
                            RichText::new("NATIVE LINUX")
                                .color(SECONDARY_TEXT)
                                .size(11.0),
                        );
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new("MOD MANAGEMENT INSPIRED BY")
                                .color(SECONDARY_TEXT)
                                .size(10.0),
                        );
                        ui.hyperlink_to(
                            RichText::new("MHW Mod Manager · BoltMan")
                                .color(SECONDARY_TEXT)
                                .size(10.0),
                            "https://github.com/UncleClapton/MHWModManager",
                        );
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(Frame::central_panel(&ctx.style()).fill(BACKGROUND))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Let the scroll area measure the natural height of the selected
                    // section. A fixed viewport-height child clips long pages and keeps
                    // the scrollbar from representing their full content.
                    ui.add_space(24.0);
                    if self.selected_section != AppSection::Library {
                        draw_section_title(ui, self.selected_section.title());
                        ui.add_space(18.0);
                    }

                    if self.selected_section == AppSection::Library {
                        self.draw_library_dashboard(ui, ctx);
                        self.draw_mod_library(ui, ctx);
                    }

                    if self.selected_section == AppSection::Plugins {
                        self.draw_plugins(ui);
                    }

                    if self.selected_section == AppSection::Diagnostics {
                        self.draw_diagnostics(ui);
                    }

                    if self.selected_section == AppSection::Settings {
                        self.draw_settings(ui, ctx);
                    }

                    if self.selected_section == AppSection::Log {
                        self.draw_log(ui);
                    }
                });
            });

        self.draw_mod_dialogs(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.config.save();
    }
}

// ── Drawing helpers ────────────────────────────────────────────────────

fn draw_section_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        RichText::new(title)
            .font(FontId::new(
                24.0,
                FontFamily::Name("Barlow Condensed".into()),
            ))
            .color(PRIMARY_TEXT)
            .strong(),
    );
}

fn mod_matches_query(managed: &ManagedMod, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || managed.name.to_lowercase().contains(&query)
        || managed.archive_name.to_lowercase().contains(&query)
        || managed.id.to_lowercase().contains(&query)
        || managed
            .files
            .iter()
            .any(|file| file.relative_path.to_lowercase().contains(&query))
}

fn mod_matches_filter(managed: &ManagedMod, filter: ModFilter) -> bool {
    match filter {
        ModFilter::All => true,
        ModFilter::Enabled => managed.enabled,
        ModFilter::Disabled => !managed.enabled,
    }
}

fn archive_version(archive_name: &str) -> Option<String> {
    let stem = std::path::Path::new(archive_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(archive_name);

    stem.split(|character: char| !character.is_ascii_alphanumeric() && character != '.')
        .find_map(|token| {
            let version = token
                .strip_prefix("version")
                .or_else(|| token.strip_prefix('v'))
                .or_else(|| token.strip_prefix('V'))
                .unwrap_or(token);
            let components = version.split('.').collect::<Vec<_>>();
            let is_numeric_version = (2..=4).contains(&components.len())
                && components.iter().all(|component| {
                    !component.is_empty()
                        && component.len() <= 3
                        && component.bytes().all(|byte| byte.is_ascii_digit())
                });
            is_numeric_version.then(|| format!("v{version}"))
        })
}

fn mod_metadata(managed: &ManagedMod) -> String {
    let mut parts = Vec::with_capacity(4);
    if let Some(version) = archive_version(&managed.archive_name) {
        parts.push(version);
    }
    parts.push(format!("{} archivos", managed.files.len()));
    parts.push(if managed.enabled {
        "Activo".to_owned()
    } else {
        "Desactivado".to_owned()
    });
    if managed.skipped_conflicts > 0 {
        parts.push(format!("{} conflictos omitidos", managed.skipped_conflicts));
    }
    parts.join(" · ")
}

fn display_mod_name(managed: &ManagedMod) -> &str {
    if let Some(prefix) = archive_title_before_timestamp(&managed.name) {
        if prefix.chars().any(char::is_alphabetic) {
            return prefix;
        }
    } else if !has_timestamp_or_long_id(&managed.name) {
        return &managed.name;
    }

    managed
        .files
        .iter()
        .filter_map(|file| {
            let path = std::path::Path::new(&file.relative_path);
            let is_dll = path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"));
            if !is_dll {
                return None;
            }

            let stem = path.file_stem()?.to_str()?;
            let lower = stem.to_ascii_lowercase();
            (!["dinput8", "loader", "stracker", "asteraloader", "version"]
                .contains(&lower.as_str())
                && !has_timestamp_or_long_id(stem))
            .then_some(stem)
        })
        .next()
        .unwrap_or(&managed.name)
}

fn archive_title_before_timestamp(value: &str) -> Option<&str> {
    let date_start = archive_timestamp_start(value)?;
    let mut end = value[..date_start].trim_end().len();
    let mut removed_numeric_suffixes = 0;
    loop {
        let prefix = value[..end].trim_end();
        let token_start = prefix
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        let token = &prefix[token_start..];
        let is_numeric_suffix = token.chars().any(|character| character.is_ascii_digit())
            && token
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.');
        if !is_numeric_suffix {
            end = prefix.len();
            break;
        }
        removed_numeric_suffixes += 1;
        end = token_start;
    }

    let prefix = value[..end].trim_end_matches([' ', '-', '_', '.']);
    if removed_numeric_suffixes >= 2 && !prefix.is_empty() {
        Some(prefix)
    } else {
        let full_prefix = value[..date_start].trim_end_matches([' ', '-', '_', '.']);
        (!full_prefix.is_empty()).then_some(full_prefix)
    }
}

fn archive_timestamp_start(value: &str) -> Option<usize> {
    value.as_bytes().windows(10).position(|window| {
        window[0..4].iter().all(u8::is_ascii_digit)
            && matches!(window[4], b'-' | b'_')
            && window[5..7].iter().all(u8::is_ascii_digit)
            && matches!(window[7], b'-' | b'_')
            && window[8..10].iter().all(u8::is_ascii_digit)
    })
}

fn has_timestamp_or_long_id(value: &str) -> bool {
    if archive_timestamp_start(value).is_some() {
        return true;
    }

    let mut consecutive_digits = 0;
    for byte in value.bytes().chain(std::iter::once(b' ')) {
        if byte.is_ascii_digit() {
            consecutive_digits += 1;
            if consecutive_digits >= 9 {
                return true;
            }
        } else {
            consecutive_digits = 0;
        }
    }
    false
}

fn library_filter_button(
    ui: &mut egui::Ui,
    label: &str,
    filter: ModFilter,
    selected_filter: &mut ModFilter,
) {
    let selected = *selected_filter == filter;
    let response = ui.add(
        egui::Button::new(
            RichText::new(label)
                .color(if selected {
                    PRIMARY_TEXT
                } else {
                    SECONDARY_TEXT
                })
                .size(12.0)
                .strong(),
        )
        .fill(if selected {
            ELEVATED_SURFACE
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(5))
        .stroke(Stroke::new(
            1.0_f32,
            if selected {
                INFORMATION.gamma_multiply(0.65)
            } else {
                Color32::TRANSPARENT
            },
        )),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label)
    });
    if response.clicked() {
        *selected_filter = filter;
    }
}

fn library_primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(
            RichText::new(label)
                .color(if enabled { BACKGROUND } else { SECONDARY_TEXT })
                .size(13.0)
                .strong(),
        )
        .fill(if enabled { ACCENT } else { DISABLED_SURFACE })
        .corner_radius(CornerRadius::same(5))
        .stroke(Stroke::NONE),
    )
}

fn draw_managed_mod_row(ui: &mut egui::Ui, managed: &ManagedMod) -> Option<ModRowAction> {
    let row_width = ui.available_width();
    let row_height = 62.0;
    let inner_height = row_height - 8.0;
    let content_width = (row_width - 24.0).max(0.0);
    let controls_width = 80.0_f32.min(content_width * 0.32);
    let columns_width = (content_width - controls_width).max(0.0);
    let name_column_width = (columns_width * 0.46).max(0.0);
    let metadata_column_width = (columns_width - name_column_width).max(0.0);
    let display_name = display_mod_name(managed);
    let metadata = mod_metadata(managed);

    let (rect, row_response) =
        ui.allocate_exact_size(Vec2::new(row_width, row_height), egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(4),
        if row_response.hovered() {
            ELEVATED_SURFACE
        } else {
            Color32::TRANSPARENT
        },
    );

    let mut action = None;
    let row_id = managed.id.clone();
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(12.0, 4.0)))
            .layout(Layout::left_to_right(Align::Center)),
        |row_ui| {
            row_ui.spacing_mut().item_spacing.x = 0.0;
            row_ui.allocate_ui_with_layout(
                Vec2::new(name_column_width, inner_height),
                Layout::left_to_right(Align::Center),
                |name_ui| {
                    let (icon_rect, _) =
                        name_ui.allocate_exact_size(Vec2::new(24.0, 28.0), egui::Sense::hover());
                    draw_mod_icon(name_ui.painter(), icon_rect, managed.enabled);
                    name_ui.add_space(8.0);
                    let name_width = (name_ui.available_width() - 1.0).max(0.0);
                    let name_response = name_ui.add_sized(
                        Vec2::new(name_width, 28.0),
                        egui::Label::new(
                            RichText::new(display_name)
                                .color(PRIMARY_TEXT)
                                .strong()
                                .size(14.0),
                        )
                        .truncate(),
                    );
                    name_response.on_hover_text(format!(
                        "Nombre guardado: {}\nArchivo original: {}\nIdentificador: {}",
                        managed.name, managed.archive_name, managed.id
                    ));
                },
            );
            row_ui.allocate_ui_with_layout(
                Vec2::new(metadata_column_width, inner_height),
                Layout::left_to_right(Align::Center),
                |metadata_ui| {
                    let metadata_response = metadata_ui.add_sized(
                        Vec2::new(metadata_ui.available_width(), 28.0),
                        egui::Label::new(RichText::new(&metadata).color(SECONDARY_TEXT).size(11.0))
                            .truncate(),
                    );
                    metadata_response.on_hover_text(format!(
                        "Archivo original: {}\nIdentificador: {}",
                        managed.archive_name, managed.id
                    ));
                },
            );
            row_ui.allocate_ui_with_layout(
                Vec2::new(controls_width, inner_height),
                Layout::right_to_left(Align::Center),
                |actions_ui| {
                    let menu = actions_ui.scope(|menu_ui| {
                        let visuals = menu_ui.visuals_mut();
                        visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
                        visuals.widgets.inactive.bg_stroke = Stroke::NONE;
                        visuals.widgets.hovered.bg_fill = ELEVATED_SURFACE;
                        visuals.widgets.hovered.bg_stroke = Stroke::NONE;
                        visuals.widgets.active.bg_fill = ELEVATED_SURFACE;
                        visuals.widgets.active.bg_stroke = Stroke::NONE;
                        menu_ui.menu_button(" ", |menu_ui| {
                            if menu_ui.button("Desinstalar…").clicked() {
                                action = Some(ModRowAction::Uninstall(row_id.clone()));
                                menu_ui.close_menu();
                            }
                        })
                    });
                    let menu_response = menu.inner.response.on_hover_text("Más acciones");
                    for x in [-4.0, 0.0, 4.0] {
                        actions_ui.painter().circle_filled(
                            menu_response.rect.center() + Vec2::new(x, 0.0),
                            1.25,
                            SECONDARY_TEXT,
                        );
                    }
                    if menu_response.has_focus() {
                        actions_ui.painter().rect_stroke(
                            menu_response.rect.shrink(1.0),
                            CornerRadius::same(CONTROL_RADIUS),
                            Stroke::new(1.0_f32, ACCENT),
                            egui::StrokeKind::Inside,
                        );
                    }

                    let toggle_response = draw_mod_switch(actions_ui, managed.enabled);
                    if toggle_response.clicked()
                        || (toggle_response.has_focus()
                            && actions_ui.input(|input| {
                                input.key_pressed(egui::Key::Space)
                                    || input.key_pressed(egui::Key::Enter)
                            }))
                    {
                        action = Some(ModRowAction::Toggle {
                            id: managed.id.clone(),
                            enabled: !managed.enabled,
                        });
                    }
                },
            );
        },
    );
    action
}

fn draw_mod_icon(painter: &egui::Painter, rect: egui::Rect, enabled: bool) {
    let stroke = Stroke::new(1.3_f32, if enabled { SUCCESS } else { SECONDARY_TEXT });
    let icon_rect = egui::Rect::from_center_size(rect.center(), Vec2::new(17.0, 20.0));
    painter.rect_stroke(
        icon_rect,
        CornerRadius::same(2),
        stroke,
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [
            icon_rect.left_top() + Vec2::new(3.0, 5.0),
            icon_rect.right_top() + Vec2::new(-3.0, 5.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            icon_rect.left_top() + Vec2::new(5.0, 10.0),
            icon_rect.right_top() + Vec2::new(-5.0, 10.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            icon_rect.left_top() + Vec2::new(5.0, 14.0),
            icon_rect.right_top() + Vec2::new(-5.0, 14.0),
        ],
        stroke,
    );
}

fn draw_mod_switch(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let (hit_rect, response) = ui.allocate_exact_size(Vec2::new(44.0, 32.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            true,
            enabled,
            if enabled {
                "Desactivar mod"
            } else {
                "Activar mod"
            },
        )
    });
    let track = if enabled && response.is_pointer_button_down_on() {
        ACCENT.gamma_multiply(0.82)
    } else if enabled {
        ACCENT
    } else if response.hovered() {
        ELEVATED_SURFACE
    } else {
        DISABLED_SURFACE
    };
    let rect = egui::Rect::from_center_size(hit_rect.center(), Vec2::new(36.0, 20.0));
    ui.painter()
        .rect_filled(rect, CornerRadius::same(10), track);
    if !enabled {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(10),
            Stroke::new(1.0_f32, BORDER),
            egui::StrokeKind::Inside,
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            hit_rect.shrink(1.0),
            CornerRadius::same(CONTROL_RADIUS),
            Stroke::new(1.0_f32, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    let knob_x = if enabled {
        rect.right() - 10.0
    } else {
        rect.left() + 10.0
    };
    ui.painter().circle_filled(
        egui::pos2(knob_x, rect.center().y),
        7.0,
        if enabled { BACKGROUND } else { PRIMARY_TEXT },
    );
    response.on_hover_text(if enabled {
        "Desactivar mod"
    } else {
        "Activar mod"
    })
}

fn localized_worker_message(message: &str) -> String {
    for (prefix, translation) in [
        ("Reading ", "Leyendo "),
        ("Installing ", "Instalando "),
        ("Enabling ", "Activando "),
        ("Disabling ", "Desactivando "),
        ("Uninstalling ", "Desinstalando "),
        ("Instalando ", "Instalando "),
        ("Activando ", "Activando "),
        ("Desactivando ", "Desactivando "),
        ("Desinstalando ", "Desinstalando "),
    ] {
        if let Some(name) = message.strip_prefix(prefix) {
            return format!("{translation}{name}");
        }
    }

    match message {
        "Opening archive picker…" => "Seleccionando archivo…".into(),
        "Opening folder picker…" => "Seleccionando carpeta…".into(),
        "Checking game installation…" => "Comprobando la instalación del juego…".into(),
        "Refreshing game and plugin status…" => {
            "Actualizando el estado del juego y los plugins…".into()
        }
        _ => "Procesando en segundo plano…".into(),
    }
}

fn panel_heading(title: &str) -> RichText {
    RichText::new(title)
        .font(FontId::new(
            18.0,
            FontFamily::Name("Barlow Condensed".into()),
        ))
        .color(PRIMARY_TEXT)
        .strong()
}

fn draw_nav_item(
    ui: &mut egui::Ui,
    label: &str,
    icon: NavIcon,
    selected: bool,
    count: Option<usize>,
) -> bool {
    let response = ui.add_sized(
        Vec2::new(ui.available_width(), 40.0),
        egui::Button::new("")
            .fill(if selected {
                ACCENT
            } else {
                Color32::TRANSPARENT
            })
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(CONTROL_RADIUS))
            .selected(selected),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label)
    });

    let active = selected || response.hovered();
    let icon_color = if selected {
        BACKGROUND
    } else if active {
        PRIMARY_TEXT
    } else {
        SECONDARY_TEXT
    };
    let label_color = if selected {
        BACKGROUND
    } else if active {
        PRIMARY_TEXT
    } else {
        SECONDARY_TEXT
    };
    let rect = response.rect;
    let painter = ui.painter();
    if response.hovered() && !selected {
        painter.rect_filled(
            rect,
            CornerRadius::same(CONTROL_RADIUS),
            ELEVATED_SURFACE.gamma_multiply(0.7),
        );
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect.shrink(1.0),
            CornerRadius::same(CONTROL_RADIUS),
            Stroke::new(1.0_f32, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    draw_nav_icon(
        painter,
        rect.left_center() + Vec2::new(19.0, 0.0),
        icon,
        icon_color,
    );
    painter.text(
        rect.left_center() + Vec2::new(38.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        label_color,
    );
    if let Some(count) = count {
        let badge_center = rect.right_center() - Vec2::new(16.0, 0.0);
        painter.circle_filled(
            badge_center,
            9.0,
            if selected {
                BACKGROUND.gamma_multiply(0.12)
            } else {
                ELEVATED_SURFACE
            },
        );
        painter.text(
            badge_center,
            Align2::CENTER_CENTER,
            count.to_string(),
            FontId::proportional(10.0),
            if selected { BACKGROUND } else { SECONDARY_TEXT },
        );
    }
    response.clicked()
}

fn draw_brand_mark(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(42.0, 42.0), egui::Sense::hover());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "S",
        FontId::new(40.0, FontFamily::Name("Barlow Condensed".into())),
        ACCENT,
    );
    ui.painter()
        .circle_filled(rect.center() + Vec2::new(13.0, -13.0), 2.5, SUCCESS);
}

fn draw_status_pill(ui: &mut egui::Ui, label: &str, color: Color32) {
    Frame::new()
        .fill(ELEVATED_SURFACE)
        .corner_radius(CornerRadius::same(CONTROL_RADIUS))
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(7.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 3.0, color);
                ui.label(RichText::new(label).color(PRIMARY_TEXT).size(11.0));
            });
        });
}

fn library_hero_title(game_found: bool, loader_status: LoaderStatus) -> &'static str {
    if !game_found {
        return "Configura tu instalación";
    }

    match loader_status {
        LoaderStatus::Astera | LoaderStatus::Stracker => "Todo listo para cazar",
        LoaderStatus::Conflict => "Revisa los loaders",
        LoaderStatus::AsteraIncomplete | LoaderStatus::StrackerIncomplete => {
            "Completa la instalación"
        }
        LoaderStatus::Missing => "Prepara tu instalación",
    }
}

fn loader_status_label(loader_status: LoaderStatus) -> &'static str {
    match loader_status {
        LoaderStatus::Astera => "Astera listo",
        LoaderStatus::Stracker => "Stracker listo",
        LoaderStatus::Conflict => "Conflicto de loaders",
        LoaderStatus::AsteraIncomplete => "Astera incompleto",
        LoaderStatus::StrackerIncomplete => "Stracker incompleto",
        LoaderStatus::Missing => "Loader no detectado",
    }
}

fn loader_status_color(loader_status: LoaderStatus) -> Color32 {
    match loader_status {
        LoaderStatus::Astera | LoaderStatus::Stracker => SUCCESS,
        LoaderStatus::Conflict => ERROR,
        LoaderStatus::AsteraIncomplete | LoaderStatus::StrackerIncomplete => WARNING,
        LoaderStatus::Missing => SECONDARY_TEXT,
    }
}

fn launch_blocked_message(loader_status: LoaderStatus) -> &'static str {
    match loader_status {
        LoaderStatus::Conflict => "Resuelve el conflicto entre loaders antes de iniciar.",
        LoaderStatus::AsteraIncomplete => "La instalación de Astera Loader está incompleta.",
        LoaderStatus::StrackerIncomplete => "La instalación de Stracker's Loader está incompleta.",
        LoaderStatus::Missing => "Instala Astera Loader o Stracker's Loader para iniciar.",
        LoaderStatus::Astera | LoaderStatus::Stracker => "No se detectó Monster Hunter: World.",
    }
}

fn draw_nav_icon(painter: &egui::Painter, center: egui::Pos2, icon: NavIcon, color: Color32) {
    let stroke = Stroke::new(1.5_f32, color);
    match icon {
        NavIcon::Library => {
            let rect = egui::Rect::from_center_size(center, Vec2::new(12.0, 14.0));
            painter.rect_stroke(
                rect,
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    rect.left_top() + Vec2::new(3.0, 4.5),
                    rect.right_top() + Vec2::new(-3.0, 4.5),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    rect.left_top() + Vec2::new(3.0, 8.0),
                    rect.right_top() + Vec2::new(-3.0, 8.0),
                ],
                stroke,
            );
        }
        NavIcon::Plugins => {
            for offset in [
                Vec2::new(-3.0, -3.0),
                Vec2::new(3.0, -3.0),
                Vec2::new(-3.0, 3.0),
                Vec2::new(3.0, 3.0),
            ] {
                painter.circle_stroke(center + offset, 2.0, stroke);
            }
        }
        NavIcon::Diagnostics => {
            painter.circle_stroke(center, 6.0, stroke);
            painter.line_segment(
                [center + Vec2::new(0.0, -3.0), center + Vec2::new(0.0, 1.0)],
                stroke,
            );
            painter.circle_filled(center + Vec2::new(0.0, 3.0), 0.8, color);
        }
        NavIcon::Settings => {
            for (x, y) in [(-4.0, -3.0), (0.0, 3.0), (4.0, -3.0)] {
                painter.line_segment(
                    [center + Vec2::new(-6.0, y), center + Vec2::new(6.0, y)],
                    stroke,
                );
                painter.circle_filled(center + Vec2::new(x, y), 1.6, SURFACE);
                painter.circle_stroke(center + Vec2::new(x, y), 1.6, stroke);
            }
        }
        NavIcon::Log => {
            for y in [-4.0, 0.0, 4.0] {
                painter.line_segment(
                    [center + Vec2::new(-6.0, y), center + Vec2::new(6.0, y)],
                    stroke,
                );
            }
        }
    }
}

fn draw_status_cards(ui: &mut egui::Ui, status: &GameStatus) {
    let loader_status = match status.loader_status() {
        LoaderStatus::Astera => "Astera listo",
        LoaderStatus::Stracker => "Stracker listo",
        LoaderStatus::Conflict => "Conflicto",
        LoaderStatus::AsteraIncomplete => "Astera incompleto",
        LoaderStatus::StrackerIncomplete => "Stracker incompleto",
        LoaderStatus::Missing => "No detectado",
    };
    let game_status = if status.mhw_exe_found {
        "Detectado"
    } else {
        "No encontrado"
    };
    let plugin_count = format!("{} plugins", status.plugin_count);

    if ui.available_width() < 560.0 {
        draw_status_card(ui, "JUEGO", game_status, status.mhw_exe_found);
        ui.columns(2, |columns| {
            draw_status_card(&mut columns[0], "LOADER", loader_status, status.is_ready());
            draw_status_card(&mut columns[1], "PLUGINS", &plugin_count, status.is_ready());
        });
    } else {
        ui.columns(3, |columns| {
            draw_status_card(&mut columns[0], "JUEGO", game_status, status.mhw_exe_found);
            draw_status_card(&mut columns[1], "LOADER", loader_status, status.is_ready());
            draw_status_card(&mut columns[2], "PLUGINS", &plugin_count, status.is_ready());
        });
    }
}

fn draw_status_card(ui: &mut egui::Ui, title: &str, value: &str, ok: bool) {
    let color = if ok { GREEN_OK } else { RED_ERR };

    Frame::new()
        .fill(BG_PANEL)
        .corner_radius(CornerRadius::same(PANEL_RADIUS))
        .inner_margin(Margin::symmetric(12, 10))
        .stroke(Stroke::new(1.0_f32, BORDER))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical(|ui| {
                ui.label(RichText::new(title).color(TEXT_DIM).size(12.0).strong());
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let (indicator, _) =
                        ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                    ui.painter().circle_filled(indicator.center(), 4.0, color);
                    ui.label(RichText::new(value).color(TEXT_PRIMARY).size(15.0).strong());
                });
            });
        });
}

fn draw_full_width_panel(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    // Explicitly fill the available inner width so short sections do not shrink
    // to their contents and become misaligned with the panels above them.
    let content_width = (ui.available_width() - 30.0).max(0.0);
    Frame::new()
        .fill(BG_PANEL)
        .corner_radius(CornerRadius::same(PANEL_RADIUS))
        .inner_margin(Margin::same(14))
        .stroke(Stroke::new(1.0_f32, BORDER))
        .show(ui, |ui| {
            ui.set_width(content_width);
            add_contents(ui);
        });
}

fn draw_launch_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let (bg, text_col) = if enabled {
        (BUTTON_BG, BG_DARK)
    } else {
        (BUTTON_DISABLED, TEXT_DIM)
    };

    let desired_size = Vec2::new(300.0_f32.min(ui.available_width()), 48.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(desired_size, sense);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            enabled,
            if enabled {
                "Iniciar Monster Hunter: World"
            } else {
                "Inicio no disponible"
            },
        )
    });

    let painter = ui.painter();
    let fill = if response.is_pointer_button_down_on() && enabled {
        ACCENT.gamma_multiply(0.82)
    } else if response.hovered() && enabled {
        BUTTON_HOVER
    } else {
        bg
    };

    painter.rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), fill);
    if response.has_focus() && enabled {
        painter.rect_stroke(
            rect.shrink(1.0),
            CornerRadius::same(CONTROL_RADIUS),
            Stroke::new(1.0_f32, PRIMARY_TEXT),
            egui::StrokeKind::Inside,
        );
    }

    let label = if enabled {
        "INICIAR MONSTER HUNTER: WORLD"
    } else {
        "INICIO NO DISPONIBLE"
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(15.0),
        text_col,
    );

    response.on_hover_text(if enabled {
        "Iniciar Monster Hunter: World mediante Steam"
    } else {
        "Configura el juego y un loader compatible para iniciar"
    })
}

fn styled_small_button(ui: &mut egui::Ui, text: &str) -> bool {
    let response = ui.add(
        egui::Button::new(RichText::new(text).color(PRIMARY_TEXT).size(13.0))
            .fill(ELEVATED_SURFACE)
            .corner_radius(CornerRadius::same(CONTROL_RADIUS))
            .min_size(Vec2::new(0.0, 34.0))
            .stroke(Stroke::new(1.0_f32, BORDER)),
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.shrink(1.0),
            CornerRadius::same(CONTROL_RADIUS),
            Stroke::new(1.0_f32, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    response.clicked()
}

fn styled_danger_button(ui: &mut egui::Ui, text: &str) -> bool {
    let response = ui.add(
        egui::Button::new(RichText::new(text).color(ERROR).size(13.0))
            .fill(ELEVATED_SURFACE)
            .corner_radius(CornerRadius::same(CONTROL_RADIUS))
            .min_size(Vec2::new(0.0, 34.0))
            .stroke(Stroke::new(1.0_f32, ERROR.gamma_multiply(0.55))),
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.shrink(1.0),
            CornerRadius::same(CONTROL_RADIUS),
            Stroke::new(1.0_f32, ERROR),
            egui::StrokeKind::Inside,
        );
    }
    response.clicked()
}

fn draw_plugin_directory_row(ui: &mut egui::Ui, name: &str, detected: bool) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(name)
                .color(PRIMARY_TEXT)
                .monospace()
                .size(12.0),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(if detected {
                    "Directorio detectado"
                } else {
                    "No detectado"
                })
                .color(if detected { SUCCESS } else { SECONDARY_TEXT })
                .small(),
            );
        });
    });
}

fn draw_plugin_row(ui: &mut egui::Ui, plugin: &str) {
    let (loader, plugin_name, filename, path) = plugin_display_info(plugin);
    let row_width = ui.available_width();
    let row_size = Vec2::new(row_width, 52.0);
    let (rect, response) = ui.allocate_exact_size(row_size, egui::Sense::hover());

    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(4),
            ELEVATED_SURFACE.gamma_multiply(0.55),
        );
    }

    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(10.0, 4.0)))
            .layout(Layout::left_to_right(Align::Center)),
        |row_ui| {
            row_ui.spacing_mut().item_spacing.x = 0.0;
            let available_width = row_ui.available_width();
            let first_column = (available_width * 0.52).max(0.0);
            let second_column = (available_width * 0.23).max(0.0);
            let third_column = (available_width - first_column - second_column).max(0.0);

            row_ui.allocate_ui_with_layout(
                Vec2::new(first_column, rect.height()),
                Layout::top_down(Align::Min),
                |column| {
                    column
                        .add(
                            egui::Label::new(
                                RichText::new(plugin_name)
                                    .color(PRIMARY_TEXT)
                                    .strong()
                                    .size(13.0),
                            )
                            .truncate(),
                        )
                        .on_hover_text(plugin);
                    column
                        .add(
                            egui::Label::new(
                                RichText::new(filename)
                                    .monospace()
                                    .color(SECONDARY_TEXT)
                                    .size(12.0),
                            )
                            .truncate(),
                        )
                        .on_hover_text(path);
                },
            );
            row_ui.allocate_ui_with_layout(
                Vec2::new(second_column, rect.height()),
                Layout::left_to_right(Align::Center),
                |column| {
                    column.label(
                        RichText::new(loader.unwrap_or("No identificado"))
                            .color(INFORMATION)
                            .size(12.0),
                    );
                },
            );
            row_ui.allocate_ui_with_layout(
                Vec2::new(third_column, rect.height()),
                Layout::right_to_left(Align::Center),
                |column| {
                    column.label(
                        RichText::new("Detectado")
                            .color(SUCCESS)
                            .size(12.0)
                            .strong(),
                    );
                },
            );
        },
    );
}

fn plugin_display_info(plugin: &str) -> (Option<&str>, &str, &str, &str) {
    let (loader, path) = plugin
        .split_once(": ")
        .map_or((None, plugin), |(loader, path)| (Some(loader), path));
    let plugin_path = std::path::Path::new(path);
    let filename = plugin_path
        .file_name()
        .and_then(|filename| filename.to_str())
        .filter(|filename| !filename.is_empty())
        .unwrap_or(path);
    let name = plugin_path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path);
    (loader, name, filename, path)
}

fn draw_diagnostic_group(
    ui: &mut egui::Ui,
    title: &str,
    diagnostics: &[&crate::scanner::Diagnostic],
    color: Color32,
) {
    if diagnostics.is_empty() {
        return;
    }

    draw_full_width_panel(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).color(color).strong());
            ui.label(
                RichText::new(diagnostics.len().to_string())
                    .color(SECONDARY_TEXT)
                    .small(),
            );
        });
        ui.add_space(6.0);
        for diagnostic in diagnostics {
            ui.horizontal_top(|ui| {
                ui.add_sized(
                    [88.0, 20.0],
                    egui::Label::new(
                        RichText::new(match diagnostic.level {
                            crate::scanner::DiagLevel::Error => "ERROR",
                            crate::scanner::DiagLevel::Warning => "AVISO",
                            crate::scanner::DiagLevel::Info => "INFO",
                        })
                        .color(color)
                        .monospace()
                        .small()
                        .strong(),
                    ),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(localized_diagnostic_message(&diagnostic.message))
                            .color(PRIMARY_TEXT)
                            .size(12.5),
                    )
                    .wrap(),
                );
            });
            ui.add_space(6.0);
        }
    });
    ui.add_space(10.0);
}

fn localized_diagnostic_message(message: &str) -> Cow<'_, str> {
    match message {
        "MonsterHunterWorld.exe not found in game directory." => {
            return "No se encontró MonsterHunterWorld.exe en el directorio del juego.".into();
        }
        "AsteraLoader.exe not found. Download it from the Astera Loader GitHub." => {
            return "No se encontró AsteraLoader.exe. Descárgalo desde GitHub de Astera Loader."
                .into();
        }
        "AsteraLoader/ directory not found. It should be extracted alongside AsteraLoader.exe." => {
            return "No se encontró AsteraLoader/. Debe extraerse junto a AsteraLoader.exe.".into();
        }
        "Only part of Stracker's legacy loader was detected (dinput8.dll and loader.dll). Verify the installation before launching." => {
            return "Solo se detectó parte del loader heredado de Stracker (dinput8.dll y loader.dll). Verifica la instalación antes de iniciar.".into();
        }
        "No supported loader detected. Install Astera Loader or Stracker's Loader." => {
            return "No se detectó un loader compatible. Instala Astera Loader o Stracker's Loader."
                .into();
        }
        "Game and detected loader are ready." => {
            return "El juego y el loader detectado están listos.".into();
        }
        _ => {}
    }

    for (prefix, translation) in [
        ("Directory does not exist: ", "El directorio no existe: "),
        ("Could not read ", "No se pudo leer "),
    ] {
        if let Some(remainder) = message.strip_prefix(prefix) {
            if prefix == "Could not read " {
                if let Some((path, error)) = remainder.split_once(": ") {
                    return format!("{translation}{path}: {error}").into();
                }
            }
            return format!("{translation}{remainder}").into();
        }
    }

    if message.starts_with("Astera Loader and Stracker's Loader files are both present.") {
        return "Se detectaron archivos de Astera Loader y Stracker's Loader. La documentación de Astera indica que el loader heredado puede iniciarse junto a Astera; desactiva o elimina Stracker's Loader manualmente antes de iniciar. Conserva nativePC/plugins.".into();
    }
    if message
        .starts_with("Astera-specific plugins were found, but Stracker's Loader does not load")
    {
        return "Se encontraron plugins específicos de Astera, pero Stracker's Loader no carga desde AsteraLoader/plugins/. Usa Astera Loader para estos plugins.".into();
    }

    message.into()
}

fn localized_activity_message(message: &str) -> Cow<'_, str> {
    for (prefix, translation) in [
        (
            "Game directory detected: ",
            "Directorio del juego detectado: ",
        ),
        (
            "Prepared mod archive: ",
            "Vista previa del archivo preparada: ",
        ),
        (
            "Could not prepare mod archive: ",
            "No se pudo preparar el archivo del mod: ",
        ),
        (
            "Selected game directory: ",
            "Directorio del juego elegido: ",
        ),
        ("Mod operation failed: ", "Falló la operación del mod: "),
    ] {
        if let Some(suffix) = message.strip_prefix(prefix) {
            return format!("{translation}{suffix}").into();
        }
    }

    match message {
        "Monster Hunter: World was not found automatically." => {
            "No se encontró Monster Hunter: World automáticamente.".into()
        }
        "Background worker stopped unexpectedly." => {
            "El proceso en segundo plano se detuvo inesperadamente.".into()
        }
        _ => message.into(),
    }
}

fn utc_timestamp_now() -> String {
    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return "Fecha no disponible".into();
    };

    let total_seconds = duration.as_secs();
    let days = (total_seconds / 86_400) as i64;
    let day_seconds = total_seconds % 86_400;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);

    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

#[cfg(test)]
mod library_tests {
    use super::*;
    use crate::mod_manager::ManagedFile;

    fn sample_mod(enabled: bool) -> ManagedMod {
        ManagedMod {
            id: "1731000000-42".into(),
            name: "Friendly Mod Name".into(),
            archive_name: "Fancy_Armor_v2.1.zip".into(),
            enabled,
            skipped_conflicts: 0,
            files: vec![ManagedFile {
                relative_path: "nativePC/plugins/FancyArmor.dll".into(),
                has_backup: false,
            }],
        }
    }

    #[test]
    fn library_search_matches_names_archive_ids_and_paths_case_insensitively() {
        let managed = sample_mod(true);

        assert!(mod_matches_query(&managed, "friendly mod"));
        assert!(mod_matches_query(&managed, "FANCY_ARMOR_V2"));
        assert!(mod_matches_query(&managed, "1731000000"));
        assert!(mod_matches_query(&managed, "plugins/fancyarmor.dll"));
        assert!(mod_matches_query(&managed, "  "));
        assert!(!mod_matches_query(&managed, "missing mod"));
    }

    #[test]
    fn library_filters_match_active_and_disabled_mods() {
        let enabled = sample_mod(true);
        let disabled = sample_mod(false);

        assert!(mod_matches_filter(&enabled, ModFilter::All));
        assert!(mod_matches_filter(&enabled, ModFilter::Enabled));
        assert!(!mod_matches_filter(&enabled, ModFilter::Disabled));
        assert!(mod_matches_filter(&disabled, ModFilter::Disabled));
        assert!(!mod_matches_filter(&disabled, ModFilter::Enabled));
    }

    #[test]
    fn library_prefers_a_real_plugin_name_over_archive_timestamps() {
        let mut managed = sample_mod(true);
        managed.name = "SimpleQuestComplete 0.1.4 8806 4 2026-10-17T03-14Z CxKCMJmnT".into();
        managed.archive_name = format!("{}.zip", managed.name);
        managed.files[0].relative_path = "nativePC/plugins/SimpleQuestComplete.dll".into();

        assert_eq!(display_mod_name(&managed), "SimpleQuestComplete");
        assert!(mod_matches_query(&managed, "2026-10-17"));
    }

    #[test]
    fn library_keeps_the_human_archive_title_before_trailing_ids() {
        let mut managed = sample_mod(true);
        managed.name = "Raora Panthera 8909 1 2026-10-26T15-44Z oN8oTSPd2".into();
        managed.archive_name = format!("{}.zip", managed.name);
        managed.files[0].relative_path = "nativePC/plugins/MHWI-PartPhysics.dll".into();

        assert_eq!(display_mod_name(&managed), "Raora Panthera");
    }

    #[test]
    fn library_uses_the_original_name_when_no_readable_prefix_or_file_exists() {
        let mut managed = sample_mod(true);
        managed.name = "2026-10-17 CxKCMJmnT".into();
        managed.archive_name = format!("{}.zip", managed.name);
        managed.files[0].relative_path = "nativePC/plugins/loader.dll".into();

        assert_eq!(display_mod_name(&managed), managed.name);
    }

    #[test]
    fn archive_version_only_accepts_conventional_numeric_version_tokens() {
        assert_eq!(
            archive_version("Fancy_Armor_v1.0.0.zip"),
            Some("v1.0.0".into())
        );
        assert_eq!(archive_version("cpuopt-0.18.0.zip"), Some("v0.18.0".into()));
        assert_eq!(archive_version("MHW-2026-10-17T03-14Z.zip"), None);
        assert_eq!(archive_version("Mod-2026.10.17.zip"), None);
    }

    #[test]
    fn row_metadata_keeps_file_count_and_status_without_inventing_a_version() {
        let mut managed = sample_mod(true);
        managed.archive_name = "Unversioned mod.zip".into();
        managed.files.push(crate::mod_manager::ManagedFile {
            relative_path: "nativePC/another.file".into(),
            has_backup: false,
        });

        assert_eq!(mod_metadata(&managed), "2 archivos · Activo");

        managed.enabled = false;
        managed.archive_name = "Archive_v2.4.rar".into();
        assert_eq!(mod_metadata(&managed), "v2.4 · 2 archivos · Desactivado");

        managed.skipped_conflicts = 2;
        assert_eq!(
            mod_metadata(&managed),
            "v2.4 · 2 archivos · Desactivado · 2 conflictos omitidos"
        );
    }

    #[test]
    fn navigation_titles_are_localized() {
        assert_eq!(AppSection::ALL.len(), 5);
        assert_eq!(AppSection::ALL.len(), 5);
        assert_eq!(AppSection::Library.nav_label(), "Biblioteca");
        assert_eq!(AppSection::Plugins.title(), "Plugins");
        assert_eq!(AppSection::Diagnostics.title(), "Diagnóstico");
        assert_eq!(AppSection::Settings.title(), "Ajustes");
        assert_eq!(AppSection::Log.title(), "Registro");
    }

    #[test]
    fn library_dashboard_copy_matches_the_detected_loader_state() {
        assert_eq!(
            library_hero_title(true, LoaderStatus::Astera),
            "Todo listo para cazar"
        );
        assert_eq!(
            library_hero_title(true, LoaderStatus::Conflict),
            "Revisa los loaders"
        );
        assert_eq!(
            library_hero_title(true, LoaderStatus::Missing),
            "Prepara tu instalación"
        );
        assert_eq!(
            library_hero_title(false, LoaderStatus::Astera),
            "Configura tu instalación"
        );
        assert_eq!(
            launch_blocked_message(LoaderStatus::StrackerIncomplete),
            "La instalación de Stracker's Loader está incompleta."
        );
    }

    #[test]
    fn plugin_rows_use_the_detected_dll_name_and_preserve_its_path() {
        assert_eq!(
            plugin_display_info("Astera: AsteraLoader/plugins/ExamplePlugin.dll"),
            (
                Some("Astera"),
                "ExamplePlugin",
                "ExamplePlugin.dll",
                "AsteraLoader/plugins/ExamplePlugin.dll"
            )
        );
        assert_eq!(
            plugin_display_info("nativePC/plugins/another.dll"),
            (
                None,
                "another",
                "another.dll",
                "nativePC/plugins/another.dll"
            )
        );
    }

    #[test]
    fn diagnostic_translation_preserves_paths_and_error_details() {
        assert_eq!(
            localized_diagnostic_message("Directory does not exist: /games/MHW"),
            "El directorio no existe: /games/MHW"
        );
        assert_eq!(
            localized_diagnostic_message("Could not read /games/MHW/plugins: permission denied"),
            "No se pudo leer /games/MHW/plugins: permission denied"
        );
    }

    #[test]
    fn activity_log_timestamps_include_date_time_and_timezone() {
        let timestamp = utc_timestamp_now();
        assert_eq!(timestamp.len(), 23);
        assert!(timestamp.ends_with(" UTC"));
        assert_eq!(timestamp.as_bytes()[4], b'-');
        assert_eq!(timestamp.as_bytes()[7], b'-');
        assert_eq!(timestamp.as_bytes()[10], b' ');
        assert_eq!(timestamp.as_bytes()[13], b':');
        assert_eq!(timestamp.as_bytes()[16], b':');
    }
}
