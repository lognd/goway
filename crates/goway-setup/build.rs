//! Chooses the embedded `goway.exe` payload.
//!
//! `GOWAY_PAYLOAD` names the file to embed (set by `scripts/windows/build.sh`). When it is
//! unset an empty placeholder is embedded, so the crate still builds and tests on any host;
//! such a build refuses to install.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=GOWAY_PAYLOAD");
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
}
