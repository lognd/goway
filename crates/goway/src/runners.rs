//! Shard adapters: how `goway run --shard N` splits the tests of the
//! common frameworks.
//!
//! The framework is recognised from the command line (and, only for
//! `npm test` style commands, from `package.json`). Frameworks with native
//! sharding get their own flags or environment; the others are split by
//! goway from the synced file list (test files, Go packages, Java
//! classes) with a stable sort and round-robin, so every test runs on
//! exactly one shard and the split needs no remote round trip. An unknown
//! command is left alone (it still sees `GOWAY_SHARD` and
//! `GOWAY_SHARD_COUNT`), except that the helper reads its program just
//! before running it and shards it natively when it is a `GoogleTest` or
//! Catch2 v3 binary ([`Plan::detect`], [`crate::detect`]). A command that
//! already shards itself is refused.

use std::collections::BTreeSet;
use std::path::Path;

use crate::error::{Error, Result};

/// A test framework goway knows how to shard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framework {
    /// `cargo nextest run`: `--partition count:i/N`.
    Nextest,
    /// vitest: `--shard=i/N`.
    Vitest,
    /// jest: `--shard=i/N`.
    Jest,
    /// Playwright test: `--shard=i/N`.
    Playwright,
    /// Catch2 v3: `--shard-count N --shard-index i-1` (`GOWAY_RUNNER=catch2`; otherwise the helper detects it).
    Catch2,
    /// `GoogleTest`: `GTEST_TOTAL_SHARDS` and `GTEST_SHARD_INDEX`.
    GoogleTest,
    /// `CTest`: `-I i,,N`.
    CTest,
    /// pytest: test files split by goway.
    Pytest,
    /// `go test`: packages split by goway.
    GoTest,
    /// Maven surefire: `-Dtest=` class list.
    Maven,
    /// Gradle: `--tests` class filters.
    Gradle,
    /// `RSpec`: spec files split by goway.
    Rspec,
}

impl Framework {
    /// The name shown in notes and accepted in `GOWAY_RUNNER`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Nextest => "nextest",
            Self::Vitest => "vitest",
            Self::Jest => "jest",
            Self::Playwright => "playwright",
            Self::Catch2 => "catch2",
            Self::GoogleTest => "gtest",
            Self::CTest => "ctest",
            Self::Pytest => "pytest",
            Self::GoTest => "go",
            Self::Maven => "maven",
            Self::Gradle => "gradle",
            Self::Rspec => "rspec",
        }
    }
}

/// What goway knows about the project, for adapters that split by file.
#[derive(Debug, Clone, Default)]
pub struct Project {
    /// Synced files (`git ls-files -co --exclude-standard`), relative to the project root, `/` separated.
    pub files: Vec<String>,
    /// Contents of the root `package.json`, when there is one.
    pub package_json: Option<String>,
}

impl Project {
    /// Read the file list and `package.json` of the repository at `root`.
    pub fn load(root: &Path) -> Result<Self> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["ls-files", "-co", "--exclude-standard", "-z"])
            .output()
            .map_err(|e| Error::Git {
                message: format!("cannot run git: {e}"),
            })?;
        if !out.status.success() {
            return Err(Error::Git {
                message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        let mut files: Vec<String> = out
            .stdout
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .filter(|f| root.join(f).is_file())
            .collect();
        files.sort_unstable();
        let package_json = std::fs::read_to_string(root.join("package.json")).ok();
        tracing::debug!(files = files.len(), "project file list for shard adapters");
        Ok(Self {
            files,
            package_json,
        })
    }
}

/// The command and extra environment one shard runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The framework recognised, when there is one.
    pub framework: Option<Framework>,
    /// The command line for this shard.
    pub command: Vec<String>,
    /// Extra `KEY=VALUE` environment for this shard.
    pub env: Vec<String>,
    /// The program is not a known tool, so the helper checks whether it is
    /// a `GoogleTest` or Catch2 binary just before it runs (see [`crate::detect`]).
    pub detect: bool,
}

/// The command a shard runs when the split leaves it no tests.
fn nothing_to_do(framework: Framework) -> Plan {
    Plan {
        framework: Some(framework),
        detect: false,
        command: vec!["true".to_owned()],
        env: Vec::new(),
    }
}

fn base_name(arg: &str) -> &str {
    let name = arg.rsplit(['/', '\\']).next().unwrap_or(arg);
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".cmd"))
        .unwrap_or(name)
}

