//! Speaker volume and microphone mute state through Core Audio. Needs COM initialised on the
//! calling thread. Both are watched like Nusa does: Windows calls `VolumeWatch` on every change from
//! any source (keys, the taskbar slider, apps), so their levels are never polled; only which device is
//! the default is looked at now and then.

use windows::core::implement;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl, IAudioMeterInformation};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_VOLUME_MUTE, VK_VOLUME_UP};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted on every speaker volume / mute change. WPARAM = percent, LPARAM = 1 when muted.
pub const WM_VOLUME: u32 = WM_APP + 4;
/// Same for the default microphone; LPARAM = 1 when muted.
pub const WM_MIC: u32 = WM_APP + 14;

/// 0..1 scalar to a whole percent.
fn percent(level: f32) -> u8 {
    (level.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// Runs on a Core Audio thread: hand the new level to the window (as message `.1`) and return.
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

/// A default device being watched: (device id, endpoint, registered callback).
type Watched = Option<(String, IAudioEndpointVolume, IAudioEndpointVolumeCallback)>;

pub struct Audio {
    devices: IMMDeviceEnumerator,
    speaker: Watched,
    mic: Watched,
    /// The default speaker's peak meter (made on first use, again when the default speaker changes).
    meter: Option<IAudioMeterInformation>,
}

/// The height of the pill's equaliser from the speaker's peak: scaled by the loudest of the last
/// seconds (a low volume still moves the bars (down to a peak of 0.15)), quick to rise and slow to fall.
#[derive(Default)]
pub struct Level {
    loudest: f32,
    target: f32,
    now: f32,
}

impl Level {
    /// The speaker's peak right now (0..1), read every so often.
    pub fn feed(&mut self, peak: f32) {
        self.loudest = peak.max(self.loudest * 0.99).max(0.15);
        self.target = (peak / self.loudest).min(1.0);
    }

    /// The peak cannot be read: a steady middle height.
    pub fn steady(&mut self) {
        self.target = 0.75;
    }

    /// Nothing plays: sink to the lowest.
    pub fn hush(&mut self) {
        self.target = 0.0;
    }

    /// Move `dt` seconds towards the target; the height 0..1.
    pub fn step(&mut self, dt: f32) -> f32 {
        let tau = if self.target > self.now { 0.04 } else { 0.12 };
        self.now += (self.target - self.now) * (1.0 - (-dt / tau).exp());
        self.now
    }
}

impl Audio {
    pub fn new() -> Option<Self> {
        let devices = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.ok()?;
        Some(Audio { devices, speaker: None, mic: None, meter: None })
    }

    fn endpoint(&self, flow: EDataFlow) -> Option<IAudioEndpointVolume> {
        unsafe {
            let dev = self.devices.GetDefaultAudioEndpoint(flow, eMultimedia).ok()?;
            dev.Activate(CLSCTX_ALL, None).ok()
        }
    }

    /// Watch the default device of `flow` in `slot`, moving the callback over when the default changes
    /// (headphones plugged in). Cheap when nothing changed. True when the default is another device
    /// than before.
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
        let moved = Self::follow(&self.devices, &mut self.speaker, hwnd, eRender, WM_VOLUME);
        if moved {
            self.meter = None;
        }
        moved
    }

    /// The peak (0..1) of what the default speaker plays right now; None when it cannot be read.
    pub fn peak(&mut self) -> Option<f32> {
        if self.meter.is_none() {
            let dev = unsafe { self.devices.GetDefaultAudioEndpoint(eRender, eMultimedia) }.ok()?;
            self.meter = unsafe { dev.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None) }.ok();
        }
        let peak = unsafe { self.meter.as_ref()?.GetPeakValue() }.ok();
        if peak.is_none() {
            self.meter = None;
        }
        peak
    }

    pub fn watch_mic(&mut self, hwnd: HWND) -> bool {
        Self::follow(&self.devices, &mut self.mic, hwnd, eCapture, WM_MIC)
    }

    /// Act on a volume key the way Windows does (its step size; up/down also unmute). The
    /// speaker callback then reports the change, so the notch shows it.
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

    /// Muted state of the default microphone; None when there is no mic. Read once when the mic is
    /// (re)found: after that `WM_MIC` reports every change.
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
    fn the_equaliser_follows_the_loudness_whatever_the_volume() {
        let settle = |level: &mut Level| (0..10).map(|_| level.step(0.05)).last().unwrap();
        let mut loud = Level::default();
        loud.feed(0.9);
        let mut quiet = Level::default();
        quiet.feed(0.2);
        assert!(settle(&mut loud) > 0.9 && settle(&mut quiet) > 0.9, "the first reading is the loudest so far: full height at any volume above the floor");
        loud.feed(0.45);
        assert!(settle(&mut loud) < 0.6, "half the recent loudest is half height");
        loud.hush();
        assert!(settle(&mut loud) < 0.3, "it sinks when nothing plays");
        let mut rise = Level::default();
        rise.feed(1.0);
        let up = rise.step(0.05);
        rise.hush();
        let down = up - rise.step(0.05);
        assert!(up > down, "quick to rise, slow to fall: {up} up, {down} down in the same time");
    }

    #[test]
    fn percent_rounds_and_clamps() {
        assert_eq!(percent(0.724), 72);
        assert_eq!(percent(0.725), 73);
        assert_eq!(percent(1.2), 100);
        assert_eq!(percent(-0.1), 0);
    }
}
