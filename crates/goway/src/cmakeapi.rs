//! What `CMake` itself says a project needs, instead of reading `CMakeLists.txt`
//! as text.
//!
//! Three sources, all produced on a helper and parsed strictly here:
//!
//! - the **File API replies** (`codemodel-v2`, `cache-v2`, `cmakeFiles-v1`,
//!   `toolchains-v1`) that any configure writes into a build directory once
//!   a query is there (the remote script leaves one in every build directory
//!   of a slot tree): compilers with id and version, `<Pkg>_DIR` cache
//!   entries, `FETCHCONTENT_*` entries and the libraries targets link;
//! - a **`--trace-expand --trace-format=json-v1` configure** (`goway doctor
//!   --configure`): the project's own `find_package`, `FetchContent_Declare`,
//!   `pkg_check_modules` and `CPMAddPackage` calls with their arguments
//!   resolved;
//! - the configure's **stderr**, which names a package `find_package`
//!   could not find.
//!
//! Everything arrives in one framed bundle (see [`parse_bundle`]) with size
//! limits on every part; JSON that does not match the documented shape is an
//! error, never a guess. [`analyse`] turns the parts into the system packages
//! the project wants, through one table ([`LIB_PACKAGES`]) that maps them to
//! apt, dnf and pacman names. `FetchContent` and CPM dependencies are
//! downloaded by `CMake` itself and need no system package.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::remote::Call;

/// Largest single file accepted from a helper (4 MiB).
pub const MAX_FILE: usize = 4 * 1024 * 1024;
/// Most files in one bundle.
pub const MAX_FILES: usize = 400;
/// Most bytes in one bundle (16 MiB).
pub const MAX_TOTAL: usize = 16 * 1024 * 1024;
/// Most cache entries read from one reply.
const MAX_ENTRIES: usize = 50_000;
/// Most targets read from one codemodel.
const MAX_TARGETS: usize = 2_000;
/// Most trace lines read.
const MAX_TRACE_LINES: usize = 200_000;
/// Longest string kept from a reply or trace.
const MAX_STR: usize = 300;
/// First line of every bundle.
const HEADER: &str = "goway-cmake1";

/// Why a helper's answer could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CmakeError {
    /// The bundle's framing is wrong.
    #[error("the helper's CMake answer is malformed: {0}")]
    Frame(String),
    /// A reply or trace line is not the JSON the File API documents.
    #[error("{file}: {message}")]
    Json {
        /// The reply or trace.
        file: String,
        /// What is wrong.
        message: String,
    },
    /// A reply kind this goway does not understand (a newer major version).
    #[error("CMake wrote a {kind} reply of major version {major}, which goway does not read")]
    Unsupported {
        /// The reply kind.
        kind: String,
        /// Its major version.
        major: u64,
    },
    /// A file the index names is not in the bundle.
    #[error("the reply index names {0}, which the helper did not send")]
    Missing(String),
    /// A part is over its limit.
    #[error("{0} is over goway's size limit")]
    TooLarge(&'static str),
}

// ---- the calls --------------------------------------------------------

/// The call that reads the newest File API replies of repository `repo_id` on a helper.
pub fn replies_call(remote_root: &str, repo_id: &str) -> Call {
    Call::new("cmake-replies", &[remote_root, repo_id])
}

/// The call that runs one traced configure of the snapshot synced as `run_id`
/// (at most `seconds`), in that run's own labelled work directory.
pub fn configure_call(remote_root: &str, run_id: &str, seconds: u32) -> Call {
    Call::new(
        "cmake-configure",
        &[remote_root, run_id, &seconds.to_string()],
    )
}

// ---- the bundle -------------------------------------------------------

/// The replies of one build directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildReplies {
    /// Where on the helper, relative to the repository's cache (`tree-0/build`).
    pub label: String,
    /// File name to content.
    pub files: BTreeMap<String, Vec<u8>>,
}

/// A helper's whole answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bundle {
    /// The configure's exit code (`--configure` only; 127: no cmake there).
    pub rc: Option<i32>,
    /// Loose files: `stderr` and `trace.jsonl` of a configure.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Reply sets, in the order sent.
    pub builds: Vec<BuildReplies>,
}

fn valid_name(name: &str, slash: bool) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') || (slash && b == b'/')
        })
        && !name.contains("..")
}

/// Parse a helper's framed answer: `goway-cmake1`, then `@@rc N`, `@@build LABEL`,
/// `@@file NAME SIZE` (exactly SIZE bytes and a newline follow) and a final `@@end`.
///
/// # Errors
///
/// [`CmakeError::Frame`] for any deviation, [`CmakeError::TooLarge`] over a limit.
pub fn parse_bundle(bytes: &[u8]) -> Result<Bundle, CmakeError> {
    let bad = |m: &str| CmakeError::Frame(m.to_owned());
    let mut pos = 0usize;
    let line = |pos: &mut usize| -> Option<&[u8]> {
        let rest = bytes.get(*pos..)?;
        let end = rest.iter().position(|b| *b == b'\n')?;
        *pos += end + 1;
        Some(&rest[..end])
    };
    if line(&mut pos) != Some(HEADER.as_bytes()) {
        return Err(bad("no goway-cmake1 header"));
    }
    let mut bundle = Bundle::default();
    let (mut files, mut total) = (0usize, 0usize);
    loop {
        let Some(l) = line(&mut pos) else {
            // A helper without CMake answers the header alone.
            return if bundle == Bundle::default() && pos >= bytes.len() {
                Ok(bundle)
            } else {
                Err(bad("no @@end"))
            };
        };
        let l = std::str::from_utf8(l).map_err(|_| bad("a header line is not text"))?;
        let mut words = l.split(' ');
        match (words.next(), words.next(), words.next(), words.next()) {
            (Some("@@end"), None, ..) => {
                return if bytes[pos..].iter().all(u8::is_ascii_whitespace) {
                    Ok(bundle)
                } else {
                    Err(bad("data after @@end"))
                };
            }
            (Some("@@rc"), Some(n), None, ..) => {
                bundle.rc = Some(n.parse().map_err(|_| bad("@@rc is not a number"))?);
            }
            (Some("@@build"), Some(label), None, ..) if valid_name(label, true) => {
                bundle.builds.push(BuildReplies {
                    label: label.to_owned(),
                    files: BTreeMap::new(),
                });
            }
            (Some("@@file"), Some(name), Some(size), None) if valid_name(name, false) => {
                let size: usize = size
                    .parse()
                    .map_err(|_| bad("a file size is not a number"))?;
                if size > MAX_FILE {
                    return Err(CmakeError::TooLarge("a file"));
                }
                files += 1;
                total += size;
                if files > MAX_FILES {
                    return Err(CmakeError::TooLarge("the number of files"));
                }
                if total > MAX_TOTAL {
                    return Err(CmakeError::TooLarge("the answer"));
                }
                let body = bytes
                    .get(pos..pos + size)
                    .ok_or_else(|| bad("a file is shorter than it says"))?;
                if bytes.get(pos + size) != Some(&b'\n') {
                    return Err(bad("a file is longer than it says"));
                }
                pos += size + 1;
                let target = match bundle.builds.last_mut() {
                    Some(b) => &mut b.files,
                    None => &mut bundle.files,
                };
                if target.insert(name.to_owned(), body.to_vec()).is_some() {
                    return Err(bad("a file is sent twice"));
                }
            }
            _ => return Err(bad("an unknown line")),
        }
    }
}

