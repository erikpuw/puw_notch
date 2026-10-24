//! Bluetooth radio on/off through Windows.Devices.Radios. The WinRT calls block, so they run on a
//! short-lived worker thread that posts the result back to the notch window.

use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Posted to the window with WPARAM = `Bt as usize`.
pub const WM_BT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Bt {
    Unknown = 0,
    Off = 1,
    On = 2,
    /// No Bluetooth adapter, or Windows refused access.
    Missing = 3,
}

impl Bt {
    pub fn from_wparam(w: usize) -> Bt {
        match w {
            1 => Bt::Off,
            2 => Bt::On,
            3 => Bt::Missing,
            _ => Bt::Unknown,
        }
    }
}

fn radio() -> windows::core::Result<Option<Radio>> {
    if Radio::RequestAccessAsync()?.get()? != RadioAccessStatus::Allowed {
        return Ok(None);
    }
    let radios = Radio::GetRadiosAsync()?.get()?;
    for r in radios {
        if r.Kind()? == RadioKind::Bluetooth {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

fn state(toggle: bool) -> Bt {
    let Ok(Some(r)) = radio() else { return Bt::Missing };
    let Ok(mut on) = r.State().map(|s| s == RadioState::On) else { return Bt::Missing };
    if toggle {
        let want = if on { RadioState::Off } else { RadioState::On };
        if r.SetStateAsync(want).and_then(|op| op.get()).is_ok() {
            on = !on;
        }
    }
    if on { Bt::On } else { Bt::Off }
}

/// Read (and optionally flip) the radio in the background, then post WM_BT to `hwnd`.
pub fn request(hwnd: HWND, toggle: bool) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let s = state(toggle);
        unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_BT, WPARAM(s as usize), LPARAM(0)); }
    });
}
