//! Embed the app icon (assets/puw.res, made by assets/make_icon.py) and a VERSIONINFO block in the
//! exe: the MSVC linker takes a compiled .res file as an input as-is. The version block is built
//! here from Cargo.toml so it never drifts; an exe without one looks anonymous to antivirus heuristics.

use std::env::var;

fn main() {
    println!("cargo:rerun-if-changed=assets/puw.res");
    println!("cargo:rerun-if-changed=Cargo.toml");
    let dir = std::path::PathBuf::from(var("CARGO_MANIFEST_DIR").unwrap());
    let mut res = std::fs::read(dir.join("assets").join("puw.res")).unwrap();
    res.extend(resource(16, 1, &version_info()));
    let out = std::path::PathBuf::from(var("OUT_DIR").unwrap()).join("puw.res");
    std::fs::write(&out, res).unwrap();
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// One .res entry with a numeric type and name, US English, data padded to 4 bytes.
fn resource(kind: u16, name: u16, data: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend((data.len() as u32).to_le_bytes());
    b.extend(32u32.to_le_bytes());
    for w in [0xFFFF, kind, 0xFFFF, name] {
        b.extend(w.to_le_bytes());
    }
    b.extend(0u32.to_le_bytes());
    b.extend(0x0030u16.to_le_bytes());
    b.extend(0x0409u16.to_le_bytes());
    b.extend([0; 8]);
    b.extend(data);
    pad4(&mut b);
    b
}

fn pad4(b: &mut Vec<u8>) {
    b.resize(b.len().next_multiple_of(4), 0);
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

/// A version resource node: wLength, wValueLength, wType, key, value, children, each 4-aligned.
fn node(key: &str, value: &[u8], value_len: usize, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0; 6];
    b.extend(utf16z(key));
    pad4(&mut b);
    b.extend(value);
    for c in children {
        pad4(&mut b);
        b.extend(c);
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b[2..4].copy_from_slice(&(value_len as u16).to_le_bytes());
    b[4..6].copy_from_slice(&(text as u16).to_le_bytes());
    b
}

fn version_info() -> Vec<u8> {
    let part = |k: &str| var(k).unwrap().parse::<u32>().unwrap();
    let (major, minor, patch) = (part("CARGO_PKG_VERSION_MAJOR"), part("CARGO_PKG_VERSION_MINOR"), part("CARGO_PKG_VERSION_PATCH"));
    let (ms, ls) = (major << 16 | minor, patch << 16);
    // VS_FIXEDFILEINFO: signature, struct version, file/product versions, flags mask, flags,
    // VOS_NT_WINDOWS32, VFT_APP, subtype, date.
    let fixed: Vec<u8> = [0xFEEF04BD, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0].into_iter().flat_map(u32::to_le_bytes).collect();
    let version = var("CARGO_PKG_VERSION").unwrap();
    let exe = format!("{}.exe", var("CARGO_PKG_NAME").unwrap());
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", "erikpuw"),
        ("FileDescription", "PUW Notch"),
        ("FileVersion", &version),
        ("InternalName", "puw_notch"),
        ("LegalCopyright", "Copyright (c) 2026 erikpuw. MIT License."),
        ("OriginalFilename", &exe),
        ("ProductName", "PUW Notch"),
        ("ProductVersion", &version),
    ]
    .iter()
    .map(|(k, v)| {
        let value = utf16z(v);
        node(k, &value, value.len() / 2, true, &[])
    })
    .collect();
    let table = node("040904B0", &[], 0, true, &strings);
    let string_info = node("StringFileInfo", &[], 0, true, &[table]);
    let translation = [0x0409u16, 0x04B0].into_iter().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
    let var_info = node("VarFileInfo", &[], 0, true, &[node("Translation", &translation, 4, false, &[])]);
    node("VS_VERSION_INFO", &fixed, fixed.len(), false, &[string_info, var_info])
}
