use flate2::write::GzEncoder;
use flate2::Compression;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest_dir.parent().unwrap();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed=../assets/puw.res");
    println!("cargo:rerun-if-changed=../target/release/puw_notch.exe");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // 1. Compress target/release/puw_notch.exe into OUT_DIR/puw_notch_payload.gz
    let release_exe = root.join("target").join("release").join("puw_notch.exe");
    let payload_path = out_dir.join("puw_notch_payload.gz");
    let mut encoder = GzEncoder::new(
        File::create(&payload_path).expect("failed to create payload file"),
        Compression::best(),
    );
    if release_exe.exists() {
        let bytes = fs::read(&release_exe).expect("failed to read target/release/puw_notch.exe");
        encoder.write_all(&bytes).expect("failed to compress payload");
    }
    encoder.finish().expect("failed to finalize gzip payload");

    // 2. Embed app icon + version info + Common Controls v6 / PerMonitorV2 manifest
    let mut res = fs::read(root.join("assets").join("puw.res")).unwrap_or_default();
    res.extend(resource(16, 1, &version_info()));
    res.extend(resource(24, 1, MANIFEST.as_bytes()));
    let out_res = out_dir.join("puw_setup.res");
    fs::write(&out_res, res).expect("failed to write setup resource");
    println!("cargo:rustc-link-arg-bins={}", out_res.display());
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <assemblyIdentity version="1.0.0.0" processorArchitecture="*" name="erikpuw.PuwNotchSetup" type="win32"/>
  <description>PUW Notch Setup</description>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <asmv3:application>
    <asmv3:windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/PM</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </asmv3:windowsSettings>
  </asmv3:application>
</assembly>
"#;

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
    let part = |k: &str| env::var(k).unwrap().parse::<u32>().unwrap();
    let (major, minor, patch) = (
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH"),
    );
    let (ms, ls) = (major << 16 | minor, patch << 16);
    let fixed: Vec<u8> = [0xFEEF04BD, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let str_node = |k: &str, v: &str| {
        let bytes = utf16z(v);
        node(k, &bytes, v.encode_utf16().count() + 1, true, &[])
    };
    let strings = node(
        "040904B0",
        &[],
        0,
        true,
        &[
            str_node("CompanyName", "erikpuw"),
            str_node("FileDescription", "PUW Notch Setup"),
            str_node("FileVersion", &version),
            str_node("InternalName", "puw_notch_setup"),
            str_node("OriginalFilename", "puw_notch_setup.exe"),
            str_node("ProductName", "PUW Notch"),
            str_node("ProductVersion", &version),
        ],
    );
    let sfi = node("StringFileInfo", &[], 0, true, &[strings]);
    let trans = [0x0409u16, 0x04B0u16].into_iter().flat_map(u16::to_le_bytes).collect::<Vec<_>>();
    let vfi = node("VarFileInfo", &[], 0, true, &[node("Translation", &trans, trans.len(), false, &[])]);
    node("VS_VERSION_INFO", &fixed, fixed.len(), false, &[sfi, vfi])
}