// ---- File API replies -------------------------------------------------

/// A compiler `CMake` found (from `toolchains-v1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiler {
    /// `C`, `CXX`, `CUDA`, ...
    pub language: String,
    /// `CMake`'s compiler id (`GNU`, `Clang`, `AppleClang`, ...).
    pub id: Option<String>,
    /// Its version.
    pub version: Option<String>,
    /// Where it is.
    pub path: Option<String>,
}

/// A `<Pkg>_DIR` cache entry: a package `find_package` looked for in config mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPackage {
    /// The package name.
    pub name: String,
    /// Whether it was found (`<Pkg>_DIR-NOTFOUND` is not).
    pub found: bool,
}

/// What the File API replies of one build directory say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replies {
    /// `CMake`'s own version.
    pub cmake_version: Option<String>,
    /// The generator (`Unix Makefiles`, `Ninja`).
    pub generator: Option<String>,
    /// The compilers.
    pub compilers: Vec<Compiler>,
    /// `<Pkg>_DIR` entries.
    pub packages: Vec<CachedPackage>,
    /// Dependencies `FetchContent` fetched (cache `FETCHCONTENT_SOURCE_DIR_*`).
    pub fetched: BTreeSet<String>,
    /// Programs a find command could not find (`PKG_CONFIG_EXECUTABLE` -> `pkg-config`).
    pub missing_programs: BTreeSet<String>,
    /// Libraries targets link by name or system path (`z`, `ssl`).
    pub libraries: BTreeSet<String>,
}

#[derive(Deserialize)]
struct Version {
    major: u64,
    #[serde(default)]
    string: Option<String>,
}

#[derive(Deserialize)]
struct IndexCmake {
    #[serde(default)]
    version: Option<Version>,
    #[serde(default)]
    generator: Option<IndexGenerator>,
}

#[derive(Deserialize)]
struct IndexGenerator {
    name: String,
}

#[derive(Deserialize)]
struct IndexObject {
    kind: String,
    version: Version,
    #[serde(rename = "jsonFile")]
    json_file: String,
}

#[derive(Deserialize)]
struct Index {
    cmake: IndexCmake,
    objects: Vec<IndexObject>,
}

#[derive(Deserialize)]
struct Toolchains {
    toolchains: Vec<ToolchainEntry>,
}

#[derive(Deserialize)]
struct ToolchainEntry {
    language: String,
    #[serde(default)]
    compiler: Option<ToolchainCompiler>,
}

#[derive(Deserialize)]
struct ToolchainCompiler {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
struct Cache {
    entries: Vec<CacheEntry>,
}

#[derive(Deserialize)]
struct CacheEntry {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    value: String,
}

#[derive(Deserialize)]
struct Codemodel {
    configurations: Vec<CodemodelConfig>,
}

#[derive(Deserialize)]
struct CodemodelConfig {
    #[serde(default)]
    targets: Vec<CodemodelTarget>,
}

#[derive(Deserialize)]
struct CodemodelTarget {
    #[serde(rename = "jsonFile")]
    json_file: String,
}

#[derive(Deserialize)]
struct Target {
    #[serde(default)]
    link: Option<TargetLink>,
}

#[derive(Deserialize)]
struct TargetLink {
    #[serde(rename = "commandFragments", default)]
    fragments: Vec<Fragment>,
}

#[derive(Deserialize)]
struct Fragment {
    fragment: String,
    role: String,
}

fn clip(s: &str) -> String {
    crate::render::clean(&s.chars().take(MAX_STR).collect::<String>())
}

fn parse<'a, T: Deserialize<'a>>(file: &str, bytes: &'a [u8]) -> Result<T, CmakeError> {
    serde_json::from_slice(bytes).map_err(|e| CmakeError::Json {
        file: file.to_owned(),
        message: e.to_string(),
    })
}

fn file<'a>(files: &'a BTreeMap<String, Vec<u8>>, name: &str) -> Result<&'a [u8], CmakeError> {
    if !valid_name(name, false) {
        return Err(CmakeError::Missing(clip(name)));
    }
    files
        .get(name)
        .map(Vec::as_slice)
        .ok_or_else(|| CmakeError::Missing(name.to_owned()))
}

/// Libraries every C or C++ program links implicitly: never a package.
const IMPLICIT_LIBS: &[&str] = &[
    "c", "m", "dl", "rt", "pthread", "util", "gcc", "gcc_s", "stdc++", "c++", "c++abi", "atomic",
];

/// The library name of a link fragment (`-lz`, `/usr/lib/x86_64-linux-gnu/libz.so`), when it is
/// a system library: relative paths are the build's own.
fn system_library(fragment: &str) -> Option<String> {
    let name = if let Some(n) = fragment.strip_prefix("-l") {
        n.to_owned()
    } else if fragment.starts_with("/usr/") || fragment.starts_with("/lib") {
        let base = fragment.rsplit('/').next()?;
        let stem = base.strip_prefix("lib")?;
        stem.split(".so").next()?.trim_end_matches(".a").to_owned()
    } else {
        return None;
    };
    (!name.is_empty()
        && name.len() <= 60
        && !IMPLICIT_LIBS.contains(&name.as_str())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'.')))
    .then_some(name)
}

/// Read the newest reply set in `files` (the `index-*.json` with the greatest name).
///
/// # Errors
///
/// [`CmakeError`] when the index or an object it names is not what the File API documents.
pub fn read_replies(files: &BTreeMap<String, Vec<u8>>) -> Result<Replies, CmakeError> {
    let (index_name, index_bytes) = files
        .iter()
        .rfind(|(n, _)| {
            n.starts_with("index-")
                && std::path::Path::new(n.as_str())
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        })
        .ok_or_else(|| CmakeError::Missing("index-*.json".to_owned()))?;
    let index: Index = parse(index_name, index_bytes)?;
    let mut out = Replies {
        cmake_version: index.cmake.version.and_then(|v| v.string).map(|v| clip(&v)),
        generator: index.cmake.generator.map(|g| clip(&g.name)),
        ..Replies::default()
    };
    for object in &index.objects {
        let expected = match object.kind.as_str() {
            "toolchains" | "cmakeFiles" => 1,
            "cache" | "codemodel" => 2,
            _ => continue,
        };
        if object.version.major != expected {
            return Err(CmakeError::Unsupported {
                kind: clip(&object.kind),
                major: object.version.major,
            });
        }
        let bytes = file(files, &object.json_file)?;
        match object.kind.as_str() {
            "toolchains" => read_toolchains(&mut out, &object.json_file, bytes)?,
            "cache" => read_cache(&mut out, &object.json_file, bytes)?,
            "codemodel" => read_codemodel(&mut out, files, &object.json_file, bytes)?,
            _ => {}
        }
    }
    Ok(out)
}

