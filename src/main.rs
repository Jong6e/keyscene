#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod dialog;
mod engine;
mod errlog;
mod explorer;
mod focus;
mod icons;
#[allow(dead_code)]
mod launch;
#[allow(dead_code)]
mod preset;
mod reg;
mod shell;
mod system;
mod tree;
mod winplace;

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, Wry};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const TRAY_ID: &str = "main";

/// How long the tray menu offers to undo a "하위 항목까지" change.
const TREE_UNDO_FOR: Duration = Duration::from_secs(10 * 60);

/// Where "새 버전 확인" sends the user.
const RELEASES_URL: &str = "https://github.com/Jong6e/keyscene/releases/latest";

/// "'숨긴 항목 전환'" for "preset:<id>", for messages.
fn action_name(app: &AppHandle, id: &str) -> String {
    let pid = id.strip_prefix("preset:").unwrap_or(id);
    let st = state(app);
    let cfg = st.config.lock().unwrap();
    cfg.presets.iter().find(|p| p.id == pid).map(|p| format!("'{}'", p.name)).unwrap_or_else(|| "다른 단축키".into())
}

/// "preset:<id>" of an existing shortcut.
fn known_action(app: &AppHandle, id: &str) -> bool {
    id.strip_prefix("preset:").is_some_and(|pid| state(app).config.lock().unwrap().presets.iter().any(|p| p.id == pid))
}

/// The shortcut `keyscene.exe --run X` names, by its id or by the name shown in the window.
fn command_line_action(app: &AppHandle, key: &str) -> Option<String> {
    let st = state(app);
    let cfg = st.config.lock().unwrap();
    let key = key.trim();
    let p = cfg.presets.iter().find(|p| p.id == key).or_else(|| cfg.presets.iter().find(|p| p.name.trim() == key))?;
    Some(format!("preset:{}", p.id))
}

/// The value after `--run` on a command line.
fn run_arg(args: &[String]) -> Option<&String> {
    args.iter().position(|a| a == "--run").and_then(|i| args.get(i + 1))
}

/// Who asked a shortcut to run or restore.
#[derive(Clone, Copy, PartialEq)]
enum Press {
    User,
    /// "유지 시간" ran out: restore only, never run.
    Timer,
}

struct MenuHandles {
    menu: Menu<Wry>,
    status: MenuItem<Wry>,
    status_shown: bool,
    pause: CheckMenuItem<Wry>,
    /// "실행 취소: 12개 숨김", shown only while there is something to undo.
    undo: MenuItem<Wry>,
    undo_shown: bool,
    /// (preset id, its menu item)
    presets: Vec<(String, CheckMenuItem<Wry>)>,
}

struct AppState {
    config: Mutex<config::Config>,
    config_path: PathBuf,
    menu: Mutex<Option<MenuHandles>>,
    /// Shortcuts that are registered right now, and which action each one runs.
    registered: Mutex<Vec<(Shortcut, String)>>,
    /// Action id -> why its shortcut could not be registered.
    hotkey_errors: Mutex<BTreeMap<String, String>>,
    /// The last problem worth telling the user about; shown in the settings window until dismissed.
    notice: Mutex<Option<String>>,
    /// Shortcuts that restore by themselves: id -> (timer generation, when).
    revert: Mutex<BTreeMap<String, (u64, Instant)>>,
    revert_gen: AtomicU64,
    /// When the window was requested; used only for KEYSCENE_TRACE timing.
    opened_at: Mutex<Option<Instant>>,
    /// Presets that ran and can be restored: id -> what to undo.
    running: Mutex<BTreeMap<String, Vec<engine::Undo>>>,
    /// Presets whose steps are being carried out right now (a second press is ignored meanwhile).
    busy: Mutex<BTreeSet<String>>,
    /// id -> problems from the last run or restore.
    preset_errors: Mutex<BTreeMap<String, String>>,
    /// Survives a crash: settings to put back (window info is not kept).
    journal_path: PathBuf,
    /// Name of the excluded app in front; while set, no shortcut is registered.
    suspended_by: Mutex<Option<String>>,
    /// True while the settings window records a new combination.
    capturing: AtomicBool,
    /// The last "하위 항목까지" change, offered for undo in the tray menu for a while.
    tree_undo: Mutex<Option<TreeUndo>>,
    tree_gen: AtomicU64,
}

struct TreeUndo {
    hide: bool,
    items: Vec<PathBuf>,
}

#[derive(Serialize, Clone)]
struct PresetHotkeyDto {
    spec: Option<String>,
    label: String,
    error: Option<String>,
}

#[derive(Serialize, Clone)]
struct PresetDto {
    id: String,
    name: String,
    steps: Vec<preset::Step>,
    hotkey: PresetHotkeyDto,
    revert_after_min: Option<u32>,
    /// Minutes until "유지 시간" ends and it goes back, while that is pending.
    revert_left_min: Option<u64>,
    running: bool,
    busy: bool,
    last_error: Option<String>,
}

#[derive(Serialize, Clone)]
struct PresetsDto {
    presets: Vec<PresetDto>,
}

#[derive(Serialize, Clone)]
struct StateDto {
    autostart: bool,
    paused: bool,
    notice: Option<String>,
    excluded_apps: Vec<config::ExcludedApp>,
    /// The excluded app in front right now, if any.
    suspended_by: Option<String>,
}

// ---------- shortcut text ----------

