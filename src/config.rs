//! Settings file: %APPDATA%\Keyscene\config.json

use crate::preset::{default_presets, Preset, DEFAULT_HOTKEY};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CURRENT_VERSION: u32 = 3;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExcludedApp {
    /// Shown to the user, e.g. "Photoshop".
    pub name: String,
    /// Program file name, lowercase, e.g. "photoshop.exe".
    pub exe: String,
    /// Full path when known; used only to show the program's icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A window's outer rectangle in physical screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    /// "preset:<id>" -> key combination in Tauri notation, e.g. "Alt+Backquote".
    /// A shortcut without an entry has no key yet.
    pub hotkeys: BTreeMap<String, String>,
    pub paused: bool,
    /// The shortcuts ("presets" in the code). Missing -> the first-run set; an empty list stays empty.
    #[serde(default = "default_presets")]
    pub presets: Vec<Preset>,
    /// While one of these programs is in front, the keys are released so the app gets them.
    pub excluded_apps: Vec<ExcludedApp>,
    /// Where the settings window was last (screen pixels). None -> opens centered.
    pub settings_window: Option<WindowRect>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CURRENT_VERSION,
            hotkeys: BTreeMap::from([(DEFAULT_HOTKEY.0.to_string(), DEFAULT_HOTKEY.1.to_string())]),
            paused: false,
            presets: default_presets(),
            excluded_apps: Vec::new(),
            settings_window: None,
        }
    }
}

impl Config {
    fn migrate(mut self) -> Self {
        if self.version < 3 {
            // Older files also had keys for built-in features, which are now ordinary shortcuts.
            self.hotkeys.retain(|action, _| action.starts_with("preset:"));
            self.version = CURRENT_VERSION;
        }
        for p in &mut self.presets {
            p.revert_after_min = p.revert_after_min.filter(|m| *m > 0).map(|m| m.min(24 * 60));
        }
        self
    }
}

/// Loads the settings. A damaged file is kept aside (never deleted) and defaults are used.
pub fn load(path: &Path) -> (Config, Option<String>) {
    let text = match read_with_retry(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Config::default(), None),
        Err(e) => {
            // The next save would replace the file with defaults; keep a copy of the original first.
            let backup = aside_name(path, "unreadable");
            let msg = if fs::copy(path, &backup).is_ok() {
                format!("설정을 읽지 못해 기본값으로 시작했습니다. 원본: {} ({e})", backup.display())
            } else {
                format!("설정을 읽지 못해 기본값으로 시작했습니다: {e}")
            };
            return (Config::default(), Some(msg));
        }
    };
    // A version 1 file has no "version" key; serde(default) would read it as 2.
    let version = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("version").and_then(|n| n.as_u64()))
        .unwrap_or(1) as u32;
    match serde_json::from_str::<Config>(&text) {
        Ok(mut cfg) => {
            cfg.version = version;
            (cfg.migrate(), None)
        }
        Err(_) => {
            let backup = aside_name(path, "damaged");
            let _ = fs::rename(path, &backup);
            (
                Config::default(),
                Some(format!("설정 파일이 손상되어 기본값으로 시작했습니다. 원본: {}", backup.display())),
            )
        }
    }
}

/// Reads the file, trying again briefly if another program (antivirus, OneDrive) has it open
/// at sign-in. A missing file or text that is not UTF-8 is not retried.
fn read_with_retry(path: &Path) -> std::io::Result<String> {
    let mut tries = 0;
    loop {
        match fs::read_to_string(path) {
            Err(e) if tries < 3 && !matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData) => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            other => return other,
        }
    }
}

/// `config.<why>-<seconds>.json` next to the settings file.
fn aside_name(path: &Path, why: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    path.with_file_name(format!("config.{why}-{stamp}.json"))
}

