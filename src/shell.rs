//! Hide or unhide the items selected in the active File Explorer tab or on the desktop.

use std::sync::mpsc::channel;
use std::time::Duration;
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::{
    GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_SYSTEM,
    FILE_FLAGS_AND_ATTRIBUTES, INVALID_FILE_ATTRIBUTES,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IDispatch, IServiceProvider, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    IFolderView, IShellBrowser, IShellItemArray, IShellWindows, IWebBrowserApp, SHChangeNotify, ShellWindows, CSIDL_DESKTOP,
    SHCNE_ATTRIBUTES, SHCNF_PATHW, SID_STopLevelBrowser, SIGDN_FILESYSPATH, SVGIO_SELECTION, SWC_DESKTOP,
    SWFO_NEEDDISPATCH,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetClassNameW, GetForegroundWindow};

/// Returns a message only when something went wrong.
pub fn toggle_selection() -> Option<String> {
    match selection() {
        Ok(paths) if paths.is_empty() => None,
        Ok(paths) => apply(&paths),
        Err(e) => Some(e),
    }
}

/// Paths selected in the File Explorer window or desktop in front. Empty when neither is in front.
pub fn selection() -> Result<Vec<String>, String> {
    let foreground = unsafe { GetForegroundWindow() };
    let class = class_name(foreground);
    let target = match class.as_str() {
        "CabinetWClass" => Target::Window(foreground.0 as isize),
        "Progman" | "WorkerW" => Target::Desktop,
        _ => return Ok(Vec::new()),
    };
    // Explorer's COM objects need their own STA thread; give up after 3 s if Explorer hangs.
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(unsafe { selected_paths(target) });
    });
    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(p)) => Ok(p),
        Ok(Err(_)) => Err("선택한 항목을 읽지 못했습니다.".into()),
        Err(_) => Err("탐색기가 응답하지 않습니다.".into()),
    }
}

#[derive(Clone, Copy)]
enum Target {
    Window(isize),
    Desktop,
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

unsafe fn selected_paths(target: Target) -> windows::core::Result<Vec<String>> {
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let result = (|| {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)?;
        let dispatch: IDispatch = match target {
            Target::Desktop => {
                let loc = variant_i4(CSIDL_DESKTOP as i32);
                let root = VARIANT::default();
                let mut hwnd = 0i32;
                windows.FindWindowSW(&loc, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?
            }
            Target::Window(top) => {
                // With tabs, every tab is a separate shell window; the active tab's view comes first.
                let active_tab = FindWindowExW(Some(HWND(top as _)), None, w!("ShellTabWindowClass"), PCWSTR::null()).ok();
                let mut found = None;
                for i in 0..windows.Count()? {
                    let Ok(item) = windows.Item(&variant_i4(i)) else { continue };
                    let Ok(app) = item.cast::<IWebBrowserApp>() else { continue };
                    if app.HWND().map(|h| h.0 as isize) != Ok(top) {
                        continue;
                    }
                    let browser = browser_of(&item)?;
                    let tab = browser.GetWindow().ok();
                    if active_tab.is_none() || tab == active_tab {
                        found = Some(item);
                        break;
                    }
                }
                found.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?
            }
        };
        let browser = browser_of(&dispatch)?;
        let view: IFolderView = browser.QueryActiveShellView()?.cast()?;
        let Ok(items) = view.Items::<IShellItemArray>(SVGIO_SELECTION) else {
            return Ok(Vec::new());
        };
        let mut paths = Vec::new();
        for i in 0..items.GetCount()? {
            let item = items.GetItemAt(i)?;
            // Items without a file system path (Control Panel, phones, ...) are skipped.
            if let Ok(p) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                paths.push(p.to_string().unwrap_or_default());
                CoTaskMemFree(Some(p.0 as _));
            }
        }
        Ok(paths)
    })();
    CoUninitialize();
    result
}

/// A VARIANT holding a 32-bit integer (VT_I4).
fn variant_i4(value: i32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: value },
            }),
        },
    }
}

unsafe fn browser_of(dispatch: &IDispatch) -> windows::core::Result<IShellBrowser> {
    let provider: IServiceProvider = dispatch.cast()?;
    provider.QueryService(&SID_STopLevelBrowser)
}