/// "Control+Alt+KeyH" -> "Ctrl+Alt+H", "Alt+Backquote" -> "Alt+`"
fn hotkey_label(spec: &str) -> String {
    spec.split('+')
        .map(|part| {
            let p = part.trim();
            match p {
                "Control" | "Ctrl" => "Ctrl".to_string(),
                "Super" | "Meta" | "Command" => "Win".to_string(),
                "Backquote" => "`".into(),
                "Minus" => "-".into(),
                "Equal" => "=".into(),
                "BracketLeft" => "[".into(),
                "BracketRight" => "]".into(),
                "Backslash" => "\\".into(),
                "Semicolon" => ";".into(),
                "Quote" => "'".into(),
                "Comma" => ",".into(),
                "Period" => ".".into(),
                "Slash" => "/".into(),
                "ArrowLeft" => "←".into(),
                "ArrowRight" => "→".into(),
                "ArrowUp" => "↑".into(),
                "ArrowDown" => "↓".into(),
                _ if p.starts_with("Key") && p.len() == 4 => p[3..].to_string(),
                _ if p.starts_with("Digit") && p.len() == 6 => p[5..].to_string(),
                _ if p.starts_with("Numpad") && p.len() == 7 && p.as_bytes()[6].is_ascii_digit() => format!("Num {}", &p[6..]),
                _ => p.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

// ---------- state ----------

fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

fn snapshot(app: &AppHandle) -> StateDto {
    let st = state(app);
    let cfg = st.config.lock().unwrap().clone();
    let notice = st.notice.lock().unwrap().clone();
    let suspended_by = st.suspended_by.lock().unwrap().clone();
    StateDto {
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        paused: cfg.paused,
        notice,
        excluded_apps: cfg.excluded_apps,
        suspended_by,
    }
}

fn save_config(app: &AppHandle) -> Result<(), String> {
    let st = state(app);
    let cfg = st.config.lock().unwrap().clone();
    config::save(&st.config_path, &cfg).inspect_err(|e| errlog::error(e))
}

/// The logo, with a badge while the microphone is muted or the keys are paused.
fn tray_icon(paused: bool, mic_muted: bool) -> Image<'static> {
    let light = explorer::taskbar_is_light();
    let size = tray_icon_size();
    macro_rules! variant {
        ($size:literal, $name:literal) => {
            if mic_muted {
                if light { include_bytes!(concat!("../icons/tray/", $size, "/", $name, "-light-mic.png")).as_slice() }
                else { include_bytes!(concat!("../icons/tray/", $size, "/", $name, "-dark-mic.png")).as_slice() }
            } else if light {
                include_bytes!(concat!("../icons/tray/", $size, "/", $name, "-light.png")).as_slice()
            } else {
                include_bytes!(concat!("../icons/tray/", $size, "/", $name, "-dark.png")).as_slice()
            }
        };
    }
    macro_rules! pick {
        ($name:literal) => {
            match size {
                16 => variant!("16", $name),
                20 => variant!("20", $name),
                24 => variant!("24", $name),
                _ => variant!("32", $name),
            }
        };
    }
    let bytes: &'static [u8] = if paused { pick!("paused") } else { pick!("base") };
    Image::from_bytes(bytes).expect("bundled tray icon")
}

/// The tray draws icons at 16 px times the display scale (20 px at 125%). Handing it that exact
/// size keeps the thin lines sharp instead of letting Windows shrink a larger picture.
fn tray_icon_size() -> u32 {
    use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
    use windows::Win32::UI::WindowsAndMessaging::SM_CXSMICON;
    let px = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) }.max(16) as u32;
    [16, 20, 24, 32].into_iter().find(|&s| s >= px).unwrap_or(32)
}

fn item_label(name: &str, spec: Option<&String>, paused: bool) -> String {
    match spec {
        Some(s) if !paused => format!("{name}\t{}", hotkey_label(s)),
        _ => name.to_string(),
    }
}

/// The thread that runs the event loop (set first thing in `main`).
static MAIN_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

/// Re-reads Windows state and brings tray, menu, timer and settings window in line with it.
///
/// Always runs on the main thread. Tray and menu calls made from another thread wait for the
/// main thread, so running this elsewhere while holding the menu lock could freeze the app
/// when the main thread wants the same lock.
fn sync(app: &AppHandle) {
    if MAIN_THREAD.get() == Some(&std::thread::current().id()) {
        sync_now(app);
    } else {
        let h = app.clone();
        let _ = app.run_on_main_thread(move || sync_now(&h));
    }
}

fn sync_now(app: &AppHandle) {
    let mic = audio::muted();
    let st = state(app);
    let cfg = st.config.lock().unwrap().clone();

    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(tray_icon(cfg.paused, mic == Some(true))));
        let mut tip = String::from("Keyscene");
        if mic == Some(true) {
            tip.push_str(" · 마이크 음소거됨");
        }
        if cfg.paused {
            tip.push_str(" · 단축키 일시 중지됨");
        } else if let Some(name) = st.suspended_by.lock().unwrap().as_ref() {
            tip.push_str(&format!(" · {name} 사용 중이라 단축키 쉬는 중"));
        }
        let _ = tray.set_tooltip(Some(&tip));
    }

    let status_text = {
        let errors = st.hotkey_errors.lock().unwrap();
        if !errors.is_empty() {
            Some(format!("단축키 {}개를 등록하지 못했습니다 · 열어서 확인", errors.len()))
        } else if st.notice.lock().unwrap().is_some() {
            Some("최근 작업에 문제가 있었습니다 · 열어서 확인".to_string())
        } else {
            // The shortcut that restores itself soonest.
            let revert = st.revert.lock().unwrap();
            revert.iter().min_by_key(|(_, (_, at))| *at).map(|(id, (_, at))| {
                let name = cfg.presets.iter().find(|p| &p.id == id).map(|p| p.name.as_str()).unwrap_or("단축키");
                format!("{name} · {}분 남음", minutes_left(*at))
            })
        }
    };

    if let Some(m) = st.menu.lock().unwrap().as_mut() {
        let _ = m.pause.set_checked(cfg.paused);
        let running = st.running.lock().unwrap();
        for (id, item) in &m.presets {
            if let Some(p) = cfg.presets.iter().find(|p| &p.id == id) {
                let _ = item.set_text(item_label(&p.name, cfg.hotkeys.get(&format!("preset:{id}")), cfg.paused));
                let _ = item.set_checked(running.contains_key(id));
            }
        }
        drop(running);
        match status_text {
            Some(text) => {
                let _ = m.status.set_text(text);
                if !m.status_shown && m.menu.insert(&m.status, 0).is_ok() {
                    m.status_shown = true;
                }
            }
            None => {
                if m.status_shown && m.menu.remove(&m.status).is_ok() {
                    m.status_shown = false;
                }
            }
        }
        let undo_text = st.tree_undo.lock().unwrap().as_ref().map(|u| {
            let n = tree::count(u.items.len());
            if u.hide { format!("실행 취소: {n}개 숨김") } else { format!("실행 취소: {n}개 숨김 해제") }
        });
        match undo_text {
            Some(text) => {
                let _ = m.undo.set_text(text);
                let at = m.status_shown as usize;
                if !m.undo_shown && m.menu.insert(&m.undo, at).is_ok() {
                    m.undo_shown = true;
                }
            }
            None => {
                if m.undo_shown && m.menu.remove(&m.undo).is_ok() {
                    m.undo_shown = false;
                }
            }
        }
    }
    // Only the window listens; skip the extra reads while it is closed.
    if app.get_webview_window("settings").is_some() {
        let _ = app.emit("state", snapshot(app));
        let _ = app.emit("presets", presets_snapshot(app));
    }
}

