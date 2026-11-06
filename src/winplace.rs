//! Settings window: remember where it was, and pull it to a screen edge when it is dragged close.
//! Nothing runs in the background: the snap works inside Windows' own "window is moving" message,
//! and the rectangle is read only when a move/resize ends or the window closes.

use crate::config::WindowRect;
use std::cell::Cell;
use std::sync::OnceLock;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, IsIconic, SetWindowPos, HTCAPTION, SWP_NOACTIVATE, SWP_NOZORDER, WM_ENTERSIZEMOVE,
    WM_EXITSIZEMOVE, WM_MOVING, WM_NCDESTROY, WM_NCLBUTTONDOWN,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_LBUTTON};

/// How close (at 100% scale) a window edge must come to a screen edge before it sticks.
const SNAP_PX: i32 = 12;
const SUBCLASS_ID: usize = 0x6b73_636e; // "kscn"

thread_local! {
    /// Window rectangle and cursor when the current drag began (window thread only).
    static DRAG_START: Cell<Option<(RECT, POINT)>> = const { Cell::new(None) };
    /// Where the title bar was pressed. Windows starts the move only after the mouse has
    /// travelled a few pixels, so this, not the cursor at that moment, is where the drag began.
    static PRESSED_AT: Cell<Option<POINT>> = const { Cell::new(None) };
}

/// Called after the user finishes moving or resizing the window.
static ON_PLACED: OnceLock<Box<dyn Fn(WindowRect) + Send + Sync>> = OnceLock::new();

pub fn on_placed(f: impl Fn(WindowRect) + Send + Sync + 'static) {
    let _ = ON_PLACED.set(Box::new(f));
}

/// The window's rectangle on screen (physical pixels). None while minimized.
pub fn current(hwnd: isize) -> Option<WindowRect> {
    let hwnd = HWND(hwnd as _);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).ok()?;
        Some(WindowRect { left: r.left, top: r.top, right: r.right, bottom: r.bottom })
    }
}

/// Puts the (still hidden) window at `saved`, pulled fully onto a screen if that monitor is gone
/// or the layout changed. Call on the window's own thread.
pub fn restore(hwnd: isize, saved: WindowRect) {
    let hwnd = HWND(hwnd as _);
    let r = fit_on_screen(RECT { left: saved.left, top: saved.top, right: saved.right, bottom: saved.bottom });
    let place = || unsafe {
        let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
    };
    place();
    // Landing on a monitor with another scale makes Windows rescale the window once; set the saved size again.
    place();
}

/// Starts the edge snap and the end-of-move report for this window. Call on the window's own thread.
pub fn attach(hwnd: isize) {
    unsafe {
        let _ = SetWindowSubclass(HWND(hwnd as _), Some(subclass), SUBCLASS_ID, 0);
    }
}

unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
        WM_NCLBUTTONDOWN if wp.0 == HTCAPTION as usize => {
            // Screen coordinates, signed (a monitor left of the main one has negative x).
            let (x, y) = (lp.0 as u32 as u16 as i16 as i32, (lp.0 as u32 >> 16) as u16 as i16 as i32);
            PRESSED_AT.set(Some(POINT { x, y }));
        }
        WM_ENTERSIZEMOVE => {
            let (mut r, mut c) = (RECT::default(), POINT::default());
            let pressed = PRESSED_AT.take();
            let mouse_down = GetKeyState(VK_LBUTTON.0 as i32) < 0;
            if GetWindowRect(hwnd, &mut r).is_ok() && GetCursorPos(&mut c).is_ok() {
                // A keyboard move (Alt+Space) has no press; it starts from the cursor.
                DRAG_START.set(Some((r, if mouse_down { pressed.unwrap_or(c) } else { c })));
            }
        }
        WM_MOVING => {
            let rc = &mut *(lp.0 as *mut RECT);
            // Windows moves the window step by step from where it last was. A snapped position
            // would then be the start of every next small step and the window could never leave
            // the edge, so the place is worked out from where the drag began instead.
            let mut cursor = POINT::default();
            if let (Some((start, from)), true) = (DRAG_START.get(), GetCursorPos(&mut cursor).is_ok()) {
                *rc = follow(start, from, cursor, rc);
            }
            snap(hwnd, rc);
            return LRESULT(1);
        }
        WM_EXITSIZEMOVE => {
            DRAG_START.set(None);
            if let (Some(f), Some(r)) = (ON_PLACED.get(), current(hwnd.0 as isize)) {
                f(r);
            }
        }
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

/// Moves `rc` (where the window is about to go) so a visible edge that is close to the
/// work area's edge lines up with it exactly.
unsafe fn snap(hwnd: HWND, rc: &mut RECT) {
    // Windows 10/11 windows have invisible resize borders; line up the visible frame instead.
    let (mut win, mut frame) = (RECT::default(), RECT::default());
    if GetWindowRect(hwnd, &mut win).is_err()
        || DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut _ as _, size_of::<RECT>() as u32)
            .is_err()
    {
        frame = win;
    }
    let visible = RECT {
        left: rc.left + (frame.left - win.left),
        top: rc.top + (frame.top - win.top),
        right: rc.right - (win.right - frame.right),
        bottom: rc.bottom - (win.bottom - frame.bottom),
    };
    let Some(work) = work_area(&visible) else { return };
    let reach = SNAP_PX * GetDpiForWindow(hwnd).max(96) as i32 / 96;
    let (dx, dy) = snap_offset(&visible, &work, reach);
    rc.left += dx;
    rc.right += dx;
    rc.top += dy;
    rc.bottom += dy;
}

