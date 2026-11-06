//! Opening apps, files and web pages; finding and placing app windows; listing monitors,
//! installed apps and browser profiles.
//!
//! CONTRACT (used by engine.rs and main.rs). Keep these signatures.

use crate::preset::{AppRef, AppSource, BrowserRef, IfRunning, MonitorRef, OnRestore, WindowState};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::core::{w, Interface, GUID, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HWND, LPARAM, POINT, PROPERTYKEY, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, GetProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{
    IEnumShellItems, IShellItem, IShellItem2, IShellLinkW, SHCreateItemFromParsingName, SHGetKnownFolderPath,
    ShellExecuteExW, ShellExecuteW, ShellLink, BHID_EnumItems, KF_FLAG_DEFAULT, SEE_MASK_FLAG_NO_UI,
    SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DispatchMessageW, EnumWindows, GetAncestor, GetCursorPos, GetForegroundWindow, GetWindow,
    GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, IsZoomed, MsgWaitForMultipleObjects, PeekMessageW, PostMessageW, SetForegroundWindow,
    SetWindowPos, ShowWindow, ShowWindowAsync, TranslateMessage, CHILDID_SELF, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_SHOW,
    EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, GA_ROOT, GWL_EXSTYLE, GW_OWNER, HWND_NOTOPMOST, HWND_TOPMOST,
    MSG, OBJID_WINDOW, PM_REMOVE, QS_ALLINPUT, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOWNORMAL, WINEVENT_OUTOFCONTEXT, WM_CLOSE,
    WS_EX_TOOLWINDOW,
};

#[derive(Clone, Debug, Serialize)]
pub struct InstalledApp {
    /// Display name from the Start menu, e.g. "Claude".
    pub name: String,
    /// shell:AppsFolder parsing name (AUMID for Store apps). Goes into AppRef.id with source Installed.
    pub id: String,
    /// "store" or "desktop"
    pub kind: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct BrowserProfile {
    /// "chrome" | "edge"
    pub browser: String,
    /// Profile folder, e.g. "Default", "Profile 1"
    pub dir: String,
    /// Name the user gave it in the browser, e.g. "개인"
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MonitorInfo {
    /// Stable id (monitor device path); goes into MonitorRef::Specific.id
    pub id: String,
    /// Friendly name, e.g. "DELL U2723QE"
    pub name: String,
    pub primary: bool,
    /// Work area (screen minus taskbar) in physical pixels: left, top, right, bottom
    pub work: [i32; 4],
}

/// The window an OpenApp step ended up acting on, so restore can find it again.
#[derive(Clone, Debug)]
pub struct OpenedWindow {
    pub hwnd: isize,
    pub pid: u32,
    /// false = the app was already running and its window was brought forward
    pub launched: bool,
}

/// How long open_app waits for a new app window.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(20);

// ---------- small helpers ----------

/// Runs `f` on a fresh thread with COM in single-threaded mode (what the shell APIs expect).
fn on_sta<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    std::thread::spawn(move || unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = f();
        if hr.is_ok() {
            CoUninitialize();
        }
        result
    })
    .join()
    .ok()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Takes ownership of a CoTaskMem string.
unsafe fn take_pwstr(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    s
}

fn file_name_lower(path: &str) -> Option<String> {
    Path::new(path.trim().trim_matches('"'))
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
}

fn hwnd_of(v: isize) -> HWND {
    HWND(v as _)
}

// ---------- installed apps ----------

/// Start-menu apps, sorted by name. Read only when called (the app picker opens).
pub fn installed_apps() -> Vec<InstalledApp> {
    on_sta(|| unsafe { enumerate_apps() }).unwrap_or_default()
}

unsafe fn display_name(item: &IShellItem, sigdn: SIGDN) -> Option<String> {
    item.GetDisplayName(sigdn).ok().and_then(|p| take_pwstr(p))
}

unsafe fn enumerate_apps() -> Vec<InstalledApp> {
    let mut out: Vec<InstalledApp> = Vec::new();
    let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(w!("shell:AppsFolder"), None::<&IBindCtx>) else {
        return out;
    };
    let Ok(items) = folder.BindToHandler::<_, IEnumShellItems>(None::<&IBindCtx>, &BHID_EnumItems) else {
        return out;
    };
    let mut seen = HashSet::new();
    loop {
        let mut batch: [Option<IShellItem>; 1] = [None];
        let mut fetched = 0u32;
        if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
            break;
        }
        let Some(item) = batch[0].take() else { break };
        let (Some(name), Some(id)) = (display_name(&item, SIGDN_NORMALDISPLAY), display_name(&item, SIGDN_PARENTRELATIVEPARSING))
        else {
            continue;
        };
        if name.trim().is_empty() || looks_like_uninstaller(&name) || !seen.insert(id.clone()) {
            continue;
        }
        let kind = if id.contains('!') { "store" } else { "desktop" };
        out.push(InstalledApp { name, id, kind: kind.into() });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn looks_like_uninstaller(name: &str) -> bool {
    name.to_lowercase().contains("uninstall") || name.trim_end().ends_with("제거")
}

// ---------- browsers ----------

/// Chrome and Edge profiles found on this PC (names only; no account data).
pub fn browser_profiles() -> Vec<BrowserProfile> {
    let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return Vec::new() };
    let mut out = Vec::new();
    for (browser, rel) in [
        ("chrome", r"Google\Chrome\User Data\Local State"),
        ("edge", r"Microsoft\Edge\User Data\Local State"),
    ] {
        if browser_exe(browser).is_none() {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(local.join(rel)) {
            out.extend(parse_local_state(browser, &text));
        }
    }
    out
}

/// Reads profile.info_cache.{dir}.name from a Chrome/Edge "Local State" file.
fn parse_local_state(browser: &str, text: &str) -> Vec<BrowserProfile> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let Some(cache) = json.pointer("/profile/info_cache").and_then(|v| v.as_object()) else { return Vec::new() };
    let mut out: Vec<BrowserProfile> = cache
        .iter()
        .map(|(dir, v)| BrowserProfile {
            browser: browser.into(),
            dir: dir.clone(),
            name: v.get("name").and_then(|n| n.as_str()).filter(|n| !n.is_empty()).unwrap_or(dir).to_string(),
        })
        .collect();
    // "Default" first, then "Profile 1", "Profile 2", ... in number order.
    out.sort_by_key(|p| {
        let n = p.dir.strip_prefix("Profile ").and_then(|n| n.trim().parse::<u32>().ok());
        (p.dir != "Default", n.unwrap_or(u32::MAX), p.dir.clone())
    });
    out
}

fn reg_default_string(root: HKEY, subkey: &str) -> Option<String> {
    let sub = HSTRING::from(subkey);
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(root, &sub, PCWSTR::null(), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut size))
    };
    (rc == ERROR_SUCCESS).then(|| from_wide(&buf)).filter(|s| !s.is_empty())
}