fn read_toolchains(out: &mut Replies, name: &str, bytes: &[u8]) -> Result<(), CmakeError> {
    let t: Toolchains = parse(name, bytes)?;
    for e in t.toolchains.into_iter().take(16) {
        let c = e.compiler;
        out.compilers.push(Compiler {
            language: clip(&e.language),
            id: c.as_ref().and_then(|c| c.id.as_deref()).map(clip),
            version: c.as_ref().and_then(|c| c.version.as_deref()).map(clip),
            path: c.as_ref().and_then(|c| c.path.as_deref()).map(clip),
        });
    }
    Ok(())
}

fn read_cache(out: &mut Replies, name: &str, bytes: &[u8]) -> Result<(), CmakeError> {
    let cache: Cache = parse(name, bytes)?;
    if cache.entries.len() > MAX_ENTRIES {
        return Err(CmakeError::TooLarge("the cache reply"));
    }
    for e in &cache.entries {
        if let Some(dep) = e.name.strip_prefix("FETCHCONTENT_SOURCE_DIR_") {
            out.fetched.insert(clip(dep));
        } else if e.kind == "PATH"
            && let Some(pkg) = e.name.strip_suffix("_DIR")
            && !pkg.is_empty()
            && !pkg.starts_with("CMAKE_")
            && !pkg.starts_with("FETCHCONTENT_")
            && !pkg.starts_with("CPM_")
        {
            out.packages.push(CachedPackage {
                name: clip(pkg),
                found: !e.value.ends_with("-NOTFOUND"),
            });
        } else if e.kind == "FILEPATH"
            && e.value.ends_with("-NOTFOUND")
            && let Some(var) = e.name.strip_suffix("_EXECUTABLE")
            && let Some(tool) = program_for(var)
        {
            out.missing_programs.insert(tool.to_owned());
        }
    }
    Ok(())
}

/// The program a `<VAR>_EXECUTABLE` cache variable looks for, for the few that matter.
fn program_for(var: &str) -> Option<&'static str> {
    match var {
        "PKG_CONFIG" => Some("pkg-config"),
        "DOXYGEN" => Some("doxygen"),
        "GIT" => Some("git"),
        _ => None,
    }
}

fn read_codemodel(
    out: &mut Replies,
    files: &BTreeMap<String, Vec<u8>>,
    name: &str,
    bytes: &[u8],
) -> Result<(), CmakeError> {
    let model: Codemodel = parse(name, bytes)?;
    let targets: Vec<&CodemodelTarget> = model
        .configurations
        .iter()
        .flat_map(|c| &c.targets)
        .collect();
    if targets.len() > MAX_TARGETS {
        return Err(CmakeError::TooLarge("the codemodel"));
    }
    for t in targets {
        let target: Target = parse(&t.json_file, file(files, &t.json_file)?)?;
        for f in target.link.into_iter().flat_map(|l| l.fragments) {
            if f.role == "libraries"
                && let Some(lib) = system_library(f.fragment.trim())
            {
                out.libraries.insert(lib);
            }
        }
    }
    Ok(())
}

// ---- the trace --------------------------------------------------------

/// One command `CMake` ran, from the `json-v1` trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceCall {
    /// The command, lower case.
    pub cmd: String,
    /// Its arguments, expanded.
    pub args: Vec<String>,
    /// The file it ran in.
    pub file: String,
    /// The line.
    pub line: u64,
    /// In the project's own files (not `CMake`'s modules, fetched dependencies or try-compile).
    pub in_project: bool,
}

/// A parsed `--trace-format=json-v1` trace of a configure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trace {
    /// The project's source directory (where the first `cmake_minimum_required` ran).
    pub source_dir: Option<String>,
    /// The commands goway asked to see.
    pub calls: Vec<TraceCall>,
}

#[derive(Deserialize)]
struct TraceVersion {
    version: TraceVersionNumber,
}

#[derive(Deserialize)]
struct TraceVersionNumber {
    major: u64,
}

#[derive(Deserialize)]
struct TraceLine {
    cmd: String,
    #[serde(default)]
    args: Vec<String>,
    file: String,
    #[serde(default)]
    line: u64,
}

/// Parse a trace. The first line is the version (major 1 only); every other
/// line is one command. A final line cut by the size limit is dropped.
///
/// # Errors
///
/// [`CmakeError::Json`] for any other line that is not valid.
pub fn parse_trace(text: &str) -> Result<Trace, CmakeError> {
    let err = |n: usize, m: String| CmakeError::Json {
        file: format!("trace line {n}"),
        message: m,
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() > MAX_TRACE_LINES {
        return Err(CmakeError::TooLarge("the trace"));
    }
    let Some(first) = lines.first() else {
        return Ok(Trace::default());
    };
    let v: TraceVersion = serde_json::from_str(first).map_err(|e| err(1, e.to_string()))?;
    if v.version.major != 1 {
        return Err(CmakeError::Unsupported {
            kind: "trace".to_owned(),
            major: v.version.major,
        });
    }
    let mut trace = Trace::default();
    let last = lines.len() - 1;
    for (i, l) in lines.iter().enumerate().skip(1) {
        let parsed: TraceLine = match serde_json::from_str(l) {
            Ok(p) => p,
            Err(_) if i == last => break,
            Err(e) => return Err(err(i + 1, e.to_string())),
        };
        let cmd = parsed.cmd.to_ascii_lowercase();
        if trace.source_dir.is_none() && cmd == "cmake_minimum_required" {
            trace.source_dir = parsed.file.rsplit_once('/').map(|(dir, _)| dir.to_owned());
        }
        trace.calls.push(TraceCall {
            cmd,
            args: parsed.args.iter().map(|a| clip(a)).collect(),
            line: parsed.line,
            in_project: false,
            file: clip(&parsed.file),
        });
    }
    if let Some(src) = trace.source_dir.clone() {
        let prefix = format!("{src}/");
        for c in &mut trace.calls {
            c.in_project = c.file.starts_with(&prefix)
                && !c.file.contains("/CMakeFiles/")
                && !c.file.contains("/_deps/");
        }
    }
    Ok(trace)
}

/// A `find_package` call of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageUse {
    /// The package name as written.
    pub name: String,
    /// Some call says `REQUIRED`.
    pub required: bool,
    /// The version asked for.
    pub version: Option<String>,
    /// Where.
    pub at: String,
}

/// A dependency `CMake` downloads itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// Its name.
    pub name: String,
    /// `FetchContent` or `CPM`.
    pub via: &'static str,
}

