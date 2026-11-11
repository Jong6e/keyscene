//! Runs a preset's steps and remembers how to undo each one; restore undoes them in reverse.
//!
//! Rule: restore puts back only what this preset changed, and only while it is still the way the
//! preset left it. If the user changed it again in the meantime, restore leaves it alone.

use crate::launch;
use crate::preset::{ActiveWindowOp, AppWhen, AppWindowOp, IfRunning, MediaKey, MonitorRef, OnRestore, OthersOp, SetSite, Step, WindowSet, WindowState};
use crate::system::{self, WinRef};
use crate::{audio, explorer};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Mutex;
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK};

/// One thing to undo. Window entries carry the process id so a reused window handle is never touched.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Undo {
    SpeakerMute { was: bool, set: bool },
    SpeakerVolume { was: f32, set: f32 },
    MicMute { was: bool, set: bool },
    DesktopIcons { was: bool, set: bool },
    HiddenItems { was: bool, set: bool },
    Theme { was: (u32, u32), set: (u32, u32) },
    Minimized { windows: Vec<WinRef> },
    /// The app windows that were up when 바탕 화면 보기 ran, top-most first, and the active window
    /// then: restore puts what it brings back in this order and makes that window active again.
    Stack {
        windows: Vec<WinRef>,
        #[serde(default)]
        front: Option<WinRef>,
    },
    /// Windows a step brought up (restored from minimized or newly opened); restore minimizes them.
    Shown { windows: Vec<WinRef> },
    /// Programs whose sound a window set turned off (by program file name); restore turns it back on.
    AppsMuted { exes: Vec<String> },
    /// Media sessions a window set paused (session app ids); restore plays them again.
    MediaPaused { items: Vec<system::PausedMedia> },
    Maximized { window: WinRef },
    Topmost { window: WinRef },
    /// `launched` = false: the window was already open before the preset ran.
    /// `before`: where an already-open window was, to put it back if it was up before the preset.
    Opened {
        window: WinRef,
        on_restore: OnRestore,
        #[serde(default)]
        launched: bool,
        #[serde(default)]
        before: Option<launch::Placement>,
    },
}

impl Undo {
    /// Window handles mean nothing after Keyscene restarts; only settings survive in the journal.
    pub fn survives_restart(&self) -> bool {
        // Sound a window set turned off comes back at start from muted.json instead.
        !matches!(self, Undo::Minimized { .. } | Undo::Stack { .. } | Undo::AppsMuted { .. } | Undo::Shown { .. } | Undo::MediaPaused { .. } | Undo::Maximized { .. } | Undo::Topmost { .. } | Undo::Opened { .. })
    }
}

pub struct RunResult {
    pub undo: Vec<Undo>,
    pub errors: Vec<String>,
    /// Items a "하위 항목까지" step changed, so the tray can offer to undo them: (hide, items).
    pub tree: Option<(bool, Vec<std::path::PathBuf>)>,
    /// Windows a step brought up or moved; the end-of-run check never minimizes these again.
    claimed: HashSet<isize>,
    /// Windows 바탕 화면 보기 asked to minimize, checked once all steps ran.
    settle: Vec<WinRef>,
}

impl Undo {
    /// False when the user has already put this back by hand, so restoring would change nothing.
    /// Window entries are assumed to still matter.
    pub fn still_applies(&self) -> bool {
        match self {
            Undo::SpeakerMute { set, .. } => system::speaker_muted().is_ok_and(|now| now == *set),
            Undo::SpeakerVolume { set, .. } => system::speaker_volume().is_ok_and(|now| (now - set).abs() < 0.01),
            Undo::MicMute { set, .. } => audio::muted() == Some(*set),
            Undo::DesktopIcons { set, .. } => explorer::desktop_icons_hidden().is_ok_and(|now| now == *set),
            Undo::HiddenItems { set, .. } => explorer::hidden_items_shown().is_ok_and(|now| now == *set),
            Undo::Theme { set, .. } => system::theme().is_ok_and(|now| now == *set),
            _ => true,
        }
    }
}