/// Full path of chrome.exe / msedge.exe, if installed.
fn browser_exe(browser: &str) -> Option<PathBuf> {
    let exe = match browser {
        "chrome" => "chrome.exe",
        "edge" => "msedge.exe",
        _ => return None,
    };
    let key = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        if let Some(p) = reg_default_string(root, &key).map(|s| PathBuf::from(s.trim_matches('"'))) {
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let rel = match browser {
        "chrome" => r"Google\Chrome\Application\chrome.exe",
        _ => r"Microsoft\Edge\Application\msedge.exe",
    };
    [env("ProgramFiles"), env("ProgramFiles(x86)"), env("LOCALAPPDATA")]
        .into_iter()
        .flatten()
        .map(|base| base.join(rel))
        .find(|p| p.is_file())
}

/// Arguments for chrome/edge. The URL is the last argument on its own; nothing goes through cmd.exe
/// (std::process::Command quotes each argument for us).
fn browser_args(browser: &str, profile_dir: &str, new_window: bool, private: bool, url: &str) -> Vec<String> {
    let mut args = Vec::new();
    if !profile_dir.trim().is_empty() {
        args.push(format!("--profile-directory={}", profile_dir.trim()));
    }
    if new_window {
        args.push("--new-window".into());
    }
    if private {
        args.push(if browser == "edge" { "--inprivate" } else { "--incognito" }.into());
    }
    args.push(url.to_string());
    args
}

/// Only plain http(s) addresses; anything else (file:, javascript:, other apps' schemes) is refused.
fn validate_url(url: &str) -> Result<String, String> {
    let t = url.trim();
    let lower = t.to_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"));
    match rest {
        Some(r) if !r.is_empty() && !t.chars().any(|c| c.is_whitespace() || c.is_control()) => Ok(t.to_string()),
        _ => Err("http:// 또는 https://로 시작하는 웹 주소만 열 수 있습니다.".into()),
    }
}

// ---------- monitors ----------

pub fn monitors() -> Vec<MonitorInfo> {
    let names = display_config_names();
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> windows::core::BOOL {
        (*(data.0 as *mut Vec<HMONITOR>)).push(m);
        true.into()
    }
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut handles as *mut _ as isize));
    }
    let mut out: Vec<MonitorInfo> = Vec::new();
    for (i, m) in handles.iter().enumerate() {
        let Some((device, info)) = monitor_info(*m) else { continue };
        let (friendly, path) = names.get(&device).cloned().unwrap_or_default();
        let name = if friendly.trim().is_empty() { format!("모니터 {}", i + 1) } else { friendly };
        let w = info.monitorInfo.rcWork;
        out.push(MonitorInfo {
            id: if path.is_empty() { device } else { path },
            name,
            primary: info.monitorInfo.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
            work: [w.left, w.top, w.right, w.bottom],
        });
    }
    // Two identical models would look the same; number the repeats.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for m in &out {
        *counts.entry(m.name.clone()).or_default() += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for m in &mut out {
        if counts[&m.name] > 1 {
            let n = seen.entry(m.name.clone()).or_default();
            *n += 1;
            m.name = format!("{} ({n})", m.name);
        }
    }
    out.sort_by_key(|m| (!m.primary, m.work[0], m.work[1]));
    out
}

