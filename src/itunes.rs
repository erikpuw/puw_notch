//! iTunes COM media source.

use crate::music::{Cmd, Track};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    core::{w, BSTR, GUID, PCWSTR},
    Win32::{
        System::{
            Com::{
                CoCreateInstance, CLSCTX_LOCAL_SERVER, DISPATCH_FLAGS, DISPATCH_METHOD,
                DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS, IDispatch,
            },
            Variant::{VariantClear, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BSTR,
                VT_DISPATCH, VT_I4, VT_I8, VT_R8},
        },
        UI::WindowsAndMessaging::FindWindowW,
    },
};

pub const SOURCE_ID: &str = "iTunes.Application";
const ITUNES_CLSID: GUID = GUID::from_u128(0xdc0c264014154644875c6f4d769839ba);
const MAX_ARTWORK_BYTES: u64 = 8 * 1024 * 1024;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[repr(transparent)]
#[derive(Default)]
struct Variant(VARIANT);

impl Drop for Variant {
    fn drop(&mut self) {
        unsafe { let _ = VariantClear(&mut self.0); }
    }
}

impl Variant {
    fn i32(value: i32) -> Self {
        Self(VARIANT { Anonymous: VARIANT_0 { Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
            vt: VT_I4, wReserved1: 0, wReserved2: 0, wReserved3: 0,
            Anonymous: VARIANT_0_0_0 { lVal: value },
        }) } })
    }

    fn f64(value: f64) -> Self {
        Self(VARIANT { Anonymous: VARIANT_0 { Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
            vt: VT_R8, wReserved1: 0, wReserved2: 0, wReserved3: 0,
            Anonymous: VARIANT_0_0_0 { dblVal: value },
        }) } })
    }

    fn bstr(value: &str) -> Self {
        Self(VARIANT { Anonymous: VARIANT_0 { Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
            vt: VT_BSTR, wReserved1: 0, wReserved2: 0, wReserved3: 0,
            Anonymous: VARIANT_0_0_0 { bstrVal: std::mem::ManuallyDrop::new(BSTR::from(value)) },
        }) } })
    }

    fn as_dispatch(&self) -> Option<IDispatch> {
        unsafe {
            if self.0.Anonymous.Anonymous.vt != VT_DISPATCH { return None; }
            self.0.Anonymous.Anonymous.Anonymous.pdispVal.as_ref().cloned()
        }
    }

    fn as_string(&self) -> Option<String> {
        unsafe {
            (self.0.Anonymous.Anonymous.vt == VT_BSTR)
                .then(|| self.0.Anonymous.Anonymous.Anonymous.bstrVal.to_string())
        }
    }

    fn as_i32(&self) -> Option<i32> {
        unsafe {
            let value = &self.0.Anonymous.Anonymous;
            match value.vt {
                VT_I4 => Some(value.Anonymous.lVal),
                VT_I8 => i32::try_from(value.Anonymous.llVal).ok(),
                _ => None,
            }
        }
    }

    fn as_seconds(&self) -> Option<f64> {
        unsafe {
            let value = &self.0.Anonymous.Anonymous;
            let seconds = match value.vt {
                VT_R8 => value.Anonymous.dblVal,
                VT_I4 => value.Anonymous.lVal as f64,
                VT_I8 => value.Anonymous.llVal as f64,
                _ => return None,
            };
            seconds.is_finite().then_some(seconds.max(0.0))
        }
    }
}

fn invoke(dispatch: &IDispatch, member: &str, flags: DISPATCH_FLAGS, args: &mut [Variant]) -> Option<Variant> {
    let name: Vec<u16> = member.encode_utf16().chain(Some(0)).collect();
    let name = PCWSTR(name.as_ptr());
    let mut id = 0;
    unsafe {
        let iid_null = GUID::zeroed();
        dispatch.GetIDsOfNames(&iid_null, &name, 1, 0, &mut id).ok()?;
        let mut result = Variant::default();
        let mut property_put = -3i32; // DISPID_PROPERTYPUT
        let params = DISPPARAMS {
            rgvarg: if args.is_empty() { std::ptr::null_mut() } else { args.as_mut_ptr().cast() },
            rgdispidNamedArgs: if flags == DISPATCH_PROPERTYPUT { &mut property_put } else { std::ptr::null_mut() },
            cArgs: args.len() as u32,
            cNamedArgs: u32::from(flags == DISPATCH_PROPERTYPUT),
        };
        dispatch.Invoke(id, &iid_null, 0, flags, &params, Some(&mut result.0), None, None).ok()?;
        Some(result)
    }
}

fn property(dispatch: &IDispatch, member: &str) -> Option<Variant> {
    invoke(dispatch, member, DISPATCH_PROPERTYGET, &mut [])
}

