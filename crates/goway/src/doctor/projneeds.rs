//! What a project needs from a host, read from the project's own files, and
//! the checks and fixes that follow: build tools, language toolchains and
//! whatever `goway.toml` `[toolchain]` pins. A project is only ever asked
//! for what its files require (a C++ project is never asked for cargo).
//!
//! Declarative files go through real parsers (TOML, JSON). `CMakeLists.txt`
//! has no declarative form, so it is read as text, bounded, and what it
//! yields is labelled approximate. Every download a fix makes is pinned to
//! a release and verified against a checksum before it is unpacked.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{Check, Fix, Level, install};
use crate::ecotools::{self, CargoLinking};
use crate::error::{Error, Result};

/// The largest project file goway reads.
const MAX_FILE: u64 = 1024 * 1024;

/// A language or build system a project uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Eco {
    /// `Cargo.toml`.
    Rust,
    /// `pyproject.toml`, `requirements*.txt`, `uv.lock`.
    Python,
    /// `package.json`.
    Node,
    /// `pom.xml`, `build.gradle(.kts)`.
    Java,
    /// `CMakeLists.txt`, `Makefile`.
    Cpp,
    /// `go.mod`.
    Go,
    /// `Gemfile`.
    Ruby,
    /// `*.csproj`, `*.sln`, `global.json`.
    DotNet,
}

impl Eco {
    /// The name shown to the user.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::Node => "Node",
            Self::Java => "Java",
            Self::Cpp => "C/C++",
            Self::Go => "Go",
            Self::Ruby => "Ruby",
            Self::DotNet => ".NET",
        }
    }
}

/// A version requirement: at least, or a leading prefix (`14` accepts 14.2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionReq {
    /// `>=X.Y`.
    AtLeast(Vec<u64>),
    /// `X` or `X.Y`: the found version starts with it.
    Prefix(Vec<u64>),
}

impl VersionReq {
    /// Parse `>=3.24`, `3.24` or `14`; `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        if let Some(rest) = t.strip_prefix(">=") {
            return numbers(rest.trim()).map(Self::AtLeast);
        }
        numbers(t).map(Self::Prefix)
    }

    /// Whether `found` satisfies the requirement.
    pub fn matches(&self, found: &[u64]) -> bool {
        match self {
            Self::AtLeast(min) => {
                let n = min.len().max(found.len());
                let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
                (0..n)
                    .map(|i| at(found, i).cmp(&at(min, i)))
                    .find(|o| o.is_ne())
                    .is_none_or(std::cmp::Ordering::is_gt)
            }
            Self::Prefix(p) => found.len() >= p.len() && found[..p.len()] == p[..],
        }
    }
}

impl std::fmt::Display for VersionReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let join = |v: &[u64]| v.iter().map(u64::to_string).collect::<Vec<_>>().join(".");
        match self {
            Self::AtLeast(v) => write!(f, ">={}", join(v)),
            Self::Prefix(v) => write!(f, "{}", join(v)),
        }
    }
}

/// `3.24.1` as numbers; `None` when it is not dotted numbers only.
fn numbers(text: &str) -> Option<Vec<u64>> {
    let parts: Vec<u64> = text
        .split('.')
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then_some(parts)
}

/// The first dotted version number in a tool's version report.
pub fn first_version(text: &str) -> Option<Vec<u64>> {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut fallback = None;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let token = text[start..i].trim_end_matches('.');
            if let Some(v) = numbers(token) {
                if v.len() > 1 {
                    return Some(v);
                }
                fallback.get_or_insert(v);
            }
        } else {
            i += 1;
        }
    }
    fallback
}

/// One tool a project (or `goway.toml`) needs on a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Req {
    /// The command to look for.
    pub tool: String,
    /// The version it must have.
    pub min: Option<VersionReq>,
    /// What asks for it, shown with the verdict.
    pub why: String,
    /// Missing is a warning, not a failure.
    pub optional: bool,
    /// Read from text, not from a parser or the tool itself.
    pub approximate: bool,
}

/// `goway.toml` `[toolchain]`: version pins and extra tools.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toolchain {
    /// `tool = ">=3.24"` entries.
    pub versions: BTreeMap<String, String>,
    /// `tools = ["protoc"]`.
    pub tools: Vec<String>,
}

/// What a project needs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Needs {
    /// The ecosystems detected.
    pub ecosystems: BTreeSet<Eco>,
    /// The tools, one per name.
    pub reqs: Vec<Req>,
    /// How cargo links per target triple (the linker and `-fuse-ld` backend
    /// its config and environment name); the host's own triple picks one.
    pub linking: Vec<CargoLinking>,
}

/// Whether `name` is safe to put in a probe command (and a file name).
pub fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

impl Needs {
    fn eco(&mut self, eco: Eco) {
        self.ecosystems.insert(eco);
    }

    fn add(&mut self, tool: &str, min: Option<VersionReq>, why: &str) {
        self.push(Req {
            tool: tool.to_owned(),
            min,
            why: why.to_owned(),
            optional: false,
            approximate: false,
        });
    }

    fn push(&mut self, req: Req) {
        match self.reqs.iter_mut().find(|r| r.tool == req.tool) {
            Some(have) => {
                if have.min.is_none() {
                    have.min = req.min;
                    have.approximate = req.approximate;
                }
                have.optional &= req.optional;
            }
            None => self.reqs.push(req),
        }
    }

    /// Whether the project is Rust, or nothing was detected (goway's own
    /// default: Rust first).
    pub fn wants_rust(&self) -> bool {
        self.ecosystems.is_empty() || self.ecosystems.contains(&Eco::Rust)
    }