/// Whole minutes left until `at`, at least 1.
fn minutes_left(at: Instant) -> u64 {
    at.saturating_duration_since(Instant::now()).as_secs().div_ceil(60).max(1)
}

fn presets_snapshot(app: &AppHandle) -> PresetsDto {
    let st = state(app);
    let cfg = st.config.lock().unwrap().clone();
    let running = st.running.lock().unwrap();
    let busy = st.busy.lock().unwrap();
    let errors = st.preset_errors.lock().unwrap();
    let hotkey_errors = st.hotkey_errors.lock().unwrap();
    let revert = st.revert.lock().unwrap();
    PresetsDto {
        presets: cfg
            .presets
            .iter()
            .map(|p| {
                let key = format!("preset:{}", p.id);
                let spec = cfg.hotkeys.get(&key).cloned();
                PresetDto {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    steps: p.steps.clone(),
                    hotkey: PresetHotkeyDto {
                        label: spec.as_deref().map(hotkey_label).unwrap_or_default(),
                        spec,
                        error: hotkey_errors.get(&key).cloned(),
                    },
                    revert_after_min: p.revert_after_min,
                    revert_left_min: revert.get(&p.id).map(|(_, at)| minutes_left(*at)),
                    running: running.contains_key(&p.id),
                    busy: busy.contains(&p.id),
                    last_error: errors.get(&p.id).cloned(),
                }
            })
            .collect(),
    }
}

/// Records a failure. Successes need no message: the change itself is visible in Explorer.
fn report(app: &AppHandle, message: String) {
    errlog::error(&message);
    *state(app).notice.lock().unwrap() = Some(message);
    sync(app);
}

// ---------- 유지 시간 ----------

/// Restores the shortcut `minutes` after it ran, unless it is pressed again first.
fn start_revert(app: &AppHandle, id: &str, minutes: u32) {
    let st = state(app);
    let gen = st.revert_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let deadline = Instant::now() + Duration::from_secs(minutes as u64 * 60);
    st.revert.lock().unwrap().insert(id.to_string(), (gen, deadline));
    let app = app.clone();
    let id = id.to_string();
    // Wakes once a minute only while waiting, to keep the time left in the menu current.
    std::thread::spawn(move || loop {
        let left = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(left.min(Duration::from_secs(60)));
        let st = state(&app);
        if st.revert.lock().unwrap().get(&id).map(|(g, _)| *g) != Some(gen) {
            return; // pressed again, deleted, or started over
        }
        if Instant::now() >= deadline {
            toggle_preset(&app, &id, None, Press::Timer);
            return;
        }
        sync(&app);
    });
}

fn cancel_revert(app: &AppHandle, id: &str) {
    state(app).revert.lock().unwrap().remove(id);
}

// ---------- actions ----------

fn perform(app: &AppHandle, action: &str) {
    if let Some(id) = action.strip_prefix("preset:") {
        toggle_preset(app, id, system::foreground_app_window(), Press::User);
    }
}

// ---------- 하위 항목까지: undo from the tray ----------

/// Offers the tray's "실행 취소" for the items a "하위 항목까지" step just changed.
fn set_tree_undo(app: &AppHandle, hide: bool, items: Vec<PathBuf>) {
    let st = state(app);
    let gen = st.tree_gen.fetch_add(1, Ordering::SeqCst) + 1;
    *st.tree_undo.lock().unwrap() = Some(TreeUndo { hide, items });
    expire_tree_undo(app, gen);
}

fn expire_tree_undo(app: &AppHandle, gen: u64) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(TREE_UNDO_FOR);
        let st = state(&app);
        if st.tree_gen.load(Ordering::SeqCst) == gen {
            *st.tree_undo.lock().unwrap() = None;
            sync(&app);
        }
    });
}

fn undo_tree(app: &AppHandle) {
    let st = state(app);
    let Some(undo) = st.tree_undo.lock().unwrap().take() else { return };
    st.tree_gen.fetch_add(1, Ordering::SeqCst);
    sync(app);
    let app = app.clone();
    std::thread::spawn(move || {
        let failed = tree::undo(&undo.items, undo.hide);
        if failed > 0 {
            report(&app, format!("{}개는 실행 취소하지 못했습니다. 사용 중이거나 권한이 없는 항목입니다.", tree::count(failed)));
        }
    });
}

// ---------- presets ----------

fn save_journal(app: &AppHandle) {
    // Two presets can finish at once; one writer at a time so an older copy never lands last.
    static WRITING: Mutex<()> = Mutex::new(());
    let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let st = state(app);
    let keep: BTreeMap<String, Vec<engine::Undo>> = st
        .running
        .lock()
        .unwrap()
        .iter()
        .map(|(id, u)| (id.clone(), u.iter().filter(|x| x.survives_restart()).cloned().collect::<Vec<_>>()))
        .filter(|(_, u)| !u.is_empty())
        .collect();
    if keep.is_empty() {
        let _ = std::fs::remove_file(&st.journal_path);
    } else if let Ok(json) = serde_json::to_vec_pretty(&keep) {
        let tmp = st.journal_path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &st.journal_path);
        }
    }
}