/// Words that may precede the tool in a command (`npx vitest`, `uv run pytest`, `python -m pytest`).
fn is_wrapper(arg: &str) -> bool {
    const WRAPPERS: &[&str] = &[
        "npx", "pnpx", "bunx", "pnpm", "yarn", "npm", "bun", "exec", "dlx", "run", "uv", "uvx",
        "poetry", "pipenv", "python", "python3", "-m", "bundle", "env", "time", "nice", "--",
    ];
    arg.contains('=') && !arg.starts_with('-') || WRAPPERS.contains(&base_name(arg))
}

/// Index of the first of `names` among the leading tool/wrapper words of `command`.
fn find_tool(command: &[String], names: &[&str]) -> Option<usize> {
    for (i, arg) in command.iter().enumerate() {
        if names.contains(&base_name(arg)) {
            return Some(i);
        }
        if !is_wrapper(arg) {
            return None;
        }
    }
    None
}

/// Where flags can be inserted: before the first `--` after `from`, else at the end.
fn insert_point(command: &[String], from: usize) -> usize {
    command[from..]
        .iter()
        .position(|a| a == "--")
        .map_or(command.len(), |p| from + p)
}

fn has_flag(command: &[String], names: &[&str]) -> bool {
    command.iter().any(|a| {
        names
            .iter()
            .any(|n| a == n || a.strip_prefix(n).is_some_and(|r| r.starts_with('=')))
    })
}

fn env_has(env: &[String], key: &str) -> bool {
    env.iter().any(|e| e.split('=').next() == Some(key))
}

fn already(framework: Framework, what: &str) -> Error {
    Error::Usage(format!(
        "this {} command already splits its tests ({what}), so goway cannot shard it\n  next: remove it, or run without --shard",
        framework.name()
    ))
}

fn with_inserted(command: &[String], at: usize, extra: &[String]) -> Vec<String> {
    let mut out = command[..at].to_vec();
    out.extend_from_slice(extra);
    out.extend_from_slice(&command[at..]);
    out
}

/// Recognise the framework a command runs, if any.
pub fn detect(command: &[String], env: &[String], project: &Project) -> Option<Framework> {
    let runner = env
        .iter()
        .rev()
        .find_map(|e| e.strip_prefix("GOWAY_RUNNER="))
        .map(str::to_ascii_lowercase);
    if let Some(r) = runner {
        return match r.as_str() {
            "catch2" => Some(Framework::Catch2),
            "gtest" | "googletest" => Some(Framework::GoogleTest),
            _ => None,
        };
    }
    if command
        .windows(2)
        .any(|w| base_name(&w[0]).ends_with("nextest") && w[1] == "run")
    {
        return Some(Framework::Nextest);
    }
    if find_tool(command, &["vitest"]).is_some() {
        return Some(Framework::Vitest);
    }
    if find_tool(command, &["jest"]).is_some() {
        return Some(Framework::Jest);
    }
    if let Some(i) = find_tool(command, &["playwright"])
        && command.get(i + 1).is_some_and(|a| a == "test")
    {
        return Some(Framework::Playwright);
    }
    if find_tool(command, &["ctest"]).is_some() {
        return Some(Framework::CTest);
    }
    if find_tool(command, &["pytest", "py.test"]).is_some() {
        return Some(Framework::Pytest);
    }
    if find_tool(command, &["rspec"]).is_some() {
        return Some(Framework::Rspec);
    }
    if let Some(i) = find_tool(command, &["go"])
        && command.get(i + 1).is_some_and(|a| a == "test")
    {
        return Some(Framework::GoTest);
    }
    if find_tool(command, &["mvn", "mvnw"]).is_some()
        && command
            .iter()
            .any(|a| ["test", "verify", "package", "install"].contains(&a.as_str()))
    {
        return Some(Framework::Maven);
    }
    if find_tool(command, &["gradle", "gradlew"]).is_some()
        && command.iter().any(|a| a == "test" || a.ends_with(":test"))
    {
        return Some(Framework::Gradle);
    }
    if find_tool(command, &["gtest"]).is_some() || command.iter().any(|a| a.starts_with("--gtest_"))
    {
        return Some(Framework::GoogleTest);
    }
    npm_script_framework(command, project)
}