    /// The tool names the host should report on.
    pub fn probe_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.reqs.iter().map(|r| r.tool.clone()).collect();
        for l in &self.linking {
            for tool in l.linker.iter().chain(l.backend.iter()) {
                if !names.contains(tool) {
                    names.push(tool.clone());
                }
            }
        }
        names
    }

    /// The requirements of the linker setup cargo uses for `triple`: the
    /// linker program and its `-fuse-ld` backend.
    fn linking_reqs(&self, triple: &str) -> Vec<Req> {
        let Some(l) = self.linking.iter().find(|l| l.triple == triple) else {
            return Vec::new();
        };
        let override_hint = format!(
            "to run once without installing it: goway run --env {}=cc --env \"RUSTFLAGS=-C link-arg=-fuse-ld=lld\" -- ... (a non-empty RUSTFLAGS replaces the config's rustflags and Rust's bundled lld needs nothing installed; cargo-nextest's inner `cargo test` ignores --config and an empty CARGO_TARGET_*_RUSTFLAGS does not override config rustflags); goway never applies this itself",
            ecotools::target_var(triple, "LINKER")
        );
        l.linker
            .iter()
            .map(|t| ("linker", t))
            .chain(l.backend.iter().map(|t| ("-fuse-ld backend", t)))
            .map(|(role, tool)| Req {
                tool: tool.clone(),
                min: None,
                why: format!(
                    "cargo's {role} for {triple} ({}); {override_hint}",
                    l.source
                ),
                optional: false,
                approximate: false,
            })
            .collect()
    }

    /// One line naming what was detected, for the report.
    pub fn summary(&self) -> String {
        if self.ecosystems.is_empty() {
            return "no project files recognised; checking goway's own needs".to_owned();
        }
        let names: Vec<&str> = self.ecosystems.iter().map(|e| e.name()).collect();
        format!("project needs: {}", names.join(", "))
    }
}

/// Read a project file, at most [`MAX_FILE`] bytes, `None` when absent.
fn read(root: &Path, name: &str) -> Option<String> {
    let path = root.join(name);
    let meta = std::fs::metadata(&path)
        .ok()
        .filter(std::fs::Metadata::is_file)?;
    if meta.len() > MAX_FILE {
        tracing::warn!(file = %path.display(), "project file too large; not read");
        return None;
    }
    std::fs::read_to_string(&path).ok()
}

fn exists(root: &Path, name: &str) -> bool {
    root.join(name).exists()
}

/// Names of files in `root` (not below) that end in `suffix`.
fn with_suffix(root: &Path, suffix: &str) -> bool {
    std::fs::read_dir(root).is_ok_and(|d| {
        d.flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(suffix))
    })
}

fn rust(root: &Path, needs: &mut Needs) {
    if !exists(root, "Cargo.toml") {
        return;
    }
    needs.eco(Eco::Rust);
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cargo")));
    needs.linking = ecotools::cargo_linking(root, cargo_home.as_deref(), &|name| {
        std::env::var(name).ok()
    });
    // The toolchain checks (cargo, nextest, sccache) are goway's own; the
    // channel in rust-toolchain.toml is rustup's to install.
    if let Some(text) = read(root, "rust-toolchain.toml")
        && let Ok(v) = toml::from_str::<toml::Table>(&text)
    {
        tracing::debug!(channel = ?v.get("toolchain").and_then(|t| t.get("channel")), "rust-toolchain.toml");
    }
}

fn python(root: &Path, needs: &mut Needs) {
    let pyproject =
        read(root, "pyproject.toml").and_then(|t| toml::from_str::<toml::Table>(&t).ok());
    let requirements: Vec<String> = std::fs::read_dir(root)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| {
                    n.starts_with("requirements")
                        && Path::new(n)
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("txt"))
                })
                .collect()
        })
        .unwrap_or_default();
    if pyproject.is_none() && requirements.is_empty() && !exists(root, "uv.lock") {
        return;
    }
    needs.eco(Eco::Python);
    let min = pyproject
        .as_ref()
        .and_then(|p| p.get("project")?.get("requires-python")?.as_str())
        .and_then(VersionReq::parse);
    needs.add("python3", min, "pyproject.toml / requirements");
    let uv = exists(root, "uv.lock")
        || pyproject
            .as_ref()
            .is_some_and(|p| p.get("tool").and_then(|t| t.get("uv")).is_some());
    if uv {
        needs.add("uv", None, "uv.lock / [tool.uv]");
    }
    let mut pytest = pyproject.as_ref().is_some_and(|p| {
        p.get("tool").and_then(|t| t.get("pytest")).is_some() || mentions_dep(p, "pytest")
    }) || exists(root, "pytest.ini");
    for name in &requirements {
        if read(root, name).is_some_and(|t| {
            t.lines()
                .any(|l| l.trim_start().to_ascii_lowercase().starts_with("pytest"))
        }) {
            pytest = true;
        }
    }
    if pytest {
        needs.add("pytest", None, "declared test dependency");
    }
}

/// Whether any dependency list of a pyproject names `dep`.
fn mentions_dep(pyproject: &toml::Table, dep: &str) -> bool {
    fn strings(v: &toml::Value, out: &mut Vec<String>) {
        match v {
            toml::Value::String(s) => out.push(s.clone()),
            toml::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
            toml::Value::Table(t) => t.values().for_each(|x| strings(x, out)),
            _ => {}
        }
    }
    let mut all = Vec::new();
    if let Some(p) = pyproject.get("project") {
        for key in ["dependencies", "optional-dependencies"] {
            if let Some(v) = p.get(key) {
                strings(v, &mut all);
            }
        }
    }
    if let Some(v) = pyproject.get("dependency-groups") {
        strings(v, &mut all);
    }
    all.iter().any(|s| {
        let name: String = s
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .collect();
        name.eq_ignore_ascii_case(dep)
    })
}