/// Where the window goes when the cursor moved from `from` to `cursor`, with the size Windows
/// proposes in `now` (it changes when the window crosses to a monitor with another scale).
/// The cursor keeps the same relative spot on the window.
fn follow(start: RECT, from: POINT, cursor: POINT, now: &RECT) -> RECT {
    let (sw, sh) = ((start.right - start.left).max(1), (start.bottom - start.top).max(1));
    let (w, h) = (now.right - now.left, now.bottom - now.top);
    let left = cursor.x - ((from.x - start.left) as i64 * w as i64 / sw as i64) as i32;
    let top = cursor.y - ((from.y - start.top) as i64 * h as i64 / sh as i64) as i32;
    RECT { left, top, right: left + w, bottom: top + h }
}

/// How far to shift `r` so its nearest edge within `reach` meets the matching edge of `work`.
fn snap_offset(r: &RECT, work: &RECT, reach: i32) -> (i32, i32) {
    let pick = |a: i32, b: i32| if (a - b).abs() <= reach { Some(b - a) } else { None };
    let dx = pick(r.left, work.left).or_else(|| pick(r.right, work.right)).unwrap_or(0);
    let dy = pick(r.top, work.top).or_else(|| pick(r.bottom, work.bottom)).unwrap_or(0);
    (dx, dy)
}

fn work_area(r: &RECT) -> Option<RECT> {
    unsafe {
        let mon = MonitorFromRect(r, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(mon, &mut info).as_bool().then_some(info.rcWork)
    }
}

/// The window's invisible resize border may hang past the screen edge (a window snapped to an
/// edge does exactly that); this allows for it at up to 200% scale.
const BORDER_SLACK: i32 = 16;

/// Keeps the window on the nearest monitor's work area (top-left wins if it is too big).
fn fit_on_screen(r: RECT) -> RECT {
    let Some(work) = work_area(&r) else { return r };
    place_on(r, &work)
}

/// Leaves a window whose border only just hangs over an edge alone; anything further out
/// (a monitor that is gone, a changed layout) comes fully inside.
fn place_on(r: RECT, work: &RECT) -> RECT {
    if fit_inside(r, work, BORDER_SLACK) == r {
        r
    } else {
        fit_inside(r, work, 0)
    }
}

fn fit_inside(r: RECT, work: &RECT, slack: i32) -> RECT {
    // A window taller or wider than the screen (small laptop screen, high scale) is shrunk to
    // fit, or its title bar or bottom would be out of reach.
    let w = (r.right - r.left).min(work.right - work.left + 2 * slack);
    let h = (r.bottom - r.top).min(work.bottom - work.top + 2 * slack);
    let left = r.left.min(work.right + slack - w).max(work.left - slack);
    let top = r.top.min(work.bottom + slack - h).max(work.top - slack);
    RECT { left, top, right: left + w, bottom: top + h }
}

#[cfg(test)]
mod tests {
    use super::*;
    const WORK: RECT = RECT { left: 0, top: 0, right: 2560, bottom: 1380 };
    fn rect(l: i32, t: i32, w: i32, h: i32) -> RECT {
        RECT { left: l, top: t, right: l + w, bottom: t + h }
    }

    #[test]
    fn snaps_only_within_reach() {
        assert_eq!(snap_offset(&rect(10, 500, 400, 600), &WORK, 15), (-10, 0));
        assert_eq!(snap_offset(&rect(20, 500, 400, 600), &WORK, 15), (0, 0));
        assert_eq!(snap_offset(&rect(2150, 770, 400, 600), &WORK, 15), (10, 10));
        assert_eq!(snap_offset(&rect(-8, -5, 400, 600), &WORK, 15), (8, 5));
    }

    #[test]
    fn small_steps_can_leave_a_snapped_edge() {
        // Window snapped to the left edge; the user drags right 1px at a time.
        let start = rect(0, 300, 400, 600);
        let from = POINT { x: 100, y: 310 };
        let mut placed = start;
        for step in 1..=30 {
            let cursor = POINT { x: 100 + step, y: 310 };
            let mut r = follow(start, from, cursor, &placed);
            let (dx, dy) = snap_offset(&r, &WORK, 15);
            r.left += dx;
            r.right += dx;
            r.top += dy;
            r.bottom += dy;
            placed = r;
        }
        assert_eq!(placed.left, 30, "window should have come off the edge");
    }

    #[test]
    fn follow_keeps_cursor_spot_when_size_changes() {
        let start = rect(0, 0, 400, 600);
        let r = follow(start, POINT { x: 200, y: 20 }, POINT { x: 1000, y: 500 }, &rect(0, 0, 500, 750));
        assert_eq!((r.left, r.top, r.right - r.left), (750, 475, 500));
    }

    #[test]
    fn pulls_window_back_on_screen() {
        let r = place_on(rect(-3000, 200, 500, 700), &WORK);
        assert_eq!((r.left, r.top, r.right - r.left), (0, 200, 500));
        let r = place_on(rect(2400, 1300, 500, 700), &WORK);
        assert_eq!((r.left, r.top), (2060, 680));
        let r = place_on(rect(100, 100, 500, 700), &WORK);
        assert_eq!((r.left, r.top), (100, 100));
        // Snapped to an edge: only the invisible border is off screen, so it stays put.
        let r = place_on(rect(-8, 0, 593, 847), &WORK);
        assert_eq!((r.left, r.top), (-8, 0));
        let r = place_on(rect(1975, 541, 593, 847), &WORK);
        assert_eq!((r.left, r.top), (1975, 541));
    }

    #[test]
    fn shrinks_window_taller_than_screen() {
        // 1366x768 laptop at 125%: the work area is shorter than the window.
        let small = RECT { left: 0, top: 0, right: 1366, bottom: 728 };
        let r = place_on(rect(400, -20, 590, 840), &small);
        assert_eq!((r.top, r.bottom), (0, 728));
        assert_eq!(r.right - r.left, 590);
    }
}
