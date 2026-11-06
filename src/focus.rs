//! Tells us when another window comes to the front, using a Windows event (no polling).

use std::sync::OnceLock;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, EVENT_SYSTEM_FOREGROUND, MSG, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
};

type Sink = Box<dyn Fn(isize) + Send + Sync>;
static SINK: OnceLock<Sink> = OnceLock::new();

unsafe extern "system" fn on_event(_: HWINEVENTHOOK, _event: u32, hwnd: HWND, id_object: i32, _: i32, _: u32, _: u32) {
    if id_object == OBJID_WINDOW.0 {
        if let Some(sink) = SINK.get() {
            sink(hwnd.0 as isize);
        }
    }
}

/// Calls `on_change(hwnd)` each time the foreground window changes. Starts once.
pub fn watch(on_change: impl Fn(isize) + Send + Sync + 'static) {
    if SINK.set(Box::new(on_change)).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("foreground".into())
        .spawn(|| unsafe {
            let hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if hook.is_invalid() {
                crate::errlog::error("앞 창 감시를 시작하지 못했습니다(제외할 앱이 동작하지 않음)");
                return;
            }
            // Out-of-context hooks are delivered through this thread's message loop.
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .ok();
}
