//! Small system actions used by presets: speaker, theme, power, clipboard, keyboard input, windows.
//! Functions that use COM expect the caller's thread to have called CoInitializeEx.

use crate::reg::{wide, Key};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

// ---------- speaker ----------

fn speaker() -> Result<IAudioEndpointVolume, String> {
    unsafe {
        let devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|_| "오디오 장치를 열지 못했습니다.")?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|_| "연결된 스피커가 없습니다.")?;
        device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).map_err(|_| "스피커를 제어하지 못했습니다.".into())
    }
}

pub fn speaker_muted() -> Result<bool, String> {
    unsafe { speaker()?.GetMute().map(|b| b.as_bool()).map_err(|_| "스피커 상태를 읽지 못했습니다.".into()) }
}

pub fn set_speaker_muted(mute: bool) -> Result<(), String> {
    unsafe { speaker()?.SetMute(mute, std::ptr::null()).map_err(|_| "스피커 음소거를 바꾸지 못했습니다.".into()) }
}

/// 0.0..=1.0
pub fn speaker_volume() -> Result<f32, String> {
    unsafe { speaker()?.GetMasterVolumeLevelScalar().map_err(|_| "음량을 읽지 못했습니다.".into()) }
}

pub fn set_speaker_volume(level: f32) -> Result<(), String> {
    unsafe {
        speaker()?
            .SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null())
            .map_err(|_| "음량을 바꾸지 못했습니다.".into())
    }
}

// ---------- dark / light theme ----------

/// (AppsUseLightTheme, SystemUsesLightTheme); 1 = light.
pub fn theme() -> Result<(u32, u32), String> {
    let key = Key::open_read(PERSONALIZE)?;
    Ok((key.get_dword("AppsUseLightTheme").unwrap_or(1), key.get_dword("SystemUsesLightTheme").unwrap_or(1)))
}

pub fn set_theme(apps_light: u32, system_light: u32) -> Result<(), String> {
    let key = Key::open_write(PERSONALIZE)?;
    key.set_dword("AppsUseLightTheme", apps_light)?;
    key.set_dword("SystemUsesLightTheme", system_light)?;
    let name = wide("ImmersiveColorSet");
    let mut result = 0usize;
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(name.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            300,
            Some(&mut result),
        );
    }
    Ok(())
}

// ---------- power, clipboard ----------

pub fn monitor_off() {
    // 2 = power off. Any input turns the screens back on.
    unsafe {
        let _ = PostMessageW(Some(HWND_BROADCAST), WM_SYSCOMMAND, WPARAM(SC_MONITORPOWER as usize), LPARAM(2));
    }
}

pub fn lock() -> Result<(), String> {
    unsafe { LockWorkStation().map_err(|_| "PC를 잠그지 못했습니다.".into()) }
}

pub fn sleep() -> Result<(), String> {
    let ok = unsafe { SetSuspendState(false, false, false) };
    if ok { Ok(()) } else { Err("절전 모드로 전환하지 못했습니다.".into()) }
}