fn monitor_info(m: HMONITOR) -> Option<(String, MONITORINFOEXW)> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(m, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };
    ok.as_bool().then(|| (from_wide(&info.szDevice), info))
}

/// GDI device name (\\.\DISPLAY1) -> (friendly name, monitor device path)
fn display_config_names() -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return map;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None)
            != ERROR_SUCCESS
        {
            return map;
        }
        for path in paths.iter().take(np as usize) {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            target.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            target.header.adapterId = path.targetInfo.adapterId;
            target.header.id = path.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut target.header) != 0 {
                continue;
            }
            map.insert(
                from_wide(&source.viewGdiDeviceName),
                (from_wide(&target.monitorFriendlyDeviceName), from_wide(&target.monitorDevicePath)),
            );
        }
    }
    map
}

fn work_of(m: HMONITOR) -> Option<RECT> {
    monitor_info(m).map(|(_, i)| i.monitorInfo.rcWork)
}

fn primary_work() -> Option<RECT> {
    work_of(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) })
}

/// Work area the MonitorRef points at. None = leave the window's monitor alone.
fn target_work(monitor: &MonitorRef) -> Option<RECT> {
    match monitor {
        MonitorRef::AppDefault => None,
        MonitorRef::Primary => primary_work(),
        MonitorRef::Cursor => {
            let mut pt = POINT::default();
            unsafe {
                if GetCursorPos(&mut pt).is_err() {
                    return primary_work();
                }
                work_of(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST))
            }
        }
        // A monitor that is not connected falls back to the primary one, so a window never lands off-screen.
        MonitorRef::Specific { id, .. } => monitors()
            .into_iter()
            .find(|m| &m.id == id)
            .map(|m| RECT { left: m.work[0], top: m.work[1], right: m.work[2], bottom: m.work[3] })
            .or_else(primary_work),
    }
}

// ---------- open files and web pages ----------

/// Opens a file or folder with its default program.
pub fn open_path(path: &str) -> Result<(), String> {
    let p = path.trim().trim_matches('"');
    if p.is_empty() || !Path::new(p).exists() {
        return Err(format!("파일이나 폴더를 찾을 수 없습니다: {p}"));
    }
    let result = unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(p), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("열지 못했습니다: {p}"))
    }
}

/// Opens an http(s) URL in the chosen browser/profile. Other schemes are refused.
pub fn open_url(url: &str, browser: &BrowserRef, new_window: bool, private: bool) -> Result<(), String> {
    let url = validate_url(url)?;
    match browser.browser.as_str() {
        b @ ("chrome" | "edge") => {
            let label = if b == "chrome" { "Chrome" } else { "Microsoft Edge" };
            let exe = browser_exe(b).ok_or_else(|| format!("{label}이(가) 설치되어 있지 않습니다."))?;
            std::process::Command::new(exe)
                .args(browser_args(b, &browser.profile_dir, new_window, private, &url))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("{label}을(를) 실행하지 못했습니다: {e}"))
        }
        // The default browser gets the plain URL; new-window/private options are browser specific.
        _ => {
            let r = unsafe {
                ShellExecuteW(None, w!("open"), &HSTRING::from(url.as_str()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
            };
            if r.0 as isize > 32 {
                Ok(())
            } else {
                Err("기본 브라우저로 웹 주소를 열지 못했습니다.".into())
            }
        }
    }
}

// ---------- recognising an app's windows ----------

/// Splits "{GUID}\rest" (a known-folder relative path) into ("GUID", "rest").
fn split_known_folder_id(id: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix('{')?;
    let close = rest.find('}')?;
    let guid = &rest[..close];
    let tail = rest[close + 1..].trim_start_matches('\\');
    (guid.len() == 36 && !tail.is_empty()).then(|| (guid.to_string(), tail.to_string()))
}

unsafe fn expand_known_folder(id: &str) -> Option<String> {
    let (guid, tail) = split_known_folder_id(id)?;
    let guid = GUID::try_from(guid.as_str()).ok()?;
    let base = take_pwstr(SHGetKnownFolderPath(&guid, KF_FLAG_DEFAULT, None).ok()?)?;
    Some(format!("{}\\{}", base.trim_end_matches('\\'), tail))
}

/// System.Link.TargetParsingPath of a Start-menu entry (the exe behind a desktop shortcut).
const PKEY_LINK_TARGET_PARSING_PATH: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0xb9b4b3fc_2b51_4a42_b5d8_324146afcf25), pid: 2 };
/// System.AppUserModel.ID
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3), pid: 5 };

