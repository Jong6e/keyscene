//! Presets: an ordered list of steps run with one shortcut; pressing it again restores what they changed.
//!
//! This file holds the data model shared by the engine (engine.rs), the app launcher (launch.rs)
//! and the settings window (ui/presets.js). JSON shape = serde shape below.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preset {
    /// Stable id, e.g. "p1730620000123". Shortcut key in config.hotkeys is "preset:<id>".
    pub id: String,
    pub name: String,
    pub steps: Vec<Step>,
    /// Restore by itself this many minutes after running. None = only when pressed again.
    #[serde(default)]
    pub revert_after_min: Option<u32>,
}

/// One step. JSON: {"kind": "speaker_mute", ...fields}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    // --- apps, files, web ---
    OpenApp {
        app: AppRef,
        #[serde(default)]
        if_running: IfRunning,
        #[serde(default)]
        monitor: MonitorRef,
        #[serde(default)]
        window: WindowState,
        #[serde(default)]
        on_restore: OnRestore,
    },
    OpenPath { path: String },
    OpenUrl {
        url: String,
        #[serde(default)]
        browser: BrowserRef,
        #[serde(default)]
        new_window: bool,
        #[serde(default)]
        private: bool,
    },
    /// Minimize or close every window of one app.
    AppWindows { app: AppRef, op: AppWindowOp },

    // --- windows ---
    /// Minimize all app windows (like Win+D). Restore brings back the ones it minimized.
    ShowDesktop,
    /// Acts on the window that was in front when the preset started.
    ActiveWindow { op: ActiveWindowOp },

    // --- sound, media ---
    /// `toggle`: flip the current state instead of setting `mute` (the same for the others below).
    SpeakerMute { mute: bool, #[serde(default)] toggle: bool },
    /// 0..=100
    SpeakerVolume { percent: u8 },
    MicMute { mute: bool, #[serde(default)] toggle: bool },
    Media { key: MediaKey },

    // --- screen, Explorer ---
    DesktopIcons { hide: bool, #[serde(default)] toggle: bool },
    HiddenItems { show: bool, #[serde(default)] toggle: bool },
    /// Hide or unhide a folder's contents (and the folder itself when `include_self`). Not restored
    /// by pressing again; the tray offers to undo it for a while instead.
    FolderTree {
        path: String,
        hide: bool,
        #[serde(default = "yes")]
        include_self: bool,
        /// Use the folders selected in Explorer instead of `path`.
        #[serde(default)]
        selected: bool,
    },
    /// Flips the hidden attribute of the items selected in Explorer or on the desktop. Not restored.
    SelectionHidden,
    Theme { dark: bool, #[serde(default)] toggle: bool },
    MonitorOff,

    // --- system ---
    Lock,
    Sleep,
    ClearClipboard,
    Screenshot,

    // --- input, flow ---
    /// Shortcut in Tauri notation, e.g. "Super+Control+ArrowRight".
    SendKeys { spec: String },
    TypeText { text: String },
    /// Milliseconds, 100..=60000
    Wait { ms: u32 },
}

/// Which app. Installed apps come from the Start menu (shell:AppsFolder); files are .exe/.lnk paths.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AppRef {
    /// "installed" = `id` is the AppsFolder parsing name (an AUMID for Store apps);
    /// "file" = `id` is a full path to an .exe or .lnk.
    pub source: AppSource,
    pub id: String,
    /// Shown to the user, e.g. "Claude".
    pub name: String,
    /// Extra command-line text for file apps (e.g. a document to open). Empty for most.
    #[serde(default)]
    pub args: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppSource {
    Installed,
    File,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IfRunning {
    /// Bring the existing window to the front (and apply monitor/window options to it).
    #[default]
    Front,
    /// Start another copy.
    New,
}

/// Where to place the app window.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MonitorRef {
    /// Leave it where the app puts it.
    #[default]
    AppDefault,
    Primary,
    /// The monitor the mouse pointer is on.
    Cursor,
    /// A specific monitor. `id` = stable device path; `name` = friendly name for display.
    /// If it is not connected, the primary monitor is used.
    Specific { id: String, name: String },
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    #[default]
    AppDefault,
    Maximized,
    Minimized,
    LeftHalf,
    RightHalf,
}

/// What restoring does with an app the preset opened or brought forward.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OnRestore {
    #[default]
    Keep,
    Minimize,
    /// Asks the app to close (like clicking X); the app may ask to save.
    Close,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct BrowserRef {
    /// "default" | "chrome" | "edge"
    #[serde(default = "default_browser")]
    pub browser: String,
    /// Chrome/Edge profile folder name, e.g. "Default", "Profile 1". Empty = browser's last used.
    #[serde(default)]
    pub profile_dir: String,
    /// Profile display name for the UI, e.g. "개인".
    #[serde(default)]
    pub profile_name: String,
}

fn yes() -> bool {
    true
}

fn default_browser() -> String {
    "default".into()
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppWindowOp {
    Minimize,
    Close,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActiveWindowOp {
    Minimize,
    Maximize,
    Topmost,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaKey {
    PlayPause,
    Next,
    Previous,
}

impl Step {
    /// True when restoring can undo this step.
    pub fn restorable(&self) -> bool {
        matches!(
            self,
            Step::OpenApp { on_restore: OnRestore::Minimize | OnRestore::Close, .. }
                | Step::AppWindows { op: AppWindowOp::Minimize, .. }
                | Step::ShowDesktop
                | Step::ActiveWindow { .. }
                | Step::SpeakerMute { .. }
                | Step::SpeakerVolume { .. }
                | Step::MicMute { .. }
                | Step::DesktopIcons { .. }
                | Step::HiddenItems { .. }
                | Step::Theme { .. }
        )
    }
}

/// Shortcuts made on first run, so a new user sees what the app can do.
pub fn default_presets() -> Vec<Preset> {
    vec![
        Preset {
            id: "hidden-toggle".into(),
            name: "숨긴 항목 전환".into(),
            steps: vec![Step::HiddenItems { show: true, toggle: true }],
            revert_after_min: None,
        },
        Preset {
            id: "screen-share".into(),
            name: "화면 공유 모드".into(),
            steps: vec![
                Step::HiddenItems { show: false, toggle: false },
                Step::DesktopIcons { hide: true, toggle: false },
                Step::MicMute { mute: true, toggle: false },
            ],
            revert_after_min: None,
        },
        Preset {
            id: "mic-toggle".into(),
            name: "마이크 음소거 전환".into(),
            steps: vec![Step::MicMute { mute: true, toggle: true }],
            revert_after_min: None,
        },
    ]
}

/// The key the first-run "숨긴 항목 전환" shortcut starts with.
pub const DEFAULT_HOTKEY: (&str, &str) = ("preset:hidden-toggle", "Alt+Backquote");