fn dispatch_property(dispatch: &IDispatch, member: &str) -> Option<IDispatch> {
    property(dispatch, member)?.as_dispatch()
}

fn app() -> Option<IDispatch> {
    unsafe {
        // Only activate the COM server after confirming its verified iTunes window exists.
        FindWindowW(None, w!("iTunes")).ok()?;
        CoCreateInstance::<_, IDispatch>(&ITUNES_CLSID, None, CLSCTX_LOCAL_SERVER).ok()
    }
}

fn seconds_to_ms(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 { (seconds * 1000.0).round() as u64 } else { 0 }
}

pub fn playing() -> Option<bool> {
    property(&app()?, "PlayerState")?.as_i32().map(|state| state == 1)
}

pub fn read() -> Option<Track> {
    let app = app()?;
    let current = dispatch_property(&app, "CurrentTrack")?;
    let title = property(&current, "Name")?.as_string()?.trim().to_owned();
    if title.is_empty() { return None; }
    let artist = property(&current, "Artist")?.as_string()?.trim().to_owned();
    let duration_ms = property(&current, "Duration")?.as_seconds().map(seconds_to_ms)?;
    let position_ms = property(&app, "PlayerPosition")?.as_seconds().map(seconds_to_ms)?;
    let playing = property(&app, "PlayerState")?.as_i32()? == 1;
    Some(Track {
        source_id: SOURCE_ID.to_owned(), title, artist, playing, duration_ms, position_ms,
        start_ticks: 0, rate: 1.0, reported_position: (position_ms as i64).saturating_mul(10_000),
        updated_ticks: 0, at: Some(Instant::now()),
        can_toggle: true, can_next: true, can_previous: true, can_seek: duration_ms > 0,
        ..Track::default()
    })
}

pub fn command(cmd: &Cmd) -> bool {
    let Some(app) = app() else { return false; };
    match cmd {
        Cmd::PlayPause => invoke(&app, "PlayPause", DISPATCH_METHOD, &mut []).is_some(),
        Cmd::Next => invoke(&app, "NextTrack", DISPATCH_METHOD, &mut []).is_some(),
        Cmd::Previous => invoke(&app, "PreviousTrack", DISPATCH_METHOD, &mut []).is_some(),
        Cmd::Seek(position_ms) => {
            let mut args = [Variant::f64(*position_ms as f64 / 1000.0)];
            invoke(&app, "PlayerPosition", DISPATCH_PROPERTYPUT, &mut args).is_some()
        }
    }
}

struct TempArtwork(PathBuf);

impl Drop for TempArtwork {
    fn drop(&mut self) { let _ = fs::remove_file(&self.0); }
}

pub fn artwork(title: &str, artist: &str) -> Option<Vec<u8>> {
    let app = app()?;
    let track = dispatch_property(&app, "CurrentTrack")?;
    if property(&track, "Name")?.as_string()?.trim() != title
        || property(&track, "Artist")?.as_string()?.trim() != artist { return None; }
    let artwork = dispatch_property(&track, "Artwork")?;
    let mut index = [Variant::i32(1)];
    let item = invoke(&artwork, "Item", DISPATCH_PROPERTYGET, &mut index)?.as_dispatch()?;
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_nanos();
    let path = std::env::temp_dir().join(format!("puwnotch-itunes-{}-{stamp}-{id}.jpg", std::process::id()));
    let cleanup = TempArtwork(path.clone());
    let path_arg = path.to_str()?;
    let mut args = [Variant::bstr(path_arg)];
    invoke(&item, "SaveArtworkToFile", DISPATCH_METHOD, &mut args)?;
    let bytes = fs::metadata(&cleanup.0).ok()?.len();
    if bytes == 0 || bytes > MAX_ARTWORK_BYTES { return None; }
    fs::read(&cleanup.0).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    #[test]
    fn converts_seconds_to_milliseconds() {
        assert_eq!(seconds_to_ms(66.125), 66_125);
        assert_eq!(seconds_to_ms(-1.0), 0);
        assert_eq!(seconds_to_ms(f64::NAN), 0);
    }

    #[test]
    #[ignore = "read-only live iTunes COM check"]
    fn live_read_only_check() {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().expect("initialize test COM"); }
        let track = read();
        let art = track.as_ref().and_then(|track| artwork(&track.title, &track.artist));
        unsafe { CoUninitialize(); }
        let track = track.expect("iTunes must be running with a current track");
        eprintln!("{} | {} | playing={} | position={}ms / {}ms", track.title, track.artist,
            track.playing, track.position_ms, track.duration_ms);
        eprintln!("artwork bytes={}", art.as_ref().map_or(0, Vec::len));
    }
}