/// The lowest version a Node range such as `>=18`, `^20.1` or `22` allows.
fn node_floor(range: &str) -> Option<VersionReq> {
    let t = range
        .trim()
        .trim_start_matches(['>', '=', '^', '~', 'v', ' '])
        .split([' ', '|'])
        .next()?;
    let t = t.trim_end_matches(".x").trim_end_matches(".*");
    numbers(t).map(VersionReq::AtLeast)
}

fn node(root: &Path, needs: &mut Needs) {
    let package = read(root, "package.json");
    if package.is_none() && !exists(root, ".nvmrc") {
        return;
    }
    let json: Option<serde_json::Value> = package
        .as_deref()
        .and_then(|t| serde_json::from_str(t).ok());
    needs.eco(Eco::Node);
    let engines = json
        .as_ref()
        .and_then(|j| j.get("engines")?.get("node")?.as_str())
        .and_then(node_floor);
    let nvmrc = read(root, ".nvmrc").and_then(|t| node_floor(t.trim()));
    needs.add("node", engines.or(nvmrc), "package.json engines / .nvmrc");
    let declared = json
        .as_ref()
        .and_then(|j| j.get("packageManager")?.as_str())
        .and_then(|p| p.split('@').next())
        .map(str::to_owned);
    let pm = match declared.as_deref() {
        Some(p @ ("npm" | "pnpm" | "yarn")) => p,
        _ if exists(root, "pnpm-lock.yaml") => "pnpm",
        _ if exists(root, "yarn.lock") => "yarn",
        _ => "npm",
    };
    needs.add(pm, None, "package.json packageManager / lockfile");
}

fn java(root: &Path, needs: &mut Needs) {
    let maven = exists(root, "pom.xml");
    let gradle = [
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ]
    .iter()
    .any(|f| exists(root, f));
    if !maven && !gradle {
        return;
    }
    needs.eco(Eco::Java);
    needs.add("java", None, "pom.xml / build.gradle");
    if maven && !exists(root, "mvnw") {
        needs.add("mvn", None, "pom.xml (no mvnw)");
    }
    if gradle && !exists(root, "gradlew") {
        needs.add("gradle", None, "build.gradle (no gradlew)");
    }
}

/// The `VERSION` in `cmake_minimum_required(VERSION 3.24...3.30)`.
fn cmake_minimum(text: &str) -> Option<VersionReq> {
    let lower = text.to_ascii_lowercase();
    let at = lower.find("cmake_minimum_required")?;
    let rest = &text[at..];
    let open = rest.find('(')?;
    let close = rest.find(')')?;
    let body = rest.get(open + 1..close)?;
    let mut words = body.split_whitespace();
    while let Some(w) = words.next() {
        if w.eq_ignore_ascii_case("VERSION") {
            let v = words.next()?;
            let low = v.split("...").next()?;
            return numbers(low).map(VersionReq::AtLeast);
        }
    }
    None
}

/// Whether the text declares the language `lang` in `project(... LANGUAGES ...)`.
/// No `LANGUAGES` (or no project call) means `CMake`'s default: C and C++.
fn cmake_language(text: &str, lang: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let Some(at) = lower.find("project(") else {
        return true;
    };
    let rest = &text[at + "project(".len()..];
    let Some(close) = rest.find(')') else {
        return true;
    };
    let words: Vec<&str> = rest[..close].split_whitespace().collect();
    let Some(pos) = words
        .iter()
        .position(|w| w.eq_ignore_ascii_case("LANGUAGES"))
    else {
        // `project(name C)` style: language names follow the project name.
        let langs: Vec<&&str> = words
            .iter()
            .skip(1)
            .filter(|w| ["C", "CXX"].contains(w))
            .collect();
        return langs.is_empty() || langs.iter().any(|w| w.eq_ignore_ascii_case(lang));
    };
    words[pos + 1..]
        .iter()
        .any(|w| w.eq_ignore_ascii_case(lang))
}

fn cpp(root: &Path, needs: &mut Needs) {
    let cmake = read(root, "CMakeLists.txt");
    let make = exists(root, "Makefile") || exists(root, "makefile");
    if cmake.is_none() && !make {
        return;
    }
    needs.eco(Eco::Cpp);
    let Some(text) = cmake else {
        needs.add("make", None, "Makefile");
        needs.add("cc", None, "Makefile");
        return;
    };
    let approx = |tool: &str, min, why: &str| Req {
        tool: tool.to_owned(),
        min,
        why: format!("{why} (approximate: read from CMakeLists.txt text)"),
        optional: false,
        approximate: true,
    };
    needs.push(approx(
        "cmake",
        cmake_minimum(&text),
        "CMakeLists.txt cmake_minimum_required",
    ));
    if cmake_language(&text, "C") {
        needs.add("cc", None, "CMakeLists.txt");
    }
    if cmake_language(&text, "CXX") {
        needs.add("c++", None, "CMakeLists.txt");
    }
    let ninja = read(root, "CMakePresets.json")
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|j| {
            j.get("configurePresets")?.as_array().map(|ps| {
                ps.iter().any(|p| {
                    p.get("generator")
                        .and_then(|g| g.as_str())
                        .is_some_and(|g| g.starts_with("Ninja"))
                })
            })
        })
        .unwrap_or(false);
    needs.add(
        if ninja { "ninja" } else { "make" },
        None,
        "CMake generator",
    );
    let lower = text.to_ascii_lowercase();
    if lower.contains("git_repository") || lower.contains("cpmaddpackage") {
        needs.push(approx(
            "git",
            None,
            "FetchContent / CPM fetch from git (and need network at configure time)",
        ));
    }
    needs.push(Req {
        tool: "ccache".to_owned(),
        min: None,
        why: "optional compiler launcher".to_owned(),
        optional: true,
        approximate: false,
    });
}

fn go(root: &Path, needs: &mut Needs) {
    let Some(text) = read(root, "go.mod") else {
        return;
    };
    needs.eco(Eco::Go);
    let min = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("go "))
        .find_map(|v| numbers(v.trim()))
        .map(VersionReq::AtLeast);
    needs.add("go", min, "go.mod");
}

