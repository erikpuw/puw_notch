use windows::core::implement;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_VOLUME_MUTE, VK_VOLUME_UP};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_VOLUME: u32 = WM_APP + 4;
pub const WM_MIC: u32 = WM_APP + 14;

fn percent(level: f32) -> u8 {
    (level.clamp(0.0, 1.0) * 100.0).round() as u8
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeWatch(isize, u32);

impl IAudioEndpointVolumeCallback_Impl for VolumeWatch_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        if let Some(d) = unsafe { data.as_ref() } {
            let msg = (WPARAM(percent(d.fMasterVolume) as usize), LPARAM(d.bMuted.as_bool() as isize));
            unsafe { let _ = PostMessageW(Some(HWND(self.0 as _)), self.1, msg.0, msg.1); }
        }
        Ok(())
    }
}

type Watched = Option<(String, IAudioEndpointVolume, IAudioEndpointVolumeCallback)>;

pub struct Audio {
    devices: IMMDeviceEnumerator,
    speaker: Watched,
    mic: Watched,
}

impl Audio {
    pub fn new() -> Option<Self> {
        let devices = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.ok()?;
        Some(Audio { devices, speaker: None, mic: None })
    }

    fn endpoint(&self, flow: EDataFlow) -> Option<IAudioEndpointVolume> {
        unsafe {
            let dev = self.devices.GetDefaultAudioEndpoint(flow, eMultimedia).ok()?;
            dev.Activate(CLSCTX_ALL, None).ok()
        }
    }

    fn follow(devices: &IMMDeviceEnumerator, slot: &mut Watched, hwnd: HWND, flow: EDataFlow, msg: u32) -> bool {
        unsafe {
            let Ok(dev) = devices.GetDefaultAudioEndpoint(flow, eMultimedia) else { return false };
            let Ok(raw) = dev.GetId() else { return false };
            let id = raw.to_string().unwrap_or_default();
            CoTaskMemFree(Some(raw.0 as _));
            if slot.as_ref().is_some_and(|w| w.0 == id) {
                return false;
            }
            let moved = slot.is_some();
            if let Some((_, ep, cb)) = slot.take() {
                let _ = ep.UnregisterControlChangeNotify(&cb);
            }
            let Ok(ep) = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) else { return false };
            let cb: IAudioEndpointVolumeCallback = VolumeWatch(hwnd.0 as isize, msg).into();
            if ep.RegisterControlChangeNotify(&cb).is_ok() {
                *slot = Some((id, ep, cb));
            }
            moved
        }
    }

    pub fn watch_speaker(&mut self, hwnd: HWND) -> bool {
        Self::follow(&self.devices, &mut self.speaker, hwnd, eRender, WM_VOLUME)
    }

    pub fn watch_mic(&mut self, hwnd: HWND) -> bool {
        Self::follow(&self.devices, &mut self.mic, hwnd, eCapture, WM_MIC)
    }

    pub fn volume_key(&self, vk: u16) {
        let Some(ep) = self.endpoint(eRender) else { return };
        unsafe {
            if vk == VK_VOLUME_MUTE.0 {
                let muted = ep.GetMute().map_or(false, |m| m.as_bool());
                let _ = ep.SetMute(!muted, std::ptr::null());
                return;
            }
            let _ = ep.SetMute(false, std::ptr::null());
            let _ = if vk == VK_VOLUME_UP.0 { ep.VolumeStepUp(std::ptr::null()) } else { ep.VolumeStepDown(std::ptr::null()) };
        }
    }

    pub fn mic_muted(&self) -> Option<bool> {
        let ep = self.endpoint(eCapture)?;
        unsafe { Some(ep.GetMute().ok()?.as_bool()) }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        for (_, ep, cb) in [self.speaker.take(), self.mic.take()].into_iter().flatten() {
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
