//! What an ecosystem's own configuration says a build needs: for Rust, the
//! linker and the linker backend cargo would use for a target triple, read
//! from every `.cargo/config.toml` cargo reads plus the environment.
//!
//! `cargo config get` is unstable, so the files are parsed with a real TOML
//! parser instead, in cargo's own precedence: a closer file wins for the
//! linker, `RUSTFLAGS` (when set and non-empty) replaces every configured
//! rustflags, otherwise `target.<triple>.rustflags` (or the
//! `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` variable) replaces `build.rustflags`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The largest config file goway reads.
const MAX_CONFIG: u64 = 256 * 1024;

/// The triples checked even when no config names them (the helpers' own).
const DEFAULT_TRIPLES: [&str; 2] = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"];

/// How cargo links for one target triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoLinking {
    /// The target triple.
    pub triple: String,
    /// The linker program (a bare command name), if one is configured.
    pub linker: Option<String>,
    /// The `-fuse-ld=` backend, as the program that provides it (`mold`, `ld.lld`).
    pub backend: Option<String>,
    /// Where the setting came from, for the verdict.
    pub source: String,
}

/// The environment variable name cargo reads for `triple`'s `suffix`.
pub fn target_var(triple: &str, suffix: &str) -> String {
    let upper: String = triple
        .chars()
        .map(|c| {
            if c == '-' || c == '.' {
                '_'
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect();
    format!("CARGO_TARGET_{upper}_{suffix}")
}

/// The `.cargo/config.toml` files cargo reads for a build in `root`, nearest first.
fn config_files(root: &Path, cargo_home: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs: Vec<PathBuf> = root.ancestors().map(|d| d.join(".cargo")).collect();
    if let Some(home) = cargo_home {
        dirs.push(home.to_path_buf());
    }
    for dir in dirs {
        for name in ["config.toml", "config"] {
            let path = dir.join(name);
            if path.is_file() && !out.contains(&path) {
                out.push(path);
                break;
            }
        }
    }
    out
}

/// Parse one config file; `None` when unreadable, too large or not TOML.
fn load(path: &Path) -> Option<toml::Table> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_CONFIG {
        tracing::warn!(file = %path.display(), "cargo config too large; not read");
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    match toml::from_str(&text) {
        Ok(table) => Some(table),
        Err(e) => {
            tracing::warn!(file = %path.display(), error = %e, "cargo config is not valid TOML; ignored");
            None
        }
    }
}

/// A config value that is a string or an array of strings, as words.
fn words(value: &toml::Value) -> Vec<String> {
    match value {
        toml::Value::String(s) => s.split_whitespace().map(str::to_owned).collect(),
        toml::Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// The `-fuse-ld=X` values among rustflags words (`-C link-arg=..`,
/// `-Clink-arg=..`, `-C link-args=..`); the last one wins, like the linker.
fn fuse_ld(flags: &[String]) -> Option<String> {
    let mut found = None;
    let mut i = 0;
    while i < flags.len() {
        let word = flags[i].as_str();
        let codegen = if word == "-C" {
            i += 1;
            flags.get(i).map(String::as_str)
        } else {
            word.strip_prefix("-C")
        };
        if let Some(c) = codegen
            && let Some(args) = c
                .strip_prefix("link-arg=")
                .or_else(|| c.strip_prefix("link-args="))
        {
            for arg in args.split_whitespace() {
                if let Some(x) = arg.strip_prefix("-fuse-ld=") {
                    found = Some(x.to_owned());
                }
            }
        }
        i += 1;
    }
    found
}

/// The program that provides a `-fuse-ld=` backend (`None` for ones that
/// need no extra program or are paths).
fn backend_program(name: &str) -> Option<String> {
    match name {
        "mold" => Some("mold".to_owned()),
        "lld" => Some("ld.lld".to_owned()),
        "gold" => Some("ld.gold".to_owned()),
        "wild" => Some("wild".to_owned()),
        _ => None,
    }
}

/// A linker setting that is a bare, safe command name worth probing.
fn bare_program(value: &str) -> Option<String> {
    let ok = !value.is_empty()
        && value.len() <= 40
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b));
    ok.then(|| value.to_owned())
}

/// How cargo would link for each target triple a config names (and the
/// helpers' own triples), for a build in `root`. `env` reads the
/// environment, `cargo_home` is `$CARGO_HOME` (or `~/.cargo`). Triples with
/// neither a linker nor a backend are left out.
pub fn cargo_linking(
    root: &Path,
    cargo_home: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<CargoLinking> {
    let files: Vec<(PathBuf, toml::Table)> = config_files(root, cargo_home)
        .into_iter()
        .filter_map(|p| load(&p).map(|t| (p, t)))
        .collect();
    let mut triples: BTreeSet<String> = DEFAULT_TRIPLES.iter().map(|t| (*t).to_owned()).collect();
    for (_, table) in &files {
        if let Some(targets) = table.get("target").and_then(toml::Value::as_table) {
            triples.extend(targets.keys().filter(|k| !k.starts_with("cfg(")).cloned());
        }
    }
    let nonempty = |name: &str| env(name).filter(|v| !v.trim().is_empty());
    let mut out = Vec::new();
    for triple in triples {
        // Linker: the environment, then the nearest file that sets it.
        let mut linker = None;
        let mut source = String::new();
        let var = target_var(&triple, "LINKER");
        if let Some(v) = nonempty(&var) {
            linker = bare_program(v.trim());
            source = format!("${var}");
        } else {
            for (path, table) in &files {
                if let Some(v) = table
                    .get("target")
                    .and_then(|t| t.get(&triple))
                    .and_then(|t| t.get("linker"))
                    .and_then(toml::Value::as_str)
                {
                    linker = bare_program(v);
                    source = format!("{} target.{triple}.linker", path.display());
                    break;
                }
            }
        }
        // Rustflags: first of the encoded/plain environment variables, the
        // target's own (variable, then files), then build.rustflags.
        let (flags, flag_source) = if let Some(v) = nonempty("CARGO_ENCODED_RUSTFLAGS") {
            (
                v.split('\u{1f}').map(str::to_owned).collect(),
                "$CARGO_ENCODED_RUSTFLAGS".to_owned(),
            )
        } else if let Some(v) = nonempty("RUSTFLAGS") {
            (
                v.split_whitespace().map(str::to_owned).collect(),
                "$RUSTFLAGS".to_owned(),
            )
        } else if let Some(v) = nonempty(&target_var(&triple, "RUSTFLAGS")) {
            (
                v.split_whitespace().map(str::to_owned).collect(),
                format!("${}", target_var(&triple, "RUSTFLAGS")),
            )
        } else {
            // Config arrays from every file are joined, nearest first.
            let mut target_flags = Vec::new();
            let mut build_flags = Vec::new();
            let mut tsrc = String::new();
            let mut bsrc = String::new();
            for (path, table) in &files {
                if let Some(v) = table
                    .get("target")
                    .and_then(|t| t.get(&triple))
                    .and_then(|t| t.get("rustflags"))
                {
                    target_flags.extend(words(v));
                    tsrc = format!("{} target.{triple}.rustflags", path.display());
                }
                if let Some(v) = table.get("build").and_then(|b| b.get("rustflags")) {
                    build_flags.extend(words(v));
                    bsrc = format!("{} build.rustflags", path.display());
                }
            }
            if target_flags.is_empty() {
                (build_flags, bsrc)
            } else {
                (target_flags, tsrc)
            }
        };
        let backend = fuse_ld(&flags).and_then(|b| backend_program(&b));
        if linker.is_none() && backend.is_none() {
            continue;
        }
        if linker.is_none() || source.is_empty() {
            source = flag_source;
        }
        tracing::debug!(%triple, ?linker, ?backend, %source, "cargo linking");
        out.push(CargoLinking {
            triple,
            linker,
            backend,
            source,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(config: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".cargo")).unwrap();
        std::fs::write(dir.path().join(".cargo/config.toml"), config).unwrap();
        dir
    }

    const FROB: &str = "[target.x86_64-unknown-linux-gnu]\nlinker = \"clang\"\nrustflags = [\"-C\", \"link-arg=-fuse-ld=mold\"]\n\n[target.aarch64-unknown-linux-gnu]\nlinker = \"clang\"\nrustflags = [\"-C\", \"link-arg=-fuse-ld=mold\"]\n";

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn cargo_config_linker_and_fuse_ld_backend_are_read_per_triple() {
        let dir = project(FROB);
        let found = cargo_linking(dir.path(), None, &none);
        assert_eq!(found.len(), 2);
        for l in &found {
            assert_eq!(l.linker.as_deref(), Some("clang"));
            assert_eq!(l.backend.as_deref(), Some("mold"));
        }
    }

    #[test]
    fn a_plain_rustflags_variable_replaces_the_configured_flags() {
        let dir = project(FROB);
        let env = |n: &str| (n == "RUSTFLAGS").then(|| "-C link-arg=-fuse-ld=lld".to_owned());
        let found = cargo_linking(dir.path(), None, &env);
        assert_eq!(found[0].backend.as_deref(), Some("ld.lld"));
    }

    #[test]
    fn a_linker_variable_beats_the_config_and_build_rustflags_apply_to_every_triple() {
        let dir = project("[build]\nrustflags = \"-Clink-arg=-fuse-ld=lld\"\n");
        let env = |n: &str| {
            (n == "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER").then(|| "cc".to_owned())
        };
        let found = cargo_linking(dir.path(), None, &env);
        let x86 = found
            .iter()
            .find(|l| l.triple.starts_with("x86_64"))
            .unwrap();
        assert_eq!(x86.linker.as_deref(), Some("cc"));
        assert_eq!(x86.backend.as_deref(), Some("ld.lld"));
        let arm = found
            .iter()
            .find(|l| l.triple.starts_with("aarch64"))
            .unwrap();
        assert_eq!(arm.linker, None);
    }

    #[test]
    fn a_parent_directorys_config_is_read_and_the_nearest_linker_wins() {
        let parent = project("[target.x86_64-unknown-linux-gnu]\nlinker = \"clang\"\n");
        let child = parent.path().join("sub");
        std::fs::create_dir_all(child.join(".cargo")).unwrap();
        std::fs::write(
            child.join(".cargo/config.toml"),
            "[target.x86_64-unknown-linux-gnu]\nlinker = \"gcc\"\n",
        )
        .unwrap();
        let found = cargo_linking(&child, None, &none);
        assert_eq!(found[0].linker.as_deref(), Some("gcc"));
        let found = cargo_linking(parent.path(), None, &none);
        assert_eq!(found[0].linker.as_deref(), Some("clang"));
    }

    #[test]
    fn paths_and_unknown_backends_are_never_probed() {
        let dir = project(
            "[target.x86_64-unknown-linux-gnu]\nlinker = \"/opt/x/clang\"\nrustflags = [\"-Clink-arg=-fuse-ld=/opt/x/ld\"]\n",
        );
        assert!(cargo_linking(dir.path(), None, &none).is_empty());
    }
}