/// `npm test`, `pnpm test`, `yarn test`: the framework named in `scripts.test`.
fn npm_script_framework(command: &[String], project: &Project) -> Option<Framework> {
    let i = find_tool(command, &["npm", "pnpm", "yarn", "bun"])?;
    let rest = &command[i + 1..];
    let is_test = matches!(rest.first().map(String::as_str), Some("test" | "t"))
        || (rest.first().is_some_and(|a| a == "run") && rest.get(1).is_some_and(|a| a == "test"));
    if !is_test {
        return None;
    }
    let pkg: serde_json::Value = serde_json::from_str(project.package_json.as_deref()?).ok()?;
    let script = pkg.get("scripts")?.get("test")?.as_str()?;
    let words: Vec<&str> = script
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.')
        .collect();
    [
        ("vitest", Framework::Vitest),
        ("jest", Framework::Jest),
        ("playwright", Framework::Playwright),
    ]
    .into_iter()
    .find(|(w, _)| words.contains(w))
    .map(|(_, f)| f)
}

/// The command and environment for shard `index` (1-based) of `count`.
///
/// # Errors
///
/// [`Error::Usage`] when the command already shards itself, or when a
/// file-splitting adapter cannot tell what to split.
#[allow(clippy::too_many_lines)] // one match arm per framework
pub fn plan(
    command: &[String],
    env: &[String],
    project: &Project,
    index: usize,
    count: usize,
) -> Result<Plan> {
    let Some(framework) = detect(command, env, project) else {
        return Ok(Plan {
            framework: None,
            command: command.to_vec(),
            env: Vec::new(),
            // A GOWAY_RUNNER (even an unknown one) is the user's say-so; no detection then.
            detect: !command.is_empty() && !env.iter().any(|e| e.starts_with("GOWAY_RUNNER=")),
        });
    };
    tracing::debug!(framework = framework.name(), index, count, "shard adapter");
    let flag = |pair: String| vec![pair];
    let mut plan = Plan {
        framework: Some(framework),
        detect: false,
        command: command.to_vec(),
        env: Vec::new(),
    };
    match framework {
        Framework::Nextest => {
            if has_flag(command, &["--partition"]) {
                return Err(already(framework, "--partition"));
            }
            let at = command
                .windows(2)
                .position(|w| base_name(&w[0]).ends_with("nextest") && w[1] == "run")
                .map_or(0, |p| p + 2);
            let at = insert_point(command, at);
            plan.command = with_inserted(
                command,
                at,
                &["--partition".to_owned(), format!("count:{index}/{count}")],
            );
        }
        Framework::Vitest | Framework::Jest | Framework::Playwright => {
            if has_flag(command, &["--shard"]) {
                return Err(already(framework, "--shard"));
            }
            let shard = format!("--shard={index}/{count}");
            plan.command = if find_tool(command, &["npm", "pnpm", "yarn", "bun"]).is_some()
                && command.iter().any(|a| a == "test" || a == "t")
                && !command
                    .iter()
                    .any(|a| ["vitest", "jest", "playwright"].contains(&base_name(a)))
            {
                // `npm test -- --shard=i/N`: the script's own command gets the flag.
                let mut c = command.to_vec();
                if !c.iter().any(|a| a == "--") && base_name(&c[0]) == "npm" {
                    c.push("--".to_owned());
                }
                c.push(shard);
                c
            } else {
                let from = find_tool(command, &["vitest", "jest", "playwright"]).unwrap_or(0);
                let at = insert_point(command, from);
                with_inserted(command, at, &flag(shard))
            };
        }
        Framework::Catch2 => {
            if has_flag(command, &["--shard-count", "--shard-index"]) {
                return Err(already(framework, "--shard-count"));
            }
            plan.command.extend([
                "--shard-count".to_owned(),
                count.to_string(),
                "--shard-index".to_owned(),
                (index - 1).to_string(),
            ]);
        }
        Framework::GoogleTest => {
            if env_has(env, "GTEST_TOTAL_SHARDS") || env_has(env, "GTEST_SHARD_INDEX") {
                return Err(already(framework, "GTEST_TOTAL_SHARDS"));
            }
            plan.env = vec![
                format!("GTEST_TOTAL_SHARDS={count}"),
                format!("GTEST_SHARD_INDEX={}", index - 1),
            ];
        }
        Framework::CTest => {
            if command
                .iter()
                .any(|a| a == "-I" || a.starts_with("-I") || a == "--tests-information")
            {
                return Err(already(framework, "-I"));
            }
            let at = find_tool(command, &["ctest"]).map_or(0, |p| p + 1);
            plan.command =
                with_inserted(command, at, &["-I".to_owned(), format!("{index},,{count}")]);
        }
        Framework::Pytest => {
            return split_by_path(
                framework,
                command,
                project,
                index,
                count,
                find_tool(command, &["pytest", "py.test"]).unwrap_or(0),
                &PYTEST,
            );
        }
        Framework::Rspec => {
            return split_by_path(
                framework,
                command,
                project,
                index,
                count,
                find_tool(command, &["rspec"]).unwrap_or(0),
                &RSPEC,
            );
        }
        Framework::GoTest => return split_go(command, project, index, count),
        Framework::Maven | Framework::Gradle => {
            return split_java(framework, command, project, index, count);
        }
    }
    Ok(plan)
}