fn load_journal(path: &std::path::Path) -> BTreeMap<String, Vec<engine::Undo>> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// First press runs the shortcut; the next press restores what it changed. When the user has
/// already put everything back by hand, a press runs it again instead of doing nothing.
fn toggle_preset(app: &AppHandle, id: &str, front: Option<system::WinRef>, by: Press) {
    let st = state(app);
    let preset = st.config.lock().unwrap().presets.iter().find(|p| p.id == id).cloned();
    let Some(preset) = preset else { return };
    if !st.busy.lock().unwrap().insert(id.to_string()) {
        return; // still carrying out the previous press
    }
    let _ = app.emit("presets", presets_snapshot(app));
    let app = app.clone();
    let id = id.to_string();
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let st = state(&app);
        cancel_revert(&app, &id);
        let pending = st.running.lock().unwrap().remove(&id);
        let pending = pending.filter(|undo| undo.iter().any(engine::Undo::still_applies));
        let errors = match (pending, by) {
            (Some(undo), by) => engine::restore(undo, by == Press::User),
            (None, Press::Timer) => Vec::new(),
            (None, Press::User) => {
                let result = engine::run(&preset.steps, front);
                if let Some((hide, items)) = result.tree {
                    set_tree_undo(&app, hide, items);
                }
                // Nothing to undo (e.g. only "open a web page") -> the next press simply runs again.
                // Same when it cycles window sets: the next press shows the next set, it doesn't
                // first undo the other steps (that took an extra press per switch).
                if !result.undo.is_empty() && !preset.cycles() {
                    st.running.lock().unwrap().insert(id.clone(), result.undo);
                    if let Some(minutes) = preset.revert_after_min {
                        start_revert(&app, &id, minutes);
                    }
                }
                result.errors
            }
        };
        save_journal(&app);
        if errors.is_empty() {
            st.preset_errors.lock().unwrap().remove(&id);
        } else {
            let text = errors.join(" · ");
            errlog::error(&format!("단축키 '{}': {text}", preset.name));
            st.preset_errors.lock().unwrap().insert(id.clone(), text.clone());
            *st.notice.lock().unwrap() = Some(format!("단축키 '{}': {text}", preset.name));
        }
        st.busy.lock().unwrap().remove(&id);
        sync(&app);
    });
}

fn rebuild_menu(app: &AppHandle) {
    if let Ok(handles) = build_menu(app) {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            let _ = tray.set_menu(Some(handles.menu.clone()));
        }
        *state(app).menu.lock().unwrap() = Some(handles);
    }
    sync(app);
}

fn new_preset_id() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("p{ms}")
}

// ---------- apps that pause the shortcuts ----------

/// Runs when another window comes to the front. Releases or takes back the shortcuts.
fn on_foreground(app: &AppHandle, hwnd: isize) {
    // While a new combination is recorded nothing is registered anyway; the window in front is
    // looked at again when the recording ends.
    if state(app).capturing.load(Ordering::SeqCst) {
        return;
    }
    if update_suspended(app, hwnd) {
        apply_hotkeys(app);
        sync(app);
    }
}

/// Works out whether `hwnd`, the window in front, belongs to an excluded app and remembers it.
/// Returns true when that changed (the caller then registers or releases the shortcuts).
fn update_suspended(app: &AppHandle, hwnd: isize) -> bool {
    let st = state(app);
    // Nothing excluded and nothing paused: skip looking up the program behind every switch.
    if st.config.lock().unwrap().excluded_apps.is_empty() && st.suspended_by.lock().unwrap().is_none() {
        return false;
    }
    let exe = launch::window_exe(hwnd);
    // Keyscene's own settings window is not a reason to change anything.
    if exe.as_deref() == std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase())).as_deref() {
        return false;
    }
    let hit = {
        let cfg = st.config.lock().unwrap();
        exe.and_then(|e| cfg.excluded_apps.iter().find(|a| a.exe == e).map(|a| a.name.clone()))
    };
    let mut suspended = st.suspended_by.lock().unwrap();
    if *suspended == hit {
        return false;
    }
    *suspended = hit;
    true
}

/// Looks at the window in front right now. Used when shortcut recording ends: window switches made
/// meanwhile were not followed, and an excluded app may have come to the front since.
fn recheck_foreground(app: &AppHandle) {
    let front = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize;
    update_suspended(app, front);
}

fn exe_file_name(input: &str) -> Option<String> {
    let name = std::path::Path::new(input.trim().trim_matches('"')).file_name()?.to_string_lossy().to_lowercase();
    name.ends_with(".exe").then_some(name)
}

// ---------- shortcuts ----------

/// Registers the shortcuts that should work right now (none while paused or while an excluded app
/// is in front) and records which could not be registered.
///
/// Always runs on the main thread, like `sync`. Each register call is handed to the main thread
/// anyway, so a run started on another thread could be cut in two by one started on the main
/// thread: the second then finds its keys "taken" and one of the two lists ends up wrong.
fn apply_hotkeys(app: &AppHandle) {
    if MAIN_THREAD.get() == Some(&std::thread::current().id()) {
        apply_hotkeys_now(app);
    } else {
        // Waited for, so the caller can read the result (which keys failed) right after.
        // Callers hold no lock here; the main thread never waits for them.
        let (done, wait) = std::sync::mpsc::channel();
        let h = app.clone();
        let sent = app.run_on_main_thread(move || {
            apply_hotkeys_now(&h);
            let _ = done.send(());
        });
        if sent.is_ok() {
            let _ = wait.recv(); // an error means the app is closing and the work was dropped
        }
    }
}

