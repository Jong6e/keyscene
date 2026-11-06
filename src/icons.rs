//! App and file icons for the settings window, served as PNG through the `icon` URI scheme.
//!
//! URL (made by the page with convertFileSrc): http://icon.localhost/<encoded target>?s=<pixels>
//!   target = "app:<AppsFolder parsing name>" | "path:<file or folder path>"
//! Work happens on a few background threads that start on the first request and end when idle,
//! so the tray app carries no icon code or cache while the settings window is closed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, IBindCtx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY};

const WORKERS: usize = 3;
/// Worker threads stop after this long without work.
pub const IDLE: Duration = Duration::from_secs(10);
/// A worker inside one icon for longer than this (a hung drive or shell extension) stops counting
/// toward WORKERS, so a new one can serve the rest.
const STUCK: Duration = Duration::from_secs(5);
const CACHE_MAX: usize = 600;
/// GetDriveTypeW results (WindowsProgramming constants, not in this crate's feature list).
const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_REMOTE: u32 = 4;

type Png = Option<Arc<Vec<u8>>>;
type Reply = Box<dyn FnOnce(Png) + Send>;

struct Job {
    target: String,
    size: u32,
    reply: Reply,
}

struct Pool {
    tx: Mutex<Sender<Job>>,
    rx: Arc<Mutex<Receiver<Job>>>,
    /// Running workers by id, with when each started its current icon (None while waiting for work).
    workers: Mutex<HashMap<u64, Option<Instant>>>,
    next_id: AtomicU64,
    cache: Mutex<HashMap<(String, u32), Png>>,
}

static POOL: OnceLock<Pool> = OnceLock::new();

fn pool() -> &'static Pool {
    POOL.get_or_init(|| {
        let (tx, rx) = channel();
        Pool {
            tx: Mutex::new(tx),
            rx: Arc::new(Mutex::new(rx)),
            workers: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            cache: Mutex::new(HashMap::new()),
        }
    })
}

/// Marks worker `id` as inside an icon (Some) or done with it (None).
fn mark(id: u64, busy: Option<Instant>) {
    if let Some(slot) = pool().workers.lock().unwrap().get_mut(&id) {
        *slot = busy;
    }
}

/// Drops cached icons; called when the settings window closes.
pub fn clear_cache() {
    if let Some(p) = POOL.get() {
        let mut c = p.cache.lock().unwrap();
        c.clear();
        c.shrink_to_fit();
    }
}

/// Answers one request from the page.
pub fn handle(request: tauri::http::Request<Vec<u8>>, responder: tauri::UriSchemeResponder) {
    let send = move |png: Png| {
        let resp = match png {
            Some(bytes) => tauri::http::Response::builder()
                .status(200)
                .header("Content-Type", "image/png")
                .header("Cache-Control", "max-age=3600")
                .body(bytes.as_ref().clone()),
            None => tauri::http::Response::builder().status(404).body(Vec::new()),
        };
        if let Ok(r) = resp {
            responder.respond(r);
        }
    };
    let uri = request.uri();
    let target = percent_decode(uri.path().trim_start_matches('/'));
    let size = uri
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("s=")))
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(32)
        .clamp(16, 256);
    let Some(target) = target.filter(|t| allowed(t)) else {
        send(None);
        return;
    };
    let p = pool();
    // Bound first, so the cache lock is let go before the reply is built.
    let hit = p.cache.lock().unwrap().get(&(target.clone(), size)).cloned();
    if let Some(hit) = hit {
        send(hit);
        return;
    }
    let _ = p.tx.lock().unwrap().send(Job { target, size, reply: Box::new(send) });
    let mut workers = p.workers.lock().unwrap();
    let working = workers.values().filter(|busy| busy.is_none_or(|since| since.elapsed() < STUCK)).count();
    if working < WORKERS {
        let id = p.next_id.fetch_add(1, Ordering::Relaxed);
        let rx = p.rx.clone();
        // Registered while still holding the lock, before the new thread can look for its slot.
        if std::thread::Builder::new().name("icons".into()).spawn(move || worker(id, rx)).is_ok() {
            workers.insert(id, None);
        }
    }
}

/// Only shell items: no network paths (a lookup there could make Windows contact another computer).
fn allowed(target: &str) -> bool {
    if let Some(id) = target.strip_prefix("app:") {
        return !id.is_empty() && !id.contains("..") && !id.starts_with(['\\', '/']);
    }
    if let Some(p) = target.strip_prefix("path:") {
        let b = p.as_bytes();
        return b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    }
    false
}