/// Round-robin share of the sorted, de-duplicated `units` for shard `index` (1-based).
fn share(units: BTreeSet<String>, index: usize, count: usize) -> Vec<String> {
    units
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % count == index - 1)
        .map(|(_, u)| u)
        .collect()
}

/// How a file-splitting adapter finds test files and skips option values.
struct PathRules {
    /// Whether a project file is a test file.
    is_test: fn(&str) -> bool,
    /// Options whose next word is a value, not a path.
    value_flags: &'static [&'static str],
    /// The part of a `file::test` or `file:line` argument that names the file.
    file_of: fn(&str) -> &str,
}

#[allow(clippy::case_sensitive_file_extension_comparisons)] // Python file names are lower-case by convention
fn pytest_is_test(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py"))
}

fn pytest_file_of(arg: &str) -> &str {
    arg.split("::").next().unwrap_or(arg)
}

fn rspec_is_test(path: &str) -> bool {
    path.ends_with("_spec.rb") && (path.starts_with("spec/") || path.contains("/spec/"))
}

fn rspec_file_of(arg: &str) -> &str {
    match arg.rsplit_once(':') {
        Some((file, line)) if !line.is_empty() && line.chars().all(|c| c.is_ascii_digit()) => file,
        _ => arg.split('[').next().unwrap_or(arg),
    }
}

const PYTEST: PathRules = PathRules {
    is_test: pytest_is_test,
    value_flags: &[
        "-k",
        "-m",
        "-n",
        "-p",
        "-c",
        "-o",
        "-W",
        "--ignore",
        "--ignore-glob",
        "--deselect",
        "--rootdir",
        "--junitxml",
        "--cov",
        "--cov-report",
        "--maxfail",
        "--basetemp",
        "--durations",
        "--dist",
        "--tb",
        "--color",
        "--log-level",
        "--confcutdir",
        "--import-mode",
    ],
    file_of: pytest_file_of,
};

const RSPEC: PathRules = PathRules {
    is_test: rspec_is_test,
    value_flags: &[
        "-r",
        "--require",
        "-e",
        "--example",
        "-t",
        "--tag",
        "-f",
        "--format",
        "-o",
        "--out",
        "-I",
        "--exclude-pattern",
        "-P",
        "--pattern",
        "--seed",
        "--order",
    ],
    file_of: rspec_file_of,
};

/// Split by test file (pytest, RSpec): explicit path arguments narrow the
/// set, otherwise every test file of the project is in it.
#[allow(clippy::too_many_lines)] // one pass over the arguments
fn split_by_path(
    framework: Framework,
    command: &[String],
    project: &Project,
    index: usize,
    count: usize,
    tool_at: usize,
    rules: &PathRules,
) -> Result<Plan> {
    let mut kept: Vec<String> = command[..=tool_at].to_vec();
    let mut units = BTreeSet::new();
    let mut explicit = false;
    let mut after_end_of_options = false;
    let mut skip_next = false;
    let mut ignoring = false;
    let mut ignored: Vec<String> = Vec::new();
    for arg in &command[tool_at + 1..] {
        if std::mem::take(&mut skip_next) {
            if std::mem::take(&mut ignoring) {
                ignored.push(
                    arg.trim_start_matches("./")
                        .trim_end_matches('/')
                        .to_owned(),
                );
            }
            kept.push(arg.clone());
            continue;
        }
        if let Some(path) = arg.strip_prefix("--ignore=") {
            ignored.push(
                path.trim_start_matches("./")
                    .trim_end_matches('/')
                    .to_owned(),
            );
        }
        if arg == "--" {
            after_end_of_options = true;
            kept.push(arg.clone());
            continue;
        }
        if !after_end_of_options && arg.starts_with('-') {
            skip_next = rules.value_flags.contains(&arg.as_str());
            ignoring = skip_next && arg == "--ignore";
            kept.push(arg.clone());
            continue;
        }
        let file = (rules.file_of)(arg).trim_start_matches("./");
        let dir = file.trim_end_matches('/');
        let dir = if dir.is_empty() || dir == "." {
            ""
        } else {
            dir
        };
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        if project.files.iter().any(|f| f == file) {
            explicit = true;
            units.insert(if file == arg {
                file.to_owned()
            } else {
                arg.clone()
            });
        } else if project
            .files
            .iter()
            .any(|f| (rules.is_test)(f) && f.starts_with(&prefix))
            && (dir.is_empty() || project.files.iter().any(|f| f.starts_with(&prefix)))
            && !project.files.iter().any(|f| f == dir)
        {
            explicit = true;
            for f in project
                .files
                .iter()
                .filter(|f| (rules.is_test)(f) && f.starts_with(&prefix))
            {
                units.insert(f.clone());
            }
        } else {
            kept.push(arg.clone());
        }
    }
    if !explicit {
        units = project
            .files
            .iter()
            .filter(|f| (rules.is_test)(f))
            .cloned()
            .collect();
    }
    units.retain(|u| {
        !ignored
            .iter()
            .any(|i| u == i || u.starts_with(&format!("{i}/")))
    });
    if units.is_empty() {
        return Err(Error::Usage(format!(
            "goway found no {} test files in the synced project to split across shards",
            framework.name()
        )));
    }
    let mine = share(units, index, count);
    if mine.is_empty() {
        return Ok(nothing_to_do(framework));
    }
    kept.extend(mine);
    Ok(Plan {
        framework: Some(framework),
        detect: false,
        command: kept,
        env: Vec::new(),
    })
}