fn apply_hotkeys_now(app: &AppHandle) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let st = state(app);
    let cfg = st.config.lock().unwrap().clone();
    let mut registered = Vec::new();
    let mut errors = BTreeMap::new();
    let suspended = st.suspended_by.lock().unwrap().is_some();
    if !cfg.paused && !suspended {
        for (action, spec) in &cfg.hotkeys {
            match spec.parse::<Shortcut>() {
                Ok(s) if gs.register(s).is_ok() => registered.push((s, action.clone())),
                Ok(_) => {
                    errors.insert(action.clone(), format!("{}은(는) 다른 프로그램이 이미 쓰고 있습니다", hotkey_label(spec)));
                }
                Err(_) => {
                    errors.insert(action.clone(), "단축키 형식이 올바르지 않습니다".into());
                }
            }
        }
    }
    *st.registered.lock().unwrap() = registered;
    // This runs whenever shortcuts pause or resume (e.g. an excluded app comes to the front),
    // so a failure is logged only when it is new since the last time shortcuts were registered.
    static LOGGED: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
    if !cfg.paused && !suspended {
        let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
        for (action, text) in &errors {
            if logged.get(action) != Some(text) {
                errlog::error(&format!("단축키 '{action}': {text}"));
            }
        }
        *logged = errors.clone();
    }
    *st.hotkey_errors.lock().unwrap() = errors;
}

// ---------- settings window ----------

fn open_settings(app: &AppHandle) {
    *state(app).opened_at.lock().unwrap() = Some(Instant::now());
    // A shortcut taken by another app at sign-in may be free now (e.g. that app was closed).
    if !state(app).hotkey_errors.lock().unwrap().is_empty() {
        apply_hotkeys(app);
        sync(app);
    }
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    // A double-click (or a second launch) arrives while the window is still being made; a
    // second build of the same window would fail. The first one shows itself when ready.
    static OPENING: AtomicBool = AtomicBool::new(false);
    if OPENING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let saved = state(&app).config.lock().unwrap().settings_window;
    // Build off the event thread; Tauri hands the work to the main thread itself.
    std::thread::spawn(move || {
        // Created hidden so it can be moved to its last place before anyone sees it.
        let built = tauri::WebviewWindowBuilder::new(&app, "settings", tauri::WebviewUrl::App("index.html".into()))
            .title("Keyscene")
            .inner_size(460.0, 640.0)
            .min_inner_size(420.0, 420.0)
            .maximizable(false)
            .center()
            .visible(false)
            .build();
        OPENING.store(false, Ordering::SeqCst);
        let w = match built {
            Ok(w) => w,
            Err(e) => return settings_failed(&e.to_string()),
        };
        let _ = app.run_on_main_thread(move || {
            if let Ok(hwnd) = w.hwnd() {
                let hwnd = hwnd.0 as isize;
                // First opening: still pulled inside the screen, which may be shorter than the window.
                if let Some(r) = saved.or_else(|| winplace::current(hwnd)) {
                    winplace::restore(hwnd, r);
                }
                winplace::attach(hwnd);
            }
            let _ = w.show();
            let _ = w.set_focus();
        });
    });
}

/// The settings window could not be made. Without the WebView2 runtime that always happens,
/// so offer its download page; anything else is logged and reported briefly.
fn settings_failed(detail: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNO, SW_SHOWNORMAL,
    };
    errlog::error(&format!("설정 창을 열지 못했습니다: {detail}"));
    // Several clicks while a message is up should not stack more messages.
    static SHOWING: AtomicBool = AtomicBool::new(false);
    if SHOWING.swap(true, Ordering::SeqCst) {
        return;
    }
    let title = HSTRING::from("Keyscene");
    unsafe {
        if webview2_installed() {
            let text = HSTRING::from("설정 창을 열지 못했습니다. 잠시 뒤 다시 시도하고, 계속되면 Keyscene을 종료했다가 다시 실행하세요.");
            MessageBoxW(None, &text, &title, MB_OK | MB_ICONWARNING);
        } else {
            let text = HSTRING::from("설정 창을 열려면 Microsoft Edge WebView2 런타임이 필요합니다. 설치 페이지를 열까요?");
            if MessageBoxW(None, &text, &title, MB_YESNO | MB_ICONINFORMATION) == IDYES {
                let url = HSTRING::from("https://developer.microsoft.com/microsoft-edge/webview2/");
                ShellExecuteW(None, &HSTRING::from("open"), &url, None, None, SW_SHOWNORMAL);
            }
        }
    }
    SHOWING.store(false, Ordering::SeqCst);
}

/// Where Microsoft says to look for an installed WebView2 runtime (per machine or per user).
fn webview2_installed() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    const CLIENT: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    [
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\{CLIENT}")),
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\{CLIENT}")),
        (HKEY_CURRENT_USER, format!(r"Software\{CLIENT}")),
    ]
    .iter()
    .filter_map(|(root, path)| reg::read_string(*root, path, "pv"))
    .any(|v| !v.is_empty() && v != "0.0.0.0")
}

/// Keeps the settings window's last place. Called when a move/resize ends and when it closes.
fn remember_settings_window(app: &AppHandle, rect: config::WindowRect) {
    let st = state(app);
    {
        let mut cfg = st.config.lock().unwrap();
        if cfg.settings_window == Some(rect) {
            return;
        }
        cfg.settings_window = Some(rect);
    }
    let _ = save_config(app);
}

// ---------- commands for the settings window ----------