fn ruby(root: &Path, needs: &mut Needs) {
    if !exists(root, "Gemfile") {
        return;
    }
    needs.eco(Eco::Ruby);
    let min = read(root, ".ruby-version")
        .and_then(|t| numbers(t.trim().trim_start_matches("ruby-")))
        .map(VersionReq::AtLeast);
    needs.add("ruby", min, "Gemfile / .ruby-version");
    needs.add("bundle", None, "Gemfile");
}

fn dotnet(root: &Path, needs: &mut Needs) {
    let global =
        read(root, "global.json").and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    if global.is_none() && !with_suffix(root, ".csproj") && !with_suffix(root, ".sln") {
        return;
    }
    needs.eco(Eco::DotNet);
    let min = global
        .as_ref()
        .and_then(|g| g.get("sdk")?.get("version")?.as_str())
        .and_then(numbers)
        .map(|v| VersionReq::AtLeast(v.into_iter().take(1).collect()));
    needs.add("dotnet", min, ".csproj / .sln / global.json");
}

/// Work out what the project at `root` needs, then lay `goway.toml`'s
/// `[toolchain]` over it: its entries are checked like detected ones and
/// take precedence over a detected version.
///
/// # Errors
///
/// [`Error::Config`] for a tool name or version in `toolchain` that is not
/// valid (names go into a command, so they are strict).
pub fn analyse(root: &Path, toolchain: &Toolchain) -> Result<Needs> {
    let mut needs = Needs::default();
    rust(root, &mut needs);
    python(root, &mut needs);
    node(root, &mut needs);
    java(root, &mut needs);
    cpp(root, &mut needs);
    go(root, &mut needs);
    ruby(root, &mut needs);
    dotnet(root, &mut needs);
    let bad = |message: String| Error::Config {
        path: root.join("goway.toml"),
        message,
    };
    for (name, version) in &toolchain.versions {
        if !valid_tool_name(name) {
            return Err(bad(format!("[toolchain] `{name}` is not a tool name")));
        }
        let min = VersionReq::parse(version).ok_or_else(|| {
            bad(format!(
                "[toolchain] {name} = \"{version}\": use \">=3.24\" or \"14\""
            ))
        })?;
        needs.reqs.retain(|r| r.tool != *name);
        needs.reqs.push(Req {
            tool: name.clone(),
            min: Some(min),
            why: "goway.toml [toolchain]".to_owned(),
            optional: false,
            approximate: false,
        });
    }
    for name in &toolchain.tools {
        if !valid_tool_name(name) {
            return Err(bad(format!(
                "[toolchain] tools: `{name}` is not a tool name"
            )));
        }
        needs.add(name, None, "goway.toml [toolchain] tools");
    }
    tracing::info!(ecosystems = ?needs.ecosystems, tools = needs.reqs.len(), "project needs");
    Ok(needs)
}

/// One pinned user-level install: unpacked under `~/.local/opt/goway-TOOL`,
/// its binaries linked into `~/.local/bin`.
struct UserPin {
    tool: &'static str,
    version: &'static str,
    /// Binaries inside the unpacked directory, linked by file name.
    bins: &'static [&'static str],
    /// Used only where the distribution has no package for the tool (see
    /// [`packaged`]); elsewhere the system package is the fix.
    fallback_only: bool,
    /// `(arch, url, hash algorithm, hash)`.
    builds: &'static [(&'static str, &'static str, &'static str, &'static str)],
}