/// Directories holding `*_test.go` files (one Go package each), skipping
/// `testdata`, hidden and `_` directories and nested modules.
fn go_packages(files: &[String]) -> BTreeSet<String> {
    let modules: Vec<&str> = files
        .iter()
        .filter_map(|f| f.strip_suffix("go.mod"))
        .filter(|d| !d.is_empty())
        .collect();
    files
        .iter()
        .filter(|f| f.ends_with("_test.go"))
        .map(|f| f.rsplit_once('/').map_or("", |(d, _)| d))
        .filter(|d| {
            !d.split('/')
                .any(|c| c == "testdata" || c == "vendor" || c.starts_with(['.', '_']))
                && !modules.iter().any(|m| format!("{d}/").starts_with(m))
        })
        .map(str::to_owned)
        .collect()
}

fn go_pattern_arg(arg: &str) -> bool {
    arg == "." || arg.starts_with("./") || arg.starts_with("../") || arg.contains("...")
}

fn split_go(command: &[String], project: &Project, index: usize, count: usize) -> Result<Plan> {
    let framework = Framework::GoTest;
    let tool = find_tool(command, &["go"]).unwrap_or(0);
    let test_at = tool + 1;
    let end = command[test_at..]
        .iter()
        .position(|a| a == "-args")
        .map_or(command.len(), |p| test_at + p);
    let patterns: Vec<&String> = command[test_at + 1..end]
        .iter()
        .filter(|a| go_pattern_arg(a))
        .collect();
    if patterns.is_empty() {
        return Err(Error::Usage(
            "goway needs a package pattern to shard `go test` (such as `./...`)".to_owned(),
        ));
    }
    let all = go_packages(&project.files);
    let mut units = BTreeSet::new();
    for p in &patterns {
        let p = p.trim_start_matches("./");
        if p.is_empty() || p == "." {
            if all.contains("") {
                units.insert(String::new());
            }
        } else if let Some(root) = p.strip_suffix("...") {
            let root = root.trim_end_matches('/');
            units.extend(
                all.iter()
                    .filter(|d| root.is_empty() || *d == root || d.starts_with(&format!("{root}/")))
                    .cloned(),
            );
        } else if all.contains(p.trim_end_matches('/')) {
            units.insert(p.trim_end_matches('/').to_owned());
        }
    }
    if units.is_empty() {
        return Err(Error::Usage(
            "goway found no Go test packages matching the pattern in the synced project".to_owned(),
        ));
    }
    let mine = share(units, index, count);
    if mine.is_empty() {
        return Ok(nothing_to_do(framework));
    }
    let mut out: Vec<String> = command
        .iter()
        .enumerate()
        .filter(|(i, a)| !(*i > test_at && *i < end && go_pattern_arg(a)))
        .map(|(_, a)| a.clone())
        .collect();
    let at = out.iter().position(|a| a == "-args").unwrap_or(out.len());
    let pkgs: Vec<String> = mine
        .into_iter()
        .map(|d| {
            if d.is_empty() {
                ".".to_owned()
            } else {
                format!("./{d}")
            }
        })
        .collect();
    out.splice(at..at, pkgs);
    Ok(Plan {
        framework: Some(framework),
        detect: false,
        command: out,
        env: Vec::new(),
    })
}