#[tauri::command]
fn get_state(app: AppHandle) -> StateDto {
    snapshot(&app)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<StateDto, String> {
    let launcher = app.autolaunch();
    let result = if enabled { launcher.enable() } else { launcher.disable() };
    result.map_err(|e| format!("시작 프로그램 설정을 바꾸지 못했습니다: {e}"))?;
    if enabled {
        quote_autostart_path();
    }
    Ok(snapshot(&app))
}

/// The autostart plugin writes the path without quotes. A path with spaces
/// (C:\Users\Hong Gildong\...) then relies on Windows guessing where the program name ends,
/// so write it again in quotes.
fn quote_autostart_path() {
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let Ok(exe) = std::env::current_exe() else { return };
    if let Ok(key) = reg::Key::open_write(RUN) {
        let _ = key.set_string("Keyscene", &format!("\"{}\"", exe.display()));
    }
}

#[tauri::command]
fn set_paused(app: AppHandle, paused: bool) -> Result<StateDto, String> {
    state(&app).config.lock().unwrap().paused = paused;
    // Act first, so a failed save never leaves the menu and the shortcuts disagreeing.
    apply_hotkeys(&app);
    sync(&app);
    save_config(&app)?;
    Ok(snapshot(&app))
}

/// While the user presses a new combination, our own shortcuts must not fire.
#[tauri::command]
fn begin_hotkey_capture(app: AppHandle) {
    state(&app).capturing.store(true, Ordering::SeqCst);
    let _ = app.global_shortcut().unregister_all();
    state(&app).registered.lock().unwrap().clear();
}

#[tauri::command]
fn end_hotkey_capture(app: AppHandle) -> StateDto {
    state(&app).capturing.store(false, Ordering::SeqCst);
    recheck_foreground(&app);
    apply_hotkeys(&app);
    sync(&app);
    snapshot(&app)
}

#[tauri::command]
fn set_hotkey(app: AppHandle, action: String, spec: Option<String>) -> Result<StateDto, String> {
    state(&app).capturing.store(false, Ordering::SeqCst);
    if !known_action(&app, &action) {
        return Err("알 수 없는 단축키입니다.".into());
    }
    let st = state(&app);
    if let Some(s) = &spec {
        let wanted = s.parse::<Shortcut>().map_err(|_| "이 조합은 단축키로 쓸 수 없습니다.".to_string())?;
        let cfg = st.config.lock().unwrap().clone();
        for (other, other_spec) in &cfg.hotkeys {
            if *other != action && other_spec.parse::<Shortcut>().ok() == Some(wanted) {
                apply_hotkeys(&app);
                let other_name = action_name(&app, other);
                return Err(format!("{}은(는) 이미 {}에 쓰고 있습니다.", hotkey_label(s), other_name));
            }
        }
    }
    let old = {
        let mut c = st.config.lock().unwrap();
        match &spec {
            Some(s) => c.hotkeys.insert(action.clone(), s.clone()),
            None => c.hotkeys.remove(&action),
        }
    };
    apply_hotkeys(&app);
    let failed = st.hotkey_errors.lock().unwrap().get(&action).cloned();
    if let (Some(err), Some(_)) = (failed, &spec) {
        // Keep the previous shortcut working when the new one is taken.
        {
            let mut c = st.config.lock().unwrap();
            match old {
                Some(o) => c.hotkeys.insert(action.clone(), o),
                None => c.hotkeys.remove(&action),
            };
        }
        apply_hotkeys(&app);
        sync(&app);
        return Err(format!("{err}. 이전 단축키를 유지합니다."));
    }
    save_config(&app)?;
    sync(&app);
    Ok(snapshot(&app))
}

#[tauri::command]
async fn list_running_apps() -> Vec<launch::RunningApp> {
    tauri::async_runtime::spawn_blocking(launch::running_apps).await.unwrap_or_default()
}

/// The apps with a window open now, for recording a "창 세트".
#[tauri::command]
async fn list_open_window_apps() -> Vec<launch::OpenWindowApp> {
    tauri::async_runtime::spawn_blocking(launch::open_window_apps).await.unwrap_or_default()
}

/// Replaces the list. `exe` may be a full path or a file name; it is stored as a lowercase file name.
#[tauri::command]
fn set_excluded_apps(app: AppHandle, apps: Vec<config::ExcludedApp>) -> Result<StateDto, String> {
    let mut clean: Vec<config::ExcludedApp> = Vec::new();
    for a in apps {
        // A desktop shortcut is fine: use the program it points to.
        let target = launch::resolve_shortcut(&a.exe);
        let exe = exe_file_name(&target).ok_or_else(|| format!("'{}'은(는) 프로그램 파일(.exe)이 아닙니다.", a.exe))?;
        if exe == "explorer.exe" {
            return Err("파일 탐색기는 제외할 수 없습니다. 바탕 화면과 작업 표시줄도 같은 프로그램이라 단축키가 거의 꺼집니다.".into());
        }
        if !clean.iter().any(|c| c.exe == exe) {
            let name = if a.name.trim().is_empty() { exe.trim_end_matches(".exe").to_string() } else { a.name.trim().to_string() };
            // Keep a full path for the icon: given by the page, or the program the input pointed to.
            let path = a.path.filter(|p| !p.trim().is_empty()).or_else(|| target.contains('\\').then(|| target.clone()));
            clean.push(config::ExcludedApp { name, exe, path });
        }
    }
    state(&app).config.lock().unwrap().excluded_apps = clean;
    save_config(&app)?;
    // Re-check the window in front right now, so the change applies immediately.
    *state(&app).suspended_by.lock().unwrap() = None;
    recheck_foreground(&app);
    apply_hotkeys(&app);
    sync(&app);
    Ok(snapshot(&app))
}

#[tauri::command]
fn get_presets(app: AppHandle) -> PresetsDto {
    presets_snapshot(&app)
}

#[tauri::command]
fn save_preset(app: AppHandle, preset: preset::Preset) -> Result<PresetsDto, String> {
    let mut preset = preset;
    preset.name = preset.name.trim().chars().take(60).collect();
    if preset.name.is_empty() {
        preset.name = "새 단축키".into();
    }
    preset.revert_after_min = preset.revert_after_min.filter(|m| *m > 0).map(|m| m.min(24 * 60));
    if preset.steps.len() > 50 {
        return Err("동작은 50개까지 넣을 수 있습니다.".into());
    }
    let menu_changed = {
        let st = state(&app);
        let mut c = st.config.lock().unwrap();
        if preset.id.is_empty() {
            preset.id = new_preset_id();
            c.presets.push(preset);
            true
        } else if let Some(slot) = c.presets.iter_mut().find(|p| p.id == preset.id) {
            let renamed = slot.name != preset.name;
            *slot = preset;
            renamed
        } else {
            return Err("단축키를 찾지 못했습니다. 창을 닫았다가 다시 여세요.".into());
        }
    };
    save_config(&app)?;
    if menu_changed {
        rebuild_menu(&app);
    }
    Ok(presets_snapshot(&app))
}

#[tauri::command]
fn delete_preset(app: AppHandle, id: String) -> Result<PresetsDto, String> {
    {
        let st = state(&app);
        if st.busy.lock().unwrap().contains(&id) {
            return Err("단축키가 동작하는 중입니다. 끝난 뒤 삭제하세요.".into());
        }
        if st.running.lock().unwrap().contains_key(&id) {
            return Err("실행 중인 단축키는 먼저 원래대로 한 뒤 삭제하세요.".into());
        }
        let mut c = st.config.lock().unwrap();
        c.presets.retain(|p| p.id != id);
        c.hotkeys.remove(&format!("preset:{id}"));
        st.preset_errors.lock().unwrap().remove(&id);
    }
    cancel_revert(&app, &id);
    save_config(&app)?;
    apply_hotkeys(&app);
    rebuild_menu(&app);
    Ok(presets_snapshot(&app))
}

#[tauri::command]
fn run_preset(app: AppHandle, id: String) -> PresetsDto {
    // From the window, "the window in front" is Keyscene's own window, so none.
    toggle_preset(&app, &id, None, Press::User);
    presets_snapshot(&app)
}

/// "새 버전 확인" found a newer version: open the download page in the browser.
#[tauri::command]
fn open_release_page() {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe {
        ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(RELEASES_URL), None, None, SW_SHOWNORMAL);
    }
}