impl Trace {
    /// The project's `find_package` calls, one per package (first call's place, any `REQUIRED`).
    pub fn find_packages(&self) -> Vec<PackageUse> {
        let mut out: Vec<PackageUse> = Vec::new();
        for c in self
            .calls
            .iter()
            .filter(|c| c.in_project && c.cmd == "find_package")
        {
            let Some(name) = c.args.first().filter(|n| valid_package(n)) else {
                continue;
            };
            let required = c.args.iter().any(|a| a == "REQUIRED");
            let version = c
                .args
                .get(1)
                .filter(|v| v.starts_with(|ch: char| ch.is_ascii_digit()))
                .cloned();
            if let Some(known) = out.iter_mut().find(|p| p.name.eq_ignore_ascii_case(name)) {
                known.required |= required;
                continue;
            }
            out.push(PackageUse {
                name: name.clone(),
                required,
                version,
                at: format!("{}:{}", short(&c.file), c.line),
            });
        }
        out
    }

    /// The modules the project's `pkg_check_modules` and `pkg_search_module` calls ask pkg-config
    /// for (version constraints removed).
    pub fn pkg_config_modules(&self) -> Vec<String> {
        let mut out = Vec::new();
        for c in self.calls.iter().filter(|c| {
            c.in_project && matches!(c.cmd.as_str(), "pkg_check_modules" | "pkg_search_module")
        }) {
            for a in c.args.iter().skip(1) {
                if matches!(
                    a.as_str(),
                    "REQUIRED"
                        | "QUIET"
                        | "IMPORTED_TARGET"
                        | "GLOBAL"
                        | "NO_CMAKE_PATH"
                        | "NO_CMAKE_ENVIRONMENT_PATH"
                ) {
                    continue;
                }
                let name = a.split(['<', '>', '=', '!']).next().unwrap_or("");
                if valid_package(name) && !out.iter().any(|m| m == name) {
                    out.push(name.to_owned());
                }
            }
        }
        out
    }

    /// The dependencies the project has `CMake` download.
    pub fn fetched(&self) -> Vec<Fetched> {
        let mut out: Vec<Fetched> = Vec::new();
        for c in self.calls.iter().filter(|c| c.in_project) {
            let found = match c.cmd.as_str() {
                "fetchcontent_declare" => c.args.first().map(|n| (n.clone(), "FetchContent")),
                "cpmaddpackage" | "cpmfindpackage" | "cpmdeclarepackage" => {
                    cpm_name(&c.args).map(|n| (n, "CPM"))
                }
                _ => None,
            };
            if let Some((name, via)) = found
                && valid_package(&name)
                && !out.iter().any(|f| f.name == name)
            {
                out.push(Fetched { name, via });
            }
        }
        out
    }
}

/// The name a `CPMAddPackage` call gives: `NAME x`, or the repository of `gh:user/repo#tag`.
fn cpm_name(args: &[String]) -> Option<String> {
    if let Some(i) = args.iter().position(|a| a == "NAME") {
        return args.get(i + 1).cloned();
    }
    let first = args.first()?;
    if args.len() == 1 && !first.contains(char::is_whitespace) {
        let spec = first.split_once(':').map_or(first.as_str(), |(_, r)| r);
        let repo = spec.rsplit('/').next()?;
        return Some(repo.split(['#', '@']).next()?.to_owned());
    }
    args.iter()
        .position(|a| a == "GITHUB_REPOSITORY" || a == "GITLAB_REPOSITORY")
        .and_then(|i| args.get(i + 1))
        .and_then(|r| r.rsplit('/').next())
        .map(str::to_owned)
}

fn short(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether `name` is plausible as a package, module or dependency name.
fn valid_package(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 60
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'.'))
}

/// The packages a configure's stderr says `find_package` could not find, in order.
///
/// Recognised: `Could NOT find NAME (...)` (a find module) and `Could not find a
/// package configuration file provided by "NAME"` (config mode).
pub fn missing_from_stderr(stderr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let text = stderr.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut add = |name: &str| {
        if valid_package(name) && !out.iter().any(|m| m.eq_ignore_ascii_case(name)) {
            out.push(name.to_owned());
        }
    };
    let mut rest = text.as_str();
    while let Some(i) = rest.find("Could NOT find ") {
        rest = &rest[i + "Could NOT find ".len()..];
        let name = rest
            .split(|c: char| c.is_whitespace() || c == ':' || c == '(')
            .next()
            .unwrap_or("");
        add(name);
    }
    for marker in [
        "Could not find a package configuration file provided by \"",
        "Could not find a configuration file for package \"",
    ] {
        let mut rest = text.as_str();
        while let Some(i) = rest.find(marker) {
            rest = &rest[i + marker.len()..];
            add(rest.split('"').next().unwrap_or(""));
        }
    }
    out
}

// ---- the package table ------------------------------------------------

/// A system package `CMake` projects commonly want, with its names per package manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibPackage {
    /// The `find_package` name.
    pub cmake: &'static str,
    /// The library name the package provides (`-lz`), or empty.
    pub library: &'static str,
    /// The pkg-config module name, or empty.
    pub pkg_config: &'static str,
    /// Debian and Ubuntu.
    pub apt: &'static str,
    /// Fedora and the RPM family.
    pub dnf: &'static str,
    /// Arch.
    pub pacman: &'static str,
}