/// Fully qualified names of the JVM test classes in `files` (surefire's
/// default name patterns: `Test*`, `*Test`, `*Tests`, `*TestCase`).
fn java_classes(files: &[String]) -> BTreeSet<String> {
    files
        .iter()
        .filter_map(|f| {
            let at = if f.starts_with("src/test/") {
                0
            } else {
                f.find("/src/test/")? + 1
            };
            let rest = f[at..].strip_prefix("src/test/")?;
            let (lang, rest) = rest.split_once('/')?;
            if !["java", "kotlin", "groovy", "scala"].contains(&lang) {
                return None;
            }
            let (path, ext) = rest.rsplit_once('.')?;
            if !["java", "kt", "groovy", "scala"].contains(&ext) {
                return None;
            }
            let simple = path.rsplit('/').next()?;
            let is_test = simple.starts_with("Test")
                || simple.ends_with("Test")
                || simple.ends_with("Tests")
                || simple.ends_with("TestCase");
            is_test.then(|| path.replace('/', "."))
        })
        .collect()
}

fn split_java(
    framework: Framework,
    command: &[String],
    project: &Project,
    index: usize,
    count: usize,
) -> Result<Plan> {
    let maven = framework == Framework::Maven;
    if maven && command.iter().any(|a| a.starts_with("-Dtest=")) {
        return Err(already(framework, "-Dtest="));
    }
    if !maven && command.iter().any(|a| a == "--tests") {
        return Err(already(framework, "--tests"));
    }
    let classes = java_classes(&project.files);
    if classes.is_empty() {
        return Err(Error::Usage(format!(
            "goway found no JVM test classes under src/test in the synced project to split across {} shards",
            framework.name()
        )));
    }
    let mine = share(classes, index, count);
    if mine.is_empty() {
        return Ok(nothing_to_do(framework));
    }
    let mut out = command.to_vec();
    if maven {
        out.push(format!("-Dtest={}", mine.join(",")));
        out.push("-Dsurefire.failIfNoSpecifiedTests=false".to_owned());
        out.push("-DfailIfNoTests=false".to_owned());
    } else {
        for c in mine {
            out.push("--tests".to_owned());
            out.push(c);
        }
    }
    Ok(Plan {
        framework: Some(framework),
        detect: false,
        command: out,
        env: Vec::new(),
    })
}

