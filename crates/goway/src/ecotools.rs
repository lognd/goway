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

/// The longest an ecosystem tool may run for one question.
const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The most output read from an ecosystem tool.
const MAX_OUTPUT: u64 = 1024 * 1024;

/// What a successful tool run printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Printed {
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

/// Read at most [`MAX_OUTPUT`] bytes of a stream on its own thread.
fn drain<R: std::io::Read + Send + 'static>(stream: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        use std::io::Read as _;
        let mut text = String::new();
        let mut bytes = Vec::new();
        let _ = stream.take(MAX_OUTPUT).read_to_end(&mut bytes);
        text.push_str(&String::from_utf8_lossy(&bytes));
        text
    })
}

/// Run `program args` (no shell) in `dir` with `envs` added, for at most
/// [`TOOL_TIMEOUT`], reading at most [`MAX_OUTPUT`] bytes per stream. `None`
/// when it is missing, fails, or is too slow (then it is killed).
pub fn bounded_output(
    program: &str,
    args: &[&str],
    dir: Option<&Path>,
    envs: &[(&str, &str)],
) -> Option<Printed> {
    use crate::spawn::CommandExt as _;
    use std::process::{Command, Stdio};
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command.spawn_locked().ok()?;
    let out = drain(child.stdout.take()?);
    let err = drain(child.stderr.take()?);
    let deadline = std::time::Instant::now() + TOOL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                tracing::warn!(program, "ecosystem tool timed out; killed");
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let (stdout, stderr) = (out.join().ok()?, err.join().ok()?);
    status
        .filter(std::process::ExitStatus::success)
        .map(|_| Printed { stdout, stderr })
}

/// What an ecosystem's own answer says a project needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    /// The version, as numbers.
    pub min: Vec<u64>,
    /// Where it came from, shown with the verdict.
    pub why: String,
    /// Read from text, not from the tool or a parser.
    pub approximate: bool,
}

/// `1.74`, `1.74.1` or `17` as numbers.
fn dotted(text: &str) -> Option<Vec<u64>> {
    let t = text.trim().trim_matches('"');
    let parts: Option<Vec<u64>> = t.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

/// The newest of several versions.
fn newest(versions: impl IntoIterator<Item = Vec<u64>>) -> Option<Vec<u64>> {
    versions.into_iter().max()
}

/// The Rust version a project declares: the greatest `rust-version` among
/// the packages `cargo metadata --no-deps --offline` lists. When cargo is
/// missing, the root `Cargo.toml` is parsed instead and the answer says to
/// install cargo first (members' versions are not seen).
pub fn rust_min(root: &Path) -> Option<Detected> {
    rust_min_with(root, "cargo")
}

/// [`rust_min`] with the cargo program named (so the fallback is testable).
pub fn rust_min_with(root: &Path, cargo: &str) -> Option<Detected> {
    let printed = bounded_output(
        cargo,
        &[
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ],
        Some(root),
        // Never let rustup install a toolchain on the laptop for a question.
        &[("RUSTUP_AUTO_INSTALL", "0")],
    );
    if let Some(p) = printed
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&p.stdout)
    {
        let min = newest(
            v.get("packages")?
                .as_array()?
                .iter()
                .filter_map(|pk| dotted(pk.get("rust_version")?.as_str()?)),
        )?;
        return Some(Detected {
            min,
            why: "cargo metadata rust-version".to_owned(),
            approximate: false,
        });
    }
    let text = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    let version = table
        .get("package")
        .and_then(|p| p.get("rust-version"))
        .or_else(|| {
            table
                .get("workspace")
                .and_then(|w| w.get("package"))
                .and_then(|p| p.get("rust-version"))
        })?
        .as_str()?;
    Some(Detected {
        min: dotted(version)?,
        why: "Cargo.toml rust-version (approximate: members not read; install cargo first for the exact answer)"
            .to_owned(),
        approximate: true,
    })
}

/// The Go version `go.mod` requires, from `go list -m -json` (offline, no
/// toolchain download); without go, from the `go` line of `go.mod`, labelled
/// approximate with the advice to install go first.
pub fn go_min(root: &Path, go_mod: &str) -> Option<Detected> {
    go_min_with(root, go_mod, "go")
}

/// [`go_min`] with the go program named (so the fallback is testable).
pub fn go_min_with(root: &Path, go_mod: &str, go: &str) -> Option<Detected> {
    let printed = bounded_output(
        go,
        &["list", "-m", "-json"],
        Some(root),
        &[
            ("GOFLAGS", "-mod=readonly"),
            ("GOPROXY", "off"),
            ("GOTOOLCHAIN", "local"),
        ],
    );
    if let Some(p) = printed
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&p.stdout)
        && let Some(min) = v.get("GoVersion").and_then(|g| g.as_str()).and_then(dotted)
    {
        return Some(Detected {
            min,
            why: "go list -m -json".to_owned(),
            approximate: false,
        });
    }
    let min = go_mod
        .lines()
        .filter_map(|l| l.trim().strip_prefix("go "))
        .find_map(dotted)?;
    Some(Detected {
        min,
        why: "go.mod text (approximate: install go first for the exact answer)".to_owned(),
        approximate: true,
    })
}