unsafe fn apps_folder_target(id: &str) -> Option<String> {
    let item: IShellItem2 =
        SHCreateItemFromParsingName(&HSTRING::from(format!("shell:AppsFolder\\{id}")), None::<&IBindCtx>).ok()?;
    take_pwstr(item.GetString(&PKEY_LINK_TARGET_PARSING_PATH).ok()?)
}

unsafe fn resolve_lnk(path: &str) -> Option<String> {
    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
    let file: IPersistFile = link.cast().ok()?;
    file.Load(&HSTRING::from(path), STGM_READ).ok()?;
    let mut buf = [0u16; 1024];
    link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
    Some(from_wide(&buf)).filter(|s| !s.is_empty())
}

/// What identifies an app's windows: its AUMID, its exe name, and (right after launch) its process id.
struct Matcher {
    aumid: Option<String>,
    exe: Option<String>,
    pid: Option<u32>,
    cache: RefCell<HashMap<u32, (Option<String>, Option<String>)>>,
}

unsafe fn matcher_for(app: &AppRef) -> Matcher {
    let (aumid, exe) = match app.source {
        AppSource::File => {
            let p = app.id.trim().trim_matches('"');
            let target = if p.to_lowercase().ends_with(".lnk") { resolve_lnk(p) } else { None };
            (None, file_name_lower(target.as_deref().unwrap_or(p)).filter(|n| n.ends_with(".exe")))
        }
        AppSource::Installed if app.id.contains('!') => (Some(app.id.clone()), None),
        AppSource::Installed => {
            let path = expand_known_folder(&app.id).or_else(|| apps_folder_target(&app.id));
            let exe = path.as_deref().and_then(file_name_lower).filter(|n| n.ends_with(".exe"));
            // Desktop apps listed by their own AUMID (e.g. "Chrome") tag their windows with it.
            let aumid = (!app.id.contains('\\')).then(|| app.id.clone());
            (aumid, exe)
        }
    };
    Matcher { aumid, exe, pid: None, cache: RefCell::new(HashMap::new()) }
}

unsafe fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    pid
}

/// (process AUMID if packaged, exe file name lowercase)
unsafe fn process_identity(pid: u32) -> (Option<String>, Option<String>) {
    let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return (None, None) };
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    let aumid = (GetApplicationUserModelId(h, &mut len, Some(PWSTR(buf.as_mut_ptr()))) == ERROR_SUCCESS)
        .then(|| from_wide(&buf));
    let mut path = [0u16; 1024];
    let mut size = path.len() as u32;
    let exe = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut size)
        .ok()
        .and_then(|_| file_name_lower(&from_wide(&path)));
    let _ = CloseHandle(h);
    (aumid, exe)
}

unsafe fn window_aumid(hwnd: HWND) -> Option<String> {
    let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
    let value = store.GetValue(&PKEY_APP_USER_MODEL_ID).ok()?;
    take_pwstr(PropVariantToStringAlloc(&value).ok()?).filter(|s| !s.is_empty())
}

impl Matcher {
    unsafe fn matches(&self, hwnd: HWND) -> bool {
        let pid = window_pid(hwnd);
        if pid == 0 {
            return false;
        }
        if self.pid == Some(pid) {
            return true;
        }
        let (proc_aumid, exe) = self.cache.borrow_mut().entry(pid).or_insert_with(|| process_identity(pid)).clone();
        if let Some(a) = &self.aumid {
            let same = |x: &Option<String>| x.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(a));
            if same(&proc_aumid) || same(&window_aumid(hwnd)) {
                return true;
            }
        }
        matches!((&self.exe, &exe), (Some(want), Some(got)) if want == got)
    }
}

/// A real app window: visible, top-level, unowned, not a tool window, has a title, not cloaked.
unsafe fn is_candidate(hwnd: HWND) -> bool {
    if hwnd.is_invalid() || !IsWindowVisible(hwnd).as_bool() || GetAncestor(hwnd, GA_ROOT) != hwnd {
        return false;
    }
    if GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
        return false;
    }
    if (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW.0 != 0 || GetWindowTextLengthW(hwnd) == 0 {
        return false;
    }
    let mut cloaked = 0u32;
    let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as _, 4);
    cloaked == 0
}

/// Matching windows in z-order (front-most first).
unsafe fn matching_windows(m: &Matcher) -> Vec<isize> {
    let mut all: Vec<isize> = Vec::new();
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> windows::core::BOOL {
        (*(data.0 as *mut Vec<isize>)).push(hwnd.0 as isize);
        true.into()
    }
    let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
    all.into_iter().filter(|&h| is_candidate(hwnd_of(h)) && m.matches(hwnd_of(h))).collect()
}

/// Visible top-level windows that belong to `app` (for the AppWindows step).
pub fn app_windows(app: &AppRef) -> Vec<isize> {
    let app = app.clone();
    on_sta(move || unsafe { matching_windows(&matcher_for(&app)) }).unwrap_or_default()
}

// ---------- waiting for a new window (event driven) ----------

struct Wait {
    matcher: Matcher,
    exclude: HashSet<isize>,
    found: Option<isize>,
}

