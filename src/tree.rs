//! "하위 항목까지 숨기기 / 숨김 해제": set or clear the hidden attribute on a folder and everything under it.
//!
//! Windows' own "apply to this folder, subfolders and files" rewrites every item. Here the directory
//! listing already carries each item's attributes, so only items that actually change are written.

use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, GetFileAttributesW,
    GetFinalPathNameByHandleW, SetFileAttributesW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN,
    FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SYSTEM, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_FLAG_BACKUP_SEMANTICS, FIND_FIRST_EX_LARGE_FETCH, INVALID_FILE_ATTRIBUTES, VOLUME_NAME_DOS,
    WIN32_FIND_DATAW,
};
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ATTRIBUTES, SHCNE_UPDATEDIR, SHCNF_PATHW};

/// Above this many changes, Explorer is told "this folder changed" once per folder instead of per item.
const PER_ITEM_NOTIFY_LIMIT: usize = 200;

#[derive(Default, Debug)]
pub struct Outcome {
    /// Items whose attribute was changed, in the order they were changed.
    pub changed: Vec<PathBuf>,
    /// Items that could not be changed (in use, no permission, ...).
    pub failed: usize,
    /// Items with the system attribute left visible when hiding: hidden + system would turn them into
    /// protected OS files, which are never unhidden.
    pub skipped: usize,
    /// Messages for chosen folders that were left alone on purpose or not found.
    pub problems: Vec<String>,
}

impl Outcome {
    /// One line for the user, or None when everything went fine.
    pub fn message(&self) -> Option<String> {
        let mut parts = self.problems.clone();
        if self.failed > 0 {
            parts.push(format!("{}개는 바꾸지 못했습니다. 사용 중이거나 권한이 없는 항목입니다.", count(self.failed)));
        }
        if self.skipped > 0 {
            parts.push(format!("시스템 속성이 있는 {}개는 숨기지 않았습니다.", count(self.skipped)));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// Changes one item and records the result.
    fn change(&mut self, path: PathBuf, attrs: u32, hide: bool) {
        if hide && !is_hidden(attrs) && attrs & FILE_ATTRIBUTE_SYSTEM.0 != 0 {
            self.skipped += 1;
            return;
        }
        match set(&path, attrs, hide) {
            Ok(true) => self.changed.push(path),
            Ok(false) => {}
            Err(()) => self.failed += 1,
        }
    }
}

/// "1234" -> "1,234"
pub fn count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Plain wide string, for the shell.
fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect()
}

/// Wide string for file calls, with the `\\?\` prefix so paths over 260 characters work.
/// The prefix turns off Windows' own clean-up ("/", "..", trailing dots), so `p` must already be
/// a clean full path, as `apply` makes it.
fn long(p: &Path) -> Vec<u16> {
    let s = p.as_os_str().to_string_lossy();
    let s = if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
        s.into_owned()
    } else if let Some(share) = s.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{share}")
    } else if s.as_bytes().get(1..3) == Some(b":\\") {
        format!(r"\\?\{s}")
    } else {
        s.into_owned() // not a full path: left to Windows as is
    };
    s.encode_utf16().chain(Some(0)).collect()
}

fn norm(p: &Path) -> String {
    p.to_string_lossy().trim_end_matches('\\').to_lowercase()
}

/// Where `p` really is, after links, subst drives, short names and other spellings of the same path:
/// lowercase, without `\\?\` or a trailing `\`. None when it cannot be opened.
fn real(p: &Path) -> Option<String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    // No access asked: only the name is read, and items in use still open. Closed when dropped.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0) // needed to open folders
        .open(p)
        .ok()?;
    let h = HANDLE(file.as_raw_handle());
    let mut buf = vec![0u16; 512];
    let mut n = unsafe { GetFinalPathNameByHandleW(h, &mut buf, VOLUME_NAME_DOS) } as usize;
    if n > buf.len() {
        buf.resize(n, 0);
        n = unsafe { GetFinalPathNameByHandleW(h, &mut buf, VOLUME_NAME_DOS) } as usize;
    }
    if n == 0 || n > buf.len() {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..n]);
    let s = match s.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned(),
    };
    Some(s.trim_end_matches('\\').to_lowercase())
}

