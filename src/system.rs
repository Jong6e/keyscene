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
        "NumLock" => VK_NUMLOCK,
        "ScrollLock" => VK_SCROLL,
        "Pause" => VK_PAUSE,
        "ContextMenu" => VK_APPS,
        // 한/영, 한자
        "Lang1" => VK_HANGUL,
        "Lang2" => VK_HANJA,
        "NumpadAdd" => VK_ADD,
        "NumpadSubtract" => VK_SUBTRACT,
        "NumpadMultiply" => VK_MULTIPLY,
        "NumpadDivide" => VK_DIVIDE,
        "NumpadDecimal" => VK_DECIMAL,
        "NumpadEnter" => VK_RETURN,
        "AudioVolumeUp" => VK_VOLUME_UP,
        "AudioVolumeDown" => VK_VOLUME_DOWN,
        "AudioVolumeMute" => VK_VOLUME_MUTE,
        "MediaPlayPause" => VK_MEDIA_PLAY_PAUSE,
        "MediaTrackNext" => VK_MEDIA_NEXT_TRACK,
        "MediaTrackPrevious" => VK_MEDIA_PREV_TRACK,
        "MediaStop" => VK_MEDIA_STOP,
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
        c if c.starts_with("Numpad") && c.len() == 7 && c.as_bytes()[6].is_ascii_digit() => {
            VIRTUAL_KEY(VK_NUMPAD0.0 + (c.as_bytes()[6] - b'0') as u16)
        }
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
/// A line break is typed as the Enter key and a tab as the Tab key: as characters, many programs
/// (browsers, chat apps) drop them.
pub fn type_text(text: &str) -> Result<(), String> {
    check_foreground_accepts_input()?;
    release_held_modifiers();
    send(&text_inputs(text));
    Ok(())
}

fn text_inputs(text: &str) -> Vec<INPUT> {
    let mut inputs = Vec::new();
    // "\r\n" and a lone "\r" count as one line break.
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    for ch in text.chars() {
        let key = match ch {
            '\n' => Some(VK_RETURN),
            '\t' => Some(VK_TAB),
            _ => None,
        };
        if let Some(vk) = key {
            inputs.push(key_input(vk, false));
            inputs.push(key_input(vk, true));
            continue;
        }
        let mut units = [0u16; 2];
        for unit in ch.encode_utf16(&mut units) {
            for up in [false, true] {
                let mut flags = KEYEVENTF_UNICODE;
                if up {
                    flags |= KEYEVENTF_KEYUP;
                }
                inputs.push(INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: *unit, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
                });
            }
        }
    }
    inputs
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
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)) };
    elevated_above_us(pid)
}

/// True when the process runs with more rights than Keyscene ("Run as administrator").
/// Windows then ignores Keyscene's keys and window commands for it.
fn elevated_above_us(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        if pid == 0 || is_elevated(GetCurrentProcess()) == Some(true) {
            return false;
        }
        // A program Windows won't even let us look at runs with more rights (another user's
        // elevated program, a protected process); so does one whose rights can't be read.
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return true };
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

/// Turns the sound of whole programs off or on, like the volume mixer, on the default speaker.
/// `exes` are lowercase program file names. Returns the ones whose sound actually changed.
pub fn set_apps_muted(exes: &std::collections::BTreeSet<String>, mute: bool) -> Vec<String> {
    set_apps_muted_found(exes, mute).0
}

/// Like `set_apps_muted`, and also returns the programs that had a sound session at all (a program
/// that isn't running can't be changed now).
pub fn set_apps_muted_found(exes: &std::collections::BTreeSet<String>, mute: bool) -> (Vec<String>, std::collections::BTreeSet<String>) {
    let mut found = std::collections::BTreeSet::new();
    let changed = apps_muted(exes, mute, &mut found);
    (changed, found)
}

fn apps_muted(exes: &std::collections::BTreeSet<String>, mute: bool, found: &mut std::collections::BTreeSet<String>) -> Vec<String> {
    use windows::core::Interface;
    use windows::Win32::Media::Audio::{IAudioSessionControl2, IAudioSessionManager2, ISimpleAudioVolume};
    let mut changed: Vec<String> = Vec::new();
    if exes.is_empty() {
        return changed;
    }
    unsafe {
        let Ok(devices) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return changed };
        let Ok(device) = devices.GetDefaultAudioEndpoint(eRender, eConsole) else { return changed };
        let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { return changed };
        let Ok(sessions) = manager.GetSessionEnumerator() else { return changed };
        for i in 0..sessions.GetCount().unwrap_or(0) {
            let Ok(session) = sessions.GetSession(i) else { continue };
            let Ok(control) = session.cast::<IAudioSessionControl2>() else { continue };
            let Ok(pid) = control.GetProcessId() else { continue };
            // A browser plays sound from a helper process with the same program file.
            let Some(exe) = crate::launch::process_exe(pid).filter(|e| exes.contains(e)) else { continue };
            found.insert(exe.clone());
            let Ok(volume) = session.cast::<ISimpleAudioVolume>() else { continue };
            let now = volume.GetMute().map(|b| b.as_bool()).unwrap_or(mute);
            if now != mute && volume.SetMute(mute, std::ptr::null()).is_ok() && !changed.contains(&exe) {
                changed.push(exe);
            }
        }
    }
    changed
}