/// Empties the clipboard and the Win+V clipboard history. Cannot be undone.
pub fn clear_clipboard() -> Result<(), String> {
    let mut opened = false;
    for _ in 0..5 {
        // Another app may hold the clipboard for a moment.
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !opened {
        return Err("다른 프로그램이 클립보드를 쓰고 있어 비우지 못했습니다.".into());
    }
    unsafe {
        let _ = EmptyClipboard();
        let _ = CloseClipboard();
    }
    let _ = windows::ApplicationModel::DataTransfer::Clipboard::ClearHistory();
    Ok(())
}

// ---------- keyboard input ----------

fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let extended = matches!(
        vk,
        VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END | VK_PRIOR | VK_NEXT | VK_INSERT | VK_DELETE
            | VK_RCONTROL | VK_RMENU | VK_LWIN | VK_RWIN | VK_SNAPSHOT | VK_DIVIDE | VK_NUMLOCK
    );
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn send(inputs: &[INPUT]) {
    if !inputs.is_empty() {
        unsafe {
            SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

/// Releases modifier keys the user is still holding from the shortcut that started the preset,
/// so they don't mix with the keys we send.
pub fn release_held_modifiers() {
    let mut inputs = Vec::new();
    let win_down = [VK_LWIN, VK_RWIN].iter().any(|k| unsafe { GetAsyncKeyState(k.0 as i32) } < 0);
    if win_down {
        // A key in between keeps the Start menu from opening when Win is released.
        inputs.push(key_input(VIRTUAL_KEY(0xFF), false));
        inputs.push(key_input(VIRTUAL_KEY(0xFF), true));
    }
    for k in [VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LSHIFT, VK_RSHIFT, VK_LWIN, VK_RWIN] {
        if unsafe { GetAsyncKeyState(k.0 as i32) } < 0 {
            inputs.push(key_input(k, true));
        }
    }
    send(&inputs);
}

fn code_to_vk(code: &str) -> Option<VIRTUAL_KEY> {
    let vk = match code {
        "Control" | "Ctrl" => VK_CONTROL,
        "Alt" => VK_MENU,
        "Shift" => VK_SHIFT,
        "Super" | "Meta" | "Win" => VK_LWIN,
        "ArrowLeft" => VK_LEFT,
        "ArrowRight" => VK_RIGHT,
        "ArrowUp" => VK_UP,
        "ArrowDown" => VK_DOWN,
        "Enter" => VK_RETURN,
        "Escape" => VK_ESCAPE,
        "Tab" => VK_TAB,
        "Space" => VK_SPACE,
        "Backspace" => VK_BACK,
        "Delete" => VK_DELETE,
        "Insert" => VK_INSERT,
        "Home" => VK_HOME,
        "End" => VK_END,
        "PageUp" => VK_PRIOR,
        "PageDown" => VK_NEXT,
        "PrintScreen" => VK_SNAPSHOT,
        "CapsLock" => VK_CAPITAL,
        "Backquote" => VK_OEM_3,
        "Minus" => VK_OEM_MINUS,
        "Equal" => VK_OEM_PLUS,
        "BracketLeft" => VK_OEM_4,
        "BracketRight" => VK_OEM_6,
        "Backslash" => VK_OEM_5,
        "Semicolon" => VK_OEM_1,
        "Quote" => VK_OEM_7,
        "Comma" => VK_OEM_COMMA,
        "Period" => VK_OEM_PERIOD,
        "Slash" => VK_OEM_2,
        c if c.starts_with("Key") && c.len() == 4 && c.as_bytes()[3].is_ascii_alphabetic() => VIRTUAL_KEY(c.as_bytes()[3].to_ascii_uppercase() as u16),
        c if c.starts_with("Digit") && c.len() == 6 && c.as_bytes()[5].is_ascii_digit() => VIRTUAL_KEY(c.as_bytes()[5] as u16),
        c if c.starts_with('F') && c[1..].parse::<u16>().is_ok_and(|n| (1..=24).contains(&n)) => {
            VIRTUAL_KEY(VK_F1.0 + c[1..].parse::<u16>().unwrap() - 1)
        }
        _ => return None,
    };
    Some(vk)
}

/// Presses a combination like "Super+Control+ArrowRight": modifiers down, key down/up, modifiers up.
pub fn send_keys(spec: &str) -> Result<(), String> {
    check_foreground_accepts_input()?;
    let keys: Vec<VIRTUAL_KEY> = spec
        .split('+')
        .map(|p| code_to_vk(p.trim()).ok_or_else(|| format!("보낼 수 없는 키입니다: {p}")))
        .collect::<Result<_, _>>()?;
    if keys.is_empty() {
        return Err("보낼 키가 없습니다.".into());
    }
    release_held_modifiers();
    let mut inputs: Vec<INPUT> = keys.iter().map(|k| key_input(*k, false)).collect();
    inputs.extend(keys.iter().rev().map(|k| key_input(*k, true)));
    send(&inputs);
    Ok(())
}

/// Types text as Unicode characters (works with any keyboard layout, including Korean).
pub fn type_text(text: &str) -> Result<(), String> {
    check_foreground_accepts_input()?;
    release_held_modifiers();
    let mut inputs = Vec::new();
    for unit in text.encode_utf16() {
        for up in [false, true] {
            let mut flags = KEYEVENTF_UNICODE;
            if up {
                flags |= KEYEVENTF_KEYUP;
            }
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
            });
        }
    }
    send(&inputs);
    Ok(())
}

/// Windows silently drops keys sent to a program running with more rights than Keyscene
/// ("Run as administrator"); say so instead of looking as if it worked.
fn check_foreground_accepts_input() -> Result<(), String> {
    if foreground_is_elevated_above_us() {
        Err("관리자 권한으로 실행 중인 창에는 키를 보낼 수 없습니다.".into())
    } else {
        Ok(())
    }
}

fn foreground_is_elevated_above_us() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        if is_elevated(GetCurrentProcess()) == Some(true) {
            return false;
        }
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return false;
        }
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        // An elevated program's token can't be read from a normal one: that refusal itself
        // means it runs with more rights.
        let elevated = is_elevated(process).unwrap_or(true);
        let _ = CloseHandle(process);
        elevated
    }
}

/// None when the process's rights can't be read.
fn is_elevated(process: windows::Win32::Foundation::HANDLE) -> Option<bool> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::OpenProcessToken;
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut info = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let read = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut info as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        read.ok()?;
        Some(info.TokenIsElevated != 0)
    }
}

pub fn media_key(vk: VIRTUAL_KEY) {
    send(&[key_input(vk, false), key_input(vk, true)]);
}

pub fn screenshot() {
    let _ = send_keys("Super+Shift+KeyS");
}

// ---------- windows ----------

/// A window we may act on: (hwnd, owning process id). The pid check stops us acting on a
/// different window that later reuses the same handle.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct WinRef {
    pub hwnd: isize,
    pub pid: u32,
}