fn step_name(step: &Step) -> &'static str {
    match step {
        Step::OpenApp { .. } => "앱 열기",
        Step::OpenPath { .. } => "파일·폴더 열기",
        Step::OpenUrl { .. } => "웹사이트 열기",
        Step::AppWindows { op: AppWindowOp::Minimize, .. } => "앱 최소화",
        Step::AppWindows { op: AppWindowOp::Close, .. } => "앱 닫기",
        Step::ShowDesktop => "바탕 화면 보기",
        Step::ActiveWindow { .. } => "현재 창",
        Step::WindowSets { .. } => "창 세트 전환",
        Step::SpeakerMute { .. } => "스피커 음소거",
        Step::SpeakerVolume { .. } => "음량 맞추기",
        Step::MicMute { toggle: true, .. } => "마이크 음소거 전환",
        Step::MicMute { .. } => "마이크 음소거",
        Step::Media { .. } => "미디어 키",
        Step::DesktopIcons { .. } => "바탕 화면 아이콘",
        Step::HiddenItems { .. } => "숨긴 항목",
        Step::SelectionHidden => "선택한 항목 숨기기",
        Step::FolderTree { hide: true, .. } => "하위 항목까지 숨기기",
        Step::FolderTree { hide: false, .. } => "하위 항목까지 숨김 해제",
        Step::Theme { .. } => "어둡게·밝게 모드",
        Step::MonitorOff => "모니터 끄기",
        Step::Lock => "PC 잠금",
        Step::Sleep => "절전",
        Step::ClearClipboard => "클립보드 비우기",
        Step::Screenshot => "화면 캡처",
        Step::SendKeys { .. } => "단축키 보내기",
        Step::TypeText { .. } => "글자 입력",
        Step::Wait { .. } => "기다리기",
    }
}

/// Runs the steps in order on the calling thread (which must have initialized COM).
/// `front` is the window that was in front when the preset was started.
pub fn run(steps: &[Step], front: Option<WinRef>) -> RunResult {
    let mut out = RunResult { undo: Vec::new(), errors: Vec::new(), tree: None, claimed: HashSet::new(), settle: Vec::new() };
    let mut i = 0;
    while i < steps.len() {
        let batch = open_app_batch(&steps[i..]);
        if batch > 1 {
            open_apps_together(&steps[i..i + batch], &mut out);
            i += batch;
            continue;
        }
        let result = match &steps[i] {
            Step::ShowDesktop => show_desktop(&steps[i + 1..], front, &mut out),
            step => run_step(step, front, &mut out),
        };
        if let Err(e) = result {
            out.errors.push(format!("{}: {e}", step_name(&steps[i])));
        }
        i += 1;
    }
    if !out.settle.is_empty() {
        system::settle_minimized(&out.settle, &out.claimed);
    }
    out
}

/// Programs already warned about this session for running as administrator (a warning on every
/// press would only repeat what the user can't change).
static WARNED_ELEVATED: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());

/// 바탕 화면 보기. Windows that a later 앱 열기 step brings forward anyway stay up, so they don't
/// go down and come straight back (a visible flash).
fn show_desktop(later: &[Step], front: Option<WinRef>, out: &mut RunResult) -> Result<(), String> {
    let keep: HashSet<isize> = later
        .iter()
        .filter_map(|step| match step {
            Step::OpenApp { app, if_running: IfRunning::Front, window, .. } if *window != WindowState::Minimized => {
                launch::app_windows(app).first().copied()
            }
            _ => None,
        })
        .collect();
    let done = system::show_desktop(&keep);
    // Only worth keeping when something went down (otherwise the next press would undo nothing).
    if !done.minimized.is_empty() {
        out.undo.push(Undo::Stack { windows: done.stack, front });
    }
    if !done.minimized.is_empty() {
        out.settle.extend(done.minimized.iter().copied());
        out.undo.push(Undo::Minimized { windows: done.minimized });
    }
    let new: Vec<u32> = {
        let mut warned = WARNED_ELEVATED.lock().unwrap_or_else(|e| e.into_inner());
        done.elevated.into_iter().filter(|pid| warned.insert(*pid)).collect()
    };
    if !new.is_empty() {
        return Err("관리자 권한으로 실행 중인 프로그램의 창은 내리지 못했습니다. Keyscene도 관리자 권한으로 실행하면 내릴 수 있습니다.".into());
    }
    Ok(())
}

/// How many 앱 열기 steps at the start of `steps` can open at the same time. Steps whose apps could
/// share windows (same program, a browser and its web apps, …) stay in order, so two waits never
/// claim the same new window.
fn open_app_batch(steps: &[Step]) -> usize {
    if !matches!(steps, [Step::OpenApp { .. }, Step::OpenApp { .. }, ..]) {
        return usize::from(matches!(steps.first(), Some(Step::OpenApp { .. })));
    }
    let mut ids: Vec<launch::AppIdentity> = Vec::new();
    for step in steps {
        let Step::OpenApp { app, .. } = step else { break };
        let id = launch::app_identity(app);
        if ids.iter().any(|x| x.overlaps(&id)) {
            break;
        }
        ids.push(id);
    }
    ids.len()
}