// ---------- media sessions (play/pause per app) ----------

/// The media sessions Windows lists (what the play/pause key controls), with the app id of each.
fn media_sessions() -> Vec<(String, windows::Media::Control::GlobalSystemMediaTransportControlsSession)> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager;
    let Ok(manager) = Manager::RequestAsync().and_then(|op| op.join()) else { return Vec::new() };
    let Ok(sessions) = manager.GetSessions() else { return Vec::new() };
    let count = sessions.Size().unwrap_or(0);
    (0..count)
        .filter_map(|i| sessions.GetAt(i).ok())
        .filter_map(|s| s.SourceAppUserModelId().ok().map(|id| (id.to_string().to_lowercase(), s)))
        .collect()
}

/// A window as media sessions see it: the names its app may use (see `launch::media_window`)
/// and its title, which for a browser holds the page (and so the video) title.
pub struct MediaWindow {
    pub ids: Vec<String>,
    pub title: String,
}

impl MediaWindow {
    fn is_app(&self, id: &str) -> bool {
        let stem = id.trim_end_matches(".exe");
        self.ids.iter().any(|x| x == id || x == stem)
    }

    fn shows(&self, media_title: &str) -> bool {
        !media_title.is_empty() && self.title.to_lowercase().contains(&media_title.to_lowercase())
    }
}

/// A session Keyscene paused: its app id and the title of what was playing.
pub type PausedMedia = (String, String);

fn media_title(session: &windows::Media::Control::GlobalSystemMediaTransportControlsSession) -> String {
    session.TryGetMediaPropertiesAsync().and_then(|op| op.join()).and_then(|p| p.Title()).map(|t| t.to_string()).unwrap_or_default()
}

/// False for media with no known length, which is how live streams report themselves: pausing a
/// live stream only makes it fall behind, so it keeps playing (its sound can still be turned off).
fn has_length(session: &windows::Media::Control::GlobalSystemMediaTransportControlsSession) -> bool {
    session
        .GetTimelineProperties()
        .and_then(|t| Ok(t.EndTime()?.Duration - t.StartTime()?.Duration))
        .is_ok_and(|length| length > 0)
}

/// Whether a playing session belongs to the windows being put away rather than the ones shown.
/// A browser has one app for all its windows, so there the window whose title holds the media's
/// title decides; other apps are decided by the app alone.
fn belongs_to_hidden(id: &str, title: &str, hidden: &[MediaWindow], shown: &[MediaWindow]) -> bool {
    if shown.iter().any(|w| w.is_app(id) && w.shows(title)) {
        return false;
    }
    hidden.iter().any(|w| w.is_app(id) && w.shows(title)) || (hidden.iter().any(|w| w.is_app(id)) && !shown.iter().any(|w| w.is_app(id)))
}

/// Pauses the media playing in the windows being put away (live streams excluded, see `has_length`).
/// Returns what it paused, to play it again later.
pub fn pause_media(hidden: &[MediaWindow], shown: &[MediaWindow]) -> Vec<PausedMedia> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status;
    let mut paused = Vec::new();
    if hidden.is_empty() {
        return paused;
    }
    for (id, session) in media_sessions() {
        let playing = session.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).is_ok_and(|s| s == Status::Playing);
        if !playing || !hidden.iter().any(|w| w.is_app(&id)) || !has_length(&session) {
            continue;
        }
        let title = media_title(&session);
        if belongs_to_hidden(&id, &title, hidden, shown) && session.TryPauseAsync().and_then(|op| op.join()).unwrap_or(false) {
            paused.push((id, title));
        }
    }
    paused
}

