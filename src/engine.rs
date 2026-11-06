//! Runs a preset's steps and remembers how to undo each one; restore undoes them in reverse.
//!
//! Rule: restore puts back only what this preset changed, and only while it is still the way the
//! preset left it. If the user changed it again in the meantime, restore leaves it alone.

use crate::launch;
use crate::preset::{ActiveWindowOp, AppWindowOp, MediaKey, OnRestore, Step};
use crate::system::{self, WinRef};
use crate::{audio, explorer};
use serde::{Deserialize, Serialize};
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
    Maximized { window: WinRef },
    Topmost { window: WinRef },
    /// `launched` = false: the window was already open before the preset ran.
    Opened { window: WinRef, on_restore: OnRestore, #[serde(default)] launched: bool },
}

impl Undo {
    /// Window handles mean nothing after Keyscene restarts; only settings survive in the journal.
    pub fn survives_restart(&self) -> bool {
        !matches!(self, Undo::Minimized { .. } | Undo::Maximized { .. } | Undo::Topmost { .. } | Undo::Opened { .. })
    }
}

pub struct RunResult {
    pub undo: Vec<Undo>,
    pub errors: Vec<String>,
    /// Items a "하위 항목까지" step changed, so the tray can offer to undo them: (hide, items).
    pub tree: Option<(bool, Vec<std::path::PathBuf>)>,
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
    let mut out = RunResult { undo: Vec::new(), errors: Vec::new(), tree: None };
    for step in steps {
        if let Err(e) = run_step(step, front, &mut out) {
            out.errors.push(format!("{}: {e}", step_name(step)));
        }
    }
    out
}

/// The value to set: the opposite of now when toggling, otherwise the step's own value.
fn target(toggle: bool, now: bool, value: bool) -> bool {
    if toggle { !now } else { value }
}

fn run_step(step: &Step, front: Option<WinRef>, out: &mut RunResult) -> Result<(), String> {
    let undo = &mut out.undo;
    match step {
        Step::OpenApp { app, if_running, monitor, window, on_restore } => {
            let opened = launch::open_app(app, *if_running, monitor, *window)?;
            // "그대로" has nothing to undo; recording it would make the next press do nothing.
            if *on_restore != OnRestore::Keep {
                if let Some(w) = system::win_ref(opened.hwnd).filter(|w| w.pid == opened.pid) {
                    undo.push(Undo::Opened { window: w, on_restore: *on_restore, launched: opened.launched });
                }
            }
        }
        Step::OpenPath { path } => launch::open_path(path)?,
        Step::OpenUrl { url, browser, new_window, private } => launch::open_url(url, browser, *new_window, *private)?,
        Step::AppWindows { app, op } => {
            let windows: Vec<WinRef> = launch::app_windows(app).into_iter().filter_map(system::win_ref).collect();
            match op {
                AppWindowOp::Minimize => {
                    let done: Vec<WinRef> = windows.into_iter().filter(|w| system::minimize(w)).collect();
                    if !done.is_empty() {
                        undo.push(Undo::Minimized { windows: done });
                    }
                }
                AppWindowOp::Close => windows.iter().for_each(|w| launch::close_window(w.hwnd)),
            }
        }
        Step::ShowDesktop => {
            let windows = system::show_desktop();
            if !windows.is_empty() {
                undo.push(Undo::Minimized { windows });
            }
        }
        Step::ActiveWindow { op } => {
            let Some(w) = front else { return Ok(()) };
            match op {
                ActiveWindowOp::Minimize if system::minimize(&w) => undo.push(Undo::Minimized { windows: vec![w] }),
                ActiveWindowOp::Maximize if system::maximize(&w) => undo.push(Undo::Maximized { window: w }),
                ActiveWindowOp::Topmost if system::set_topmost(&w) => undo.push(Undo::Topmost { window: w }),
                _ => {}
            }
        }
        Step::SpeakerMute { mute, toggle } => {
            let was = system::speaker_muted()?;
            let set = target(*toggle, was, *mute);
            if was != set {
                system::set_speaker_muted(set)?;
                undo.push(Undo::SpeakerMute { was, set });
            }
        }
        Step::SpeakerVolume { percent } => {
            let was = system::speaker_volume()?;
            let set = (*percent).min(100) as f32 / 100.0;
            if (was - set).abs() > 0.005 {
                system::set_speaker_volume(set)?;
                // Store what Windows actually applied, so the "unchanged since" check is exact.
                let set = system::speaker_volume().unwrap_or(set);
                undo.push(Undo::SpeakerVolume { was, set });
            }
        }
        Step::MicMute { mute, toggle } => {
            // No microphone: nothing can be heard, so there is nothing to do (not an error,
            // or the default "screen sharing" shortcut would complain on every desktop PC).
            let Some(was) = audio::muted() else { return Ok(()) };
            let set = target(*toggle, was, *mute);
            if was != set {
                audio::set_muted(set)?;
                undo.push(Undo::MicMute { was, set });
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
                undo.push(Undo::DesktopIcons { was, set });
            }
        }
        Step::HiddenItems { show, toggle } => {
            let was = explorer::hidden_items_shown()?;
            let set = target(*toggle, was, *show);
            if was != set {
                explorer::set_hidden_items_shown(set)?;
                undo.push(Undo::HiddenItems { was, set });
            }
        }
        Step::Theme { dark, toggle } => {
            let was = system::theme()?;
            // (apps, system) light flags; (0, 0) = dark.
            let dark = target(*toggle, was == (0, 0), *dark);
            let set = if dark { (0, 0) } else { (1, 1) };
            if was != set {
                system::set_theme(set.0, set.1)?;
                undo.push(Undo::Theme { was, set });
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

/// Undoes in reverse order. Returns problems worth telling the user about.
pub fn restore(undo: Vec<Undo>) -> Vec<String> {
    let mut errors = Vec::new();
    for item in undo.into_iter().rev() {
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
            Undo::Minimized { windows } => {
                system::restore_minimized(&windows);
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
            Undo::Opened { window, on_restore, launched } => {
                let opened = launch::OpenedWindow { hwnd: window.hwnd, pid: window.pid, launched };
                launch::restore_opened(&opened, on_restore);
                Ok(())
            }
        };
        if let Err(e) = result {
            errors.push(e);
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::{AppRef, AppSource, IfRunning, MonitorRef, WindowState};

    fn notepad_step(on_restore: OnRestore) -> Step {
        Step::OpenApp {
            app: AppRef { source: AppSource::File, id: r"C:\Windows\System32\notepad.exe".into(), name: "메모장".into(), args: String::new() },
            if_running: IfRunning::Front,
            monitor: MonitorRef::AppDefault,
            window: WindowState::AppDefault,
            on_restore,
        }
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
        restore(first.undo);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!alive(&window), "a window the preset opened should close");

        // 2) Already running: brought to the front only, so restore minimizes and keeps it.
        let _ = std::process::Command::new("notepad.exe").spawn();
        std::thread::sleep(Duration::from_secs(2));
        let second = run(&[notepad_step(OnRestore::Close)], None);
        let Some(Undo::Opened { window, launched, .. }) = second.undo.first().cloned() else { panic!("no undo") };
        assert!(!launched);
        restore(second.undo);
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