/// "c:" or "\\server\share": a whole drive.
fn whole_drive(real: &str) -> bool {
    match real.strip_prefix(r"\\") {
        Some(unc) => unc.split('\\').count() <= 2,
        None => !real.contains('\\'),
    }
}

/// Folders where a bulk change could confuse Windows or programs. Returns why, if `p` is one.
/// `p` must be a clean full path. Paths are compared by where they really lead, so other spellings
/// ("C:/Windows", "C:\PROGRA~1", a junction or subst drive pointing inside) are caught too.
fn off_limits(p: &Path) -> Option<&'static str> {
    if p.parent().is_none() {
        return Some("드라이브 전체");
    }
    let Some(me) = real(p) else {
        // Missing items are reported by the caller; anything else that cannot be traced is left alone.
        let w = long(p);
        let exists = unsafe { GetFileAttributesW(PCWSTR(w.as_ptr())) } != INVALID_FILE_ATTRIBUTES;
        return exists.then_some("실제 위치를 확인할 수 없는 항목");
    };
    if whole_drive(&me) {
        return Some("드라이브 전체");
    }
    let place = |p: &Path| {
        let p = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        real(&p).unwrap_or_else(|| norm(&p))
    };
    let env = |k: &str| std::env::var_os(k).map(|v| place(Path::new(&v)));
    // Inside these, nothing is changed.
    for key in ["SystemRoot", "ProgramFiles", "ProgramFiles(x86)", "ProgramData"] {
        if let Some(base) = env(key) {
            if me == base || me.starts_with(&format!("{base}\\")) {
                return Some("Windows나 프로그램이 쓰는 폴더");
            }
        }
    }
    // These themselves hold settings folders (AppData, ...), but their subfolders are fine.
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let profile = Path::new(&profile);
        let users = profile.parent().map(place);
        if me == place(profile) || Some(&me) == users.as_ref() {
            return Some("사용자 폴더 전체");
        }
    }
    None
}

struct Entry {
    attrs: u32,
    /// Symbolic link, junction or mount point: its contents live elsewhere, so it is not entered.
    /// Its own hidden attribute is still changed: SetFileAttributesW changes the link, not the target.
    link: bool,
}

fn entry_of(data: &WIN32_FIND_DATAW) -> Entry {
    let attrs = data.dwFileAttributes;
    // dwReserved0 holds the reparse tag; "name surrogate" tags are the ones that point elsewhere.
    // OneDrive's cloud folders are reparse points too, but not name surrogates, so they are entered.
    let link = attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 && data.dwReserved0 & 0x2000_0000 != 0;
    Entry { attrs, link }
}

fn stat(path: &Path) -> Option<Entry> {
    let w = long(path);
    let mut data = WIN32_FIND_DATAW::default();
    unsafe {
        let h = FindFirstFileExW(PCWSTR(w.as_ptr()), FindExInfoBasic, &mut data as *mut _ as _, FindExSearchNameMatch, None, Default::default()).ok()?;
        let _ = FindClose(h);
    }
    Some(entry_of(&data))
}

/// Calls `f` for every item directly inside `dir`.
fn list(dir: &Path, mut f: impl FnMut(&str, Entry)) -> bool {
    let w = long(&dir.join("*"));
    let mut data = WIN32_FIND_DATAW::default();
    unsafe {
        let Ok(h): windows::core::Result<HANDLE> = FindFirstFileExW(
            PCWSTR(w.as_ptr()),
            FindExInfoBasic,
            &mut data as *mut _ as _,
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        ) else {
            return false;
        };
        loop {
            let len = data.cFileName.iter().position(|&c| c == 0).unwrap_or(data.cFileName.len());
            let name = String::from_utf16_lossy(&data.cFileName[..len]);
            if name != "." && name != ".." {
                f(&name, entry_of(&data));
            }
            if FindNextFileW(h, &mut data).is_err() {
                break;
            }
        }
        let _ = FindClose(h);
    }
    true
}