/// Plays again what `pause_media` paused, if it is still paused and still the same media.
pub fn resume_media(items: &[PausedMedia]) {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status;
    if items.is_empty() {
        return;
    }
    for (id, session) in media_sessions() {
        let paused = session.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).is_ok_and(|s| s == Status::Paused);
        if !paused || !items.iter().any(|(i, _)| *i == id) {
            continue;
        }
        let title = media_title(&session);
        if items.iter().any(|(i, t)| *i == id && (t.is_empty() || *t == title)) {
            let _ = session.TryPlayAsync().and_then(|op| op.join());
        }
    }
}

/// Whether a paused item should play again now that `shown` are up and `hidden` are put away.
pub fn media_comes_back(item: &PausedMedia, shown: &[MediaWindow], hidden: &[MediaWindow]) -> bool {
    let (id, title) = item;
    if shown.iter().any(|w| w.is_app(id) && w.shows(title)) {
        return true;
    }
    // A browser has one app for all its windows: only the window showing the media brings it back,
    // or a video in a window that is still minimized would start playing.
    if is_browser(id) {
        return false;
    }
    shown.iter().any(|w| w.is_app(id)) && (title.is_empty() || !hidden.iter().any(|w| w.is_app(id)))
}

fn is_browser(id: &str) -> bool {
    let stem = id.trim_end_matches(".exe");
    ["chrome", "msedge", "firefox", "whale", "brave", "opera", "vivaldi"].iter().any(|b| stem == *b || stem.starts_with(&format!("{b}.")))
}

#[cfg(test)]
mod media_tests {
    use super::*;

    fn w(ids: &[&str], title: &str) -> MediaWindow {
        MediaWindow { ids: ids.iter().map(|s| s.to_string()).collect(), title: title.into() }
    }

    #[test]
    fn browser_media_follows_the_window_title() {
        let video = w(&["chrome"], "재밌는 영상 - YouTube - Google Chrome");
        let docs = w(&["chrome"], "GitHub - Google Chrome");
        // The YouTube window goes away while another Chrome window comes up: pause it.
        assert!(belongs_to_hidden("chrome", "재밌는 영상", &[video], std::slice::from_ref(&docs)));
        // The YouTube window comes up: leave it playing.
        let video = w(&["chrome"], "재밌는 영상 - YouTube - Google Chrome");
        assert!(!belongs_to_hidden("chrome", "재밌는 영상", &[docs], &[video]));
        // Spotify's window title does not name the song; the app alone decides.
        assert!(belongs_to_hidden("spotify.exe", "노래", &[w(&["spotify"], "Spotify Premium")], &[w(&["code"], "x")]));
    }

    #[test]
    fn browser_media_comes_back_only_with_its_window() {
        let gmail = w(&["chrome"], "받은편지함 - Gmail - Google Chrome");
        let video = w(&["chrome"], "재밌는 영상 - YouTube - Google Chrome");
        let item = ("chrome".to_string(), "재밌는 영상".to_string());
        // Another Chrome window comes up while the video's window stays minimized: keep it paused.
        assert!(!media_comes_back(&item, std::slice::from_ref(&gmail), &[]));
        assert!(media_comes_back(&item, &[video], &[gmail]));
        // Other apps: the app coming back is enough.
        assert!(media_comes_back(&("spotify.exe".into(), "노래".into()), &[w(&["spotify"], "Spotify")], &[]));
    }
}

// ---------- windows ----------

