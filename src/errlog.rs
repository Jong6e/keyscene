//! Error log: `%APPDATA%\Keyscene\keyscene.log`.
//! Only failures are written, so nothing touches the disk while things work.
//! When the file passes MAX_BYTES it becomes `keyscene.old.log` (one generation kept).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 256 * 1024;

static PATH: OnceLock<PathBuf> = OnceLock::new();
static WRITE: Mutex<()> = Mutex::new(());

/// Sets the log folder and records panics. Call once, before anything else can fail.
pub fn init(dir: PathBuf) {
    if PATH.set(dir.join("keyscene.log")).is_err() {
        return;
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        error(&format!("중단됨(panic) [{}] {info}", thread.name().unwrap_or("이름 없는 스레드")));
        default_hook(info);
    }));
}

/// Appends one line. Never fails loudly: a broken log must not break the app.
pub fn error(message: &str) {
    let Some(path) = PATH.get() else { return };
    // A panic while holding the lock poisons it; the log should still work afterwards.
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    if fs::metadata(path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = fs::rename(path, path.with_file_name("keyscene.old.log"));
    }
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let line = format!("{}  {}\n", now(), message.replace(['\r', '\n'], " "));
        let _ = file.write_all(line.as_bytes());
    }
}

fn now() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test owns the global log path, so everything that needs it lives here.
    #[test]
    fn writes_lines_panics_and_rotates() {
        let dir = std::env::temp_dir().join(format!("keyscene-errlog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        init(dir.clone());
        let log = dir.join("keyscene.log");

        error("첫 줄\n둘째 줄");
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.ends_with("첫 줄 둘째 줄\n"), "{text}");

        let _ = std::thread::Builder::new().name("시험".into()).spawn(|| panic!("일부러")).unwrap().join();
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.contains("중단됨(panic) [시험]") && text.contains("일부러"), "{text}");

        fs::write(&log, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        error("새 파일");
        assert!(dir.join("keyscene.old.log").exists());
        assert!(fs::read_to_string(&log).unwrap().ends_with("새 파일\n"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn time_format() {
        let s = super::now();
        assert_eq!(s.len(), 19);
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[13..14], ":");
    }
}
