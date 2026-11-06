//! File Explorer view options and refreshing open Explorer windows.

use crate::reg::{wide, Key};
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, PostMessageW, SendMessageTimeoutW, HWND_BROADCAST,
    SMTO_ABORTIFHUNG, WM_COMMAND, WM_SETTINGCHANGE,
};

pub const ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

/// Explorer's "Hidden" value: 1 = show hidden items, 2 = do not show.
pub fn hidden_items_shown() -> Result<bool, String> {
    let key = Key::open_read(ADVANCED)?;
    Ok(key.get_dword("Hidden").unwrap_or(2) == 1)
}

pub fn set_hidden_items_shown(show: bool) -> Result<(), String> {
    let key = Key::open_write(ADVANCED)?;
    key.set_dword("Hidden", if show { 1 } else { 2 })?;
    if hidden_items_shown()? != show {
        return Err("설정을 저장했지만 확인하지 못했습니다.".into());
    }
    // Explorer keeps its own copy of these options and writes it back on some commands
    // (e.g. the desktop-icons toggle). Tell it now and wait, so the next step can't undo this.
    notify_shell_state();
    request_refresh();
    Ok(())
}

/// Tells Explorer to re-read its view options; returns once it has (or after 1 s if it hangs).
///
/// Only Explorer's own desktop and taskbar windows are waited for. Every other window hears
/// about it from the background refresh, so one slow app can't freeze the tray meanwhile
/// (a broadcast waits for each window in turn).
fn notify_shell_state() {
    let shell_state = wide("ShellState");
    for class in ["Progman", "Shell_TrayWnd"] {
        let mut result = 0usize;
        unsafe {
            let hwnd = FindWindowW(wide(class).as_ptr(), std::ptr::null());
            if !hwnd.is_null() {
                SendMessageTimeoutW(hwnd, WM_SETTINGCHANGE, 0, shell_state.as_ptr() as LPARAM, SMTO_ABORTIFHUNG, 1000, &mut result);
            }
        }
    }
}

/// Explorer's "HideIcons" value: 1 = desktop icons hidden.
pub fn desktop_icons_hidden() -> Result<bool, String> {
    let key = Key::open_read(ADVANCED)?;
    Ok(key.get_dword("HideIcons").unwrap_or(0) == 1)
}

/// Explorer's own "Show desktop icons" command; it flips the state and saves HideIcons itself.
const CMD_TOGGLE_DESKTOP_ICONS: usize = 0x7402;

pub fn set_desktop_icons_hidden(hide: bool) -> Result<(), String> {
    if desktop_icons_hidden()? == hide {
        return Ok(());
    }
    let view = desktop_view().ok_or("바탕 화면을 찾지 못했습니다. 탐색기가 실행 중인지 확인하세요.")?;
    let hidden_before = hidden_items_shown().ok();
    let mut result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(view, WM_COMMAND, CMD_TOGGLE_DESKTOP_ICONS, 0, SMTO_ABORTIFHUNG, 1000, &mut result)
    };
    if sent == 0 {
        return Err("탐색기가 응답하지 않아 바탕 화면 아이콘을 바꾸지 못했습니다.".into());
    }
    // This command makes Explorer save all its view options; if it wrote back an old
    // "show hidden items" value, put ours back.
    if let Some(before) = hidden_before {
        if hidden_items_shown().ok() != Some(before) {
            set_hidden_items_shown(before)?;
        }
    }
    Ok(())
}

/// The desktop's SHELLDLL_DefView lives under Progman, or under a WorkerW when a slideshow wallpaper runs.
fn desktop_view() -> Option<HWND> {
    unsafe {
        let defview = wide("SHELLDLL_DefView");
        let progman = FindWindowW(wide("Progman").as_ptr(), std::ptr::null());
        if !progman.is_null() {
            let v = FindWindowExW(progman, std::ptr::null_mut(), defview.as_ptr(), std::ptr::null());
            if !v.is_null() {
                return Some(v);
            }
        }
        let worker = wide("WorkerW");
        let mut w = FindWindowExW(std::ptr::null_mut(), std::ptr::null_mut(), worker.as_ptr(), std::ptr::null());
        while !w.is_null() {
            let v = FindWindowExW(w, std::ptr::null_mut(), defview.as_ptr(), std::ptr::null());
            if !v.is_null() {
                return Some(v);
            }
            w = FindWindowExW(std::ptr::null_mut(), w, worker.as_ptr(), std::ptr::null());
        }
        None
    }
}

/// True when the taskbar uses the light theme (dark tray glyphs are needed).
pub fn taskbar_is_light() -> bool {
    Key::open_read(PERSONALIZE)
        .ok()
        .and_then(|k| k.get_dword("SystemUsesLightTheme"))
        .unwrap_or(0)
        == 1
}

pub const PERSONALIZE_KEY: &str = PERSONALIZE;

// One worker refreshes Explorer; requests that arrive while it works are merged into one more pass.
static REFRESH: OnceLock<Sender<()>> = OnceLock::new();

fn request_refresh() {
    let tx = REFRESH.get_or_init(|| {
        let (tx, rx) = channel::<()>();
        std::thread::Builder::new()
            .name("explorer refresh".into())
            .spawn(move || {
                while rx.recv().is_ok() {
                    while rx.try_recv().is_ok() {}
                    refresh_now();
                }
            })
            .expect("refresh thread");
        tx
    });
    let _ = tx.send(());
}

/// Explorer's own "Refresh" command id for a shell view.
const CMD_REFRESH: usize = 28931;

fn refresh_now() {
    unsafe {
        let shell_state = wide("ShellState");
        let mut result = 0usize;
        // A hung window must not block us: give up on it after 200 ms.
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            shell_state.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            200,
            &mut result,
        );
        EnumWindows(Some(visit_top_window), 0);
    }
}

unsafe extern "system" fn visit_top_window(hwnd: HWND, _: LPARAM) -> i32 {
    let mut buf = [0u16; 64];
    let len = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    let class = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
    // Desktop, Explorer windows and file dialogs host a SHELLDLL_DefView somewhere below.
    let depth = match class.as_str() {
        "Progman" | "WorkerW" => 2,
        "CabinetWClass" => 8,
        "#32770" => 6,
        _ => return 1,
    };
    refresh_views(hwnd, depth);
    1
}

unsafe fn refresh_views(parent: HWND, depth: u32) {
    let defview = wide("SHELLDLL_DefView");
    let view = FindWindowExW(parent, std::ptr::null_mut(), defview.as_ptr(), std::ptr::null());
    if !view.is_null() {
        PostMessageW(view, WM_COMMAND, CMD_REFRESH, 0);
        return;
    }
    if depth == 0 {
        return;
    }
    let mut child = FindWindowExW(parent, std::ptr::null_mut(), std::ptr::null(), std::ptr::null());
    while !child.is_null() {
        refresh_views(child, depth - 1);
        child = FindWindowExW(parent, child, std::ptr::null(), std::ptr::null());
    }
}