/// The one table that maps what `CMake` looks for to apt, dnf and pacman packages.
#[rustfmt::skip]
pub const LIB_PACKAGES: &[LibPackage] = &[
    LibPackage {
        cmake: "GTest",
        library: "gtest",
        pkg_config: "gtest",
        apt: "libgtest-dev",
        dnf: "gtest-devel",
        pacman: "gtest",
    },
    LibPackage {
        cmake: "GMock",
        library: "gmock",
        pkg_config: "gmock",
        apt: "libgmock-dev",
        dnf: "gmock-devel",
        pacman: "gtest",
    },
    LibPackage {
        cmake: "Catch2",
        library: "",
        pkg_config: "catch2",
        apt: "catch2",
        dnf: "catch2-devel",
        pacman: "catch2",
    },
    LibPackage {
        cmake: "benchmark",
        library: "benchmark",
        pkg_config: "benchmark",
        apt: "libbenchmark-dev",
        dnf: "google-benchmark-devel",
        pacman: "benchmark",
    },
    LibPackage {
        cmake: "ZLIB",
        library: "z",
        pkg_config: "zlib",
        apt: "zlib1g-dev",
        dnf: "zlib-devel",
        pacman: "zlib",
    },
    LibPackage {
        cmake: "BZip2",
        library: "bz2",
        pkg_config: "bzip2",
        apt: "libbz2-dev",
        dnf: "bzip2-devel",
        pacman: "bzip2",
    },
    LibPackage {
        cmake: "LibLZMA",
        library: "lzma",
        pkg_config: "liblzma",
        apt: "liblzma-dev",
        dnf: "xz-devel",
        pacman: "xz",
    },
    LibPackage {
        cmake: "OpenSSL",
        library: "ssl",
        pkg_config: "openssl",
        apt: "libssl-dev",
        dnf: "openssl-devel",
        pacman: "openssl",
    },
    LibPackage {
        cmake: "CURL",
        library: "curl",
        pkg_config: "libcurl",
        apt: "libcurl4-openssl-dev",
        dnf: "libcurl-devel",
        pacman: "curl",
    },
    LibPackage {
        cmake: "Boost",
        library: "boost_system",
        pkg_config: "",
        apt: "libboost-all-dev",
        dnf: "boost-devel",
        pacman: "boost",
    },
    LibPackage {
        cmake: "Protobuf",
        library: "protobuf",
        pkg_config: "protobuf",
        apt: "libprotobuf-dev protobuf-compiler",
        dnf: "protobuf-devel",
        pacman: "protobuf",
    },
    LibPackage {
        cmake: "fmt",
        library: "fmt",
        pkg_config: "fmt",
        apt: "libfmt-dev",
        dnf: "fmt-devel",
        pacman: "fmt",
    },
    LibPackage {
        cmake: "spdlog",
        library: "spdlog",
        pkg_config: "spdlog",
        apt: "libspdlog-dev",
        dnf: "spdlog-devel",
        pacman: "spdlog",
    },
    LibPackage {
        cmake: "Eigen3",
        library: "",
        pkg_config: "eigen3",
        apt: "libeigen3-dev",
        dnf: "eigen3-devel",
        pacman: "eigen",
    },
    LibPackage {
        cmake: "nlohmann_json",
        library: "",
        pkg_config: "nlohmann_json",
        apt: "nlohmann-json3-dev",
        dnf: "json-devel",
        pacman: "nlohmann-json",
    },
    LibPackage {
        cmake: "yaml-cpp",
        library: "yaml-cpp",
        pkg_config: "yaml-cpp",
        apt: "libyaml-cpp-dev",
        dnf: "yaml-cpp-devel",
        pacman: "yaml-cpp",
    },
    LibPackage {
        cmake: "PNG",
        library: "png",
        pkg_config: "libpng",
        apt: "libpng-dev",
        dnf: "libpng-devel",
        pacman: "libpng",
    },
    LibPackage {
        cmake: "JPEG",
        library: "jpeg",
        pkg_config: "libjpeg",
        apt: "libjpeg-dev",
        dnf: "libjpeg-turbo-devel",
        pacman: "libjpeg-turbo",
    },
    LibPackage {
        cmake: "SQLite3",
        library: "sqlite3",
        pkg_config: "sqlite3",
        apt: "libsqlite3-dev",
        dnf: "sqlite-devel",
        pacman: "sqlite",
    },
    LibPackage {
        cmake: "LibXml2",
        library: "xml2",
        pkg_config: "libxml-2.0",
        apt: "libxml2-dev",
        dnf: "libxml2-devel",
        pacman: "libxml2",
    },
    LibPackage {
        cmake: "SDL2",
        library: "SDL2",
        pkg_config: "sdl2",
        apt: "libsdl2-dev",
        dnf: "SDL2-devel",
        pacman: "sdl2",
    },
    LibPackage {
        cmake: "OpenGL",
        library: "GL",
        pkg_config: "gl",
        apt: "libgl-dev",
        dnf: "mesa-libGL-devel",
        pacman: "libglvnd",
    },
    LibPackage {
        cmake: "GLEW",
        library: "GLEW",
        pkg_config: "glew",
        apt: "libglew-dev",
        dnf: "glew-devel",
        pacman: "glew",
    },
    LibPackage {
        cmake: "TBB",
        library: "tbb",
        pkg_config: "tbb",
        apt: "libtbb-dev",
        dnf: "tbb-devel",
        pacman: "onetbb",
    },
    LibPackage {
        cmake: "LAPACK",
        library: "lapack",
        pkg_config: "lapack",
        apt: "liblapack-dev",
        dnf: "lapack-devel",
        pacman: "lapack",
    },
    LibPackage {
        cmake: "BLAS",
        library: "blas",
        pkg_config: "blas",
        apt: "libblas-dev",
        dnf: "blas-devel",
        pacman: "blas",
    },
    LibPackage {
        cmake: "Python3",
        library: "",
        pkg_config: "python3",
        apt: "python3-dev",
        dnf: "python3-devel",
        pacman: "python",
    },
    LibPackage {
        cmake: "PkgConfig",
        library: "",
        pkg_config: "",
        apt: "pkg-config",
        dnf: "pkgconf-pkg-config",
        pacman: "pkgconf",
    },
    LibPackage {
        cmake: "Doxygen",
        library: "",
        pkg_config: "",
        apt: "doxygen",
        dnf: "doxygen",
        pacman: "doxygen",
    },
];

/// Packages that need nothing installed (`CMake`'s own modules and compiler support).
const BUILT_IN: &[&str] = &[
    "Threads",
    "OpenMP",
    "CUDAToolkit",
    "Git",
    "Perl",
    "Java",
    "Ruby",
];

impl LibPackage {
    /// The package named `name` in `find_package(name)` (case-insensitive).
    pub fn by_cmake(name: &str) -> Option<&'static Self> {
        LIB_PACKAGES
            .iter()
            .find(|p| p.cmake.eq_ignore_ascii_case(name))
    }

    /// The package that provides library `lib` (`-lz`); `boost_*` all map to Boost.
    pub fn by_library(lib: &str) -> Option<&'static Self> {
        if lib.starts_with("boost_") {
            return Self::by_cmake("Boost");
        }
        let lib = if lib == "crypto" { "ssl" } else { lib };
        LIB_PACKAGES
            .iter()
            .find(|p| !p.library.is_empty() && p.library == lib)
    }

    /// The package that provides the pkg-config module `module`.
    pub fn by_pkg_config(module: &str) -> Option<&'static Self> {
        LIB_PACKAGES
            .iter()
            .find(|p| !p.pkg_config.is_empty() && p.pkg_config.eq_ignore_ascii_case(module))
    }
}

// ---- the analysis -----------------------------------------------------

/// Whether a package `CMake` looked for is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// `CMake` did not find it.
    Missing,
    /// `CMake` found it (or linked it).
    Present,
    /// Nothing says.
    Unknown,
}

/// One system package the project wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemNeed {
    /// What `CMake` calls it (`GTest`, `zlib` for a pkg-config module, `-lz` as `z`).
    pub name: String,
    /// Whether it is there.
    pub presence: Presence,
    /// The packages that provide it, when the table knows.
    pub package: Option<&'static LibPackage>,
    /// What asks for it, for the verdict line.
    pub why: String,
    /// The project cannot configure without it.
    pub required: bool,
}

/// Everything `CMake` told goway about a project, as doctor uses it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    /// `CMake`'s own version, when a reply says.
    pub cmake_version: Option<String>,
    /// The compilers `CMake` found.
    pub compilers: Vec<Compiler>,
    /// System packages the project wants.
    pub system: Vec<SystemNeed>,
    /// Dependencies `CMake` downloads itself: no system package.
    pub fetched: Vec<Fetched>,
    /// Programs a find command lacked (`pkg-config`).
    pub missing_programs: Vec<String>,
    /// The configure failed (`--configure`).
    pub configure_failed: bool,
}

