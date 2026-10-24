//! Speaker volume and microphone mute state through Core Audio. Needs COM initialised on the
//! calling thread. The speaker is watched like Nusa does: Windows calls `VolumeWatch` on every
//! change from any source (keys, the taskbar slider, apps), so nothing is polled.

use windows::core::implement;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted on every speaker volume / mute change. WPARAM = percent, LPARAM = 1 when muted.
pub const WM_VOLUME: u32 = WM_APP + 4;

/// 0..1 scalar to a whole percent.
fn percent(level: f32) -> u8 {
    (level.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// Runs on a Core Audio thread: hand the new level to the window and return.
#[implement(IAudioEndpointVolumeCallback)]
struct VolumeWatch(isize);

impl IAudioEndpointVolumeCallback_Impl for VolumeWatch_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        if let Some(d) = unsafe { data.as_ref() } {
            let msg = (WPARAM(percent(d.fMasterVolume) as usize), LPARAM(d.bMuted.as_bool() as isize));
            unsafe { let _ = PostMessageW(Some(HWND(self.0 as _)), WM_VOLUME, msg.0, msg.1); }
        }
        Ok(())
    }
}

pub struct Audio {
    devices: IMMDeviceEnumerator,
    /// Default speaker being watched: (device id, endpoint, registered callback).
    watched: Option<(String, IAudioEndpointVolume, IAudioEndpointVolumeCallback)>,
}

impl Audio {
    pub fn new() -> Option<Self> {
        let devices = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.ok()?;
        Some(Audio { devices, watched: None })
    }

    fn endpoint(&self, flow: EDataFlow) -> Option<IAudioEndpointVolume> {
        unsafe {
            let dev = self.devices.GetDefaultAudioEndpoint(flow, eMultimedia).ok()?;
            dev.Activate(CLSCTX_ALL, None).ok()
        }
    }

    /// Watch the default speaker, moving the callback over when the default changes
    /// (headphones plugged in). Cheap when nothing changed; called on every check.
    pub fn watch_speaker(&mut self, hwnd: HWND) {
        unsafe {
            let Ok(dev) = self.devices.GetDefaultAudioEndpoint(eRender, eMultimedia) else { return };
            let Ok(raw) = dev.GetId() else { return };
            let id = raw.to_string().unwrap_or_default();
            CoTaskMemFree(Some(raw.0 as _));
            if self.watched.as_ref().is_some_and(|w| w.0 == id) {
                return;
            }
            if let Some((_, ep, cb)) = self.watched.take() {
                let _ = ep.UnregisterControlChangeNotify(&cb);
            }
            let Ok(ep) = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) else { return };
            let cb: IAudioEndpointVolumeCallback = VolumeWatch(hwnd.0 as isize).into();
            if ep.RegisterControlChangeNotify(&cb).is_ok() {
                self.watched = Some((id, ep, cb));
            }
        }
    }

    /// Muted state of the default microphone; None when there is no mic.
    pub fn mic_muted(&self) -> Option<bool> {
        let ep = self.endpoint(eCapture)?;
        unsafe { Some(ep.GetMute().ok()?.as_bool()) }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        if let Some((_, ep, cb)) = self.watched.take() {
            unsafe { let _ = ep.UnregisterControlChangeNotify(&cb); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_rounds_and_clamps() {
        assert_eq!(percent(0.724), 72);
        assert_eq!(percent(0.725), 73);
        assert_eq!(percent(1.2), 100);
        assert_eq!(percent(-0.1), 0);
    }
}