fn protected(attrs: u32) -> bool {
    attrs & FILE_ATTRIBUTE_HIDDEN.0 != 0 && attrs & FILE_ATTRIBUTE_SYSTEM.0 != 0
}

fn is_hidden(attrs: u32) -> bool {
    attrs & FILE_ATTRIBUTE_HIDDEN.0 != 0
}

/// Writes the new attribute when it differs. Ok(true) = changed.
fn set(path: &Path, attrs: u32, hide: bool) -> Result<bool, ()> {
    if is_hidden(attrs) == hide {
        return Ok(false);
    }
    let mut next = if hide { attrs | FILE_ATTRIBUTE_HIDDEN.0 } else { attrs & !FILE_ATTRIBUTE_HIDDEN.0 };
    // Only attributes Windows lets programs set are passed; the rest are ignored by SetFileAttributes anyway.
    next &= !(FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0);
    if next == 0 {
        next = FILE_ATTRIBUTE_NORMAL.0;
    }
    let w = long(path);
    unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(next)) }.map(|_| true).map_err(|_| ())
}

fn display(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Hides (or unhides) each chosen item; for folders, everything under them too.
/// Files are always changed; `include_self` decides whether a chosen folder itself changes.
/// Windows' protected items (hidden + system, e.g. desktop.ini) are never touched.
pub fn apply(roots: &[PathBuf], hide: bool, include_self: bool) -> Outcome {
    let mut out = Outcome::default();
    for chosen in roots {
        // One clean full spelling ("C:/a/b." -> "C:\a\b"), as the long-path calls below need.
        let root = std::path::absolute(chosen).unwrap_or_else(|_| chosen.clone());
        if let Some(why) = off_limits(&root) {
            out.problems.push(format!("'{}'은(는) {why}라 바꾸지 않았습니다.", chosen.display()));
            continue;
        }
        let Some(me) = stat(&root) else {
            out.problems.push(format!("'{}'을(를) 찾을 수 없습니다.", display(chosen)));
            continue;
        };
        if protected(me.attrs) {
            continue;
        }
        let is_dir = me.attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
        if !is_dir || include_self {
            out.change(root.clone(), me.attrs, hide);
        }
        if !is_dir || me.link {
            continue;
        }
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let mut sub = Vec::new();
            let listed = list(&dir, |name, e| {
                if protected(e.attrs) {
                    return; // also not entered: $RECYCLE.BIN, System Volume Information, ...
                }
                let path = dir.join(name);
                if e.attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0 && !e.link {
                    sub.push(path.clone());
                }
                out.change(path, e.attrs, hide);
            });
            if !listed && dir != root {
                out.failed += 1; // a subfolder we may not open
            }
            stack.extend(sub);
        }
    }
    notify(&out.changed);
    out
}

/// Puts back what `apply` changed, but only items still in the state it left them in.
/// Returns how many could not be put back.
pub fn undo(items: &[PathBuf], was_hide: bool) -> usize {
    let mut failed = 0;
    let mut done = Vec::new();
    // Reverse order: folders are restored after their contents, like a real undo.
    for p in items.iter().rev() {
        let w = long(p);
        let attrs = unsafe { GetFileAttributesW(PCWSTR(w.as_ptr())) };
        if attrs == INVALID_FILE_ATTRIBUTES || is_hidden(attrs) != was_hide {
            continue; // gone, or someone changed it since
        }
        if !was_hide && attrs & FILE_ATTRIBUTE_SYSTEM.0 != 0 {
            continue; // now a system item: hiding it would make it a protected OS file
        }
        match set(p, attrs, !was_hide) {
            Ok(true) => done.push(p.clone()),
            Ok(false) => {}
            Err(()) => failed += 1,
        }
    }
    notify(&done);
    failed
}