/// Opens several apps at once (each waits for its own window), keeping undo and errors in step order.
/// The last step's window ends up active, as when they ran one after another.
fn open_apps_together(steps: &[Step], out: &mut RunResult) {
    type Opened = Result<(Option<Undo>, Option<launch::OpenedWindow>), String>;
    let results: Vec<Opened> = std::thread::scope(|s| {
        let handles: Vec<_> = steps.iter().map(|step| s.spawn(move || open_app_step(step))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("앱을 여는 중 문제가 생겼습니다.".into()))).collect()
    });
    let mut last_up = None;
    for (step, result) in steps.iter().zip(results) {
        match result {
            Ok((undo, opened)) => {
                out.undo.extend(undo);
                if let Some(o) = opened {
                    out.claimed.insert(o.hwnd);
                    if !matches!(step, Step::OpenApp { window: WindowState::Minimized, .. }) {
                        last_up = Some(o);
                    }
                }
            }
            Err(e) => out.errors.push(format!("{}: {e}", step_name(step))),
        }
    }
    if let Some(o) = last_up {
        launch::focus(o.hwnd, o.pid);
    }
}

/// One 앱 열기 step. Returns what to undo, if anything, and the window it acted on.
fn open_app_step(step: &Step) -> Result<(Option<Undo>, Option<launch::OpenedWindow>), String> {
    let Step::OpenApp { app, when, if_running, monitor, window, on_restore } = step else { return Ok((None, None)) };
    if *when == AppWhen::IfRunning && !launch::app_running(app) {
        return Ok((None, None)); // "앱이 실행 중일 때만": nothing to do while it is not running
    }
    let opened = launch::open_app(app, *if_running, monitor, *window)?;
    let window_ref = system::win_ref(opened.hwnd).filter(|w| w.pid == opened.pid);
    // "그대로" has nothing to undo; recording it would make the next press do nothing.
    let undo = window_ref
        .filter(|_| *on_restore != OnRestore::Keep)
        .map(|w| Undo::Opened { window: w, on_restore: *on_restore, launched: opened.launched, before: opened.before });
    Ok((undo, Some(opened)))
}

/// The value to set: the opposite of now when toggling, otherwise the step's own value.
fn target(toggle: bool, now: bool, value: bool) -> bool {
    if toggle { !now } else { value }
}