/// A window we may act on: (hwnd, owning process id). The pid check stops us acting on a
/// different window that later reuses the same handle.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
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
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        // Owned windows are dialogs, unless the owner is a hidden helper window (some apps' main
        // windows work that way) or the window asks for a taskbar button. Same rule as Alt+Tab.
        if let Ok(owner) = GetWindow(h, GW_OWNER) {
            if !owner.is_invalid() && IsWindowVisible(owner).as_bool() && ex & WS_EX_APPWINDOW.0 == 0 {
                return false;
            }
        }
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        // Click-through overlays (game and GPU overlays, some pop-ups), as Alt+Tab leaves them out.
        if ex & WS_EX_NOACTIVATE.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0 {
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

/// App windows that are up (not minimized), except Keyscene's own, top-most first.
pub fn visible_app_windows() -> Vec<WinRef> {
    let own_pid = std::process::id();
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
    handles.into_iter().filter_map(win_ref).filter(|w| w.pid != own_pid).collect()
}

/// What 바탕 화면 보기 did.
pub struct Desktop {
    /// The windows it minimized, top-most first.
    pub minimized: Vec<WinRef>,
    /// Every app window that was up, top-most first, to put back in this order.
    pub stack: Vec<WinRef>,
    /// Processes whose windows it could not touch because they run as administrator.
    pub elevated: Vec<u32>,
}

/// Minimizes every app window except Keyscene's own and those in `keep` (windows a later step
/// brings forward anyway, so they don't go down and come straight back up). Frozen windows are left
/// alone: a minimize queued for them would land long after the preset is undone.
pub fn show_desktop(keep: &std::collections::HashSet<isize>) -> Desktop {
    let mut out = Desktop { minimized: Vec::new(), stack: Vec::new(), elevated: Vec::new() };
    for w in visible_app_windows() {
        if elevated_above_us(w.pid) {
            if !out.elevated.contains(&w.pid) {
                out.elevated.push(w.pid);
            }
            continue;
        }
        if !answers(w.hwnd) {
            continue;
        }
        out.stack.push(w);
        // Async: a slow app must not stall the preset.
        if !keep.contains(&w.hwnd) && unsafe { ShowWindowAsync(hwnd(w.hwnd), SW_MINIMIZE).as_bool() } {
            out.minimized.push(w);
        }
    }
    out
}

/// Waits (up to about a second) until the windows 바탕 화면 보기 minimized really are minimized, and
/// asks once more, the way the title bar's button does, those that ignored the first request.
/// Skips windows a later step of the same preset brought up (`claimed`) and the active window.
/// Called at the end of the preset, so the other steps don't wait for it and a quick second press
/// can't start undoing while windows are still going down.
pub fn settle_minimized(list: &[WinRef], claimed: &std::collections::HashSet<isize>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1000);
    let retry_at = std::time::Instant::now() + std::time::Duration::from_millis(400);
    let mut retried = false;
    loop {
        let fg = unsafe { GetForegroundWindow() }.0 as isize;
        let pending: Vec<&WinRef> = list
            .iter()
            .filter(|w| !claimed.contains(&w.hwnd) && w.hwnd != fg && still_valid(w))
            .filter(|w| unsafe { IsWindowVisible(hwnd(w.hwnd)).as_bool() && !IsIconic(hwnd(w.hwnd)).as_bool() })
            .collect();
        let now = std::time::Instant::now();
        if pending.is_empty() || now >= deadline {
            return;
        }
        if !retried && now >= retry_at {
            retried = true;
            for w in pending {
                unsafe {
                    let _ = PostMessageW(Some(hwnd(w.hwnd)), WM_SYSCOMMAND, WPARAM(SC_MINIMIZE as usize), LPARAM(0));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}

/// False when Windows considers the window's app frozen ("응답 없음"): calls that wait for it, such as
/// moving or reordering its window, could block. Instant to ask, unlike pinging the window.
pub fn answers(h: isize) -> bool {
    unsafe { IsWindow(Some(hwnd(h))).as_bool() && !IsHungAppWindow(hwnd(h)).as_bool() }
}

/// Brings back windows that are still minimized, skipping `skip` and frozen windows.
/// Async, so the order they come up in is not reliable; `restack` fixes that afterwards.
/// Returns the windows it asked to come back.
pub fn restore_minimized(list: &[WinRef], skip: &std::collections::HashSet<isize>) -> Vec<WinRef> {
    let mut restored = Vec::new();
    for w in list.iter().rev() {
        if skip.contains(&w.hwnd) || !still_valid(w) || unsafe { !IsIconic(hwnd(w.hwnd)).as_bool() } || !answers(w.hwnd) {
            continue;
        }
        unsafe {
            let _ = ShowWindowAsync(hwnd(w.hwnd), SW_RESTORE);
        }
        restored.push(*w);
    }
    restored.reverse();
    restored
}

/// The window at the top of `h`'s owner chain (itself when it has no owner).
pub fn root_owner(h: isize) -> isize {
    let r = unsafe { GetAncestor(hwnd(h), GA_ROOTOWNER) };
    if r.is_invalid() { h } else { r.0 as isize }
}

pub fn win_is_up(w: &WinRef) -> bool {
    is_up(w)
}

fn is_up(w: &WinRef) -> bool {
    still_valid(w) && unsafe { IsWindowVisible(hwnd(w.hwnd)).as_bool() && !IsIconic(hwnd(w.hwnd)).as_bool() }
}

fn is_cloaked(h: isize) -> bool {
    let mut cloaked = 0u32;
    unsafe { DwmGetWindowAttribute(hwnd(h), DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok() && cloaked != 0 }
}

pub fn is_topmost_window(h: isize) -> bool {
    unsafe { (GetWindowLongW(hwnd(h), GWL_EXSTYLE) as u32) & WS_EX_TOPMOST.0 != 0 }
}

/// Puts `moved` (windows the undo brought back or put back) in their old stacking order, given by
/// `order` (top-most first; may hold other windows too). Each app handles its restore in its own
/// time, so without this a slow app (often a browser) could end up on top of windows that were above
/// it. Waits up to 3 s for windows still coming up; any still not up then are fitted in later from a
/// background thread. Frozen, closed and still-minimized windows are left alone.
pub fn restack(order: &[WinRef], moved: &[WinRef]) {
    let moved: std::collections::HashSet<WinRef> = moved.iter().copied().collect();
    let mut seen = std::collections::HashSet::new();
    let list: Vec<WinRef> = order.iter().copied().filter(|w| moved.contains(w) && seen.insert(*w)).collect();
    if list.is_empty() {
        return;
    }
    let coming_up = |w: &WinRef| still_valid(w) && unsafe { IsIconic(hwnd(w.hwnd)).as_bool() };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while list.iter().any(coming_up) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Bottom-most first, each to the top of its band (an always-on-top window stays always on top).
    for w in list.iter().rev().filter(|w| is_up(w) && answers(w.hwnd)) {
        unsafe {
            let _ = SetWindowPos(hwnd(w.hwnd), Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
    let late: Vec<WinRef> = list.iter().copied().filter(coming_up).collect();
    if late.is_empty() {
        return;
    }
    // Very slow windows: slot each one in under the window that was above it, once it is up.
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut waiting = late;
        while !waiting.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(100));
            waiting.retain(|w| {
                if !still_valid(w) {
                    return false;
                }
                if !is_up(w) {
                    return true;
                }
                let at = list.iter().position(|x| x == w).unwrap_or(0);
                // Never under an always-on-top window: that would make this one always on top too.
                if let Some(above) = list[..at].iter().rev().find(|x| is_up(x) && !is_topmost_window(x.hwnd)) {
                    unsafe {
                        let _ = SetWindowPos(hwnd(w.hwnd), Some(hwnd(above.hwnd)), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                    }
                }
                false
            });
        }
    });
}

/// The window to make active after an undo: the one that was active when the preset ran, if it is up
/// again, otherwise the top-most window brought back that isn't always on top. None for windows on
/// another virtual desktop (activating one would switch desktops).
pub fn pick_focus(front: Option<WinRef>, order: &[WinRef], moved: &[WinRef]) -> Option<WinRef> {
    let ok = |w: &WinRef| is_up(w) && !is_cloaked(w.hwnd);
    if let Some(f) = front.filter(|f| moved.contains(f) && ok(f)) {
        return Some(f);
    }
    order.iter().chain(moved).find(|w| moved.contains(w) && ok(w) && !is_topmost_window(w.hwnd)).copied()
}

/// True when the user is not busy in another window: the desktop, the taskbar, Keyscene or one of `ours` is in front.
pub fn foreground_is_free(ours: &[WinRef]) -> bool {
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_invalid() {
        return true;
    }
    let h = fg.0 as isize;
    if ours.iter().any(|w| w.hwnd == h) {
        return true;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    pid == std::process::id() || matches!(class_name(fg).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
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
        // The number pad and other keys the recorder in the window can hand over.
        assert_eq!(code_to_vk("Numpad0"), Some(VK_NUMPAD0));
        assert_eq!(code_to_vk("Numpad9"), Some(VK_NUMPAD9));
        assert_eq!(code_to_vk("NumpadAdd"), Some(VK_ADD));
        assert_eq!(code_to_vk("NumpadX"), None);
        assert_eq!(code_to_vk("Lang1"), Some(VK_HANGUL));
        assert_eq!(code_to_vk("AudioVolumeUp"), Some(VK_VOLUME_UP));
    }

    #[test]
    fn line_breaks_and_tabs_are_typed_as_keys() {
        let keys = |text: &str| -> Vec<(u16, u16, bool)> {
            text_inputs(text)
                .iter()
                .map(|i| unsafe { (i.Anonymous.ki.wVk.0, i.Anonymous.ki.wScan, i.Anonymous.ki.dwFlags.contains(KEYEVENTF_KEYUP)) })
                .collect()
        };
        let (enter, tab) = (VK_RETURN.0, VK_TAB.0);
        assert_eq!(keys("a\n"), [(0, 'a' as u16, false), (0, 'a' as u16, true), (enter, 0, false), (enter, 0, true)]);
        // Windows line endings are one Enter, not two.
        assert_eq!(keys("\r\n\t"), [(enter, 0, false), (enter, 0, true), (tab, 0, false), (tab, 0, true)]);
        // Characters outside the basic plane take two UTF-16 units, each pressed and released.
        assert_eq!(keys("가😀").len(), 2 + 4);
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