#[tauri::command]
async fn list_apps() -> Vec<launch::InstalledApp> {
    tauri::async_runtime::spawn_blocking(launch::installed_apps).await.unwrap_or_default()
}

#[tauri::command]
async fn list_browsers() -> Vec<launch::BrowserProfile> {
    tauri::async_runtime::spawn_blocking(launch::browser_profiles).await.unwrap_or_default()
}

#[tauri::command]
async fn list_monitors() -> Vec<launch::MonitorInfo> {
    tauri::async_runtime::spawn_blocking(launch::monitors).await.unwrap_or_default()
}

fn settings_hwnd(app: &AppHandle) -> Option<isize> {
    app.get_webview_window("settings").and_then(|w| w.hwnd().ok()).map(|h| h.0 as isize)
}

#[tauri::command]
async fn pick_file(app: AppHandle) -> Option<String> {
    let owner = settings_hwnd(&app);
    tauri::async_runtime::spawn_blocking(move || dialog::pick(dialog::Kind::Program, owner)).await.ok().flatten()
}

#[tauri::command]
async fn pick_path(app: AppHandle, folder: bool) -> Option<String> {
    let owner = settings_hwnd(&app);
    let kind = if folder { dialog::Kind::Folder } else { dialog::Kind::File };
    tauri::async_runtime::spawn_blocking(move || dialog::pick(kind, owner)).await.ok().flatten()
}

/// Called by the page after its first render. With KEYSCENE_TRACE set, logs how long opening took.
#[tauri::command]
fn ui_ready(app: AppHandle) {
    if std::env::var_os("KEYSCENE_TRACE").is_none() {
        return;
    }
    if let Some(t) = state(&app).opened_at.lock().unwrap().take() {
        let line = format!("{}\n", t.elapsed().as_millis());
        let path = std::env::temp_dir().join("keyscene-open-ms.txt");
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| {
            use std::io::Write;
            f.write_all(line.as_bytes())
        });
    }
}

#[tauri::command]
fn dismiss_notice(app: AppHandle) {
    *state(&app).notice.lock().unwrap() = None;
    sync(&app);
}

// ---------- startup ----------

fn build_menu(app: &AppHandle) -> tauri::Result<MenuHandles> {
    let status = MenuItem::with_id(app, "status", "", false, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "단축키 일시 중지", true, false, None::<&str>)?;
    let undo = MenuItem::with_id(app, "tree-undo", "실행 취소", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "settings", "Keyscene 열기", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
    let menu = Menu::new(app)?;
    let cfg = state(app).config.lock().unwrap().clone();
    let mut presets = Vec::new();
    for p in &cfg.presets {
        let item = CheckMenuItem::with_id(app, format!("preset:{}", p.id), &p.name, true, false, None::<&str>)?;
        menu.append(&item)?;
        presets.push((p.id.clone(), item));
    }
    if !cfg.presets.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&pause)?;
    menu.append(&open)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&quit)?;
    Ok(MenuHandles { menu, status, status_shown: false, pause, undo, undo_shown: false, presets })
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let handles = build_menu(app)?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_icon(false, false))
        .tooltip("Keyscene")
        .menu(&handles.menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "pause" => {
                let paused = !state(app).config.lock().unwrap().paused;
                let _ = set_paused(app.clone(), paused);
            }
            "settings" => open_settings(app),
            "tree-undo" => undo_tree(app),
            "quit" => {
                engine::unmute_all_sets();
                app.exit(0)
            }
            id if id.starts_with("preset:") => toggle_preset(app, &id["preset:".len()..], None, Press::User),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                // Left click opens the window; a double-click arrives as two clicks and opens it once.
                open_settings(tray.app_handle());
            }
        })
        .build(app)?;

    *state(app).menu.lock().unwrap() = Some(handles);
    Ok(())
}

/// Settings, restore record and error log: %APPDATA%\Keyscene.
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Keyscene"))
}

