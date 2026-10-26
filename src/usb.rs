//! USB devices plugged in and pulled out: Windows tells the window (WM_DEVICECHANGE) when a device
//! of the USB device interface class arrives or goes; the name comes from the device node, which
//! Windows keeps after the device is gone, so a removed device can be named too.

use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{CM_Get_DevNode_PropertyW, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_PHANTOM, CR_SUCCESS};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_BusReportedDeviceDesc, DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_FriendlyName, DEVPROPTYPE};
use windows::Win32::Foundation::{DEVPROPKEY, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::i18n::tr;

/// The USB device interface class: one interface per physical device.
const USB_DEVICE: GUID = GUID::from_u128(0xA5DCBF10_6530_11D2_901F_00C04FB951ED);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    Arrived,
    Removed,
}

/// Ask Windows to send WM_DEVICECHANGE to `hwnd` for USB devices.
pub fn register(hwnd: HWND) {
    let filter = DEV_BROADCAST_DEVICEINTERFACE_W {
        dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
        dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE.0,
        dbcc_classguid: USB_DEVICE,
        ..Default::default()
    };
    unsafe {
        let _ = RegisterDeviceNotificationW(HANDLE(hwnd.0), &filter as *const _ as *const _, DEVICE_NOTIFY_WINDOW_HANDLE);
    }
}

/// What a WM_DEVICECHANGE says, when it is a USB device arriving or leaving: the change and the
/// device interface path.
pub fn parse_message(wp: WPARAM, lp: LPARAM) -> Option<(Change, String)> {
    let change = match wp.0 as u32 {
        DBT_DEVICEARRIVAL => Change::Arrived,
        DBT_DEVICEREMOVECOMPLETE => Change::Removed,
        _ => return None,
    };
    if lp.0 == 0 {
        return None;
    }
    unsafe {
        let header = &*(lp.0 as *const DEV_BROADCAST_HDR);
        if header.dbch_devicetype != DBT_DEVTYP_DEVICEINTERFACE {
            return None;
        }
        let p = lp.0 as *const DEV_BROADCAST_DEVICEINTERFACE_W;
        if (*p).dbcc_classguid != USB_DEVICE {
            return None;
        }
        // The name runs on past the struct; its length comes from the message's size.
        let offset = std::mem::offset_of!(DEV_BROADCAST_DEVICEINTERFACE_W, dbcc_name);
        let chars = ((*p).dbcc_size as usize).saturating_sub(offset) / 2;
        let name = std::slice::from_raw_parts((p as *const u8).add(offset) as *const u16, chars);
        Some((change, utf16_text(name)))
    }
}

/// One plug can make Windows say the same thing twice: the same device and change again within
/// this long is dropped.
const REPEAT: std::time::Duration = std::time::Duration::from_millis(1500);

pub fn is_repeat(last: Option<&(String, Change, std::time::Instant)>, id: &str, change: Change, now: std::time::Instant) -> bool {
    last.is_some_and(|(last_id, last_change, at)| last_id == id && *last_change == change && now.duration_since(*at) < REPEAT)
}