/// Tells open Explorer windows to show the new state.
fn notify(changed: &[PathBuf]) {
    unsafe {
        if changed.len() <= PER_ITEM_NOTIFY_LIMIT {
            for p in changed {
                let w = wide(p);
                SHChangeNotify(SHCNE_ATTRIBUTES, SHCNF_PATHW, Some(w.as_ptr() as _), None);
            }
            return;
        }
        let mut dirs: Vec<&Path> = changed.iter().filter_map(|p| p.parent()).collect();
        dirs.sort();
        dirs.dedup();
        for d in dirs {
            let w = wide(d);
            SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_PATHW, Some(w.as_ptr() as _), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_tree() -> PathBuf {
        let base = std::env::temp_dir().join(format!("keyscene-tree-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a").join("b")).unwrap();
        std::fs::write(base.join("one.txt"), "1").unwrap();
        std::fs::write(base.join("a").join("two.txt"), "2").unwrap();
        std::fs::write(base.join("a").join("b").join("three.txt"), "3").unwrap();
        base
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("keyscene-tree-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn attrs(p: &Path) -> u32 {
        let w = long(&std::path::absolute(p).unwrap());
        unsafe { GetFileAttributesW(PCWSTR(w.as_ptr())) }
    }

    fn set_attrs(p: &Path, a: u32) {
        let w = long(&std::path::absolute(p).unwrap());
        unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(a)) }.unwrap();
    }

    fn hidden(p: &Path) -> bool {
        is_hidden(attrs(p))
    }

    fn junction(link: &Path, target: &Path) {
        let out = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
        assert!(out.status.success(), "mklink failed");
    }

    #[test]
    fn hide_unhide_and_undo() {
        let base = temp_tree();
        let all = [base.join("one.txt"), base.join("a"), base.join("a/two.txt"), base.join("a/b"), base.join("a/b/three.txt")];

        // Without the folder itself.
        let out = apply(&[base.clone()], true, false);
        assert_eq!(out.changed.len(), 5);
        assert!(out.message().is_none());
        assert!(all.iter().all(|p| hidden(p)));
        assert!(!hidden(&base));

        // Second run changes nothing: already hidden items are skipped.
        assert_eq!(apply(&[base.clone()], true, true).changed, vec![base.clone()]);

        // Unhide everything, then undo puts the hidden attribute back.
        let out = apply(&[base.clone()], false, true);
        assert_eq!(out.changed.len(), 6);
        assert!(!hidden(&base) && all.iter().all(|p| !hidden(p)));
        assert_eq!(undo(&out.changed, false), 0);
        assert!(hidden(&base) && all.iter().all(|p| hidden(p)));

        // Clean up: unhide then delete.
        apply(&[base.clone()], false, true);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn refuses_system_places() {
        let out = apply(&[PathBuf::from("C:\\")], true, true);
        assert!(out.changed.is_empty() && out.problems.len() == 1);
        let win = PathBuf::from(std::env::var("SystemRoot").unwrap()).join("Temp");
        assert!(apply(&[win], true, true).changed.is_empty());
        let profile = PathBuf::from(std::env::var("USERPROFILE").unwrap());
        assert!(apply(&[profile], false, true).changed.is_empty());
        assert_eq!(count(1234567), "1,234,567");
    }

    fn refused(p: &str) -> Option<&'static str> {
        off_limits(&std::path::absolute(p).unwrap())
    }

    #[test]
    fn refuses_other_spellings() {
        let win = std::env::var("SystemRoot").unwrap();
        let pf = std::env::var("ProgramFiles").unwrap();
        let profile = std::env::var("USERPROFILE").unwrap();
        assert!(refused(&pf.replace('\\', "/")).is_some());
        assert!(refused(&format!("{win}.")).is_some());
        assert!(refused(&format!(r"{win}\System32\..")).is_some());
        assert!(refused(&format!(r"\\?\{win}")).is_some());
        assert!(refused(&format!(r"\\?\{win}\Temp")).is_some());
        assert_eq!(refused(&format!(r"{profile}\.")), Some("사용자 폴더 전체"));
        assert_eq!(refused("C:/"), Some("드라이브 전체"));
        assert_eq!(refused(r"\\?\C:\"), Some("드라이브 전체"));
        // 8.3 short name, if the drive still makes them.
        let w: Vec<u16> = pf.encode_utf16().chain(Some(0)).collect();
        let mut buf = [0u16; 260];
        let n = unsafe { windows::Win32::Storage::FileSystem::GetShortPathNameW(PCWSTR(w.as_ptr()), Some(&mut buf)) } as usize;
        let short = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        if n > 0 && !short.eq_ignore_ascii_case(&pf) {
            assert!(refused(&short).is_some(), "{short}");
        }
        // Ordinary folders inside the profile are fine.
        assert_eq!(refused(&std::env::temp_dir().to_string_lossy()), None);
    }

    #[test]
    fn refuses_a_junction_into_windows() {
        let base = temp_dir("junction-win");
        let link = base.join("win");
        junction(&link, Path::new(&std::env::var("SystemRoot").unwrap()));
        let through = off_limits(&link).is_some() && off_limits(&link.join("Temp")).is_some();
        std::fs::remove_dir(&link).unwrap(); // removes the junction only
        std::fs::remove_dir_all(&base).unwrap();
        assert!(through);
    }

    #[test]
    fn links_change_only_themselves() {
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_READONLY;
        let base = temp_dir("links");
        let (target, parent, link) = (base.join("target"), base.join("parent"), base.join("parent").join("link"));
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::write(target.join("inside.txt"), "x").unwrap();
        set_attrs(&target, FILE_ATTRIBUTE_READONLY.0);
        junction(&link, &target);
        let before = attrs(&target);
        assert!(before & FILE_ATTRIBUTE_READONLY.0 != 0);

        let hid = apply(&[parent.clone()], true, false);
        let after_hide = (attrs(&target), hidden(&link), hidden(&target.join("inside.txt")));
        let shown = apply(&[parent.clone()], false, false);
        let after_unhide = attrs(&target);

        std::fs::remove_dir(&link).unwrap();
        set_attrs(&target, FILE_ATTRIBUTE_NORMAL.0);
        std::fs::remove_dir_all(&base).unwrap();

        // The link itself is hidden; the folder it points to and its contents are untouched.
        assert_eq!(hid.changed, vec![link.clone()]);
        assert_eq!(after_hide, (before, true, false));
        assert_eq!(shown.changed, vec![link]);
        assert_eq!(after_unhide, before);
    }

    #[test]
    fn system_items_stay_visible() {
        let base = temp_dir("system");
        let (sys, plain) = (base.join("sys.txt"), base.join("plain.txt"));
        std::fs::write(&sys, "s").unwrap();
        std::fs::write(&plain, "p").unwrap();
        set_attrs(&sys, FILE_ATTRIBUTE_SYSTEM.0);

        let out = apply(&[base.clone()], true, false);
        assert_eq!(out.changed, vec![plain.clone()]);
        assert_eq!(out.skipped, 1);
        assert!(out.message().unwrap().contains("시스템"));
        assert!(!hidden(&sys));
        assert_eq!(apply(&[sys.clone()], true, true).skipped, 1);
        assert!(!hidden(&sys));

        set_attrs(&sys, FILE_ATTRIBUTE_NORMAL.0);
        set_attrs(&plain, FILE_ATTRIBUTE_NORMAL.0);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn long_paths() {
        let base = temp_dir("long");
        let mut deep = base.clone();
        while deep.as_os_str().len() < 300 {
            deep.push("a-fairly-long-folder-name");
        }
        std::fs::create_dir_all(&deep).unwrap();
        let file = deep.join("f.txt");
        std::fs::write(&file, "x").unwrap();

        // Given with forward slashes: cleaned up before the long-path calls.
        let out = apply(&[PathBuf::from(base.to_string_lossy().replace('\\', "/"))], true, false);
        assert!(out.message().is_none(), "{:?}", out.message());
        assert!(hidden(&file));
        assert_eq!(undo(&out.changed, true), 0);
        assert!(!hidden(&file));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
