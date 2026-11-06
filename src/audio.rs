//! Microphone mute for the default recording devices, kept current by Windows notifications.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;
use std::time::Duration;
use windows::core::{implement, Result as WinResult, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};

/// 0 = no microphone, 1 = on, 2 = muted
static STATE: AtomicU8 = AtomicU8::new(0);
static TX: OnceLock<Sender<Cmd>> = OnceLock::new();

enum Cmd {
    Rebind,
    Changed,
    Set(bool, Sender<Result<(), String>>),
}

pub fn muted() -> Option<bool> {
    match STATE.load(Ordering::SeqCst) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

pub fn set_muted(mute: bool) -> Result<(), String> {
    let tx = TX.get().ok_or("오디오 장치를 준비하지 못했습니다.")?;
    let (reply, wait) = channel();
    tx.send(Cmd::Set(mute, reply)).map_err(|_| "오디오 장치를 준비하지 못했습니다.".to_string())?;
    wait.recv_timeout(Duration::from_secs(3))
        .map_err(|_| "마이크가 응답하지 않습니다.".to_string())?
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeWatcher {
    tx: Sender<Cmd>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeWatcher_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> WinResult<()> {
        let _ = self.tx.send(Cmd::Changed);
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceWatcher {
    tx: Sender<Cmd>,
}

// Callbacks only post a message; the audio thread does the work, so Windows is never blocked here.
impl IMMNotificationClient_Impl for DeviceWatcher_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> WinResult<()> {
        let _ = self.tx.send(Cmd::Rebind);
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> WinResult<()> {
        let _ = self.tx.send(Cmd::Rebind);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> WinResult<()> {
        let _ = self.tx.send(Cmd::Rebind);
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, _role: ERole, _id: &PCWSTR) -> WinResult<()> {
        if flow == eCapture {
            let _ = self.tx.send(Cmd::Rebind);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> WinResult<()> {
        Ok(())
    }
}

/// Starts the audio thread. `on_change` runs whenever the mute state or the device changes.
pub fn start(on_change: impl Fn() + Send + 'static) {
    let (tx, rx) = channel::<Cmd>();
    if TX.set(tx.clone()).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || unsafe {
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                crate::errlog::error("마이크 기능을 시작하지 못했습니다(COM 초기화 실패)");
                return;
            }
            let Ok(devices) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            else {
                crate::errlog::error("마이크 기능을 시작하지 못했습니다(오디오 장치 목록을 열 수 없음)");
                return;
            };
            let device_watcher: IMMNotificationClient = DeviceWatcher { tx: tx.clone() }.into();
            let _ = devices.RegisterEndpointNotificationCallback(&device_watcher);
            let volume_watcher: IAudioEndpointVolumeCallback = VolumeWatcher { tx: tx.clone() }.into();

            // Console default first: its state is the one shown. The communications default is added if different.
            let mut bound: Vec<IAudioEndpointVolume> = Vec::new();
            let rebind = |bound: &mut Vec<IAudioEndpointVolume>| {
                for v in bound.drain(..) {
                    let _ = v.UnregisterControlChangeNotify(&volume_watcher);
                }
                let mut seen: Vec<String> = Vec::new();
                for role in [eConsole, eCommunications] {
                    let Ok(device) = devices.GetDefaultAudioEndpoint(eCapture, role) else { continue };
                    let id = match device.GetId() {
                        Ok(p) => {
                            let s = p.to_string().unwrap_or_default();
                            CoTaskMemFree(Some(p.0 as _));
                            s
                        }
                        Err(_) => continue,
                    };
                    if seen.contains(&id) {
                        continue;
                    }
                    if let Ok(volume) = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                        let _ = volume.RegisterControlChangeNotify(&volume_watcher);
                        bound.push(volume);
                        seen.push(id);
                    }
                }
            };
            let refresh = |bound: &Vec<IAudioEndpointVolume>| {
                let state = match bound.first().map(|v| v.GetMute()) {
                    Some(Ok(m)) if m.as_bool() => 2,
                    Some(Ok(_)) => 1,
                    _ => 0,
                };
                STATE.store(state, Ordering::SeqCst);
            };

            rebind(&mut bound);
            refresh(&bound);
            on_change();

            while let Ok(cmd) = rx.recv() {
                match cmd {
                    Cmd::Rebind => {
                        // Let a burst of device events (unplugging sends several) settle first.
                        std::thread::sleep(Duration::from_millis(150));
                        rebind(&mut bound);
                    }
                    Cmd::Changed => {}
                    Cmd::Set(mute, reply) => {
                        let result = if bound.is_empty() {
                            Err("연결된 마이크가 없습니다.".to_string())
                        } else if bound.iter().all(|v| v.SetMute(mute, std::ptr::null()).is_ok()) {
                            Ok(())
                        } else {
                            Err("마이크 음소거를 바꾸지 못했습니다.".to_string())
                        };
                        let _ = reply.send(result);
                    }
                }
                let before = STATE.load(Ordering::SeqCst);
                refresh(&bound);
                if STATE.load(Ordering::SeqCst) != before {
                    on_change();
                }
            }
        })
        .ok();
}