/// The Java major version `text` asks for, in `<tag>N</tag>` form.
fn xml_java(text: &str) -> Option<u64> {
    for tag in [
        "maven.compiler.release",
        "maven.compiler.source",
        "maven.compiler.target",
        "java.version",
        "release",
    ] {
        let open = format!("<{tag}>");
        if let Some(start) = text.find(&open) {
            let rest = &text[start + open.len()..];
            if let Some(end) = rest.find("</") {
                let v = rest[..end].trim();
                // 1.8 means 8.
                let v = v.strip_prefix("1.").unwrap_or(v);
                if let Ok(n) = v.parse() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// The Java major version a Gradle build file asks for.
fn gradle_java(text: &str) -> Option<u64> {
    for marker in [
        "JavaLanguageVersion.of(",
        "jvmToolchain(",
        "JavaVersion.VERSION_",
        "sourceCompatibility = ",
    ] {
        if let Some(start) = text.find(marker) {
            let rest = text[start + marker.len()..]
                .trim_start_matches(['"', '\'', '_'])
                .trim_start_matches("1.");
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(n) = digits.parse() {
                return Some(n);
            }
        }
    }
    None
}

/// The Java version a project asks for, read from its Maven or Gradle
/// files. goway never runs `mvn` or `gradle` for this: both execute the
/// project's own build scripts and plugins, which a diagnostic must not do on
/// the laptop. So the answer is read from the files' text and labelled
/// approximate.
pub fn java_min(root: &Path) -> Option<Detected> {
    let read = |name: &str| {
        let path = root.join(name);
        let meta = std::fs::metadata(&path).ok()?;
        (meta.len() <= MAX_CONFIG * 4)
            .then(|| std::fs::read_to_string(path).ok())
            .flatten()
    };
    if let Some(n) = read("pom.xml").as_deref().and_then(xml_java) {
        return Some(Detected {
            min: vec![n],
            why: "pom.xml text (approximate: goway does not run mvn, which would run the project's plugins)"
                .to_owned(),
            approximate: true,
        });
    }
    for name in ["build.gradle.kts", "build.gradle"] {
        if let Some(n) = read(name).as_deref().and_then(gradle_java) {
            return Some(Detected {
                min: vec![n],
                why: format!(
                    "{name} text (approximate: goway does not run gradle, which would run the project's build scripts)"
                ),
                approximate: true,
            });
        }
    }
    None
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

    #[test]
    fn rust_version_comes_from_cargo_metadata_and_falls_back_to_the_toml_with_a_label() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"fx\"\nversion = \"0.0.0\"\nedition = \"2021\"\nrust-version = \"1.70\"\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
        let missing = rust_min_with(dir.path(), "goway-no-such-cargo").unwrap();
        assert_eq!(missing.min, [1, 70]);
        assert!(missing.approximate && missing.why.contains("install cargo first"));
        if let Some(real) = rust_min(dir.path())
            && !real.approximate
        {
            assert_eq!(real.min, [1, 70]);
            assert_eq!(real.why, "cargo metadata rust-version");
        }
    }

    #[test]
    fn go_version_falls_back_to_the_go_line_labelled_approximate() {
        let dir = tempfile::tempdir().unwrap();
        let got = go_min_with(dir.path(), "module x\n\ngo 1.22.1\n", "goway-no-such-go").unwrap();
        assert_eq!(got.min, [1, 22, 1]);
        assert!(got.approximate && got.why.contains("install go first"));
    }

    #[test]
    fn java_is_read_from_pom_and_gradle_text_and_always_labelled_approximate() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pom.xml"),
            "<project><properties><maven.compiler.source>1.8</maven.compiler.source></properties></project>",
        )
        .unwrap();
        let got = java_min(dir.path()).unwrap();
        assert_eq!(got.min, [8]);
        assert!(
            got.approximate && got.why.contains("does not run mvn"),
            "{}",
            got.why
        );
        std::fs::remove_file(dir.path().join("pom.xml")).unwrap();
        std::fs::write(
            dir.path().join("build.gradle.kts"),
            "kotlin { jvmToolchain(17) }\n",
        )
        .unwrap();
        let got = java_min(dir.path()).unwrap();
        assert_eq!(got.min, [17]);
        assert!(got.why.contains("does not run gradle"));
        assert_eq!(
            gradle_java("java { toolchain { languageVersion.set(JavaLanguageVersion.of(21)) } }"),
            Some(21)
        );
        assert_eq!(
            gradle_java("sourceCompatibility = JavaVersion.VERSION_11"),
            Some(11)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_or_missing_tool_yields_nothing_and_both_streams_are_read() {
        assert!(bounded_output("goway-no-such-tool", &[], None, &[]).is_none());
        assert!(bounded_output("false", &[], None, &[]).is_none());
        let ok = bounded_output("sh", &["-c", "echo hi; echo err >&2"], None, &[]).unwrap();
        assert_eq!((ok.stdout.trim(), ok.stderr.trim()), ("hi", "err"));
    }
}