fn hwnd(h: isize) -> HWND {
    HWND(h as *mut core::ffi::c_void)
}

pub fn win_ref(h: isize) -> Option<WinRef> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd(h), Some(&mut pid)) };
    (pid != 0).then_some(WinRef { hwnd: h, pid })
}

/// True when `w` still exists and belongs to the same process.
pub fn still_valid(w: &WinRef) -> bool {
    let exists = unsafe { IsWindow(Some(hwnd(w.hwnd))).as_bool() };
    exists && win_ref(w.hwnd).is_some_and(|r| r.pid == w.pid)
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 64];
    let len = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// Ordinary app windows: visible, not minimized, unowned, titled, not tool or shell windows.
fn is_app_window(h: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
            return false;
        }
        if GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
            return false;
        }
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        if GetWindowTextLengthW(h) == 0 {
            return false;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok() && cloaked != 0 {
            return false;
        }
        !matches!(
            class_name(h).as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Windows.UI.Core.CoreWindow"
        )
    }
}

/// Minimizes every app window except Keyscene's own. Returns them top-most first.
pub fn show_desktop() -> Vec<WinRef> {
    let own_pid = std::process::id();
    let mut found: Vec<WinRef> = Vec::new();
    unsafe extern "system" fn collect(h: HWND, lp: LPARAM) -> windows::core::BOOL {
        let list = &mut *(lp.0 as *mut Vec<isize>);
        if is_app_window(h) {
            list.push(h.0 as isize);
        }
        true.into()
    }
    let mut handles: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut Vec<isize> as isize));
    }
    for h in handles {
        if let Some(w) = win_ref(h).filter(|w| w.pid != own_pid) {
            // Async: a hung app must not stall the preset.
            unsafe {
                let _ = ShowWindowAsync(hwnd(h), SW_MINIMIZE);
            }
            found.push(w);
        }
    }
    found
}

/// Brings back windows that are still minimized, bottom-most first so the old top window ends on top.
pub fn restore_minimized(list: &[WinRef]) {
    for w in list.iter().rev() {
        if still_valid(w) && unsafe { IsIconic(hwnd(w.hwnd)).as_bool() } {
            unsafe {
                let _ = ShowWindowAsync(hwnd(w.hwnd), SW_RESTORE);
            }
        }
    }
}

pub fn minimize(w: &WinRef) -> bool {
    if !still_valid(w) || unsafe { IsIconic(hwnd(w.hwnd)).as_bool() } {
        return false;
    }
    unsafe { ShowWindowAsync(hwnd(w.hwnd), SW_MINIMIZE).as_bool() }
}

pub fn maximize(w: &WinRef) -> bool {
    if !still_valid(w) || unsafe { IsZoomed(hwnd(w.hwnd)).as_bool() } {
        return false;
    }
    unsafe { ShowWindowAsync(hwnd(w.hwnd), SW_MAXIMIZE).as_bool() }
}

/// Undo of maximize: only if it is still maximized.
pub fn unmaximize(w: &WinRef) {
    if still_valid(w) && unsafe { IsZoomed(hwnd(w.hwnd)).as_bool() } {
        unsafe {
            let _ = ShowWindowAsync(hwnd(w.hwnd), SW_RESTORE);
        }
    }
}

fn is_topmost(w: &WinRef) -> bool {
    unsafe { GetWindowLongW(hwnd(w.hwnd), GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0 }
}

pub fn set_topmost(w: &WinRef) -> bool {
    if !still_valid(w) || is_topmost(w) {
        return false;
    }
    unsafe {
        SetWindowPos(hwnd(w.hwnd), Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).is_ok()
    }
}

pub fn clear_topmost(w: &WinRef) {
    if still_valid(w) && is_topmost(w) {
        unsafe {
            let _ = SetWindowPos(hwnd(w.hwnd), Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// The window in front, unless it is the desktop, the taskbar or Keyscene itself.
pub fn foreground_app_window() -> Option<WinRef> {
    let h = unsafe { GetForegroundWindow() };
    if h.is_invalid() || !is_app_window(h) {
        return None;
    }
    win_ref(h.0 as isize).filter(|w| w.pid != std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_codes_map() {
        assert_eq!(code_to_vk("KeyH"), Some(VIRTUAL_KEY(b'H' as u16)));
        assert_eq!(code_to_vk("Digit7"), Some(VIRTUAL_KEY(b'7' as u16)));
        assert_eq!(code_to_vk("F12"), Some(VK_F12));
        assert_eq!(code_to_vk("ArrowRight"), Some(VK_RIGHT));
        assert_eq!(code_to_vk("Super"), Some(VK_LWIN));
        assert_eq!(code_to_vk("F25"), None);
        assert_eq!(code_to_vk("Nope"), None);
    }
}

/// Lets Windows take back memory pages Keyscene is not using right now (e.g. after the settings window
/// closed). Pages that are needed again are simply read back in; nothing is lost.
pub fn trim_memory() {
    use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}