/// UTF-16 up to the first NUL.
fn utf16_text(units: &[u16]) -> String {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// Device instance id from a device interface path:
/// `\\?\USB#VID_046D&PID_C52B#5&1a2b3c&0&2#{a5dcbf10-...}` -> `USB\VID_046D&PID_C52B\5&1a2b3c&0&2`.
pub fn instance_id(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\?\").or_else(|| path.strip_prefix(r"\??\")).unwrap_or(path);
    let (id, _class) = rest.rsplit_once("#{")?;
    (!id.is_empty()).then(|| id.replace('#', "\\"))
}

/// Root hubs come and go with the controller, not with anything the user plugs in.
pub fn is_root_hub(path: &str) -> bool {
    path.to_ascii_uppercase().contains("ROOT_HUB")
}

/// The name Windows has for the device, best first: what the device reports about itself, its
/// friendly name, its description. Falls back to "USB device".
pub fn device_name(path: &str) -> String {
    let named = instance_id(path).and_then(|id| {
        [&DEVPKEY_Device_BusReportedDeviceDesc, &DEVPKEY_Device_FriendlyName, &DEVPKEY_Device_DeviceDesc]
            .into_iter()
            .find_map(|key| node_text(&id, key))
    });
    named.unwrap_or_else(|| tr("Thiết bị USB", "USB device").to_string())
}

/// A string property of the device node `instance`, if it has one.
fn node_text(instance: &str, key: &DEVPROPKEY) -> Option<String> {
    unsafe {
        let mut node = 0u32;
        let id = HSTRING::from(instance);
        if CM_Locate_DevNodeW(&mut node, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_PHANTOM) != CR_SUCCESS {
            return None;
        }
        let mut kind = DEVPROPTYPE::default();
        let mut buf = [0u8; 512];
        let mut len = buf.len() as u32;
        if CM_Get_DevNode_PropertyW(node, key, &mut kind, Some(buf.as_mut_ptr()), &mut len, 0) != CR_SUCCESS {
            return None;
        }
        let units: Vec<u16> = buf[..len as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = utf16_text(&units);
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_id_from_a_device_interface_path() {
        assert_eq!(
            instance_id(r"\\?\USB#VID_046D&PID_C52B#5&1a2b3c&0&2#{a5dcbf10-6530-11d2-901f-00c04fb951ed}").as_deref(),
            Some(r"USB\VID_046D&PID_C52B\5&1a2b3c&0&2")
        );
        assert_eq!(instance_id(r"\??\USB#VID_0781&PID_5567#4C530001#{a5dcbf10-6530-11d2-901f-00c04fb951ed}").as_deref(), Some(r"USB\VID_0781&PID_5567\4C530001"));
        assert_eq!(instance_id("not a path"), None);
        assert_eq!(instance_id(r"\\?\#{a5dcbf10-6530-11d2-901f-00c04fb951ed}"), None, "no id");
    }

    #[test]
    fn root_hubs_are_not_devices_the_user_plugged() {
        assert!(is_root_hub(r"\\?\USB#ROOT_HUB30#4&1c2d3e4f&0&0#{a5dcbf10-6530-11d2-901f-00c04fb951ed}"));
        assert!(is_root_hub(r"\\?\usb#root_hub20#4&1c2d3e4f&0&0#{a5dcbf10-6530-11d2-901f-00c04fb951ed}"));
        assert!(!is_root_hub(r"\\?\USB#VID_046D&PID_C52B#5&1a2b3c&0&2#{a5dcbf10-6530-11d2-901f-00c04fb951ed}"));
    }

    #[test]
    fn the_same_change_twice_in_a_row_is_one_plug() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let last = ("USB\\A".to_string(), Change::Arrived, t0);
        assert!(!is_repeat(None, "USB\\A", Change::Arrived, t0));
        assert!(is_repeat(Some(&last), "USB\\A", Change::Arrived, t0 + Duration::from_millis(300)));
        assert!(!is_repeat(Some(&last), "USB\\A", Change::Removed, t0 + Duration::from_millis(300)), "pulled out right after");
        assert!(!is_repeat(Some(&last), "USB\\B", Change::Arrived, t0 + Duration::from_millis(300)), "another device");
        assert!(!is_repeat(Some(&last), "USB\\A", Change::Arrived, t0 + Duration::from_secs(5)), "plugged in again later");
    }

    #[test]
    fn utf16_text_stops_at_the_first_nul() {
        let units: Vec<u16> = "USB Receiver".encode_utf16().chain([0, 0x41]).collect();
        assert_eq!(utf16_text(&units), "USB Receiver");
        assert_eq!(utf16_text(&[]), "");
        assert_eq!(utf16_text(&"Bàn phím".encode_utf16().collect::<Vec<_>>()), "Bàn phím");
    }

    #[test]
    fn a_device_that_is_not_there_gets_the_generic_name() {
        assert_eq!(device_name(r"\\?\USB#VID_FFFF&PID_FFFF#nothing-here#{a5dcbf10-6530-11d2-901f-00c04fb951ed}"), tr("Thiết bị USB", "USB device"));
        assert_eq!(device_name("garbage"), tr("Thiết bị USB", "USB device"));
    }
}