fn apply(paths: &[String]) -> Option<String> {
    let mut targets = Vec::new();
    let mut system = 0;
    for p in paths {
        let wide: Vec<u16> = p.encode_utf16().chain(Some(0)).collect();
        let attrs = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };
        let protected = attrs != INVALID_FILE_ATTRIBUTES
            && attrs & FILE_ATTRIBUTE_HIDDEN.0 != 0
            && attrs & FILE_ATTRIBUTE_SYSTEM.0 != 0;
        // Never touch drive roots or Windows' protected (hidden + system) items.
        if attrs == INVALID_FILE_ATTRIBUTES || protected || p.trim_end_matches('\\').len() <= 2 {
            continue;
        }
        // System items are never hidden: hidden + system would make them protected OS files,
        // which are never unhidden. Not hidden now, so unhiding needs nothing from them either.
        if attrs & FILE_ATTRIBUTE_SYSTEM.0 != 0 {
            system += 1;
            continue;
        }
        targets.push((wide, attrs));
    }
    if targets.is_empty() {
        return Some(if system > 0 {
            "선택한 항목은 시스템 속성이 있어 숨기지 않았습니다.".into()
        } else {
            "선택한 항목은 드라이브이거나 Windows 보호 파일이라 바꿀 수 없습니다.".into()
        });
    }
    let hide = !targets.iter().all(|(_, a)| a & FILE_ATTRIBUTE_HIDDEN.0 != 0);
    let mut done = 0;
    for (wide, attrs) in &targets {
        let mut next = if hide { attrs | FILE_ATTRIBUTE_HIDDEN.0 } else { attrs & !FILE_ATTRIBUTE_HIDDEN.0 };
        if next == 0 {
            next = FILE_ATTRIBUTE_NORMAL.0;
        }
        unsafe {
            if SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(next)).is_ok() {
                done += 1;
                SHChangeNotify(SHCNE_ATTRIBUTES, SHCNF_PATHW, Some(wide.as_ptr() as _), None);
            }
        }
    }
    // Success shows itself in Explorer; only report what went wrong or was left alone.
    let mut parts = Vec::new();
    let failed = targets.len() - done;
    if failed > 0 {
        parts.push(format!("선택한 항목 중 {failed}개를 바꾸지 못했습니다. 권한이 있는지 확인하세요."));
    }
    if hide && system > 0 {
        parts.push(format!("시스템 속성이 있는 {system}개는 숨기지 않았습니다."));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(p: &std::path::Path) -> u32 {
        let w: Vec<u16> = p.to_string_lossy().encode_utf16().chain(Some(0)).collect();
        unsafe { GetFileAttributesW(PCWSTR(w.as_ptr())) }
    }

    fn set_attrs(p: &std::path::Path, a: u32) {
        let w: Vec<u16> = p.to_string_lossy().encode_utf16().chain(Some(0)).collect();
        unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(a)) }.unwrap();
    }

    #[test]
    fn system_items_stay_visible() {
        let base = std::env::temp_dir().join(format!("keyscene-shell-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (sys, plain) = (base.join("sys.txt"), base.join("plain.txt"));
        std::fs::write(&sys, "s").unwrap();
        std::fs::write(&plain, "p").unwrap();
        set_attrs(&sys, FILE_ATTRIBUTE_SYSTEM.0);
        let both = [sys.to_string_lossy().into_owned(), plain.to_string_lossy().into_owned()];

        let msg = apply(&both).unwrap_or_default();
        let after = (attrs(&sys), attrs(&plain) & FILE_ATTRIBUTE_HIDDEN.0 != 0);
        let only_sys = apply(&both[..1]);

        set_attrs(&sys, FILE_ATTRIBUTE_NORMAL.0);
        set_attrs(&plain, FILE_ATTRIBUTE_NORMAL.0);
        std::fs::remove_dir_all(&base).unwrap();

        assert!(msg.contains("시스템"), "{msg}");
        assert_eq!(after, (FILE_ATTRIBUTE_SYSTEM.0, true));
        assert!(only_sys.is_some_and(|m| m.contains("시스템")));
    }
}