/// The project file list for [`plan`], only when the command needs one.
///
/// # Errors
///
/// Whatever [`Project::load`] returns.
pub fn project_for(command: &[String], env: &[String], root: &Path) -> Result<Project> {
    let bare = Project::default();
    let needs_files = matches!(
        detect(command, env, &bare),
        Some(
            Framework::Pytest
                | Framework::Rspec
                | Framework::GoTest
                | Framework::Maven
                | Framework::Gradle
        )
    );
    let npm = find_tool(command, &["npm", "pnpm", "yarn", "bun"]).is_some();
    if needs_files || npm {
        Project::load(root)
    } else {
        Ok(bare)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    fn project(files: &[&str]) -> Project {
        Project {
            files: files.iter().map(|s| (*s).to_owned()).collect(),
            package_json: None,
        }
    }

    fn planned(cmd: &str, files: &[&str], i: usize, n: usize) -> Plan {
        plan(&words(cmd), &[], &project(files), i, n).unwrap()
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn nextest_gets_its_own_partition_flag() {
        assert_eq!(
            planned("cargo nextest run --workspace", &[], 2, 3).command,
            words("cargo nextest run --workspace --partition count:2/3")
        );
        assert_eq!(
            planned("cargo nextest run -E all() -- --nocapture", &[], 1, 2).command,
            words("cargo nextest run -E all() --partition count:1/2 -- --nocapture")
        );
        assert_eq!(
            planned("/x/cargo-nextest nextest run", &[], 1, 2).command,
            words("/x/cargo-nextest nextest run --partition count:1/2")
        );
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn unknown_commands_run_unchanged() {
        let p = planned("make test", &[], 1, 2);
        assert_eq!(p.command, words("make test"));
        assert_eq!(p.framework, None);
        assert!(p.env.is_empty());
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn unknown_programs_ask_the_helper_to_detect_but_known_ones_and_overrides_do_not() {
        assert!(planned("./build/tests", &[], 1, 2).detect);
        assert!(planned("make test", &[], 1, 2).detect);
        assert!(!planned("cargo nextest run", &[], 1, 2).detect);
        assert!(!planned("npx vitest run", &[], 1, 2).detect);
        let env = vec!["GOWAY_RUNNER=gtest".to_owned()];
        let p = plan(&words("./build/tests"), &env, &project(&[]), 1, 2).unwrap();
        assert!(!p.detect, "GOWAY_RUNNER overrides detection");
        assert_eq!(p.framework, Some(Framework::GoogleTest));
        let env = vec!["GOWAY_RUNNER=whatever".to_owned()];
        assert!(
            !plan(&words("./t"), &env, &project(&[]), 1, 2)
                .unwrap()
                .detect
        );
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn js_runners_get_the_shard_flag() {
        assert_eq!(
            planned("npx vitest run", &[], 2, 4).command,
            words("npx vitest run --shard=2/4")
        );
        assert_eq!(
            planned("npx jest --ci -- --x", &[], 1, 3).command,
            words("npx jest --ci --shard=1/3 -- --x")
        );
        assert_eq!(
            planned("npx playwright test --project=a", &[], 3, 3).command,
            words("npx playwright test --project=a --shard=3/3")
        );
        assert_eq!(
            planned("node_modules/.bin/vitest", &[], 1, 2).command,
            words("node_modules/.bin/vitest --shard=1/2")
        );
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn npm_test_is_resolved_through_package_json() {
        let mut p = project(&[]);
        p.package_json = Some(r#"{"scripts":{"test":"vitest run --coverage"}}"#.to_owned());
        let got = plan(&words("npm test"), &[], &p, 2, 3).unwrap();
        assert_eq!(got.framework, Some(Framework::Vitest));
        assert_eq!(got.command, words("npm test -- --shard=2/3"));
        let got = plan(&words("pnpm run test"), &[], &p, 1, 3).unwrap();
        assert_eq!(got.command, words("pnpm run test --shard=1/3"));
        p.package_json = Some(r#"{"scripts":{"test":"mocha"}}"#.to_owned());
        assert_eq!(
            plan(&words("npm test"), &[], &p, 1, 2).unwrap().framework,
            None
        );
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn native_cpp_runners_use_their_own_flags() {
        let env = vec!["GOWAY_RUNNER=catch2".to_owned()];
        let p = plan(&words("./build/tests"), &env, &project(&[]), 2, 4).unwrap();
        assert_eq!(
            p.command,
            words("./build/tests --shard-count 4 --shard-index 1")
        );
        let env = vec!["GOWAY_RUNNER=gtest".to_owned()];
        let p = plan(&words("./build/tests"), &env, &project(&[]), 3, 4).unwrap();
        assert_eq!(p.command, words("./build/tests"));
        assert_eq!(p.env, ["GTEST_TOTAL_SHARDS=4", "GTEST_SHARD_INDEX=2"]);
        let p = planned("./t --gtest_filter=A.*", &[], 1, 2);
        assert_eq!(p.framework, Some(Framework::GoogleTest));
        assert_eq!(
            planned("ctest --output-on-failure", &[], 2, 3).command,
            words("ctest -I 2,,3 --output-on-failure")
        );
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn a_command_that_already_shards_is_refused() {
        for cmd in [
            "cargo nextest run --partition count:1/2",
            "npx vitest --shard=1/2",
            "npx jest --shard 1/2",
            "ctest -I 1,,2",
            "mvn test -Dtest=Foo",
            "gradle test --tests Foo",
        ] {
            let e = plan(
                &words(cmd),
                &[],
                &project(&["src/test/java/FooTest.java"]),
                1,
                2,
            )
            .unwrap_err();
            assert!(e.to_string().contains("already splits"), "{cmd}: {e}");
        }
        let env = vec![
            "GOWAY_RUNNER=gtest".to_owned(),
            "GTEST_SHARD_INDEX=0".to_owned(),
        ];
        assert!(plan(&words("./t"), &env, &project(&[]), 1, 2).is_err());
    }

    const PY: &[&str] = &[
        "conftest.py",
        "tests/test_a.py",
        "tests/test_b.py",
        "tests/sub/test_c.py",
        "tests/sub/helpers.py",
        "pkg/mod_test.py",
        "pkg/mod.py",
    ];

    /// The test units each of `n` shards got, as the arguments after the tool.
    fn units(cmd: &str, files: &[&str], n: usize, skip: usize) -> Vec<Vec<String>> {
        (1..=n)
            .map(|i| {
                let c = planned(cmd, files, i, n).command;
                if c == ["true"] {
                    Vec::new()
                } else {
                    c[skip..].to_vec()
                }
            })
            .collect()
    }

    fn assert_exactly_once(shards: &[Vec<String>], expected: &[&str]) {
        let mut all: Vec<&str> = shards.iter().flatten().map(String::as_str).collect();
        all.sort_unstable();
        let mut want = expected.to_vec();
        want.sort_unstable();
        assert_eq!(all, want, "every unit exactly once");
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn pytest_files_are_split_exactly_once_and_compose_with_xdist() {
        for n in 1..=5 {
            let shards = units("pytest -n auto -q", PY, n, 4);
            assert_exactly_once(
                &shards,
                &[
                    "tests/test_a.py",
                    "tests/test_b.py",
                    "tests/sub/test_c.py",
                    "pkg/mod_test.py",
                ],
            );
        }
        let p = planned("python -m pytest -n auto", PY, 1, 2);
        assert_eq!(
            &p.command[..5],
            words("python -m pytest -n auto").as_slice()
        );
        assert_eq!(
            planned("pytest", PY, 1, 2),
            planned("pytest", PY, 1, 2),
            "deterministic"
        );
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn pytest_explicit_paths_narrow_the_split() {
        let shards = units("pytest -k foo --ignore tests/sub tests", PY, 2, 5);
        assert_exactly_once(&shards, &["tests/test_a.py", "tests/test_b.py"]);
        let p = planned("pytest tests/test_a.py::test_x", PY, 1, 2);
        assert_eq!(p.command, words("pytest tests/test_a.py::test_x"));
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn an_empty_shard_runs_nothing_instead_of_everything() {
        let p = planned("pytest", &["test_only.py"], 2, 3);
        assert_eq!(p.command, ["true"]);
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn rspec_spec_files_are_split_exactly_once() {
        let files = [
            "spec/a_spec.rb",
            "spec/b/c_spec.rb",
            "spec/spec_helper.rb",
            "lib/x.rb",
        ];
        for n in 1..=3 {
            let shards = units("bundle exec rspec --format doc", &files, n, 5);
            assert_exactly_once(&shards, &["spec/a_spec.rb", "spec/b/c_spec.rb"]);
        }
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn go_packages_are_split_exactly_once() {
        let files = [
            "go.mod",
            "a_test.go",
            "x/x_test.go",
            "x/y/y_test.go",
            "x/testdata/t_test.go",
            "vendor/v/v_test.go",
            "sub/go.mod",
            "sub/s_test.go",
            "z/z.go",
        ];
        for n in 1..=4 {
            let shards = units("go test -count=1 ./...", &files, n, 3);
            assert_exactly_once(&shards, &[".", "./x", "./x/y"]);
        }
        let p = planned("go test ./x/... -run T -args -v", &files, 1, 1);
        assert_eq!(p.command, words("go test -run T ./x ./x/y -args -v"));
        assert!(plan(&words("go test"), &[], &project(&files), 1, 2).is_err());
    }

    // frob:tests crates/goway/src/runners.rs::plan
    #[test]
    fn maven_and_gradle_classes_are_split_exactly_once() {
        let files = [
            "src/test/java/com/x/FooTest.java",
            "src/test/java/com/x/BarTests.java",
            "mod/src/test/java/com/y/TestBaz.java",
            "mod/src/test/java/com/y/Helper.java",
            "src/main/java/com/x/Main.java",
        ];
        let all = ["com.x.FooTest", "com.x.BarTests", "com.y.TestBaz"];
        for n in 1..=3 {
            let mvn: Vec<Vec<String>> = (1..=n)
                .map(|i| {
                    let c = planned("mvn -q test", &files, i, n).command;
                    c.iter()
                        .filter_map(|a| a.strip_prefix("-Dtest="))
                        .flat_map(|l| l.split(',').map(str::to_owned).collect::<Vec<_>>())
                        .collect()
                })
                .collect();
            assert_exactly_once(&mvn, &all);
            let gradle: Vec<Vec<String>> = (1..=n)
                .map(|i| {
                    let c = planned("./gradlew test", &files, i, n).command;
                    c.windows(2)
                        .filter(|w| w[0] == "--tests")
                        .map(|w| w[1].clone())
                        .collect()
                })
                .collect();
            assert_exactly_once(&gradle, &all);
        }
    }
}
