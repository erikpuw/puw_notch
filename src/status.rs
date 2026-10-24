//! Lock keys and screen brightness, for the short status banners.
//! Keyboard backlight has no common Windows API (vendor drivers only), so it is not covered.

use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{w, BSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::System::Wmi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL, VK_NUMLOCK, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Posted after Caps Lock / Num Lock is released, WPARAM = virtual key.
pub const WM_LOCKKEY: u32 = WM_APP + 3;
/// Posted on each volume up / down / mute key press (repeats while held).
pub const WM_VOLKEY: u32 = WM_APP + 4;

static TARGET: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn keyboard_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let vk = (*(lp.0 as *const KBDLLHOOKSTRUCT)).vkCode;
        let msg = match (wp.0 as u32, vk) {
            (WM_KEYUP, v) if v == VK_CAPITAL.0 as u32 || v == VK_NUMLOCK.0 as u32 => Some(WM_LOCKKEY),
            (WM_KEYDOWN, v) if v == VK_VOLUME_UP.0 as u32 || v == VK_VOLUME_DOWN.0 as u32 || v == VK_VOLUME_MUTE.0 as u32 => Some(WM_VOLKEY),
            _ => None,
        };
        if let Some(msg) = msg {
            let hwnd = HWND(TARGET.load(Ordering::Relaxed) as _);
            let _ = PostMessageW(Some(hwnd), msg, WPARAM(vk as usize), LPARAM(0));
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

/// Global low-level keyboard hook; runs on the calling thread's message loop.
/// Only lock and volume keys are looked at; nothing is stored or blocked.
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

/// Built-in screen brightness through WMI (laptops). External monitors usually do not report it.
pub struct Brightness {
    wmi: IWbemServices,
}

impl Brightness {
    pub fn new() -> Option<Self> {
        unsafe {
            let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
            let wmi: IWbemServices = locator.ConnectServer(&BSTR::from("ROOT\\WMI"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None).ok()?;
            // 10 = RPC_C_AUTHN_WINNT, 0 = RPC_C_AUTHZ_NONE (Rpc feature not worth it for two numbers)
            CoSetProxyBlanket(&wmi, 10, 0, None, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE).ok()?;
            let b = Brightness { wmi };
            b.percent()?;
            Some(b)
        }
    }

    pub fn percent(&self) -> Option<u8> {
        unsafe {
            let rows = self
                .wmi
                .ExecQuery(&BSTR::from("WQL"), &BSTR::from("SELECT CurrentBrightness FROM WmiMonitorBrightness"), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None)
                .ok()?;
            let mut row = [None; 1];
            let mut got = 0;
            if rows.Next(WBEM_INFINITE, &mut row, &mut got).is_err() || got == 0 {
                return None;
            }
            let mut value = VARIANT::default();
            row[0].as_ref()?.Get(w!("CurrentBrightness"), 0, &mut value, None, None).ok()?;
            // WMI gives a VT_UI1; the variant helpers convert it to i32.
            i32::try_from(&value).ok().and_then(|v| u8::try_from(v).ok())
        }
    }
}