fn run_step(step: &Step, front: Option<WinRef>, out: &mut RunResult) -> Result<(), String> {
    match step {
        Step::OpenApp { .. } => {
            let (u, opened) = open_app_step(step)?;
            out.undo.extend(u);
            out.claimed.extend(opened.map(|o| o.hwnd));
        }
        Step::OpenPath { path } => launch::open_path(path)?,
        Step::OpenUrl { url, browser, new_window, private } => launch::open_url(url, browser, *new_window, *private)?,
        Step::AppWindows { app, op } => {
            let windows: Vec<WinRef> = launch::app_windows(app).into_iter().filter_map(system::win_ref).collect();
            match op {
                AppWindowOp::Minimize => {
                    let done: Vec<WinRef> = windows.into_iter().filter(|w| system::minimize(w)).collect();
                    if !done.is_empty() {
                        out.undo.push(Undo::Minimized { windows: done });
                    }
                }
                AppWindowOp::Close => windows.iter().for_each(|w| launch::close_window(w.hwnd)),
            }
        }
        Step::ShowDesktop => show_desktop(&[], front, out)?,
        Step::WindowSets { sets, when, others, mute_hidden, pause_hidden } => {
            let shown = window_sets(sets, *when, *others, Quiet { mute: *mute_hidden, pause: *pause_hidden }, &mut out.undo)?;
            out.claimed.extend(shown);
        }
        Step::ActiveWindow { op } => {
            let Some(w) = front else { return Ok(()) };
            match op {
                ActiveWindowOp::Minimize if system::minimize(&w) => out.undo.push(Undo::Minimized { windows: vec![w] }),
                ActiveWindowOp::Maximize if system::maximize(&w) => out.undo.push(Undo::Maximized { window: w }),
                ActiveWindowOp::Topmost if system::set_topmost(&w) => out.undo.push(Undo::Topmost { window: w }),
                _ => {}
            }
        }
        Step::SpeakerMute { mute, toggle } => {
            let was = system::speaker_muted()?;
            let set = target(*toggle, was, *mute);
            if was != set {
                system::set_speaker_muted(set)?;
                out.undo.push(Undo::SpeakerMute { was, set });
            }
        }
        Step::SpeakerVolume { percent } => {
            let was = system::speaker_volume()?;
            let set = (*percent).min(100) as f32 / 100.0;
            if (was - set).abs() > 0.005 {
                system::set_speaker_volume(set)?;
                // Store what Windows actually applied, so the "unchanged since" check is exact.
                let set = system::speaker_volume().unwrap_or(set);
                out.undo.push(Undo::SpeakerVolume { was, set });
            }
        }
        Step::MicMute { mute, toggle } => {
            // No microphone: nothing can be heard, so there is nothing to do (not an error,
            // or the default "screen sharing" shortcut would complain on every desktop PC).
            let Some(was) = audio::muted() else { return Ok(()) };
            let set = target(*toggle, was, *mute);
            if was != set {
                audio::set_muted(set)?;
                out.undo.push(Undo::MicMute { was, set });
            }
        }
        Step::Media { key } => system::media_key(match key {
            MediaKey::PlayPause => VK_MEDIA_PLAY_PAUSE,
            MediaKey::Next => VK_MEDIA_NEXT_TRACK,
            MediaKey::Previous => VK_MEDIA_PREV_TRACK,
        }),
        Step::DesktopIcons { hide, toggle } => {
            let was = explorer::desktop_icons_hidden()?;
            let set = target(*toggle, was, *hide);
            if was != set {
                explorer::set_desktop_icons_hidden(set)?;
                out.undo.push(Undo::DesktopIcons { was, set });
            }
        }
        Step::HiddenItems { show, toggle } => {
            let was = explorer::hidden_items_shown()?;
            let set = target(*toggle, was, *show);
            if was != set {
                explorer::set_hidden_items_shown(set)?;
                out.undo.push(Undo::HiddenItems { was, set });
            }
        }
        Step::Theme { dark, toggle } => {
            let was = system::theme()?;
            // (apps, system) light flags; (0, 0) = dark.
            let dark = target(*toggle, was == (0, 0), *dark);
            let set = if dark { (0, 0) } else { (1, 1) };
            if was != set {
                system::set_theme(set.0, set.1)?;
                out.undo.push(Undo::Theme { was, set });
            }
        }
        Step::FolderTree { path, hide, include_self, selected } => {
            let roots: Vec<std::path::PathBuf> = if *selected {
                let picked = crate::shell::selection()?;
                if picked.is_empty() {
                    return Err("파일 탐색기에서 폴더를 선택한 뒤 실행하세요.".into());
                }
                picked.into_iter().map(std::path::PathBuf::from).collect()
            } else if path.trim().is_empty() {
                return Err("폴더가 지정되지 않았습니다.".into());
            } else {
                vec![std::path::PathBuf::from(path.trim())]
            };
            let changed = crate::tree::apply(&roots, *hide, *include_self);
            if !changed.changed.is_empty() {
                out.tree = Some((*hide, changed.changed.clone()));
            }
            if let Some(msg) = changed.message() {
                return Err(msg);
            }
        }
        Step::SelectionHidden => {
            if let Some(msg) = crate::shell::toggle_selection() {
                return Err(msg);
            }
        }
        Step::MonitorOff => system::monitor_off(),
        Step::Lock => system::lock()?,
        Step::Sleep => system::sleep()?,
        Step::ClearClipboard => system::clear_clipboard()?,
        Step::Screenshot => system::screenshot(),
        Step::SendKeys { spec } => system::send_keys(spec)?,
        Step::TypeText { text } => system::type_text(text)?,
        Step::Wait { ms } => std::thread::sleep(Duration::from_millis((*ms).clamp(100, 60_000) as u64)),
    }
    Ok(())
}

// ---------- 창 세트 ----------

/// Which set each multi-set step showed last, by the step's content. Forgotten on restart, so the
/// first press after starting shows the first set; editing the sets starts over too.
static SET_CYCLE: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());

/// The set to show on this press: the one after the last shown, wrapping around.
fn next_set(sets: &[&WindowSet]) -> usize {
    let key = serde_json::to_string(sets).unwrap_or_default();
    let mut cycle = SET_CYCLE.lock().unwrap_or_else(|e| e.into_inner());
    let next = cycle.get(&key).map_or(0, |last| (last + 1) % sets.len());
    cycle.insert(key, next);
    next
}

/// Apps whose sound a window set turned off, so showing their set again turns it back on
/// (and nothing the user muted themselves is touched). Program file names, lowercase.
static SET_MUTED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// Media a window set paused, played again when its window shows.
static SET_PAUSED: Mutex<Vec<system::PausedMedia>> = Mutex::new(Vec::new());

/// What a switch does to the apps it minimizes.
#[derive(Clone, Copy)]
struct Quiet {
    mute: bool,
    pause: bool,
}

/// Takes the entries of `set` that `wanted` also has out of it.
fn take_from(set: &Mutex<BTreeSet<String>>, wanted: impl Fn(&String) -> bool) -> BTreeSet<String> {
    let mut set = set.lock().unwrap_or_else(|e| e.into_inner());
    let taken: BTreeSet<String> = set.iter().filter(|x| wanted(x)).cloned().collect();
    set.retain(|x| !taken.contains(x));
    taken
}