const PINS: &[UserPin] = &[
    UserPin {
        tool: "uv",
        version: "0.12.23",
        bins: &["uv", "uvx"],
        fallback_only: false,
        builds: &[
            (
                "x86_64",
                "https://github.com/astral-sh/uv/releases/download/0.12.23/uv-x86_64-unknown-linux-gnu.tar.gz",
                "sha256",
                "9167d72b3319674b6303c4cbe071854bba13ebdf3d76b1a7cbdc175471fb66d6",
            ),
            (
                "aarch64",
                "https://github.com/astral-sh/uv/releases/download/0.12.23/uv-aarch64-unknown-linux-gnu.tar.gz",
                "sha256",
                "6524bd338177ed50d035d39354e12545e993bbeba2ecbddf0480c5b3a81d313f",
            ),
        ],
    },
    UserPin {
        tool: "go",
        version: "1.27.1",
        bins: &["bin/go", "bin/gofmt"],
        fallback_only: false,
        builds: &[
            (
                "x86_64",
                "https://go.dev/dl/go1.27.1.linux-amd64.tar.gz",
                "sha256",
                "63d339f0da5ab53635a56f2490a7984dfe12dfcff22ad749f63edaf590168445",
            ),
            (
                "aarch64",
                "https://go.dev/dl/go1.27.1.linux-arm64.tar.gz",
                "sha256",
                "3450b45a3f9ee8568792736a5c5e70a1f2e9b36c35a8f74958c03e51d7d92bec",
            ),
        ],
    },
    UserPin {
        tool: "cmake",
        version: "3.30.5",
        bins: &["bin/cmake", "bin/ctest", "bin/cpack"],
        fallback_only: false,
        builds: &[
            (
                "x86_64",
                "https://github.com/Kitware/CMake/releases/download/v3.30.5/cmake-3.30.5-linux-x86_64.tar.gz",
                "sha256",
                "f747d9b23e1a252a8beafb4ed2bc2ddf78cff7f04a8e4de19f4ff88e9b51dc9d",
            ),
            (
                "aarch64",
                "https://github.com/Kitware/CMake/releases/download/v3.30.5/cmake-3.30.5-linux-aarch64.tar.gz",
                "sha256",
                "da7dead2c92c1747b40d506d7f7d68590f5bab175316d2e7af73e48a2e417e48",
            ),
        ],
    },
    UserPin {
        tool: "node",
        version: "24.21.0",
        bins: &["bin/node", "bin/npm", "bin/npx", "bin/corepack"],
        fallback_only: false,
        builds: &[
            (
                "x86_64",
                "https://nodejs.org/dist/v24.21.0/node-v24.21.0-linux-x64.tar.gz",
                "sha256",
                "6e1db87ef58b8819e5d5402eff1536491b18edd8eb7bee5ef7897876e88dc5ff",
            ),
            (
                "aarch64",
                "https://nodejs.org/dist/v24.21.0/node-v24.21.0-linux-arm64.tar.gz",
                "sha256",
                "724282c3b43aec998aa9527380465b45d229e021b58035f5f4f63095eabfe5d5",
            ),
        ],
    },
    UserPin {
        tool: "java",
        version: "21.0.12",
        bins: &["bin/java", "bin/javac", "bin/jar"],
        fallback_only: false,
        builds: &[
            (
                "x86_64",
                "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jdk_x64_linux_hotspot_21.0.12.1_1.tar.gz",
                "sha256",
                "ce79869e1307ed8ee1e2baa86a412b1eb5b75d10a01006d788a6f968bcfaee94",
            ),
            (
                "aarch64",
                "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jdk_aarch64_linux_hotspot_21.0.12.1_1.tar.gz",
                "sha256",
                "23e37e026f12f3e706f18938ff611db3032d075b09d0879a25d06718c773e223",
            ),
        ],
    },
    UserPin {
        tool: "mvn",
        version: "3.9.16",
        bins: &["bin/mvn"],
        fallback_only: false,
        builds: &[
            (
                "x86_64",
                "https://archive.apache.org/dist/maven/maven-3/3.9.16/binaries/apache-maven-3.9.16-bin.tar.gz",
                "sha512",
                "831a8591fe20c8243b1dbe7d71e3244f31d1665b0804b2e825e38cbbe5ce0cafb8338851f90780735568773e0a6cd07bbec107cda0b896b008b861075358b6f6",
            ),
            (
                "aarch64",
                "https://archive.apache.org/dist/maven/maven-3/3.9.16/binaries/apache-maven-3.9.16-bin.tar.gz",
                "sha512",
                "831a8591fe20c8243b1dbe7d71e3244f31d1665b0804b2e825e38cbbe5ce0cafb8338851f90780735568773e0a6cd07bbec107cda0b896b008b861075358b6f6",
            ),
        ],
    },
    UserPin {
        tool: "mold",
        version: "2.42.1",
        bins: &["bin/mold", "bin/ld.mold"],
        fallback_only: true,
        builds: &[
            (
                "x86_64",
                "https://github.com/rui314/mold/releases/download/v2.42.1/mold-2.42.1-x86_64-linux.tar.gz",
                "sha256",
                "6ff270c9bf07d2bec5c98aa324eb7c4daf6a1a4d815c05ff1708049616047855",
            ),
            (
                "aarch64",
                "https://github.com/rui314/mold/releases/download/v2.42.1/mold-2.42.1-aarch64-linux.tar.gz",
                "sha256",
                "16b025652d3d7456689e6025a77e1903bb2a15e7630877c26cc133f5df95b9c6",
            ),
        ],
    },
];

/// The directory a pinned tool is unpacked into.
fn pin_dir(tool: &str) -> String {
    format!("\"$HOME/.local/opt/goway-{tool}\"")
}

/// Download, verify, then unpack and link; the architecture the host
/// reports only selects a table row, it is never pasted into the command.
fn pin_install(pin: &UserPin, arch: &str) -> Option<String> {
    let (_, url, algo, sum) = pin.builds.iter().find(|(a, ..)| *a == arch)?;
    let dir = pin_dir(pin.tool);
    let links = pin.bins.iter().fold(String::new(), |mut acc, b| {
        use std::fmt::Write as _;
        let name = b.rsplit('/').next().unwrap_or(b);
        let _ = write!(acc, " && ln -sf {dir}/{b} \"$HOME/.local/bin/{name}\"");
        acc
    });
    Some(format!(
        "t=$(mktemp -d) && curl -fsSL {url} -o \"$t/a\" && echo \"{sum}  $t/a\" | {algo}sum -c --quiet && rm -rf {dir} && mkdir -p {dir} \"$HOME/.local/bin\" && tar xf \"$t/a\" -C {dir} --strip-components=1{links}; rc=$?; rm -rf \"$t\"; exit $rc"
    ))
}

/// Package names for the system package manager: `(tool, apt, dnf, pacman)`.
const PACKAGES: &[(&str, &str, &str, &str)] = &[
    ("git", "git", "git", "git"),
    ("make", "make", "make", "make"),
    ("cc", "build-essential", "gcc", "base-devel"),
    ("c++", "build-essential", "gcc-c++", "base-devel"),
    ("gcc", "gcc", "gcc", "gcc"),
    ("g++", "g++", "gcc-c++", "gcc"),
    ("clang", "clang", "clang", "clang"),
    ("mold", "mold", "mold", "mold"),
    ("ld.lld", "lld", "lld", "lld"),
    ("ninja", "ninja-build", "ninja-build", "ninja"),
    ("ccache", "ccache", "ccache", "ccache"),
    ("cmake", "cmake", "cmake", "cmake"),
    ("python3", "python3", "python3", "python"),
    (
        "pytest",
        "python3-pytest",
        "python3-pytest",
        "python-pytest",
    ),
    (
        "java",
        "default-jdk-headless",
        "java-latest-openjdk-devel",
        "jdk-openjdk",
    ),
    ("mvn", "maven", "maven", "maven"),
    ("gradle", "gradle", "gradle", "gradle"),
    ("go", "golang-go", "golang", "go"),
    ("ruby", "ruby", "ruby", "ruby"),
    ("node", "nodejs npm", "nodejs npm", "nodejs npm"),
    ("dotnet", "dotnet-sdk-8.0", "dotnet-sdk-8.0", "dotnet-sdk"),
    (
        "protoc",
        "protobuf-compiler",
        "protobuf-compiler",
        "protobuf",
    ),
];