thread_local! {
    static WAIT: RefCell<Option<Wait>> = const { RefCell::new(None) };
}

unsafe extern "system" fn on_win_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 {
        return;
    }
    WAIT.with(|w| {
        if let Ok(mut guard) = w.try_borrow_mut() {
            if let Some(st) = guard.as_mut() {
                let h = hwnd.0 as isize;
                if st.found.is_none() && !st.exclude.contains(&h) && is_candidate(hwnd) && st.matcher.matches(hwnd) {
                    st.found = Some(h);
                }
            }
        }
    });
}

struct Hooks(Vec<HWINEVENTHOOK>);

impl Hooks {
    unsafe fn install() -> Hooks {
        let events = [EVENT_OBJECT_SHOW, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND];
        Hooks(
            events
                .iter()
                .map(|&e| SetWinEventHook(e, e, None, Some(on_win_event), 0, 0, WINEVENT_OUTOFCONTEXT))
                .filter(|h| !h.is_invalid())
                .collect(),
        )
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        for h in &self.0 {
            unsafe {
                let _ = UnhookWinEvent(*h);
            }
        }
    }
}

/// Pumps this thread's messages (window events arrive that way) until a match or the deadline.
unsafe fn pump_until_found(deadline: Instant) -> Option<isize> {
    loop {
        let found = WAIT.with(|w| w.borrow().as_ref().and_then(|s| s.found));
        if found.is_some() {
            return found;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        MsgWaitForMultipleObjects(None, false, left.as_millis() as u32, QS_ALLINPUT);
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ---------- launching ----------

/// Returns the started process id when Windows reports one.
unsafe fn launch(app: &AppRef) -> Result<Option<u32>, String> {
    let args = HSTRING::from(app.args.trim());
    let params = if app.args.trim().is_empty() { PCWSTR::null() } else { PCWSTR(args.as_ptr()) };
    match app.source {
        AppSource::Installed => {
            let target = HSTRING::from(format!("shell:AppsFolder\\{}", app.id));
            let r = ShellExecuteW(None, w!("open"), &target, params, PCWSTR::null(), SW_SHOWNORMAL);
            if r.0 as isize > 32 {
                Ok(None)
            } else {
                Err(format!("'{}'을(를) 실행하지 못했습니다.", app.name))
            }
        }
        AppSource::File => {
            let path = app.id.trim().trim_matches('"');
            if !Path::new(path).is_file() {
                return Err(format!("프로그램 파일을 찾을 수 없습니다: {path}"));
            }
            let file = HSTRING::from(path);
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
                lpVerb: w!("open"),
                lpFile: PCWSTR(file.as_ptr()),
                lpParameters: params,
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            ShellExecuteExW(&mut info).map_err(|_| format!("'{}'을(를) 실행하지 못했습니다.", app.name))?;
            if info.hProcess.is_invalid() {
                return Ok(None);
            }
            let pid = GetProcessId(info.hProcess);
            let _ = CloseHandle(info.hProcess);
            Ok((pid != 0).then_some(pid))
        }
    }
}

/// Starts the app (or finds its running window when `if_running` is Front), waits for its main
/// window without polling (up to 20 s), moves it to `monitor`, applies `window`, brings it to the front.
/// Blocks the calling thread; call from a worker thread.
pub fn open_app(app: &AppRef, if_running: IfRunning, monitor: &MonitorRef, window: WindowState) -> Result<OpenedWindow, String> {
    let (app, monitor) = (app.clone(), monitor.clone());
    on_sta(move || unsafe { open_app_sta(&app, if_running, &monitor, window) })
        .unwrap_or_else(|| Err("앱을 여는 중 문제가 생겼습니다.".into()))
}

unsafe fn open_app_sta(app: &AppRef, if_running: IfRunning, monitor: &MonitorRef, window: WindowState) -> Result<OpenedWindow, String> {
    let matcher = matcher_for(app);
    let existing = matching_windows(&matcher);
    if if_running == IfRunning::Front {
        if let Some(&h) = existing.first() {
            finish(hwnd_of(h), monitor, window);
            return Ok(OpenedWindow { hwnd: h, pid: window_pid(hwnd_of(h)), launched: false });
        }
    }

    // Hooks go in before launching so a fast window is not missed.
    WAIT.with(|w| {
        *w.borrow_mut() = Some(Wait { matcher, exclude: existing.iter().copied().collect(), found: None });
    });
    let hooks = Hooks::install();
    let result = (|| {
        let pid = launch(app)?;
        WAIT.with(|w| {
            if let Some(st) = w.borrow_mut().as_mut() {
                st.matcher.pid = pid;
                // The window may already be up before the first event is delivered.
                for h in matching_windows(&st.matcher) {
                    if !st.exclude.contains(&h) {
                        st.found = Some(h);
                        break;
                    }
                }
            }
        });
        Ok::<_, String>(pump_until_found(Instant::now() + WINDOW_TIMEOUT))
    })();
    drop(hooks);
    WAIT.with(|w| *w.borrow_mut() = None);

    // Single-instance apps reuse their existing window instead of opening another.
    // That window was open before the preset, so it is not counted as one the preset opened.
    let (hwnd, launched) = match result? {
        Some(h) => (h, true),
        None => (
            existing.first().copied().ok_or_else(|| format!("'{}'의 창을 20초 안에 찾지 못했습니다.", app.name))?,
            false,
        ),
    };
    finish(hwnd_of(hwnd), monitor, window);
    Ok(OpenedWindow { hwnd, pid: window_pid(hwnd_of(hwnd)), launched })
}

unsafe fn finish(hwnd: HWND, monitor: &MonitorRef, window: WindowState) {
    place(hwnd, monitor, window);
    if window != WindowState::AppDefault || !matches!(monitor, MonitorRef::AppDefault) {
        // Some apps restore their own saved position right after showing; check once and fix.
        std::thread::sleep(Duration::from_millis(600));
        if !placed(hwnd, monitor, window) {
            place(hwnd, monitor, window);
        }
    }
    if window != WindowState::Minimized {
        bring_front(hwnd);
    }
}

// ---------- placing windows ----------

fn rect_w(r: &RECT) -> i32 {
    r.right - r.left
}
fn rect_h(r: &RECT) -> i32 {
    r.bottom - r.top
}

unsafe fn current_work(hwnd: HWND) -> Option<RECT> {
    work_of(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST))
}

unsafe fn restore_if_needed(hwnd: HWND) {
    if IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }
}

/// Moves the window into `work`, keeping its size (shrunk to fit) and centring it.
unsafe fn move_center(hwnd: HWND, work: &RECT) {
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_err() {
        return;
    }
    let w = rect_w(&r).min(rect_w(work));
    let h = rect_h(&r).min(rect_h(work));
    let x = work.left + (rect_w(work) - w) / 2;
    let y = work.top + (rect_h(work) - h) / 2;
    let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
}

unsafe fn frame_bounds(hwnd: HWND) -> Option<RECT> {
    let mut f = RECT::default();
    DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut f as *mut RECT as _, std::mem::size_of::<RECT>() as u32)
        .ok()
        .map(|_| f)
}