/// Program file names of these windows, lowercase.
fn exes_of(windows: impl IntoIterator<Item = isize>) -> BTreeSet<String> {
    windows.into_iter().filter_map(launch::window_exe).collect()
}

/// Brings up one set's windows and minimizes the other sets' apps (and, if asked, every other window).
/// One set: records what it changed so pressing again puts it back. More: nothing to put back,
/// so every press moves on to the next set. Returns the windows it shows.
fn window_sets(sets: &[WindowSet], when: AppWhen, others: OthersOp, quiet: Quiet, undo: &mut Vec<Undo>) -> Result<Vec<isize>, String> {
    let sets: Vec<&WindowSet> = sets.iter().filter(|s| !s.apps.is_empty() || !s.sites.is_empty()).collect();
    if sets.is_empty() {
        return Err("기록한 세트가 없습니다. 동작을 열어 지금 열린 창을 기록하세요.".into());
    }
    let index = if sets.len() == 1 { 0 } else { next_set(&sets) };
    let show = sets[index];
    let mut errors = Vec::new();

    // Browser windows first: the one showing each of this set's sites, then the other sets' sites.
    // A browser recorded as an app must not take over windows that belong to another set's site.
    let mut site_up: Vec<isize> = Vec::new();
    let mut missing_sites: Vec<&SetSite> = Vec::new();
    for site in &show.sites {
        let taken: HashSet<isize> = site_up.iter().copied().collect();
        match launch::site_window(site, &taken) {
            Some(h) => site_up.push(h),
            None => missing_sites.push(site),
        }
    }
    let mut other_sites: Vec<isize> = Vec::new();
    for (i, set) in sets.iter().enumerate() {
        if i != index {
            for site in set.sites.iter().filter(|s| !show.sites.contains(s)) {
                let taken: HashSet<isize> = site_up.iter().chain(&other_sites).copied().collect();
                other_sites.extend(launch::site_window(site, &taken));
            }
        }
    }

    // The set's windows, in the set's order (the first app ends in front).
    let mut up: Vec<isize> = Vec::new();
    let mut opened: Vec<isize> = Vec::new();
    for app in &show.apps {
        let all = launch::app_windows(app);
        let windows: Vec<isize> = all.iter().copied().filter(|h| !other_sites.contains(h)).collect();
        if !windows.is_empty() {
            up.extend(windows);
        } else if !all.is_empty() {
            // Its only windows are other sets' sites (a browser recorded as an app): leave them be.
        } else if when == AppWhen::Always || launch::app_in_tray(app) {
            // Not running, or running with no window (waiting in the tray): open or call it up.
            match launch::open_app(app, IfRunning::Front, &MonitorRef::AppDefault, WindowState::AppDefault) {
                Ok(o) if o.launched => opened.push(o.hwnd),
                Ok(o) => up.push(o.hwnd),
                Err(e) => errors.push(e),
            }
        }
    }
    let sites_up: Vec<isize> = site_up.iter().copied().filter(|h| !up.contains(h)).collect();
    up.extend(sites_up);
    if when == AppWhen::Always {
        for site in missing_sites {
            match launch::open_site(site) {
                Ok(Some(h)) => opened.push(h),
                Ok(None) => {}
                Err(e) => errors.push(e),
            }
        }
    }
    let keep: HashSet<isize> = up.iter().chain(&opened).copied().collect();

    // Windows to put away: the other sets' apps and sites, and with "최소화" every other app window too.
    let mut away: Vec<isize> = other_sites.clone();
    for (i, set) in sets.iter().enumerate() {
        if i != index {
            for app in set.apps.iter().filter(|a| !show.apps.contains(a)) {
                away.extend(launch::app_windows(app));
            }
        }
    }
    // Only the sets' own apps lose their sound; a messenger minimized by "최소화" keeps its alerts.
    let set_windows: HashSet<isize> = away.iter().copied().collect();
    if others == OthersOp::Minimize {
        // Windows owned by a shown window (its dialogs, pop-out windows) stay up with it.
        away.extend(system::visible_app_windows().into_iter().map(|w| w.hwnd).filter(|&h| !keep.contains(&system::root_owner(h))));
    }
    let mut seen = HashSet::new();
    away.retain(|h| !keep.contains(h) && seen.insert(*h));
    let minimized: Vec<WinRef> = away.into_iter().filter_map(system::win_ref).filter(system::minimize).collect();

    // Windows first, so slow sound or media calls never hold them up.
    let restored = launch::show_windows(&up);

    // Sound: back on for the apps now shown (only what a set turned off), off for the ones put away.
    // Windows can only mute a whole program, so a program that still has any window up keeps its sound.
    let hidden_set_windows: Vec<isize> = minimized.iter().map(|w| w.hwnd).filter(|h| set_windows.contains(h)).collect();
    let shown_exes = exes_of(keep.iter().copied());
    unmute_sets(|e| shown_exes.contains(e));
    let mut silenced = Vec::new();
    if quiet.mute && !hidden_set_windows.is_empty() {
        let gone: HashSet<isize> = minimized.iter().map(|w| w.hwnd).collect();
        let still_up = exes_of(system::visible_app_windows().into_iter().map(|w| w.hwnd).filter(|h| !gone.contains(h)));
        let hide: BTreeSet<String> =
            exes_of(hidden_set_windows.iter().copied()).into_iter().filter(|e| !shown_exes.contains(e) && !still_up.contains(e)).collect();
        silenced = system::set_apps_muted(&hide, true);
        remember_muted(&silenced);
    }
    // Media: play again what a set paused in the windows now shown; pause what plays in the ones put away.
    let shown_media: Vec<system::MediaWindow> = keep.iter().map(|&h| launch::media_window(h)).collect();
    let hidden_media: Vec<system::MediaWindow> = hidden_set_windows.iter().map(|&h| launch::media_window(h)).collect();
    let back: Vec<system::PausedMedia> = {
        let mut list = SET_PAUSED.lock().unwrap_or_else(|e| e.into_inner());
        let (back, stay): (Vec<_>, Vec<_>) = list.drain(..).partition(|m| system::media_comes_back(m, &shown_media, &hidden_media));
        *list = stay;
        back
    };
    let pause = quiet.pause && !hidden_media.is_empty();
    // A frozen player could keep these from ever answering; never wait more than a moment. The
    // worker records what it paused itself, so nothing is forgotten if it finishes late.
    let paused = with_timeout(Duration::from_secs(3), Vec::new(), move || {
        system::resume_media(&back);
        let paused = if pause { system::pause_media(&hidden_media, &shown_media) } else { Vec::new() };
        SET_PAUSED.lock().unwrap_or_else(|e| e.into_inner()).extend(paused.iter().cloned());
        paused
    });

    if sets.len() == 1 {
        if !minimized.is_empty() {
            undo.push(Undo::Minimized { windows: minimized });
        }
        if !silenced.is_empty() {
            undo.push(Undo::AppsMuted { exes: silenced });
        }
        if !paused.is_empty() {
            undo.push(Undo::MediaPaused { items: paused });
        }
        let shown: Vec<WinRef> = restored.into_iter().chain(opened.iter().copied()).filter_map(system::win_ref).collect();
        if !shown.is_empty() {
            undo.push(Undo::Shown { windows: shown });
        }
    }
    if errors.is_empty() { Ok(keep.into_iter().collect()) } else { Err(errors.join(" · ")) }
}