fn worker(id: u64, rx: Arc<Mutex<Receiver<Job>>>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let p = pool();
    let mut last_job = Instant::now();
    loop {
        // Short waits, so no thread sits on the shared queue for long and each stops on time.
        let next = rx.lock().unwrap().recv_timeout(Duration::from_millis(250));
        let job = match next {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) if last_job.elapsed() < IDLE => continue,
            Err(RecvTimeoutError::Timeout) => {
                p.workers.lock().unwrap().remove(&id);
                // A job sent while we were deciding to stop must not be left waiting.
                let late = rx.lock().unwrap().try_recv();
                match late {
                    Ok(job) => {
                        p.workers.lock().unwrap().insert(id, None);
                        job
                    }
                    Err(_) => break,
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                p.workers.lock().unwrap().remove(&id);
                break;
            }
        };
        last_job = Instant::now();
        let key = (job.target.clone(), job.size);
        let cached = p.cache.lock().unwrap().get(&key).cloned();
        let png = cached.unwrap_or_else(|| {
            mark(id, Some(Instant::now()));
            let png = unsafe { render(&job.target, job.size) }.map(Arc::new);
            mark(id, None);
            let mut c = p.cache.lock().unwrap();
            if c.len() >= CACHE_MAX {
                c.clear();
            }
            c.insert(key, png.clone());
            png
        });
        (job.reply)(png);
    }
    // Unloads the shell extensions this thread pulled in.
    unsafe { CoUninitialize() };
}

unsafe fn render(target: &str, size: u32) -> Option<Vec<u8>> {
    let parse = if let Some(id) = target.strip_prefix("app:") {
        format!("shell:AppsFolder\\{id}")
    } else {
        let path = target.strip_prefix("path:")?.replace('/', "\\");
        if !local_drive(&path) {
            return None;
        }
        path
    };
    let w: Vec<u16> = parse.encode_utf16().chain(Some(0)).collect();
    let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None::<&IBindCtx>).ok()?;
    let hbmp = factory.GetImage(SIZE { cx: size as i32, cy: size as i32 }, SIIGBF_ICONONLY).ok()?;
    let obj = HGDIOBJ(hbmp.0);
    let mut bm = BITMAP::default();
    let ok = GetObjectW(obj, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as _)) != 0;
    let (width, height) = (bm.bmWidth, bm.bmHeight.abs());
    let mut bgra = vec![0u8; (width.max(0) * height * 4) as usize];
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let hdc = GetDC(None);
    let rows = if ok && width > 0 && height > 0 {
        GetDIBits(hdc, hbmp, 0, height as u32, Some(bgra.as_mut_ptr() as _), &mut info, DIB_RGB_COLORS)
    } else {
        0
    };
    ReleaseDC(None, hdc);
    let _ = DeleteObject(obj);
    if rows == 0 {
        return None;
    }
    to_png(&mut bgra, width as u32, height as u32)
}

/// False for a mapped network drive ("Z:\" pointing at a share) or a drive letter that is not there:
/// looking up an icon there could make Windows contact another computer. Asked here, on a worker,
/// so the page's request handler never waits on a drive.
fn local_drive(path: &str) -> bool {
    let Some(letter) = path.get(..2) else { return false };
    let root: Vec<u16> = format!("{letter}\\").encode_utf16().chain(Some(0)).collect();
    let kind = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
    kind != DRIVE_REMOTE && kind != DRIVE_NO_ROOT_DIR
}

/// BGRA from Windows -> PNG. Handles bitmaps with no alpha and with premultiplied alpha.
fn to_png(px: &mut [u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let any_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
    // Premultiplied colors can never exceed alpha; straight colors usually do somewhere.
    let premultiplied = any_alpha && px.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2); // BGRA -> RGBA
        if !any_alpha {
            p[3] = 255;
        } else if premultiplied && p[3] != 0 && p[3] != 255 {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(px).ok()?;
    }
    Some(out)
}

/// "%EC%95%B1" -> "앱". None for broken input.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_and_filter() {
        assert_eq!(percent_decode("path%3AC%3A%5CUsers%5C%EC%95%B1").as_deref(), Some("path:C:\\Users\\앱"));
        assert_eq!(percent_decode("%ZZ"), None);
        assert!(allowed("path:C:\\Windows\\notepad.exe"));
        assert!(allowed("app:Claude_pzs8sxrjxfjjc!Claude"));
        assert!(!allowed("path:\\\\server\\share\\a.exe"));
        assert!(!allowed("file:C:\\a"));
        assert!(!allowed("app:\\\\server\\share"));
        assert!(!allowed("app:/x"));
        let win = std::env::var("SystemRoot").unwrap();
        assert!(local_drive(&win));
    }

    #[test]
    fn renders_a_real_icon() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let win = std::env::var("SystemRoot").unwrap();
        let png = unsafe { render(&format!("path:{win}\\notepad.exe"), 40) }.expect("notepad icon");
        assert_eq!(&png[1..4], b"PNG");
        let folder = unsafe { render(&format!("path:{win}"), 20) }.expect("folder icon");
        assert_eq!(&folder[1..4], b"PNG");
    }
}