/// Window rect vs. visible frame: Windows 10/11 windows have invisible resize borders.
unsafe fn frame_margins(hwnd: HWND) -> (i32, i32, i32, i32) {
    let mut r = RECT::default();
    match (GetWindowRect(hwnd, &mut r), frame_bounds(hwnd)) {
        (Ok(()), Some(f)) => (f.left - r.left, f.top - r.top, r.right - f.right, r.bottom - f.bottom),
        _ => (0, 0, 0, 0),
    }
}

fn half(work: &RECT, left: bool) -> RECT {
    let mid = work.left + rect_w(work) / 2;
    if left {
        RECT { left: work.left, top: work.top, right: mid, bottom: work.bottom }
    } else {
        RECT { left: mid, top: work.top, right: work.right, bottom: work.bottom }
    }
}

/// Makes the visible frame exactly `want`.
unsafe fn set_visible_rect(hwnd: HWND, want: &RECT) {
    let (l, t, r, b) = frame_margins(hwnd);
    let _ = SetWindowPos(
        hwnd,
        None,
        want.left - l,
        want.top - t,
        rect_w(want) + l + r,
        rect_h(want) + t + b,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

unsafe fn place(hwnd: HWND, monitor: &MonitorRef, window: WindowState) {
    let target = target_work(monitor);
    match window {
        WindowState::AppDefault => {
            let Some(work) = target else { return };
            let was_max = IsZoomed(hwnd).as_bool();
            restore_if_needed(hwnd);
            move_center(hwnd, &work);
            if was_max {
                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            }
        }
        WindowState::Maximized => {
            if let Some(work) = target {
                restore_if_needed(hwnd);
                move_center(hwnd, &work);
            }
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
        }
        WindowState::Minimized => {
            if let Some(work) = target {
                restore_if_needed(hwnd);
                move_center(hwnd, &work);
            }
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
        }
        WindowState::LeftHalf | WindowState::RightHalf => {
            restore_if_needed(hwnd);
            if let Some(work) = target {
                // Moving first lets a monitor with another scale (DPI) resize the window before the final fit.
                move_center(hwnd, &work);
            }
            let Some(work) = target.or_else(|| current_work(hwnd)) else { return };
            set_visible_rect(hwnd, &half(&work, window == WindowState::LeftHalf));
        }
    }
}

/// True when the window already is where `place` would put it.
unsafe fn placed(hwnd: HWND, monitor: &MonitorRef, window: WindowState) -> bool {
    let on_target = || match target_work(monitor) {
        None => true,
        Some(t) => current_work(hwnd).is_some_and(|c| c == t),
    };
    match window {
        WindowState::AppDefault => on_target(),
        WindowState::Maximized => IsZoomed(hwnd).as_bool() && on_target(),
        WindowState::Minimized => IsIconic(hwnd).as_bool(),
        WindowState::LeftHalf | WindowState::RightHalf => {
            let Some(work) = target_work(monitor).or_else(|| current_work(hwnd)) else { return true };
            let want = half(&work, window == WindowState::LeftHalf);
            let Some(f) = frame_bounds(hwnd) else { return true };
            (f.left - want.left).abs() <= 2
                && (f.top - want.top).abs() <= 2
                && (f.right - want.right).abs() <= 2
                && (f.bottom - want.bottom).abs() <= 2
        }
    }
}

// ---------- foreground ----------

unsafe fn alt_key(up: bool) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_MENU,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
}

/// Windows only lets the app the user last interacted with take the foreground; these are the usual
/// permitted ways around it.
unsafe fn bring_front(hwnd: HWND) {
    if IsIconic(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }
    if SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() == hwnd {
        return;
    }
    // Holding Alt while switching unlocks the foreground without opening the old window's menu.
    alt_key(false);
    let ok = SetForegroundWindow(hwnd).as_bool();
    alt_key(true);
    if ok && GetForegroundWindow() == hwnd {
        return;
    }
    let fg = GetForegroundWindow();
    let fg_thread = if fg.is_invalid() { 0 } else { GetWindowThreadProcessId(fg, None) };
    let me = GetCurrentThreadId();
    let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW;
    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
    let _ = SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
    let _ = BringWindowToTop(hwnd);
    let _ = SetForegroundWindow(hwnd);
    if attached {
        let _ = AttachThreadInput(me, fg_thread, false);
    }
}

// ---------- restore ----------

/// Restore action for an OpenApp step. Does nothing if `hwnd` no longer belongs to `pid`.
/// A window that was already open before the preset is never closed, only minimized.
pub fn restore_opened(opened: &OpenedWindow, on_restore: OnRestore) {
    unsafe {
        let h = hwnd_of(opened.hwnd);
        if !IsWindow(Some(h)).as_bool() || window_pid(h) != opened.pid {
            return;
        }
        match on_restore {
            OnRestore::Keep => {}
            OnRestore::Close if opened.launched => close_window(opened.hwnd),
            // Async: a frozen app must not hold up the rest of the restore.
            OnRestore::Minimize | OnRestore::Close => {
                let _ = ShowWindowAsync(h, SW_MINIMIZE);
            }
        }
    }
}

/// Asks a window to close politely (WM_CLOSE), like clicking its X button.
pub fn close_window(hwnd: isize) {
    unsafe {
        let _ = PostMessageW(Some(hwnd_of(hwnd)), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_state_profiles() {
        let json = r#"{"profile":{"info_cache":{
            "Profile 2":{"name":"업무"},"Default":{"name":"개인"},"Profile 10":{"name":""},"Profile 1":{"name":"게임"}}}}"#;
        let p = parse_local_state("chrome", json);
        let dirs: Vec<_> = p.iter().map(|x| x.dir.as_str()).collect();
        assert_eq!(dirs, ["Default", "Profile 1", "Profile 2", "Profile 10"]);
        assert_eq!(p[0].name, "개인");
        assert_eq!(p[3].name, "Profile 10", "empty name falls back to the folder");
        assert!(parse_local_state("edge", "not json").is_empty());
        assert!(parse_local_state("edge", "{}").is_empty());
    }

    #[test]
    fn url_schemes() {
        assert_eq!(validate_url(" https://mail.google.com ").unwrap(), "https://mail.google.com");
        assert!(validate_url("HTTP://example.com/a?b=c").is_ok());
        for bad in ["", "https://", "javascript:alert(1)", "file:///C:/x", "ms-settings:", "https://a b", "www.x.com"] {
            assert!(validate_url(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn browser_arguments() {
        assert_eq!(
            browser_args("chrome", "Profile 1", true, true, "https://x.com"),
            ["--profile-directory=Profile 1", "--new-window", "--incognito", "https://x.com"]
        );
        assert_eq!(browser_args("edge", "", false, true, "https://x.com"), ["--inprivate", "https://x.com"]);
        assert_eq!(browser_args("chrome", "  ", false, false, "https://x.com"), ["https://x.com"]);
    }

    #[test]
    fn known_folder_ids() {
        assert_eq!(
            split_known_folder_id(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Notepad++\notepad++.exe"),
            Some(("6D809377-6AF0-444B-8957-A3773F02200E".into(), r"Notepad++\notepad++.exe".into()))
        );
        assert_eq!(split_known_folder_id("Chrome"), None);
        assert_eq!(split_known_folder_id("Claude_pzs8sxrjxfjjc!Claude"), None);
        assert_eq!(split_known_folder_id("{bad}\\x.exe"), None);
    }

    #[test]
    fn halves_split_the_work_area() {
        let work = RECT { left: -1920, top: 0, right: 0, bottom: 1040 };
        assert_eq!(half(&work, true), RECT { left: -1920, top: 0, right: -960, bottom: 1040 });
        assert_eq!(half(&work, false), RECT { left: -960, top: 0, right: 0, bottom: 1040 });
    }

    fn dpi_aware() {
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    /// cargo test --release -- --ignored --nocapture manual_lists
    #[test]
    #[ignore]
    fn manual_lists() {
        dpi_aware();
        for m in monitors() {
            println!("monitor: {} primary={} work={:?} id={}", m.name, m.primary, m.work, m.id);
        }
        let profiles = browser_profiles();
        println!("browser profiles: {}", profiles.len());
        for p in &profiles {
            println!("  {} / {} ({})", p.browser, p.name, p.dir);
        }
        let apps = installed_apps();
        let store = apps.iter().filter(|a| a.kind == "store").count();
        println!("installed apps: {} (store {store})", apps.len());
        if let Some(c) = apps.iter().find(|a| a.name.contains("Claude")) {
            println!("  Claude id: {}", c.id);
        }
    }

    /// Opens Notepad on the primary monitor's left half, checks it, then closes only that window.
    /// Skips when Notepad is already open so a user's window is never touched.
    #[test]
    #[ignore]
    fn manual_notepad_left_half() {
        dpi_aware();
        let app = AppRef {
            source: AppSource::File,
            id: r"C:\Windows\System32\notepad.exe".into(),
            name: "메모장".into(),
            args: String::new(),
        };
        let before = app_windows(&app);
        if !before.is_empty() {
            println!("SKIPPED: Notepad is already open ({} windows)", before.len());
            return;
        }
        let t = Instant::now();
        let opened = open_app(&app, IfRunning::New, &MonitorRef::Primary, WindowState::LeftHalf).expect("open");
        println!("window found+placed after {} ms, launched={}", t.elapsed().as_millis(), opened.launched);
        let ok = unsafe { placed(hwnd_of(opened.hwnd), &MonitorRef::Primary, WindowState::LeftHalf) };
        let frame = unsafe { frame_bounds(hwnd_of(opened.hwnd)) };
        println!("frame {:?}, primary work {:?}, on left half: {ok}", frame, primary_work());
        assert!(!before.contains(&opened.hwnd));
        restore_opened(&opened, OnRestore::Close);
        std::thread::sleep(Duration::from_millis(800));
        let still = unsafe { IsWindow(Some(hwnd_of(opened.hwnd))).as_bool() };
        println!("closed: {}", !still);
        assert!(ok, "window should cover the left half");
    }
}

// ---------- apps that pause Keyscene's shortcuts ----------

#[derive(Clone, Debug, Serialize)]
pub struct RunningApp {
    /// A readable name taken from its window title ("문서 - Word" -> "Word").
    pub name: String,
    /// Program file name, lowercase, e.g. "photoshop.exe".
    pub exe: String,
    /// Full program path, for its icon. Empty when Windows does not tell (e.g. an elevated app).
    pub path: String,
}

unsafe fn process_path(pid: u32) -> Option<String> {
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let path = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size)
        .ok()
        .map(|_| String::from_utf16_lossy(&buf[..size as usize]));
    let _ = CloseHandle(h);
    path
}

/// Program file name (lowercase) of the process that owns `hwnd`.
pub fn window_exe(hwnd: isize) -> Option<String> {
    unsafe {
        let pid = window_pid(hwnd_of(hwnd));
        if pid == 0 {
            return None;
        }
        process_identity(pid).1
    }
}

/// Apps with a window open right now, one entry per program. Keyscene itself is left out.
pub fn running_apps() -> Vec<RunningApp> {
    unsafe {
        let mut all: Vec<isize> = Vec::new();
        unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> windows::core::BOOL {
            (*(data.0 as *mut Vec<isize>)).push(hwnd.0 as isize);
            true.into()
        }
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
        let own = std::process::id();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for h in all {
            let hwnd = hwnd_of(h);
            if !is_candidate(hwnd) || window_pid(hwnd) == own {
                continue;
            }
            let Some(exe) = process_identity(window_pid(hwnd)).1 else { continue };
            if exe == "explorer.exe" || exe == "applicationframehost.exe" || !seen.insert(exe.clone()) {
                continue;
            }
            let mut buf = [0u16; 256];
            let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buf);
            let title = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
            let name = title.rsplit(" - ").next().unwrap_or(&title).trim().to_string();
            let name = if name.is_empty() { exe.trim_end_matches(".exe").to_string() } else { name };
            let path = process_path(window_pid(hwnd)).unwrap_or_default();
            out.push(RunningApp { name, exe, path });
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }
}

/// The program a shortcut (.lnk) points to, or the path itself when it is not a shortcut.
pub fn resolve_shortcut(path: &str) -> String {
    if !path.trim().to_lowercase().ends_with(".lnk") {
        return path.to_string();
    }
    let p = path.to_string();
    on_sta(move || unsafe { resolve_lnk(&p) }).flatten().unwrap_or_else(|| path.to_string())
}