fn push_need(
    system: &mut Vec<SystemNeed>,
    name: &str,
    presence: Presence,
    package: Option<&'static LibPackage>,
    why: String,
    required: bool,
) {
    if let Some(known) = system
        .iter_mut()
        .find(|n| n.name.eq_ignore_ascii_case(name))
    {
        if presence == Presence::Missing || known.presence == Presence::Unknown {
            known.presence = presence;
        }
        known.required |= required;
        return;
    }
    system.push(SystemNeed {
        name: name.to_owned(),
        presence,
        package,
        why,
        required,
    });
}

/// Combine what is known: File API replies, a traced configure and its stderr.
/// Any part may be absent.
pub fn analyse(
    replies: Option<&Replies>,
    trace: Option<&Trace>,
    stderr: Option<&str>,
    rc: Option<i32>,
) -> Analysis {
    let mut a = Analysis {
        cmake_version: replies.and_then(|r| r.cmake_version.clone()),
        compilers: replies.map(|r| r.compilers.clone()).unwrap_or_default(),
        configure_failed: rc.is_some_and(|c| c != 0),
        ..Analysis::default()
    };
    let missing_named: Vec<String> = stderr.map(missing_from_stderr).unwrap_or_default();
    if let Some(t) = trace {
        let configured = replies.is_some() || rc == Some(0);
        analyse_trace(&mut a, t, replies, &missing_named, configured);
    }
    if let Some(r) = replies {
        analyse_replies(&mut a, r);
    }
    for m in &missing_named {
        push_need(
            &mut a.system,
            m,
            Presence::Missing,
            LibPackage::by_cmake(m),
            "find_package could not find it".to_owned(),
            true,
        );
    }
    a
}

/// What the traced configure's `find_package` and `pkg_check_modules` calls add.
fn analyse_trace(
    a: &mut Analysis,
    t: &Trace,
    replies: Option<&Replies>,
    missing_named: &[String],
    configured: bool,
) {
    let cached = |name: &str| {
        replies.and_then(|r| {
            r.packages
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name))
        })
    };
    for p in t.find_packages() {
        if BUILT_IN.iter().any(|b| b.eq_ignore_ascii_case(&p.name)) {
            continue;
        }
        let presence = if missing_named
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&p.name))
            || cached(&p.name).is_some_and(|c| !c.found)
        {
            Presence::Missing
        } else if configured && (p.required || cached(&p.name).is_some_and(|c| c.found)) {
            Presence::Present
        } else {
            Presence::Unknown
        };
        let why = format!(
            "find_package({}{}) at {}",
            p.name,
            p.version
                .as_deref()
                .map_or(String::new(), |v| format!(" {v}")),
            p.at
        );
        push_need(
            &mut a.system,
            &p.name,
            presence,
            LibPackage::by_cmake(&p.name),
            why,
            p.required,
        );
    }
    for module in t.pkg_config_modules() {
        let presence = if configured {
            Presence::Present
        } else {
            Presence::Unknown
        };
        push_need(
            &mut a.system,
            &module,
            presence,
            LibPackage::by_pkg_config(&module),
            format!("pkg_check_modules({module})"),
            false,
        );
    }
    a.fetched = t.fetched();
}