/// Writes to a temporary file first, then replaces the old file, so a crash never leaves half a file.
pub fn save(path: &Path, cfg: &Config) -> Result<(), String> {
    let dir = path.parent().ok_or("설정 폴더를 찾지 못했습니다.")?;
    fs::create_dir_all(dir).map_err(|e| format!("설정 폴더를 만들지 못했습니다: {e}"))?;
    let tmp: PathBuf = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(cfg).map_err(|e| e.to_string())?;
    let mut file = fs::File::create(&tmp).map_err(|e| format!("설정을 저장하지 못했습니다: {e}"))?;
    file.write_all(&json).map_err(|e| format!("설정을 저장하지 못했습니다: {e}"))?;
    file.sync_all().map_err(|e| format!("설정을 저장하지 못했습니다: {e}"))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|e| format!("설정을 저장하지 못했습니다: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("keyscene-test-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn version2_keeps_only_shortcut_keys() {
        let path = temp_file(
            "v2",
            r#"{"version":2,"hotkeys":{"hidden":"Alt+Backquote","preset:p1":"Control+KeyS"},"rehide_enabled":true,"presets":[{"id":"p1","name":"a","steps":[{"kind":"mic_mute","mute":true}]}]}"#,
        );
        let (cfg, warning) = load(&path);
        assert!(warning.is_none());
        assert_eq!(cfg.version, 3);
        assert_eq!(cfg.hotkeys.len(), 1);
        assert_eq!(cfg.hotkeys.get("preset:p1").map(String::as_str), Some("Control+KeyS"));
        // An old step without "toggle" reads as a plain set.
        assert_eq!(cfg.presets[0].steps[0], crate::preset::Step::MicMute { mute: true, toggle: false });
    }

    #[test]
    fn save_and_load_round_trip() {
        let path = temp_file("rt", "{}");
        let mut cfg = Config::default();
        cfg.hotkeys.insert("preset:mic-toggle".into(), "Control+Alt+KeyM".into());
        save(&path, &cfg).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("hidden_hotkey"));
        let (back, _) = load(&path);
        assert_eq!(back.hotkeys, cfg.hotkeys);
    }

    #[test]
    fn presets_default_only_when_missing() {
        let path = temp_file("presets", r#"{"version":3,"hotkeys":{}}"#);
        let (cfg, _) = load(&path);
        assert_eq!(cfg.presets.len(), 3);
        assert_eq!(cfg.presets[0].name, "숨긴 항목 전환");
        let path = temp_file("nopresets", r#"{"version":3,"hotkeys":{},"presets":[]}"#);
        let (cfg, _) = load(&path);
        assert!(cfg.presets.is_empty(), "a list the user emptied must stay empty");
    }

    #[test]
    fn preset_steps_round_trip() {
        use crate::preset::*;
        let mut cfg = Config::default();
        cfg.presets.push(Preset {
            id: "p1".into(),
            name: "빠른 전환".into(),
            steps: vec![
                Step::SpeakerMute { mute: true, toggle: false },
                Step::HiddenItems { show: true, toggle: true },
                Step::FolderTree { path: String::new(), hide: true, include_self: true, selected: true },
                Step::SelectionHidden,
                Step::ShowDesktop,
                Step::OpenApp {
                    app: AppRef { source: AppSource::Installed, id: "Claude_pzs8sxrjxfjjc!Claude".into(), name: "Claude".into(), args: String::new() },
                    if_running: IfRunning::Front,
                    monitor: MonitorRef::Specific { id: r"\?\DISPLAY#X".into(), name: "DELL U2723QE".into() },
                    window: WindowState::Maximized,
                    on_restore: OnRestore::Keep,
                },
                Step::OpenUrl { url: "https://example.com".into(), browser: BrowserRef { browser: "chrome".into(), profile_dir: "Profile 1".into(), profile_name: "개인".into() }, new_window: true, private: false },
                Step::SendKeys { spec: "Super+Control+ArrowRight".into() },
                Step::Wait { ms: 500 },
            ],
            revert_after_min: Some(10),
        });
        let path = temp_file("steps", "{}");
        save(&path, &cfg).unwrap();
        let (back, _) = load(&path);
        let last = cfg.presets.len() - 1;
        assert_eq!(back.presets[last].steps, cfg.presets[last].steps);
        assert_eq!(back.presets[last].revert_after_min, Some(10));
        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains(r#""kind": "open_app""#) && json.contains(r#""kind": "specific""#));
    }

    #[test]
    fn damaged_file_is_kept_aside() {
        let path = temp_file("bad", "{ not json");
        let (cfg, warning) = load(&path);
        assert!(warning.is_some());
        assert_eq!(cfg.hotkeys.get("preset:hidden-toggle").map(String::as_str), Some("Alt+Backquote"));
        let kept = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("config.damaged-"));
        assert!(kept, "damaged original must be preserved");
    }

    #[test]
    fn unreadable_file_is_copied_aside() {
        // Saved by an editor in the Korean ANSI code page: not UTF-8.
        let path = temp_file("ansi", "");
        let original: &[u8] = b"{\"presets\":[{\"name\":\"\xc8\xad\xb8\xe9\"}]}";
        fs::write(&path, original).unwrap();
        let (_, warning) = load(&path);
        assert!(warning.is_some());
        let copy = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with("config.unreadable-"))
            .expect("a copy of the original must be kept");
        assert_eq!(fs::read(copy.path()).unwrap(), original);
    }
}
