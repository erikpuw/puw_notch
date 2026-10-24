//! Lock keys and screen brightness, for the short status banners.
//! Keyboard backlight has no common Windows API (vendor drivers only), so it is not covered.

use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::GUID;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL, VK_NUMLOCK};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Posted after Caps Lock / Num Lock is released, WPARAM = virtual key.
pub const WM_LOCKKEY: u32 = WM_APP + 3;

static TARGET: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn keyboard_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let vk = (*(lp.0 as *const KBDLLHOOKSTRUCT)).vkCode;
        if wp.0 as u32 == WM_KEYUP && (vk == VK_CAPITAL.0 as u32 || vk == VK_NUMLOCK.0 as u32) {
            let hwnd = HWND(TARGET.load(Ordering::Relaxed) as _);
            let _ = PostMessageW(Some(hwnd), WM_LOCKKEY, WPARAM(vk as usize), LPARAM(0));
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

/// Global low-level keyboard hook; runs on the calling thread's message loop.
/// Only Caps Lock / Num Lock are looked at; nothing is stored or blocked.
pub fn watch_lock_keys(hwnd: HWND) {
    TARGET.store(hwnd.0 as isize, Ordering::Relaxed);
    unsafe {
        let _ = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0);
    }
}

/// ("Caps Lock", true) for the key in a WM_LOCKKEY message.
pub fn lock_state(vk: usize) -> (&'static str, bool) {
    let on = unsafe { GetKeyState(vk as i32) } & 1 != 0;
    let name = if vk == VK_CAPITAL.0 as usize { "Caps Lock" } else { "Num Lock" };
    (name, on)
}

/// The display brightness power setting (GUID_DEVICE_POWER_POLICY_VIDEO_BRIGHTNESS). Windows
/// sends PBT_POWERSETTINGCHANGE with the new percent on every change once registered, the way
/// Nusa reads it; external monitors usually do not report it.
pub const VIDEO_BRIGHTNESS: GUID = GUID::from_u128(0xaded5e82_b909_4619_9949_f5d71dac0bcb);

/// Brightness percent from a power setting change, None for other settings.
pub fn brightness_setting(setting: &GUID, data: &[u8]) -> Option<u8> {
    let raw = u32::from_le_bytes(data.get(..4)?.try_into().ok()?);
    (*setting == VIDEO_BRIGHTNESS).then(|| raw.min(100) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_from_power_setting() {
        assert_eq!(brightness_setting(&VIDEO_BRIGHTNESS, &70u32.to_le_bytes()), Some(70));
        assert_eq!(brightness_setting(&VIDEO_BRIGHTNESS, &250u32.to_le_bytes()), Some(100), "clamped");
        assert_eq!(brightness_setting(&VIDEO_BRIGHTNESS, &[70]), None, "too short");
        assert_eq!(brightness_setting(&windows::core::GUID::zeroed(), &70u32.to_le_bytes()), None, "other setting");
    }
}