fn main() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
    // Set up before the builder so a failure while starting is recorded too.
    if let Some(dir) = data_dir() {
        errlog::init(dir);
    }
    let app = tauri::Builder::default()
        // Must come first: a second launch hands its request to the running copy.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // `keyscene.exe --run "<name>"` lets launchers such as a mouse button run a shortcut.
            match run_arg(&args).and_then(|k| command_line_action(app, k)) {
                Some(a) => perform(app, &a),
                None => open_settings(app),
            }
        }))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let action = state(app)
                        .registered
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|(s, _)| s == shortcut)
                        .map(|(_, a)| a.clone());
                    if let Some(action) = action {
                        perform(app, &action);
                    }
                })
                .build(),
        )
        // Icons for the settings window; see icons.rs.
        .register_asynchronous_uri_scheme_protocol("icon", |_ctx, request, responder| icons::handle(request, responder))
        .on_window_event(|window, event| {
            if window.label() == "settings" && matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                // Also covers a move made with the keyboard (Win+arrows), which reports no end of move.
                if let Some(r) = window.hwnd().ok().and_then(|h| winplace::current(h.0 as isize)) {
                    remember_settings_window(window.app_handle(), r);
                }
            }
            if window.label() == "settings" && matches!(event, tauri::WindowEvent::Destroyed) {
                // Closed while recording a shortcut: the page never ended the recording, so
                // put the shortcuts back here or they would stay off.
                if state(window.app_handle()).capturing.swap(false, Ordering::SeqCst) {
                    recheck_foreground(window.app_handle());
                    apply_hotkeys(window.app_handle());
                    sync(window.app_handle());
                }
                icons::clear_cache();
                // Once the icon threads have stopped, hand the window's leftover memory back to Windows.
                let app = window.app_handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(icons::IDLE + Duration::from_secs(3));
                    if app.get_webview_window("settings").is_none() {
                        system::trim_memory();
                    }
                });
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let dir = match data_dir() {
                Some(d) => d,
                None => app.path().app_config_dir()?,
            };
            let path = dir.join("config.json");
            let journal_path = dir.join("restore.json");
            // Programs a window set muted when Keyscene last quit or crashed get their sound back.
            engine::init_muted(dir.join("muted.json"));
            let first_run = !path.exists();
            let (cfg, mut warning) = config::load(&path);
            if let Some(w) = &warning {
                errlog::error(w);
            }
            let mut journal = load_journal(&journal_path);
            // A shortcut deleted since then can never be pressed again to restore; drop its entry.
            journal.retain(|id, _| cfg.presets.iter().any(|p| &p.id == id));
            if !journal.is_empty() && warning.is_none() {
                warning = Some("원래대로 하지 않은 단축키가 남아 있습니다. 트레이 메뉴에서 누르면 원래대로 돌아갑니다.".into());
            }
            app.manage(AppState {
                config: Mutex::new(cfg),
                config_path: path,
                menu: Mutex::new(None),
                registered: Mutex::new(Vec::new()),
                hotkey_errors: Mutex::new(BTreeMap::new()),
                notice: Mutex::new(warning),
                revert: Mutex::new(BTreeMap::new()),
                revert_gen: AtomicU64::new(0),
                opened_at: Mutex::new(None),
                running: Mutex::new(journal),
                busy: Mutex::new(BTreeSet::new()),
                preset_errors: Mutex::new(BTreeMap::new()),
                journal_path,
                suspended_by: Mutex::new(None),
                capturing: AtomicBool::new(false),
                tree_undo: Mutex::new(None),
                tree_gen: AtomicU64::new(0),
            });
            let h = handle.clone();
            winplace::on_placed(move |r| remember_settings_window(&h, r));
            build_tray(&handle)?;
            apply_hotkeys(&handle);
            // Windows tells us when these change; nothing polls.
            let h = handle.clone();
            reg::watch(explorer::PERSONALIZE_KEY, move || sync(&h));
            let h = handle.clone();
            audio::start(move || sync(&h));
            let h = handle.clone();
            focus::watch(move |hwnd| on_foreground(&h, hwnd));
            sync(&handle);
            // Nothing else shows that Keyscene started (new tray icons often land under ^), so
            // open the settings the very first time.
            if first_run {
                // Saved now, so closing the window without changing anything still counts as seen.
                let _ = save_config(&handle);
                open_settings(&handle);
            }
            // `keyscene.exe --run X` while Keyscene was not running: start, then run X.
            let args: Vec<String> = std::env::args().collect();
            if let Some(a) = run_arg(&args).and_then(|k| command_line_action(&handle, k)) {
                perform(&handle, &a);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_autostart,
            set_paused,
            begin_hotkey_capture,
            end_hotkey_capture,
            set_hotkey,
            dismiss_notice,
            ui_ready,
            get_presets,
            save_preset,
            delete_preset,
            run_preset,
            list_apps,
            list_browsers,
            list_monitors,
            pick_file,
            pick_path,
            list_running_apps,
            list_open_window_apps,
            set_excluded_apps,
            open_release_page
        ])
        .build(tauri::generate_context!())
        .expect("Keyscene을 시작하지 못했습니다");

    app.run(|_app, event| {
        // Closing the settings window must not quit the tray app.
        if let RunEvent::ExitRequested { code: None, api, .. } = event {
            api.prevent_exit();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_labels() {
        assert_eq!(hotkey_label("Control+Alt+KeyH"), "Ctrl+Alt+H");
        assert_eq!(hotkey_label("Alt+Backquote"), "Alt+`");
        assert_eq!(hotkey_label("Super+Control+ArrowRight"), "Win+Ctrl+→");
        assert_eq!(hotkey_label("Control+Numpad7"), "Ctrl+Num 7");
        assert_eq!(hotkey_label("Shift+NumpadAdd"), "Shift+NumpadAdd");
        assert_eq!(hotkey_label("Control+Digit0"), "Ctrl+0");
        assert_eq!(hotkey_label("F12"), "F12");
    }

    #[test]
    fn run_argument() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(run_arg(&args(&["keyscene.exe", "--run", "화면 공유 모드"])).map(String::as_str), Some("화면 공유 모드"));
        assert_eq!(run_arg(&args(&["keyscene.exe", "--run"])), None);
        assert_eq!(run_arg(&args(&["keyscene.exe"])), None);
        assert_eq!(exe_file_name(r#""C:\Games\Game.EXE""#).as_deref(), Some("game.exe"));
        assert_eq!(exe_file_name("notes.txt"), None);
    }
}
