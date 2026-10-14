//! Chooses the embedded `goway.exe` payload and embeds the Windows application manifest.
//!
//! `GOWAY_PAYLOAD` names the file to embed (set by `scripts/windows/build.sh`). When it is
//! unset an empty placeholder is embedded, so the crate still builds and tests on any host;
//! such a build refuses to install.
//!
//! On Windows targets the executable also carries a manifest asking for `asInvoker`. Without
//! one, Windows' installer detection sees "setup" in the file name (`goway-setup.exe`,
//! `goway-setup-x64.exe`) and demands elevation just to start, before the program can even
//! print `--help` or run a `--dry-run`. The real install still elevates through its own prompt.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The manifest embedded in every Windows build of `goway-setup`.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-env-changed=GOWAY_PAYLOAD");
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let chosen = match std::env::var("GOWAY_PAYLOAD") {
        Ok(path) if !path.is_empty() => {
            println!("cargo:rerun-if-changed={path}");
            std::fs::canonicalize(&path)
                .unwrap_or_else(|e| panic!("GOWAY_PAYLOAD {path} is unusable: {e}"))
        }
        _ => {
            let empty = out.join("payload-empty.bin");
            std::fs::write(&empty, []).expect("write empty payload placeholder");
            empty
        }
    };
    println!("cargo:rustc-env=GOWAY_PAYLOAD_FILE={}", chosen.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_manifest(&out);
    }
}

/// Embed [`MANIFEST`] in the executable: through the MSVC linker's own manifest support, or by
/// compiling a resource with mingw-w64 `windres` for the GNU toolchain.
fn embed_manifest(out: &Path) {
    let manifest = out.join("goway-setup.manifest");
    std::fs::write(&manifest, MANIFEST).expect("write the application manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
        return;
    }
    // RT_MANIFEST (24) resource id 1 is where Windows looks for an executable's manifest.
    let rc = out.join("goway-setup.rc");
    let manifest_name = manifest.display().to_string().replace('\\', "/");
    std::fs::write(&rc, format!("1 24 \"{manifest_name}\"\n")).expect("write the resource script");
    let object = out.join("goway-setup-manifest.o");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let tools = std::env::var("WINDRES")
        .ok()
        .into_iter()
        .chain([format!("{arch}-w64-mingw32-windres"), "windres".to_owned()]);
    for tool in tools {
        let run = Command::new(&tool)
            .arg("--input")
            .arg(&rc)
            .arg("--output")
            .arg(&object)
            .arg("--output-format=coff")
            .status();
        if run.is_ok_and(|s| s.success()) {
            println!("cargo:rustc-link-arg-bins={}", object.display());
            return;
        }
    }
    panic!(
        "cannot embed the asInvoker manifest: no working windres found (install mingw-w64 or set WINDRES)"
    );
}