/// Runs `f` on its own thread and gives up waiting after `limit` (returning `fallback`); the thread
/// finishes on its own if it ever does.
fn with_timeout<T: Send + 'static>(limit: Duration, fallback: T, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).unwrap_or(fallback)
}

// ---------- programs a window set muted, kept on disk ----------

/// Where the list of programs window sets muted is saved, so sound comes back even if Keyscene
/// quits or crashes while a set is switched away.
static MUTED_FILE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn save_muted(list: &BTreeSet<String>) {
    let Some(path) = MUTED_FILE.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
    if list.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else if let Ok(json) = serde_json::to_string(list) {
        let _ = std::fs::write(&path, json);
    }
}

fn remember_muted(exes: &[String]) {
    if exes.is_empty() {
        return;
    }
    let mut set = SET_MUTED.lock().unwrap_or_else(|e| e.into_inner());
    set.extend(exes.iter().cloned());
    save_muted(&set);
}

/// Turns sound back on for the programs window sets muted that `which` picks. A program that isn't
/// running now can't be changed (Windows remembers its mute for next time), so it stays on the list.
fn unmute_sets(which: impl Fn(&String) -> bool) {
    let back = take_from(&SET_MUTED, which);
    if !back.is_empty() {
        let (_, found) = system::set_apps_muted_found(&back, false);
        let mut set = SET_MUTED.lock().unwrap_or_else(|e| e.into_inner());
        set.extend(back.difference(&found).cloned());
        save_muted(&set);
    }
}

/// At start: remembers where the list lives and turns sound back on for programs a previous run
/// left muted (a quit or crash while a set was switched away).
pub fn init_muted(path: std::path::PathBuf) {
    let left: BTreeSet<String> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    *MUTED_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    *SET_MUTED.lock().unwrap_or_else(|e| e.into_inner()) = left;
    // On its own thread (with COM), so start-up doesn't wait. Programs not running yet stay listed
    // and get their sound back on a later switch or start.
    with_timeout(Duration::ZERO, (), || unmute_sets(|_| true));
}