/// Whether the host's distribution packages `tool`. Only mold has a known
/// gap: Ubuntu before 22.04 and Debian before 12 have no package for it.
/// An unknown or unlisted distribution counts as packaged (the package
/// manager then says so itself if it is not).
fn packaged(tool: &str, facts: &BTreeMap<String, String>) -> bool {
    if tool != "mold" {
        return true;
    }
    let os = facts.get("os").map(|o| o.to_ascii_lowercase());
    let version = os.as_deref().and_then(first_version);
    match (os.as_deref(), version) {
        (Some(o), Some(v)) if o.contains("ubuntu") => v >= vec![22, 4],
        (Some(o), Some(v)) if o.contains("debian") => v.first().is_some_and(|m| *m >= 12),
        _ => true,
    }
}

/// The fix for a missing or too-old `tool`: a pinned user-level install
/// when one satisfies the requirement, else a system package (root), else
/// a user-level helper for the few tools that have one.
fn fix_for(req: &Req, facts: &BTreeMap<String, String>) -> Option<Fix> {
    let arch = facts.get("arch").map_or("x86_64", String::as_str);
    let tool = req.tool.as_str();
    if let Some(pin) = PINS.iter().find(|p| p.tool == tool) {
        let ok = req
            .min
            .as_ref()
            .is_none_or(|m| numbers(pin.version).is_some_and(|v| m.matches(&v)));
        let wanted = !pin.fallback_only || !packaged(tool, facts);
        if ok
            && wanted
            && let Some(command) = pin_install(pin, arch)
        {
            return Some(Fix {
                command,
                root: false,
                why: format!(
                    "installs {tool} {} (pinned, checksum-verified) under ~/.local/opt and links it into ~/.local/bin; make sure ~/.local/bin is on PATH",
                    pin.version
                ),
            });
        }
    }
    match tool {
        "pnpm" | "yarn" => {
            return Some(Fix {
                command: format!("corepack enable --install-directory \"$HOME/.local/bin\" {tool}"),
                root: false,
                why: format!(
                    "corepack (shipped with node) provides {tool} at the version package.json pins"
                ),
            });
        }
        "bundle" => {
            return Some(Fix {
                command: "gem install --user-install bundler".to_owned(),
                root: false,
                why: "installs bundler for this user".to_owned(),
            });
        }
        _ => {}
    }
    let (_, apt, dnf, pacman) = PACKAGES.iter().find(|(t, ..)| *t == tool)?;
    Some(Fix {
        command: install(facts, apt, dnf, pacman),
        root: true,
        why: format!("{tool} is needed by {}; system packages need root", req.why),
    })
}

/// Checks for every requirement, from the host's `want.TOOL` facts.
pub fn checks(needs: &Needs, facts: &BTreeMap<String, String>, skip: &[String]) -> Vec<Check> {
    let mut out = Vec::new();
    let mut reqs: Vec<Req> = needs.reqs.clone();
    if facts.get("kernel").is_none_or(|k| k == "Linux")
        && let Some(arch) = facts
            .get("arch")
            .filter(|a| matches!(a.as_str(), "x86_64" | "aarch64"))
    {
        for req in needs.linking_reqs(&format!("{arch}-unknown-linux-gnu")) {
            if !reqs.iter().any(|r| r.tool == req.tool) {
                reqs.push(req);
            }
        }
    }
    for req in &reqs {
        if skip.contains(&req.tool) {
            continue;
        }
        let report = facts
            .get(&format!("want.{}", req.tool))
            .map(String::as_str)
            .filter(|v| !v.is_empty());
        let (level, detail, fix) = match report {
            None => (
                if req.optional {
                    Level::Warn
                } else {
                    Level::Fail
                },
                format!("missing; {}", req.why),
                fix_for(req, facts),
            ),
            Some(text) => {
                let mut found = first_version(text);
                // Java 8 and earlier report 1.N.
                if req.tool == "java"
                    && let Some(v) = &mut found
                    && v.first() == Some(&1)
                    && v.len() > 1
                {
                    v.remove(0);
                }
                match (&req.min, found) {
                    (Some(min), Some(v)) if !min.matches(&v) => (
                        Level::Fail,
                        format!("{text}: needs {min} ({})", req.why),
                        fix_for(req, facts),
                    ),
                    (Some(min), None) => (
                        Level::Warn,
                        format!("{text}: cannot read its version to compare with {min}"),
                        None,
                    ),
                    _ => (Level::Ok, text.to_owned(), None),
                }
            }
        };
        out.push(Check {
            name: req.tool.clone(),
            level,
            detail,
            fix,
        });
    }
    out
}

/// Whether a check is a system package goway may install (listed, never removed).
pub fn is_package_check(name: &str) -> bool {
    PACKAGES.iter().any(|(t, ..)| *t == name) && !PINS.iter().any(|p| p.tool == name)
}

