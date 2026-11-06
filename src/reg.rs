//! Minimal HKEY_CURRENT_USER helpers on top of the Win32 registry API.

use std::ptr::null_mut;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_NOTIFY_CHANGE_LAST_SET, REG_SZ,
    RRF_RT_REG_SZ,
};

/// Reads a text value under any root key (e.g. HKEY_LOCAL_MACHINE). None if it is missing.
pub fn read_string(root: HKEY, path: &str, name: &str) -> Option<String> {
    let (path_w, name_w) = (wide(path), wide(name));
    let mut buf = [0u16; 260];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(root, path_w.as_ptr(), name_w.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr().cast(), &mut size)
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A plain-language message instead of a raw Windows error number.
fn reason(what: &str, code: u32) -> String {
    match code {
        5 => format!("{what}. 권한이 없습니다. 회사·학교 PC라면 관리자 정책으로 막혀 있을 수 있습니다."),
        2 => format!("{what}. 해당 설정이 이 PC에 없습니다."),
        _ => format!("{what}. 잠시 뒤 다시 시도하세요."),
    }
}

pub struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

// The handle is only used from the thread that owns the Key.
unsafe impl Send for Key {}

impl Key {
    fn open(path: &str, access: u32) -> Result<Key, String> {
        let mut handle: HKEY = null_mut();
        let path_w = wide(path);
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path_w.as_ptr(), 0, access, &mut handle) };
        if rc != ERROR_SUCCESS {
            return Err(reason("Windows 설정을 읽지 못했습니다", rc));
        }
        Ok(Key(handle))
    }

    pub fn open_read(path: &str) -> Result<Key, String> {
        Key::open(path, KEY_READ)
    }

    pub fn open_write(path: &str) -> Result<Key, String> {
        Key::open(path, KEY_READ | KEY_SET_VALUE)
    }

    pub fn open_notify(path: &str) -> Result<Key, String> {
        Key::open(path, KEY_NOTIFY | KEY_READ)
    }

    /// Reads a DWORD value. Returns None when the value is missing or has another type.
    pub fn get_dword(&self, name: &str) -> Option<u32> {
        let name_w = wide(name);
        let mut kind = 0u32;
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let rc = unsafe {
            RegQueryValueExW(
                self.0,
                name_w.as_ptr(),
                null_mut(),
                &mut kind,
                &mut data as *mut u32 as *mut u8,
                &mut size,
            )
        };
        (rc == ERROR_SUCCESS && kind == REG_DWORD).then_some(data)
    }

    pub fn set_dword(&self, name: &str, value: u32) -> Result<(), String> {
        let name_w = wide(name);
        let rc = unsafe {
            RegSetValueExW(
                self.0,
                name_w.as_ptr(),
                0,
                REG_DWORD,
                &value as *const u32 as *const u8,
                std::mem::size_of::<u32>() as u32,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(reason("Windows 설정을 바꾸지 못했습니다", rc));
        }
        Ok(())
    }

    pub fn set_string(&self, name: &str, value: &str) -> Result<(), String> {
        let (name_w, value_w) = (wide(name), wide(value));
        let rc = unsafe {
            RegSetValueExW(self.0, name_w.as_ptr(), 0, REG_SZ, value_w.as_ptr().cast(), (value_w.len() * 2) as u32)
        };
        if rc != ERROR_SUCCESS {
            return Err(reason("Windows 설정을 바꾸지 못했습니다", rc));
        }
        Ok(())
    }

    /// Blocks until a value under this key changes. Uses no CPU while waiting.
    pub fn wait_for_change(&self) -> bool {
        let rc = unsafe { RegNotifyChangeKeyValue(self.0, 0, REG_NOTIFY_CHANGE_LAST_SET, null_mut(), 0) };
        rc == ERROR_SUCCESS
    }
}

/// Runs `on_change` on a background thread each time a value under `path` changes.
pub fn watch(path: &'static str, on_change: impl Fn() + Send + 'static) {
    std::thread::Builder::new()
        .name(format!("watch {path}"))
        .spawn(move || {
            let Ok(key) = Key::open_notify(path) else { return };
            while key.wait_for_change() {
                on_change();
            }
        })
        .ok();
}