/// At quit: turns sound back on for every program a window set muted.
pub fn unmute_all_sets() {
    with_timeout(Duration::from_secs(2), (), || unmute_sets(|_| true));
}

/// Undoes in reverse order. Returns problems worth telling the user about.
pub fn restore(undo: Vec<Undo>, may_focus: bool) -> Vec<String> {
    let mut errors = Vec::new();
    // For each window, the earliest step that minimized it. An already-open window that an 앱 열기
    // step used goes back where it was if it was up before the preset: up when the step found it, or
    // minimized by an earlier step (바탕 화면 보기). Otherwise it is minimized as the step says.
    let mut minimized_at: std::collections::HashMap<WinRef, usize> = std::collections::HashMap::new();
    for (i, u) in undo.iter().enumerate() {
        if let Undo::Minimized { windows } = u {
            for w in windows {
                minimized_at.entry(*w).or_insert(i);
            }
        }
    }
    let put_back: Vec<(usize, WinRef)> = undo
        .iter()
        .enumerate()
        .filter_map(|(i, u)| match u {
            Undo::Opened { window, launched: false, before: Some(p), .. }
                if !p.minimized || minimized_at.get(window).is_some_and(|&at| at < i) =>
            {
                Some((i, *window))
            }
            _ => None,
        })
        .collect();
    // put_back handles these; restoring them from minimized as well could undo a maximize.
    let mut skip: HashSet<isize> = put_back.iter().map(|(_, w)| w.hwnd).collect();
    let mut order: Vec<WinRef> = Vec::new();
    let mut front: Option<WinRef> = None;
    let mut moved: Vec<WinRef> = Vec::new();
    for (i, item) in undo.into_iter().enumerate().rev() {
        let result: Result<(), String> = match item {
            Undo::SpeakerMute { was, set } => match system::speaker_muted() {
                Ok(now) if now == set => system::set_speaker_muted(was),
                _ => Ok(()),
            },
            Undo::SpeakerVolume { was, set } => match system::speaker_volume() {
                Ok(now) if (now - set).abs() < 0.01 => system::set_speaker_volume(was),
                _ => Ok(()),
            },
            Undo::MicMute { was, set } => match audio::muted() {
                Some(now) if now == set => audio::set_muted(was),
                _ => Ok(()),
            },
            Undo::DesktopIcons { was, set } => match explorer::desktop_icons_hidden() {
                Ok(now) if now == set => explorer::set_desktop_icons_hidden(was),
                _ => Ok(()),
            },
            Undo::HiddenItems { was, set } => match explorer::hidden_items_shown() {
                Ok(now) if now == set => explorer::set_hidden_items_shown(was),
                _ => Ok(()),
            },
            Undo::Theme { was, set } => match system::theme() {
                Ok(now) if now == set => system::set_theme(was.0, was.1),
                _ => Ok(()),
            },
            Undo::Stack { windows, front: f } => {
                // The screen as 바탕 화면 보기 found it; windows later steps minimized go below.
                let rest: Vec<WinRef> = std::mem::take(&mut order).into_iter().filter(|w| !windows.contains(w)).collect();
                order = windows;
                order.extend(rest);
                front = f;
                Ok(())
            }
            Undo::Minimized { windows } => {
                // Each window once, even when two steps minimized it.
                let back = system::restore_minimized(&windows, &skip);
                skip.extend(back.iter().map(|w| w.hwnd));
                // Undone last step first, so an earlier step's windows end up higher.
                let list: Vec<WinRef> = windows.iter().copied().filter(|w| !order.contains(w)).collect();
                order.splice(0..0, list);
                moved.extend(back);
                Ok(())
            }
            Undo::MediaPaused { items } => {
                SET_PAUSED.lock().unwrap_or_else(|e| e.into_inner()).retain(|m| !items.contains(m));
                with_timeout(Duration::from_secs(2), (), move || system::resume_media(&items));
                Ok(())
            }
            Undo::AppsMuted { exes } => {
                let exes: BTreeSet<String> = exes.into_iter().collect();
                {
                    let mut set = SET_MUTED.lock().unwrap_or_else(|e| e.into_inner());
                    set.retain(|e| !exes.contains(e));
                    save_muted(&set);
                }
                system::set_apps_muted(&exes, false);
                Ok(())
            }
            Undo::Shown { windows } => {
                windows.iter().for_each(|w| {
                    system::minimize(w);
                });
                Ok(())
            }
            Undo::Maximized { window } => {
                system::unmaximize(&window);
                Ok(())
            }
            Undo::Topmost { window } => {
                system::clear_topmost(&window);
                Ok(())
            }
            Undo::Opened { window, on_restore, launched, before } => {
                let opened = launch::OpenedWindow { hwnd: window.hwnd, pid: window.pid, launched, before };
                match before {
                    Some(p) if put_back.contains(&(i, window)) => {
                        // Minimized by an earlier step, so it was up before the preset: show it.
                        let p = if minimized_at.get(&window).is_some_and(|&at| at < i) { launch::Placement { minimized: false, ..p } } else { p };
                        if launch::put_back(&opened, &p) && !p.minimized {
                            moved.push(window);
                        }
                    }
                    _ => launch::restore_opened(&opened, on_restore),
                }
                Ok(())
            }
        };
        if let Err(e) = result {
            errors.push(e);
        }
    }
    if !moved.is_empty() {
        // Windows put back by an 앱 열기 step and not in any snapshot go on top, as they were in use.
        let mut full: Vec<WinRef> = moved.iter().copied().filter(|w| !order.contains(w)).collect();
        full.extend(order.iter().copied());
        // Windows that stayed up through the preset (kept for a later step) take their old place too.
        let mut stacked = moved.clone();
        stacked.extend(order.iter().copied().filter(|w| !moved.contains(w) && system::win_is_up(w)));
        system::restack(&full, &stacked);
        // An automatic undo (유지 시간) doesn't pull the user out of what they are doing.
        if may_focus || system::foreground_is_free(&moved) {
            if let Some(w) = system::pick_focus(front, &full, &stacked) {
                launch::focus(w.hwnd, w.pid);
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::{AppRef, AppSource};

    fn notepad_step(on_restore: OnRestore) -> Step {
        Step::OpenApp {
            app: AppRef { source: AppSource::File, id: r"C:\Windows\System32\notepad.exe".into(), name: "메모장".into(), args: String::new() },
            when: AppWhen::Always,
            if_running: IfRunning::Front,
            monitor: MonitorRef::AppDefault,
            window: WindowState::AppDefault,
            on_restore,
        }
    }

    fn calc_step() -> Step {
        let mut step = notepad_step(OnRestore::Keep);
        if let Step::OpenApp { app, .. } = &mut step {
            app.id = r"C:\Windows\System32\calc.exe".into();
        }
        step
    }

    #[test]
    fn different_apps_open_together_but_the_same_app_waits() {
        let wait = Step::Wait { ms: 100 };
        assert_eq!(open_app_batch(&[notepad_step(OnRestore::Keep), calc_step(), wait.clone()]), 2);
        assert_eq!(open_app_batch(&[notepad_step(OnRestore::Keep), notepad_step(OnRestore::Close)]), 1);
        assert_eq!(open_app_batch(&[notepad_step(OnRestore::Keep), calc_step(), notepad_step(OnRestore::Keep)]), 2);
        assert_eq!(open_app_batch(&[wait, calc_step()]), 0);
    }

    fn alive(w: &WinRef) -> bool {
        system::win_ref(w.hwnd).is_some_and(|now| now.pid == w.pid)
    }

    /// Real PC only: a window the preset opened is closed on restore; one that was already open is
    /// only minimized. Skips when Notepad is already open so a user's window is never touched.
    #[test]
    #[ignore]
    fn manual_restore_closes_only_what_it_opened() {
        let app = match &notepad_step(OnRestore::Close) { Step::OpenApp { app, .. } => app.clone(), _ => unreachable!() };
        if !launch::app_windows(&app).is_empty() {
            println!("SKIPPED: Notepad is already open");
            return;
        }
        // 1) Not running: the preset opens it, so restore closes it.
        let first = run(&[notepad_step(OnRestore::Close)], None);
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        let Some(Undo::Opened { window, launched, .. }) = first.undo.first().cloned() else { panic!("no undo") };
        assert!(launched);
        restore(first.undo, true);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!alive(&window), "a window the preset opened should close");

        // 2) Already running: brought to the front only, so restore minimizes and keeps it.
        let _ = std::process::Command::new("notepad.exe").spawn();
        std::thread::sleep(Duration::from_secs(2));
        let second = run(&[notepad_step(OnRestore::Close)], None);
        let Some(Undo::Opened { window, launched, .. }) = second.undo.first().cloned() else { panic!("no undo") };
        assert!(!launched);
        restore(second.undo, true);
        std::thread::sleep(Duration::from_millis(800));
        let kept = alive(&window);
        let minimized = unsafe { windows::Win32::UI::WindowsAndMessaging::IsIconic(launch_hwnd(window.hwnd)).as_bool() };
        launch::close_window(window.hwnd);
        assert!(kept && minimized, "an already-open window should be minimized, not closed");

        println!("ok: closed own window, minimized existing one");
    }

    fn launch_hwnd(h: isize) -> windows::Win32::Foundation::HWND {
        windows::Win32::Foundation::HWND(h as *mut _)
    }
}
