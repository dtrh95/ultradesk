use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(feature = "gui")]
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::session::ControlSession;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardSyncState {
    Unseeded,
    Seeded,
}

pub type ClipboardSeedingState = ClipboardSyncState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardEvent {
    SystemTextRead(String),
    RemoteTextReceived(String),
    ConnectionChanged(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardAction {
    PostToRemote(String),
    WriteToSystem(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardSyncMachine {
    last_host: String,
    last_remote: String,
    state: ClipboardSyncState,
    connected: bool,
}

impl ClipboardSyncMachine {
    pub fn new() -> Self {
        Self {
            last_host: String::new(),
            last_remote: String::new(),
            state: ClipboardSyncState::Unseeded,
            connected: false,
        }
    }

    pub fn normalize(text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let replaced = text.replace("\r\n", "\n");
        replaced.trim_end_matches(['\r', '\n']).to_string()
    }

    pub fn step(&mut self, event: ClipboardEvent) -> Option<ClipboardAction> {
        match event {
            ClipboardEvent::ConnectionChanged(is_connected) => {
                self.connected = is_connected;
                None
            }
            ClipboardEvent::SystemTextRead(raw_text) => {
                let trimmed = raw_text.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let norm = Self::normalize(&raw_text);
                match self.state {
                    ClipboardSyncState::Unseeded => {
                        self.state = ClipboardSyncState::Seeded;
                        self.last_host = norm;
                        None
                    }
                    ClipboardSyncState::Seeded => {
                        if !self.connected {
                            self.last_host = norm;
                            return None;
                        }
                        if norm != self.last_host && norm != self.last_remote {
                            self.last_host = norm;
                            Some(ClipboardAction::PostToRemote(raw_text))
                        } else {
                            None
                        }
                    }
                }
            }
            ClipboardEvent::RemoteTextReceived(raw_text) => {
                let trimmed = raw_text.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let norm = Self::normalize(&raw_text);
                match self.state {
                    ClipboardSyncState::Unseeded => {
                        self.state = ClipboardSyncState::Seeded;
                        self.last_remote = norm.clone();
                        self.last_host = norm;
                        Some(ClipboardAction::WriteToSystem(raw_text))
                    }
                    ClipboardSyncState::Seeded => {
                        if norm != self.last_remote {
                            self.last_remote = norm.clone();
                            self.last_host = norm;
                            Some(ClipboardAction::WriteToSystem(raw_text))
                        } else {
                            None
                        }
                    }
                }
            }
        }
    }

    pub fn on_host_clipboard(&mut self, text: &str, is_connected: bool) -> Option<String> {
        self.step(ClipboardEvent::ConnectionChanged(is_connected));
        match self.step(ClipboardEvent::SystemTextRead(text.to_string())) {
            Some(ClipboardAction::PostToRemote(t)) => Some(t),
            _ => None,
        }
    }

    pub fn on_remote_clipboard(&mut self, text: &str) -> Option<String> {
        match self.step(ClipboardEvent::RemoteTextReceived(text.to_string())) {
            Some(ClipboardAction::WriteToSystem(t)) => Some(t),
            _ => None,
        }
    }

    pub fn seeding_state(&self) -> ClipboardSeedingState {
        self.state
    }
}

impl Default for ClipboardSyncMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolTab {
    Chat,
    Files,
    Clipboard,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    AutoTime,
    AutoOs,
    Onedark,
    Onelight,
    VscodeDark,
    VscodeLight,
    GruvboxDark,
    GruvboxLight,
}

impl ThemeName {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThemeName::AutoTime => "auto-time",
            ThemeName::AutoOs => "auto-os",
            ThemeName::Onedark => "onedark",
            ThemeName::Onelight => "onelight",
            ThemeName::VscodeDark => "vscode-dark",
            ThemeName::VscodeLight => "vscode-light",
            ThemeName::GruvboxDark => "gruvbox-dark",
            ThemeName::GruvboxLight => "gruvbox-light",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LanguageCode {
    Vi,
    En,
}

impl LanguageCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            LanguageCode::Vi => "vi",
            LanguageCode::En => "en",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecialKey {
    Super,
    CtrlAltDel,
    F9NextMonitor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", content = "payload")]
pub enum ToolbarAction {
    ToggleConnectModal,
    DisconnectRemote,
    StartHostSharing,
    StopHostSharing,
    ToggleToolsPopover,
    SelectToolTab(ToolTab),
    SetTheme(ThemeName),
    CycleTheme,
    SetLanguage(LanguageCode),
    SelectMonitor(Option<usize>),
    SendSpecialKey(SpecialKey),
    ToggleToolbarCollapse,
    SetToolbarCollapsed(bool),
    ToggleFullscreen,
    CopyHostId,
    QuitApplication,
}

#[derive(Debug, Clone)]
pub struct GuiConfig {
    pub url: String,
    pub title: String,
    pub default_width: f64,
    pub default_height: f64,
    pub icon_bytes: Option<&'static [u8]>,
    pub viewer_path: Option<PathBuf>,
    pub disable_dmabuf: bool,
    pub disable_compositing: bool,
    pub enable_native_menubar: bool,
}

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:8765/".to_string(),
            title: "UltraDesk (Rust)".to_string(),
            default_width: 1200.0,
            default_height: 820.0,
            icon_bytes: Some(crate::server::DEFAULT_APP_ICON_PNG),
            viewer_path: None,
            disable_dmabuf: true,
            disable_compositing: true,
            enable_native_menubar: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiLaunchOutcome {
    ClosedCleanly,
    FallbackToBrowser(String),
}

impl GuiLaunchOutcome {
    pub fn from_exit(exit_code: Option<i32>, elapsed: Duration) -> Self {
        match exit_code {
            Some(0) => Self::ClosedCleanly,
            Some(code) if elapsed < Duration::from_secs(3) => Self::FallbackToBrowser(format!(
                "native GUI exited early with code {}",
                code
            )),
            None if elapsed < Duration::from_secs(1) => {
                Self::FallbackToBrowser("native GUI terminated by signal on startup".to_string())
            }
            _ => Self::ClosedCleanly,
        }
    }
}

#[cfg(feature = "gui")]
enum UserEvent {
    IpcMessage(String),
    MenuEvent(muda::MenuEvent),
    ExitRequested,
}

#[cfg(feature = "gui")]
pub fn launch_native_gui(
    config: GuiConfig,
    session: ControlSession,
    shutdown: Arc<AtomicBool>,
) -> GuiLaunchOutcome {
    if config.disable_dmabuf {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    if config.disable_compositing {
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }

    let run_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_native_gui_inner(config, session, shutdown)
    }));

    match run_res {
        Ok(outcome) => outcome,
        Err(panic_err) => {
            let msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic_err.downcast_ref::<String>() {
                s.clone()
            } else {
                "native GUI panicked during execution".to_string()
            };
            GuiLaunchOutcome::FallbackToBrowser(msg)
        }
    }
}

#[cfg(not(feature = "gui"))]
pub fn launch_native_gui(
    _config: GuiConfig,
    _session: ControlSession,
    _shutdown: Arc<AtomicBool>,
) -> GuiLaunchOutcome {
    GuiLaunchOutcome::FallbackToBrowser("GUI feature disabled at compile time".to_string())
}

#[cfg(feature = "gui")]
fn run_native_gui_inner(
    config: GuiConfig,
    session: ControlSession,
    shutdown: Arc<AtomicBool>,
) -> GuiLaunchOutcome {
    use std::cell::RefCell;
    use std::rc::Rc;
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::platform::unix::WindowExtUnix;
    use tao::window::{Icon, WindowBuilder};
    use wry::WebViewBuilderExtUnix;

    if let Err(e) = gtk::init() {
        return GuiLaunchOutcome::FallbackToBrowser(format!("failed to initialize GTK: {}", e));
    }

    use gtk::prelude::CssProviderExt;
    let css_provider = gtk::CssProvider::new();
    let _ = css_provider.load_from_data(
        b"menubar, menubar menuitem, menu, menuitem { font-family: 'Inter', 'Noto Sans', 'Cantarell', sans-serif; font-size: 12px; font-weight: 500; }",
    );
    if let Some(screen) = gtk::gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &css_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    let icon = config.icon_bytes.and_then(|bytes| {
        image::load_from_memory(bytes).ok().and_then(|img| {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            Icon::from_rgba(rgba.into_raw(), w, h).ok()
        })
    });

    let mut window_builder = WindowBuilder::new()
        .with_title(&config.title)
        .with_inner_size(tao::dpi::LogicalSize::new(
            config.default_width,
            config.default_height,
        ));

    if let Some(ic) = icon {
        window_builder = window_builder.with_window_icon(Some(ic));
    }

    let window = match window_builder.build(&event_loop) {
        Ok(w) => w,
        Err(e) => {
            return GuiLaunchOutcome::FallbackToBrowser(format!("failed to build window: {}", e))
        }
    };

    let vbox = match window.default_vbox() {
        Some(v) => v,
        None => {
            return GuiLaunchOutcome::FallbackToBrowser(
                "failed to obtain default vbox for GTK window".to_string(),
            )
        }
    };

    let ipc_proxy = event_loop.create_proxy();
    let webview_builder = wry::WebViewBuilder::new()
        .with_url(&config.url)
        .with_ipc_handler(move |req| {
            let body = req.into_body();
            let _ = ipc_proxy.send_event(UserEvent::IpcMessage(body));
        });

    let webview = match webview_builder.build_gtk(vbox) {
        Ok(wv) => Rc::new(wv),
        Err(e) => {
            return GuiLaunchOutcome::FallbackToBrowser(format!(
                "failed to build native webview: {}",
                e
            ))
        }
    };

    if config.enable_native_menubar {
        let menu = build_native_menubar();
        let gtk_window = window.gtk_window();
        let vbox_menu = window.default_vbox();
        let _ = menu.init_for_gtk_window(gtk_window, vbox_menu);
        let menu_proxy = event_loop.create_proxy();
        muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
            let _ = menu_proxy.send_event(UserEvent::MenuEvent(event));
        }));
    }

    let clip_machine = Rc::new(RefCell::new(ClipboardSyncMachine::new()));
    let session_timer = session.clone();
    let shutdown_timer = shutdown.clone();
    let webview_timer = webview.clone();
    let clip_machine_timer = clip_machine.clone();
    let viewer_path = config.viewer_path.clone();
    let mut last_mtime: Option<std::time::SystemTime> = viewer_path.as_ref().and_then(|p| {
        std::fs::metadata(p).ok().and_then(|m| m.modified().ok())
    });

    let timeout_proxy = event_loop.create_proxy();

    gtk::glib::timeout_add_local(Duration::from_millis(500), move || {
        if shutdown_timer.load(Ordering::SeqCst) {
            let _ = timeout_proxy.send_event(UserEvent::ExitRequested);
            return gtk::glib::ControlFlow::Break;
        }

        if let Some(ref path) = viewer_path {
            if let Ok(metadata) = std::fs::metadata(path) {
                if let Ok(mtime) = metadata.modified() {
                    if let Some(prev) = last_mtime {
                        if mtime > prev {
                            last_mtime = Some(mtime);
                            let _ = webview_timer.evaluate_script("window.location.reload()");
                        }
                    } else {
                        last_mtime = Some(mtime);
                    }
                }
            }
        }

        let snap = session_timer.snapshot();
        let is_connected = snap.state == "connected";
        let mut machine = clip_machine_timer.borrow_mut();
        machine.step(ClipboardEvent::ConnectionChanged(is_connected));

        if !snap.clipboard_remote.is_empty() {
            if let Some(ClipboardAction::WriteToSystem(remote_text)) =
                machine.step(ClipboardEvent::RemoteTextReceived(snap.clipboard_remote))
            {
                let clip = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
                clip.set_text(&remote_text);
                clip.store();
            }
        }

        let clip = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        let machine_for_cb = clip_machine_timer.clone();
        let session_for_cb = session_timer.clone();
        clip.request_text(move |_clip, text_opt| {
            if let Some(text) = text_opt {
                let mut m = machine_for_cb.borrow_mut();
                if let Some(ClipboardAction::PostToRemote(post_text)) =
                    m.step(ClipboardEvent::SystemTextRead(text.to_string()))
                {
                    let _ = session_for_cb.send_clipboard(&post_text, true);
                }
            }
        });

        gtk::glib::ControlFlow::Continue
    });

    let webview_loop = webview.clone();
    let window_loop = &window;

    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            tao::event::Event::WindowEvent {
                event: tao::event::WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            tao::event::Event::UserEvent(user_event) => {
                match user_event {
                    UserEvent::ExitRequested => {
                        *control_flow = ControlFlow::Exit;
                    }
                    UserEvent::IpcMessage(msg) => {
                        if let Ok(action) = serde_json::from_str::<ToolbarAction>(&msg) {
                            dispatch_toolbar_action(
                                action,
                                window_loop,
                                &webview_loop,
                                &session,
                                control_flow,
                                true,
                            );
                        }
                    }
                    UserEvent::MenuEvent(menu_event) => {
                        let id_str: &str = menu_event.id.as_ref();
                        if let Some(action) = menu_id_to_action(id_str) {
                            dispatch_toolbar_action(
                                action,
                                window_loop,
                                &webview_loop,
                                &session,
                                control_flow,
                                false,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    });

    shutdown.store(true, Ordering::SeqCst);
    session.disconnect();
    GuiLaunchOutcome::ClosedCleanly
}

#[cfg(feature = "gui")]
fn dispatch_toolbar_action(
    action: ToolbarAction,
    window: &tao::window::Window,
    webview: &wry::WebView,
    session: &ControlSession,
    control_flow: &mut tao::event_loop::ControlFlow,
    from_webview: bool,
) {
    match action {
        ToolbarAction::ToggleFullscreen => {
            if window.fullscreen().is_some() {
                window.set_fullscreen(None);
            } else {
                window.set_fullscreen(Some(tao::window::Fullscreen::Borderless(None)));
            }
        }
        ToolbarAction::QuitApplication => {
            *control_flow = tao::event_loop::ControlFlow::Exit;
        }
        ref act => {
            match act {
                ToolbarAction::SetTheme(ref theme) => {
                    let _ = session.update_ui_preferences(Some(theme.as_str()), None, None);
                }
                ToolbarAction::SetLanguage(ref lang) => {
                    let _ = session.update_ui_preferences(None, Some(lang.as_str()), None);
                }
                ToolbarAction::SetToolbarCollapsed(collapsed) => {
                    let _ = session.update_ui_preferences(None, None, Some(*collapsed));
                }
                _ => {}
            }
            if !from_webview {
                if let Ok(json_str) = serde_json::to_string(act) {
                    let script = format!(
                        "if (typeof window.__ultradesk_dispatch_action === 'function') {{ const act = {}; window.__ultradesk_dispatch_action(act.action, act.payload); }}",
                        json_str
                    );
                    let _ = webview.evaluate_script(&script);
                }
            }
        }
    }
}

#[cfg(feature = "gui")]
fn build_native_menubar() -> muda::Menu {
    let menu = muda::Menu::new();
    let file_menu = muda::Submenu::new("File", true);
    let connect_item = muda::MenuItem::with_id("ToggleConnectModal", "Connect...", true, None);
    let disconnect_item = muda::MenuItem::with_id("DisconnectRemote", "Disconnect", true, None);
    let start_share_item =
        muda::MenuItem::with_id("StartHostSharing", "Start host sharing", true, None);
    let stop_share_item =
        muda::MenuItem::with_id("StopHostSharing", "Stop host sharing", true, None);
    let quit_item = muda::MenuItem::with_id("QuitApplication", "Quit", true, None);
    let _ = file_menu.append_items(&[
        &connect_item,
        &disconnect_item,
        &muda::PredefinedMenuItem::separator(),
        &start_share_item,
        &stop_share_item,
        &muda::PredefinedMenuItem::separator(),
        &quit_item,
    ]);

    let view_menu = muda::Submenu::new("View", true);
    let fullscreen_item =
        muda::MenuItem::with_id("ToggleFullscreen", "Toggle fullscreen", true, None);
    let collapse_item =
        muda::MenuItem::with_id("ToggleToolbarCollapse", "Toggle toolbar", true, None);
    let _ = view_menu.append_items(&[&fullscreen_item, &collapse_item]);

    let remote_menu = muda::Submenu::new("Remote", true);
    let super_key_item =
        muda::MenuItem::with_id("SendSpecialKey:super", "Send Super key", true, None);
    let cad_key_item =
        muda::MenuItem::with_id("SendSpecialKey:ctrl_alt_del", "Send Ctrl+Alt+Del", true, None);
    let f9_key_item = muda::MenuItem::with_id(
        "SendSpecialKey:f9_next_monitor",
        "Next monitor (F9)",
        true,
        None,
    );
    let _ = remote_menu.append_items(&[&super_key_item, &cad_key_item, &f9_key_item]);

    let tools_menu = muda::Submenu::new("Tools", true);
    let toggle_tools_item =
        muda::MenuItem::with_id("ToggleToolsPopover", "Session tools", true, None);
    let chat_tab_item = muda::MenuItem::with_id("SelectToolTab:chat", "Chat", true, None);
    let files_tab_item = muda::MenuItem::with_id("SelectToolTab:files", "Files", true, None);
    let clip_tab_item =
        muda::MenuItem::with_id("SelectToolTab:clipboard", "Clipboard", true, None);
    let audio_tab_item = muda::MenuItem::with_id("SelectToolTab:audio", "Audio", true, None);
    let _ = tools_menu.append_items(&[
        &toggle_tools_item,
        &muda::PredefinedMenuItem::separator(),
        &chat_tab_item,
        &files_tab_item,
        &clip_tab_item,
        &audio_tab_item,
    ]);

    let theme_menu = muda::Submenu::new("Theme", true);
    let cycle_theme_item = muda::MenuItem::with_id("CycleTheme", "Cycle theme", true, None);
    let theme_auto_time =
        muda::MenuItem::with_id("SetTheme:auto-time", "Auto (system time)", true, None);
    let theme_auto_os = muda::MenuItem::with_id("SetTheme:auto-os", "Auto (OS theme)", true, None);
    let theme_onedark = muda::MenuItem::with_id("SetTheme:onedark", "One Dark Pro", true, None);
    let theme_onelight = muda::MenuItem::with_id("SetTheme:onelight", "One Light", true, None);
    let theme_vsc_dark =
        muda::MenuItem::with_id("SetTheme:vscode-dark", "VS Code Dark+", true, None);
    let theme_vsc_light =
        muda::MenuItem::with_id("SetTheme:vscode-light", "VS Code Light+", true, None);
    let theme_gruv_dark =
        muda::MenuItem::with_id("SetTheme:gruvbox-dark", "Gruvbox Dark", true, None);
    let theme_gruv_light =
        muda::MenuItem::with_id("SetTheme:gruvbox-light", "Gruvbox Light", true, None);
    let _ = theme_menu.append_items(&[
        &cycle_theme_item,
        &muda::PredefinedMenuItem::separator(),
        &theme_auto_time,
        &theme_auto_os,
        &theme_onedark,
        &theme_onelight,
        &theme_vsc_dark,
        &theme_vsc_light,
        &theme_gruv_dark,
        &theme_gruv_light,
    ]);

    let lang_menu = muda::Submenu::new("Language", true);
    let lang_vi = muda::MenuItem::with_id("SetLanguage:vi", "Tiếng Việt (VI)", true, None);
    let lang_en = muda::MenuItem::with_id("SetLanguage:en", "English (EN)", true, None);
    let _ = lang_menu.append_items(&[&lang_vi, &lang_en]);

    let _ = menu.append_items(&[
        &file_menu,
        &view_menu,
        &remote_menu,
        &tools_menu,
        &theme_menu,
        &lang_menu,
    ]);

    menu
}

#[cfg(feature = "gui")]
fn menu_id_to_action(id: &str) -> Option<ToolbarAction> {
    match id {
        "ToggleConnectModal" => Some(ToolbarAction::ToggleConnectModal),
        "DisconnectRemote" => Some(ToolbarAction::DisconnectRemote),
        "StartHostSharing" => Some(ToolbarAction::StartHostSharing),
        "StopHostSharing" => Some(ToolbarAction::StopHostSharing),
        "QuitApplication" => Some(ToolbarAction::QuitApplication),
        "ToggleFullscreen" => Some(ToolbarAction::ToggleFullscreen),
        "ToggleToolbarCollapse" => Some(ToolbarAction::ToggleToolbarCollapse),
        "ToggleToolsPopover" => Some(ToolbarAction::ToggleToolsPopover),
        "CycleTheme" => Some(ToolbarAction::CycleTheme),
        "CopyHostId" => Some(ToolbarAction::CopyHostId),
        "SendSpecialKey:super" => Some(ToolbarAction::SendSpecialKey(SpecialKey::Super)),
        "SendSpecialKey:ctrl_alt_del" => {
            Some(ToolbarAction::SendSpecialKey(SpecialKey::CtrlAltDel))
        }
        "SendSpecialKey:f9_next_monitor" => {
            Some(ToolbarAction::SendSpecialKey(SpecialKey::F9NextMonitor))
        }
        "SelectToolTab:chat" => Some(ToolbarAction::SelectToolTab(ToolTab::Chat)),
        "SelectToolTab:files" => Some(ToolbarAction::SelectToolTab(ToolTab::Files)),
        "SelectToolTab:clipboard" => Some(ToolbarAction::SelectToolTab(ToolTab::Clipboard)),
        "SelectToolTab:audio" => Some(ToolbarAction::SelectToolTab(ToolTab::Audio)),
        "SetTheme:auto-time" => Some(ToolbarAction::SetTheme(ThemeName::AutoTime)),
        "SetTheme:auto-os" => Some(ToolbarAction::SetTheme(ThemeName::AutoOs)),
        "SetTheme:onedark" => Some(ToolbarAction::SetTheme(ThemeName::Onedark)),
        "SetTheme:onelight" => Some(ToolbarAction::SetTheme(ThemeName::Onelight)),
        "SetTheme:vscode-dark" => Some(ToolbarAction::SetTheme(ThemeName::VscodeDark)),
        "SetTheme:vscode-light" => Some(ToolbarAction::SetTheme(ThemeName::VscodeLight)),
        "SetTheme:gruvbox-dark" => Some(ToolbarAction::SetTheme(ThemeName::GruvboxDark)),
        "SetTheme:gruvbox-light" => Some(ToolbarAction::SetTheme(ThemeName::GruvboxLight)),
        "SetLanguage:vi" => Some(ToolbarAction::SetLanguage(LanguageCode::Vi)),
        "SetLanguage:en" => Some(ToolbarAction::SetLanguage(LanguageCode::En)),
        _ => None,
    }
}