/// What the File API replies add: cache packages, linked libraries, fetched dependencies.
fn analyse_replies(a: &mut Analysis, r: &Replies) {
    for p in &r.packages {
        if BUILT_IN.iter().any(|b| b.eq_ignore_ascii_case(&p.name)) {
            continue;
        }
        let presence = if p.found {
            Presence::Present
        } else {
            Presence::Missing
        };
        push_need(
            &mut a.system,
            &p.name,
            presence,
            LibPackage::by_cmake(&p.name),
            format!("{}_DIR in CMake's cache", p.name),
            false,
        );
    }
    for lib in &r.libraries {
        if let Some(pkg) = LibPackage::by_library(lib)
            && !a
                .system
                .iter()
                .any(|n| n.name.eq_ignore_ascii_case(pkg.cmake))
        {
            push_need(
                &mut a.system,
                pkg.cmake,
                Presence::Present,
                Some(pkg),
                format!("a target links -l{lib}"),
                false,
            );
        }
    }
    for dep in &r.fetched {
        if !a.fetched.iter().any(|f| f.name.eq_ignore_ascii_case(dep)) {
            a.fetched.push(Fetched {
                name: dep.clone(),
                via: "FetchContent",
            });
        }
    }
    a.missing_programs = r.missing_programs.iter().cloned().collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/cmake_api/{rel}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    fn reply_files(project: &str) -> BTreeMap<String, Vec<u8>> {
        let dir = format!(
            "{}/tests/fixtures/cmake_api/{project}/reply",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    std::fs::read(&p).unwrap(),
                )
            })
            .collect()
    }

    fn frame(
        rc: Option<i32>,
        loose: &[(&str, &[u8])],
        builds: &[(&str, &BTreeMap<String, Vec<u8>>)],
    ) -> Vec<u8> {
        let mut out = format!("{HEADER}\n").into_bytes();
        if let Some(rc) = rc {
            out.extend(format!("@@rc {rc}\n").bytes());
        }
        for (n, b) in loose {
            out.extend(format!("@@file {n} {}\n", b.len()).bytes());
            out.extend(*b);
            out.push(b'\n');
        }
        for (label, files) in builds {
            out.extend(format!("@@build {label}\n").bytes());
            for (n, b) in *files {
                out.extend(format!("@@file {n} {}\n", b.len()).bytes());
                out.extend(b);
                out.push(b'\n');
            }
        }
        out.extend(b"@@end\n");
        out
    }

    // frob:tests crates/goway/src/cmakeapi.rs::read_replies
    #[test]
    fn the_replies_of_the_real_fetchcontent_project_name_compilers_and_fetched_dependencies() {
        let r = read_replies(&reply_files("cxxshard")).unwrap();
        assert_eq!(r.cmake_version.as_deref(), Some("3.28.3"));
        assert_eq!(r.generator.as_deref(), Some("Unix Makefiles"));
        // googletest's own project enables C as well.
        let c = r.compilers.iter().find(|c| c.language == "CXX").unwrap();
        assert_eq!((c.language.as_str(), c.id.as_deref()), ("CXX", Some("GNU")));
        assert!(
            c.version.as_deref().is_some_and(|v| v.starts_with("13.")),
            "{c:?}"
        );
        assert_eq!(
            r.fetched.iter().map(String::as_str).collect::<Vec<_>>(),
            ["CATCH2", "GOOGLETEST"]
        );
        assert!(
            r.packages.is_empty(),
            "fetched dependencies are not _DIR packages: {:?}",
            r.packages
        );
        assert!(
            r.libraries.is_empty(),
            "the project links its own static libraries: {:?}",
            r.libraries
        );
    }

    // frob:tests crates/goway/src/cmakeapi.rs::read_replies
    #[test]
    fn a_package_that_was_not_found_shows_as_a_notfound_cache_entry() {
        let r = read_replies(&reply_files("cmakeplain")).unwrap();
        assert_eq!(
            r.packages,
            [CachedPackage {
                name: "Catch2".to_owned(),
                found: false
            }]
        );
        assert_eq!(
            r.missing_programs.iter().collect::<Vec<_>>(),
            ["pkg-config"]
        );
    }

    // frob:tests crates/goway/src/cmakeapi.rs::read_replies
    #[test]
    fn replies_that_are_not_what_the_file_api_documents_are_errors() {
        let mut files = reply_files("cmakeplain");
        let index = files
            .keys()
            .find(|k| k.starts_with("index-"))
            .unwrap()
            .clone();
        let tc = files
            .keys()
            .find(|k| k.starts_with("toolchains-"))
            .unwrap()
            .clone();
        let good = files[&index].clone();
        // A major version this goway does not read.
        let text = String::from_utf8(good.clone())
            .unwrap()
            .replace("\"major\": 2,", "\"major\": 9,");
        files.insert(index.clone(), text.into_bytes());
        assert!(
            matches!(read_replies(&files), Err(CmakeError::Unsupported { .. })),
            "{:?}",
            read_replies(&files)
        );
        // The index names a file the helper did not send.
        files.insert(index.clone(), good.clone());
        files.remove(&tc);
        assert_eq!(read_replies(&files), Err(CmakeError::Missing(tc.clone())));
        // Not JSON, wrong shape, no index at all.
        files.insert(tc.clone(), b"{not json".to_vec());
        assert!(matches!(read_replies(&files), Err(CmakeError::Json { .. })));
        files.insert(tc, br#"{"toolchains": 3}"#.to_vec());
        assert!(matches!(read_replies(&files), Err(CmakeError::Json { .. })));
        files.remove(&index);
        assert!(matches!(read_replies(&files), Err(CmakeError::Missing(_))));
        // A path in the index is never followed out of the reply set.
        let evil = String::from_utf8(good)
            .unwrap()
            .replace("toolchains-v1-", "../toolchains-v1-");
        let mut files = reply_files("cmakeplain");
        files.insert(index, evil.into_bytes());
        assert!(matches!(read_replies(&files), Err(CmakeError::Missing(_))));
    }

    // frob:tests crates/goway/src/cmakeapi.rs::parse_trace
    #[test]
    fn the_trace_of_the_real_project_lists_its_own_calls_and_not_cmakes() {
        let t = parse_trace(&String::from_utf8(fixture("cxxshard/trace.jsonl")).unwrap()).unwrap();
        assert!(
            t.source_dir.as_deref().is_some_and(|d| d.ends_with("/src")),
            "{:?}",
            t.source_dir
        );
        let names: Vec<_> = t.fetched().into_iter().map(|f| (f.name, f.via)).collect();
        assert_eq!(
            names,
            [
                ("googletest".to_owned(), "FetchContent"),
                ("Catch2".to_owned(), "FetchContent")
            ]
        );
        assert!(
            t.find_packages().is_empty(),
            "find_package(Threads) lives in googletest's own files: {:?}",
            t.find_packages()
        );
    }

    // frob:tests crates/goway/src/cmakeapi.rs::Trace
    #[test]
    fn find_package_calls_keep_their_arguments_and_nested_module_calls_are_not_the_projects() {
        let t =
            parse_trace(&String::from_utf8(fixture("cmakeplain/trace.jsonl")).unwrap()).unwrap();
        let p = t.find_packages();
        let names: Vec<_> = p
            .iter()
            .map(|p| (p.name.as_str(), p.required, p.version.as_deref()))
            .collect();
        assert_eq!(
            names,
            [
                ("Threads", true, None),
                ("PkgConfig", false, None),
                ("Catch2", false, Some("3"))
            ]
        );
        let t = parse_trace(&String::from_utf8(fixture("cmakefind/trace.jsonl")).unwrap()).unwrap();
        let p = t.find_packages();
        assert_eq!(
            p.len(),
            1,
            "FindGTest.cmake's own find_package is CMake's: {p:?}"
        );
        assert_eq!((p[0].name.as_str(), p[0].required), ("GTest", true));
    }

    #[test]
    fn pkg_config_and_cpm_calls_are_read_with_their_names() {
        let trace = |lines: &[&str]| {
            let mut s = String::from("{\"version\":{\"major\":1,\"minor\":2}}\n");
            s.push_str("{\"args\":[\"VERSION\",\"3.20\"],\"cmd\":\"cmake_minimum_required\",\"file\":\"/w/src/CMakeLists.txt\",\"line\":1}\n");
            for l in lines {
                s.push_str(l);
                s.push('\n');
            }
            parse_trace(&s).unwrap()
        };
        let t = trace(&[
            r#"{"args":["ZLIB","REQUIRED","IMPORTED_TARGET","zlib>=1.2","libpng"],"cmd":"pkg_check_modules","file":"/w/src/CMakeLists.txt","line":3}"#,
            r#"{"args":["gh:fmtlib/fmt#10.2.1"],"cmd":"CPMAddPackage","file":"/w/src/CMakeLists.txt","line":4}"#,
            r#"{"args":["NAME","json","GITHUB_REPOSITORY","nlohmann/json"],"cmd":"CPMAddPackage","file":"/w/src/CMakeLists.txt","line":5}"#,
            r#"{"args":["GITHUB_REPOSITORY","jbeder/yaml-cpp","GIT_TAG","0.8"],"cmd":"CPMAddPackage","file":"/w/src/CMakeLists.txt","line":6}"#,
        ]);
        assert_eq!(t.pkg_config_modules(), ["zlib", "libpng"]);
        let f: Vec<_> = t.fetched().into_iter().map(|f| f.name).collect();
        assert_eq!(f, ["fmt", "json", "yaml-cpp"]);
    }

    // frob:tests crates/goway/src/cmakeapi.rs::parse_trace
    #[test]
    fn a_trace_is_strict_except_for_a_last_line_cut_by_the_size_limit() {
        assert!(matches!(
            parse_trace("not json\n"),
            Err(CmakeError::Json { .. })
        ));
        assert!(matches!(
            parse_trace("{\"version\":{\"major\":2,\"minor\":0}}\n"),
            Err(CmakeError::Unsupported { .. })
        ));
        let head = "{\"version\":{\"major\":1,\"minor\":2}}\n{\"args\":[],\"cmd\":\"project\",\"file\":\"/a/CMakeLists.txt\",\"line\":1}\n";
        assert_eq!(
            parse_trace(&format!("{head}{{\"args\":[\"x\"],\"cmd\":\"fi"))
                .unwrap()
                .calls
                .len(),
            1
        );
        assert!(matches!(
            parse_trace(&format!("{head}{{broken\n{head}")),
            Err(CmakeError::Json { .. })
        ));
        assert_eq!(parse_trace("").unwrap(), Trace::default());
    }

    // frob:tests crates/goway/src/cmakeapi.rs::missing_from_stderr
    #[test]
    fn a_failed_find_package_names_the_package_and_the_table_names_its_install_command() {
        let stderr = String::from_utf8(fixture("cmakefind/stderr.txt")).unwrap();
        assert_eq!(missing_from_stderr(&stderr), ["GTest"]);
        let a = analyse(
            None,
            Some(
                &parse_trace(&String::from_utf8(fixture("cmakefind/trace.jsonl")).unwrap())
                    .unwrap(),
            ),
            Some(&stderr),
            Some(1),
        );
        assert!(a.configure_failed);
        let need = a.system.iter().find(|n| n.name == "GTest").unwrap();
        assert_eq!(need.presence, Presence::Missing);
        assert!(need.required);
        let pkg = need.package.unwrap();
        assert_eq!(
            (pkg.apt, pkg.dnf, pkg.pacman),
            ("libgtest-dev", "gtest-devel", "gtest")
        );
        let config_mode = "CMake Error at CMakeLists.txt:3 (find_package):\n  Could not find a package configuration file provided by \"fmt\" with any of\n  the following names:\n";
        assert_eq!(missing_from_stderr(config_mode), ["fmt"]);
        assert!(missing_from_stderr("Could NOT find ;rm -rf (x)").is_empty());
    }

    // frob:tests crates/goway/src/cmakeapi.rs::analyse
    #[test]
    fn replies_and_trace_together_say_what_is_present_what_is_missing_and_what_needs_no_package() {
        let replies = read_replies(&reply_files("cmakeplain")).unwrap();
        let trace =
            parse_trace(&String::from_utf8(fixture("cmakeplain/trace.jsonl")).unwrap()).unwrap();
        let a = analyse(Some(&replies), Some(&trace), None, Some(0));
        let state = |n: &str| a.system.iter().find(|s| s.name == n).map(|s| s.presence);
        assert_eq!(
            state("Catch2"),
            Some(Presence::Missing),
            "Catch2_DIR-NOTFOUND"
        );
        assert_eq!(
            state("PkgConfig"),
            Some(Presence::Unknown),
            "optional and not in the cache: nothing says whether it was found"
        );
        assert_eq!(state("Threads"), None, "Threads is CMake's own");
        assert_eq!(a.missing_programs, ["pkg-config"]);
        assert_eq!(a.compilers.len(), 1);
        let shard = analyse(
            Some(&read_replies(&reply_files("cxxshard")).unwrap()),
            Some(
                &parse_trace(&String::from_utf8(fixture("cxxshard/trace.jsonl")).unwrap()).unwrap(),
            ),
            None,
            Some(0),
        );
        assert!(
            shard.system.is_empty(),
            "FetchContent needs no system package: {:?}",
            shard.system
        );
        assert_eq!(
            shard.fetched.len(),
            2,
            "the trace and the cache name the same two: {:?}",
            shard.fetched
        );
    }

    #[test]
    fn libraries_a_target_links_map_to_packages_through_the_one_table() {
        assert_eq!(system_library("-lz").as_deref(), Some("z"));
        assert_eq!(
            system_library("/usr/lib/x86_64-linux-gnu/libssl.so").as_deref(),
            Some("ssl")
        );
        assert_eq!(
            system_library("/usr/lib/x86_64-linux-gnu/libgtest.a").as_deref(),
            Some("gtest")
        );
        assert_eq!(
            system_library("lib/libgtest.a"),
            None,
            "relative paths are the build's own"
        );
        assert_eq!(system_library("-lpthread"), None);
        assert_eq!(system_library("-Wl,-rpath,/x"), None);
        assert_eq!(
            LibPackage::by_library("boost_filesystem").map(|p| p.cmake),
            Some("Boost")
        );
        assert_eq!(
            LibPackage::by_library("crypto").map(|p| p.cmake),
            Some("OpenSSL")
        );
        assert_eq!(
            LibPackage::by_pkg_config("zlib").map(|p| p.apt),
            Some("zlib1g-dev")
        );
        assert_eq!(
            LibPackage::by_cmake("gtest").map(|p| p.apt),
            Some("libgtest-dev")
        );
        assert_eq!(LibPackage::by_cmake("nothing-like-it"), None);
        // Every row names a package for each manager.
        assert!(
            LIB_PACKAGES
                .iter()
                .all(|p| !p.apt.is_empty() && !p.dnf.is_empty() && !p.pacman.is_empty())
        );
    }

    // frob:tests crates/goway/src/cmakeapi.rs::parse_bundle
    #[test]
    fn a_bundle_round_trips_and_every_deviation_is_refused() {
        let files = reply_files("cmakeplain");
        let good = frame(
            Some(0),
            &[("stderr", b"warning\n"), ("trace.jsonl", b"{}")],
            &[("tree-0/build", &files)],
        );
        let b = parse_bundle(&good).unwrap();
        assert_eq!(b.rc, Some(0));
        assert_eq!(b.files["stderr"], b"warning\n");
        assert_eq!(b.builds.len(), 1);
        assert_eq!(b.builds[0].label, "tree-0/build");
        assert_eq!(b.builds[0].files, files);
        assert!(read_replies(&b.builds[0].files).is_ok());
        // Just the header: a helper without CMake or replies.
        assert_eq!(parse_bundle(b"goway-cmake1\n").unwrap(), Bundle::default());
        for (bytes, why) in [
            (b"".to_vec(), "empty"),
            (b"nope\n@@end\n".to_vec(), "no header"),
            (
                b"goway-cmake1\n@@file a 3\nabcd\n@@end\n".to_vec(),
                "longer than it says",
            ),
            (
                b"goway-cmake1\n@@file a 9\nabc\n@@end\n".to_vec(),
                "shorter than it says",
            ),
            (
                b"goway-cmake1\n@@file ../a 1\nx\n@@end\n".to_vec(),
                "path in a name",
            ),
            (
                b"goway-cmake1\n@@file a 1\nx\n@@file a 1\ny\n@@end\n".to_vec(),
                "sent twice",
            ),
            (b"goway-cmake1\n@@file a 1\nx\n".to_vec(), "no end"),
            (b"goway-cmake1\n@@end\nmore\n".to_vec(), "after the end"),
            (b"goway-cmake1\n@@rc x\n@@end\n".to_vec(), "rc"),
            (b"goway-cmake1\n@@wat\n@@end\n".to_vec(), "unknown"),
            (
                format!("goway-cmake1\n@@file a {}\n", MAX_FILE + 1).into_bytes(),
                "over the file limit",
            ),
        ] {
            assert!(parse_bundle(&bytes).is_err(), "{why} must be refused");
        }
        let mut many = String::new();
        for i in 0..=MAX_FILES {
            use std::fmt::Write as _;
            let _ = write!(many, "@@file f{i} 1\nx\n");
        }
        assert_eq!(
            parse_bundle(format!("goway-cmake1\n{many}@@end\n").as_bytes()),
            Err(CmakeError::TooLarge("the number of files"))
        );
    }
}
