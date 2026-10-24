//! Speaker volume and microphone mute state through Core Audio (default devices, looked up on each
//! call so switching headphones just works). Read only. Needs COM initialised on the calling thread.

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

pub struct Audio {
    devices: IMMDeviceEnumerator,
}

impl Audio {
    pub fn new() -> Option<Self> {
        let devices = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.ok()?;
        Some(Audio { devices })
    }

    fn endpoint(&self, flow: EDataFlow) -> Option<IAudioEndpointVolume> {
        unsafe {
            let dev = self.devices.GetDefaultAudioEndpoint(flow, eMultimedia).ok()?;
            dev.Activate(CLSCTX_ALL, None).ok()
        }
    }

    /// (level 0..1, muted) of the default speaker.
    pub fn volume(&self) -> Option<(f32, bool)> {
        let ep = self.endpoint(eRender)?;
        unsafe { Some((ep.GetMasterVolumeLevelScalar().ok()?, ep.GetMute().ok()?.as_bool())) }
    }

    /// Muted state of the default microphone; None when there is no mic.
    pub fn mic_muted(&self) -> Option<bool> {
        let ep = self.endpoint(eCapture)?;
        unsafe { Some(ep.GetMute().ok()?.as_bool()) }
    }
}