/// How to take back a pinned user-level install, derived from the check
/// name alone (a record never supplies a command).
pub fn undo_of(check: &str) -> Option<(String, bool)> {
    if !PINS.iter().any(|p| p.tool == check) {
        return match check {
            "pnpm" | "yarn" => Some((
                format!(
                    "rm -f \"$HOME/.local/bin/{check}\" \"$HOME/.local/bin/{check}pkg\" \"$HOME/.local/bin/{check}.cmd\""
                ),
                false,
            )),
            "bundle" => Some((
                "gem uninstall --user-install -x -a bundler".to_owned(),
                false,
            )),
            _ => None,
        };
    }
    let dir = pin_dir(check);
    Some((
        format!(
            "for l in \"$HOME\"/.local/bin/*; do case \"$(readlink \"$l\")\" in {dir}/*) rm -f \"$l\" ;; esac; done; rm -rf {dir}; rmdir \"$HOME/.local/opt\" \"$HOME/.local/bin\" 2>/dev/null; true"
        ),
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (name, body) in files {
            std::fs::write(d.path().join(name), body).unwrap();
        }
        d
    }

    fn tools(n: &Needs) -> Vec<&str> {
        n.reqs.iter().map(|r| r.tool.as_str()).collect()
    }

    fn analyse_dir(d: &tempfile::TempDir) -> Needs {
        analyse(d.path(), &Toolchain::default()).unwrap()
    }

    #[test]
    fn versions_parse_compare_and_read_from_reports() {
        let ge = VersionReq::parse(">=3.24").unwrap();
        assert!(ge.matches(&[3, 28, 3]) && ge.matches(&[4]) && !ge.matches(&[3, 9]));
        let p = VersionReq::parse("14").unwrap();
        assert!(p.matches(&[14, 2, 1]) && !p.matches(&[13, 2]) && !p.matches(&[]));
        assert_eq!(VersionReq::parse("^3"), None);
        assert_eq!(ge.to_string(), ">=3.24");
        assert_eq!(first_version("cmake version 3.28.3"), Some(vec![3, 28, 3]));
        assert_eq!(
            first_version("go version go1.22.1 linux/amd64"),
            Some(vec![1, 22, 1])
        );
        assert_eq!(
            first_version("gcc (Ubuntu 13.2.0-23) 13.2.0"),
            Some(vec![13, 2, 0])
        );
        assert_eq!(first_version("v24.21.0"), Some(vec![24, 21, 0]));
        assert_eq!(first_version("none"), None);
    }

    #[test]
    fn a_cpp_project_is_never_asked_for_cargo_or_node() {
        let d = dir_with(&[(
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.24...3.30)\nproject(x LANGUAGES CXX)\ninclude(FetchContent)\nFetchContent_Declare(g GIT_REPOSITORY https://example.invalid/g.git)\n",
        )]);
        let n = analyse_dir(&d);
        assert_eq!(tools(&n), ["cmake", "c++", "make", "git", "ccache"]);
        assert!(!n.wants_rust());
        let cmake = &n.reqs[0];
        assert_eq!(cmake.min, Some(VersionReq::AtLeast(vec![3, 24])));
        assert!(cmake.approximate && cmake.why.contains("approximate"));
        assert!(n.reqs[4].optional, "ccache is optional");
    }

    #[test]
    fn cmake_presets_asking_for_ninja_replace_make() {
        let d = dir_with(&[
            (
                "CMakeLists.txt",
                "cmake_minimum_required(VERSION 3.20)\nproject(x C CXX)\n",
            ),
            (
                "CMakePresets.json",
                r#"{"version":3,"configurePresets":[{"name":"d","generator":"Ninja"}]}"#,
            ),
        ]);
        let n = analyse_dir(&d);
        assert!(tools(&n).contains(&"ninja") && !tools(&n).contains(&"make"));
        assert!(tools(&n).contains(&"cc") && tools(&n).contains(&"c++"));
    }

    #[test]
    fn node_reads_engines_package_manager_and_lockfile() {
        let d = dir_with(&[
            (
                "package.json",
                r#"{"engines":{"node":">=20.1"},"packageManager":"pnpm@9.1.0"}"#,
            ),
            ("package-lock.json", "{}"),
        ]);
        let n = analyse_dir(&d);
        assert_eq!(
            tools(&n),
            ["node", "pnpm"],
            "the declared manager wins over a lockfile"
        );
        assert_eq!(n.reqs[0].min, Some(VersionReq::AtLeast(vec![20, 1])));
        let d = dir_with(&[("package.json", "{}"), ("yarn.lock", "")]);
        assert_eq!(tools(&analyse_dir(&d)), ["node", "yarn"]);
        let d = dir_with(&[("package.json", "{}")]);
        assert_eq!(tools(&analyse_dir(&d)), ["node", "npm"]);
    }

    #[test]
    fn python_reads_requires_python_uv_and_declared_pytest() {
        let d = dir_with(&[
            (
                "pyproject.toml",
                "[project]\nrequires-python = \">=3.11\"\ndependencies = [\"requests\"]\n[dependency-groups]\ndev = [\"pytest>=8\"]\n",
            ),
            ("uv.lock", ""),
        ]);
        let n = analyse_dir(&d);
        assert_eq!(tools(&n), ["python3", "uv", "pytest"]);
        assert_eq!(n.reqs[0].min, Some(VersionReq::AtLeast(vec![3, 11])));
        let d = dir_with(&[("requirements.txt", "flask\n")]);
        assert_eq!(tools(&analyse_dir(&d)), ["python3"]);
    }

    #[test]
    fn go_java_ruby_dotnet_and_rust_are_detected_from_their_files() {
        let d = dir_with(&[("go.mod", "module x\n\ngo 1.22\n")]);
        let n = analyse_dir(&d);
        assert_eq!(n.reqs[0].min, Some(VersionReq::AtLeast(vec![1, 22])));
        let d = dir_with(&[("pom.xml", "<project/>")]);
        assert_eq!(tools(&analyse_dir(&d)), ["java", "mvn"]);
        let d = dir_with(&[("build.gradle.kts", ""), ("gradlew", "")]);
        assert_eq!(
            tools(&analyse_dir(&d)),
            ["java"],
            "the wrapper replaces gradle"
        );
        let d = dir_with(&[("Gemfile", ""), (".ruby-version", "3.3.0\n")]);
        let n = analyse_dir(&d);
        assert_eq!(tools(&n), ["ruby", "bundle"]);
        assert_eq!(n.reqs[0].min, Some(VersionReq::AtLeast(vec![3, 3, 0])));
        let d = dir_with(&[("global.json", r#"{"sdk":{"version":"8.0.100"}}"#)]);
        assert_eq!(
            analyse_dir(&d).reqs[0].min,
            Some(VersionReq::AtLeast(vec![8]))
        );
        let d = dir_with(&[("Cargo.toml", "[package]\nname=\"x\"\n")]);
        let n = analyse_dir(&d);
        assert!(n.wants_rust() && n.reqs.is_empty());
        assert!(dir_with(&[("README.md", "")]).path().exists());
        assert!(
            analyse_dir(&dir_with(&[("README.md", "")])).wants_rust(),
            "nothing detected keeps goway's Rust default"
        );
    }

    #[test]
    fn goway_toml_toolchain_takes_precedence_and_is_validated() {
        let d = dir_with(&[(
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\nproject(x CXX)\n",
        )]);
        let tc = Toolchain {
            versions: [
                ("cmake".to_owned(), ">=3.26".to_owned()),
                ("gcc".to_owned(), "14".to_owned()),
            ]
            .into(),
            tools: vec!["protoc".to_owned()],
        };
        let n = analyse(d.path(), &tc).unwrap();
        let cmake = n.reqs.iter().find(|r| r.tool == "cmake").unwrap();
        assert_eq!(cmake.min, Some(VersionReq::AtLeast(vec![3, 26])));
        assert!(!cmake.approximate && cmake.why.contains("goway.toml"));
        assert!(tools(&n).contains(&"gcc") && tools(&n).contains(&"protoc"));
        for bad in ["a b", "x;rm", "", "$(id)"] {
            let tc = Toolchain {
                tools: vec![bad.to_owned()],
                ..Toolchain::default()
            };
            assert!(analyse(d.path(), &tc).is_err(), "{bad}");
        }
        let tc = Toolchain {
            versions: [("gcc".to_owned(), "^14".to_owned())].into(),
            ..Toolchain::default()
        };
        assert!(analyse(d.path(), &tc).is_err());
    }

    fn facts(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut f: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        f.entry("arch".to_owned())
            .or_insert_with(|| "x86_64".to_owned());
        f.entry("tool.apt-get".to_owned())
            .or_insert_with(|| "apt".to_owned());
        f
    }

    #[test]
    fn checks_flag_missing_and_too_old_tools_with_the_right_fix() {
        let d = dir_with(&[(
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.24)\nproject(x CXX)\n",
        )]);
        let n = analyse_dir(&d);
        let f = facts(&[
            ("want.cmake", "cmake version 3.16.3"),
            ("want.c++", "g++ 13.2.0"),
            ("want.make", ""),
            ("want.ccache", ""),
        ]);
        let c = checks(&n, &f, &[]);
        let by = |name: &str| c.iter().find(|c| c.name == name).unwrap();
        assert_eq!(by("cmake").level, Level::Fail);
        assert!(by("cmake").detail.contains("needs >=3.24"));
        let fix = by("cmake").fix.as_ref().unwrap();
        assert!(
            !fix.root
                && fix.command.contains("sha256sum -c")
                && fix.command.contains("cmake-3.30.5")
        );
        assert_eq!(by("c++").level, Level::Ok);
        let make = by("make");
        assert_eq!(make.level, Level::Fail);
        assert!(
            make.fix.as_ref().unwrap().root
                && make
                    .fix
                    .as_ref()
                    .unwrap()
                    .command
                    .contains("apt-get install -y make")
        );
        assert_eq!(by("ccache").level, Level::Warn, "optional tools only warn");
    }

    #[test]
    fn a_pin_that_cannot_satisfy_the_requirement_offers_no_fix() {
        let req = Req {
            tool: "go".to_owned(),
            min: Some(VersionReq::AtLeast(vec![9])),
            why: "go.mod".to_owned(),
            optional: false,
            approximate: false,
        };
        let fix = fix_for(&req, &facts(&[])).expect("falls back to the package");
        assert!(fix.root, "pinned go 1.27 cannot satisfy >=9");
    }

    #[test]
    fn every_pin_is_hashed_for_both_architectures_and_the_arch_never_reaches_the_command() {
        for pin in PINS {
            assert_eq!(pin.builds.len(), 2, "{}", pin.tool);
            for (_, url, algo, sum) in pin.builds {
                assert!(url.starts_with("https://"), "{url}");
                let want = if *algo == "sha512" { 128 } else { 64 };
                assert_eq!(sum.len(), want, "{url}");
                assert!(sum.bytes().all(|b| b.is_ascii_hexdigit()));
            }
            assert!(pin_install(pin, "x86_64; touch /tmp/pwned").is_none());
            assert!(
                pin_install(pin, "x86_64")
                    .unwrap()
                    .contains("sum -c --quiet")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn undo_is_derived_from_the_check_name_and_removes_only_its_own_links() {
        let home = tempfile::tempdir().unwrap();
        let (undo, root) = undo_of("go").unwrap();
        assert!(!root);
        let opt = home.path().join(".local/opt/goway-go/bin");
        std::fs::create_dir_all(&opt).unwrap();
        std::fs::write(opt.join("go"), "x").unwrap();
        let bin = home.path().join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(opt.join("go"), bin.join("go")).unwrap();
        std::fs::write(bin.join("other"), "mine").unwrap();
        let ok = std::process::Command::new("sh")
            .args(["-c", &undo])
            .env("HOME", home.path())
            .status()
            .unwrap();
        assert!(ok.success());
        assert!(!bin.join("go").exists() && !home.path().join(".local/opt").exists());
        assert!(bin.join("other").exists(), "foreign files stay");
        assert!(undo_of("evil; rm -rf /").is_none());
        assert!(is_package_check("make") && !is_package_check("go"));
    }
}
