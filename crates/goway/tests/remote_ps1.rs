//! Contract tests for `remote.ps1`, the Windows side of goway's remote
//! protocol: the same verbs, labelled directories, locks and output formats
//! as `remote.sh`. They drive the script with whatever PowerShell the machine
//! has (`GOWAY_PWSH`, else `pwsh`, else `powershell` on Windows), so they run
//! under pwsh on Linux and on Windows CI; without PowerShell they pass
//! trivially (the CI jobs have it).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base64::Engine as _;
use goway::remote;
use goway::sync::{self, Kind, LocalFile};

/// The PowerShell these tests drive, if there is one.
fn pwsh() -> Option<&'static Path> {
    static PS: OnceLock<Option<PathBuf>> = OnceLock::new();
    PS.get_or_init(|| {
        let mut candidates = Vec::new();
        if let Some(p) = std::env::var_os("GOWAY_PWSH") {
            candidates.push(PathBuf::from(p));
        }
        candidates.push(PathBuf::from("pwsh"));
        if cfg!(windows) {
            candidates.push(PathBuf::from("powershell"));
        }
        candidates.into_iter().find(|c| {
            Command::new(c)
                .args(["-NoProfile", "-Command", "exit 0"])
                .output()
                .is_ok_and(|o| o.status.success())
        })
    })
    .as_deref()
}

/// PowerShell source running the script with `words` (single-quoted, exactly).
fn source(script: &Path, words: &[&str]) -> String {
    let mut all = vec![script.to_string_lossy().into_owned()];
    all.extend(words.iter().map(|w| (*w).to_owned()));
    format!("{}; exit $LASTEXITCODE", goway::transport::ps_call(&all))
}

fn encoded(source: &str) -> String {
    goway::transport::encoded_command(source)
}

fn label(kind: &str) -> String {
    let json = serde_json::json!({
        "kind": kind, "repo": "proj", "repo_id": "r1",
        "worktree": "/w", "client": "c", "updated": 1
    });
    base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&json).unwrap())
}

const TTLS: &str = "604800:86400:259200";

/// One fake Windows host: a scratch state root and the script on disk.
struct Host {
    dir: tempfile::TempDir,
    ps: &'static Path,
    script: PathBuf,
    root: PathBuf,
    /// The laptop-side work tree files are sent from.
    src: PathBuf,
    attempt: std::cell::Cell<u32>,
}

impl Host {
    fn new() -> Option<Self> {
        let ps = pwsh()?;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("remote.ps1");
        std::fs::write(&script, remote::SCRIPT_PS).unwrap();
        let root = dir.path().join("goway-root");
        let src = dir.path().join("laptop");
        std::fs::create_dir(&src).unwrap();
        Some(Self {
            dir,
            ps,
            script,
            root,
            src,
            attempt: std::cell::Cell::new(0),
        })
    }

    fn root(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    fn command(&self, words: &[&str]) -> Command {
        let mut c = Command::new(self.ps);
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded(&source(&self.script, words)),
        ]);
        c.env_remove("GOWAY_TEST_CORRUPT")
            .env_remove("GOWAY_TEST_HOOK");
        c
    }

    /// Run a verb to completion with `stdin`.
    fn call(&self, words: &[&str], stdin: &[u8]) -> Output {
        run_with(self.command(words), stdin)
    }

    fn call_env(&self, words: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = self.command(words);
        for (k, v) in env {
            c.env(k, v);
        }
        run_with(c, b"")
    }

    fn ok(&self, words: &[&str], stdin: &[u8]) -> String {
        let out = self.call(words, stdin);
        assert!(out.status.success(), "{words:?}: {}", text(&out.stderr));
        text(&out.stdout)
    }

    /// Write `name` into the laptop tree with `content` and an mtime.
    fn put(&self, name: &str, content: &str, mtime: u64) {
        let path = self.src.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        let t = std::time::UNIX_EPOCH + Duration::from_secs(mtime);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    /// The current generation of seed `r1/w1` (empty before the first sync).
    fn generation(&self) -> String {
        let out = self.call(&["manifest", &self.root(), "r1/w1"], b"");
        assert!(out.status.success(), "{}", text(&out.stderr));
        let first = out.stdout.split(|b| *b == 0).next().unwrap_or_default();
        text(first).split('\t').next_back().unwrap_or("").to_owned()
    }

    /// Sync the files named in `send` (from the laptop tree) and `delete`d
    /// paths to seed `r1/w1`, snapshotting for `run_id` when given.
    fn sync(&self, send: &[&str], delete: &[&str], run_id: Option<&str>, keep: bool) -> Output {
        let n = self.attempt.get() + 1;
        self.attempt.set(n);
        let attempt = format!("att-{n}");
        let gen_ = self.generation();
        let root = self.root();
        if !delete.is_empty() {
            self.ok(&["deletions", &root, "r1/w1", &attempt], &nul(delete));
        }
        let files: Vec<LocalFile> = send
            .iter()
            .map(|p| {
                let full = self.src.join(p);
                let meta = std::fs::metadata(&full).unwrap();
                let mtime = meta
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                LocalFile {
                    path: (*p).to_owned(),
                    size: meta.len(),
                    mtime,
                    kind: Kind::File {
                        exec: Path::new(p).extension().is_some_and(|e| e == "sh"),
                    },
                }
            })
            .collect();
        if !files.is_empty() {
            self.ok(&["changes", &root, "r1/w1", &attempt], &nul(send));
        }
        let refs: Vec<&LocalFile> = files.iter().collect();
        let tar = sync::write_tar(&self.src, &refs, u64::MAX, Vec::new()).unwrap();
        let (run, work_meta) = run_id.map_or(("", String::new()), |r| (r, label("work")));
        self.call(
            &[
                "receive",
                &root,
                "r1/w1",
                &label("seed"),
                &gen_,
                run,
                &work_meta,
                if keep { "1" } else { "0" },
                &attempt,
            ],
            &tar,
        )
    }

    fn run_words<'a>(
        &'a self,
        run_id: &'a str,
        keep: &'a str,
        extra: &[&'a str],
        cmd: &[&'a str],
    ) -> Vec<String> {
        let mut w: Vec<String> = [
            "run",
            &self.root(),
            run_id,
            "r1",
            keep,
            "2",
            &label("cache"),
            TTLS,
            "low",
            "1",
            "",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        w.extend(extra.iter().map(|s| (*s).to_owned()));
        w.push("--".to_owned());
        w.extend(cmd.iter().map(|s| (*s).to_owned()));
        w
    }

    /// Sync nothing new and run `cmd` as run `run_id` (a fresh snapshot).
    fn run(&self, run_id: &str, keep: bool, extra: &[&str], cmd: &[&str]) -> Output {
        let s = self.sync(&[], &[], Some(run_id), keep);
        assert!(s.status.success(), "{}", text(&s.stderr));
        self.run_snapshot(run_id, keep, extra, cmd, &[])
    }

    fn run_snapshot(
        &self,
        run_id: &str,
        keep: bool,
        extra: &[&str],
        cmd: &[&str],
        env: &[(&str, &str)],
    ) -> Output {
        let words = self.run_words(run_id, if keep { "1" } else { "0" }, extra, cmd);
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let mut c = self.command(&refs);
        for (k, v) in env {
            c.env(k, v);
        }
        run_with(c, b"")
    }

    /// Run a PowerShell snippet as the job (cross-platform: the same shell).
    fn job(&self, script: &str) -> Vec<String> {
        vec![
            self.ps.to_string_lossy().into_owned(),
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-Command".to_owned(),
            script.to_owned(),
        ]
    }

    fn jobrun(&self, run_id: &str, script: &str) -> Output {
        let job = self.job(script);
        let refs: Vec<&str> = job.iter().map(String::as_str).collect();
        self.run(run_id, false, &[], &refs)
    }

    fn cache_dir(&self) -> PathBuf {
        self.root.join("cache").join("r1")
    }

    fn stats(&self, slot: u32) -> String {
        std::fs::read_to_string(self.cache_dir().join(format!("tree-{slot}.stats")))
            .unwrap()
            .trim()
            .to_owned()
    }

    fn work_dirs(&self) -> Vec<String> {
        std::fs::read_dir(self.root.join("work"))
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn run_with(mut c: Command, stdin: &[u8]) -> Output {
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let data = stdin.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = input.write_all(&data);
    });
    let out = child.wait_with_output().unwrap();
    let _ = writer.join();
    out
}

fn nul(items: &[&str]) -> Vec<u8> {
    let mut v = Vec::new();
    for i in items {
        v.extend_from_slice(i.as_bytes());
        v.push(0);
    }
    v
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n")
}

macro_rules! host {
    () => {
        match Host::new() {
            Some(h) => h,
            None => return,
        }
    };
}

/// Wait until `f` is true, at most `secs`.
fn wait_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    f()
}

// ---- protocol basics ---------------------------------------------------

#[test]
fn ping_answers_and_an_unknown_verb_is_a_goway_failure() {
    let h = host!();
    assert_eq!(h.ok(&["ping"], b""), "goway-remote ok\n");
    let out = h.call(&["nope"], b"");
    assert_eq!(out.status.code(), Some(125));
    assert!(text(&out.stderr).contains("unknown verb: nope"));
}

#[test]
fn an_uninstalled_script_asks_to_be_installed_and_then_runs() {
    let Some(ps) = pwsh() else { return };
    let dir = tempfile::tempdir().unwrap();
    // The installed copy lives under LocalApplicationData: point it at the scratch dir.
    let envs: &[(&str, &Path)] = if cfg!(windows) {
        &[("LOCALAPPDATA", dir.path())]
    } else {
        &[("XDG_DATA_HOME", dir.path()), ("HOME", dir.path())]
    };
    // On Windows the installed copy lives in the real local app data: start clean.
    let installed = std::env::var_os("LOCALAPPDATA").map(|d| {
        PathBuf::from(d)
            .join("goway")
            .join(format!("remote-{}.ps1", remote::ps_script_id()))
    });
    let forget = || {
        if let Some(p) = installed.as_ref().filter(|_| cfg!(windows)) {
            let _ = std::fs::remove_file(p);
        }
    };
    forget();
    let run = |src: String, stdin: &[u8]| {
        let mut c = Command::new(ps);
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded(&src),
        ]);
        for (k, v) in envs {
            c.env(k, v);
        }
        run_with(c, stdin)
    };
    let first = run(remote::ps_invocation("ping", &[]), b"");
    assert_eq!(
        first.status.code(),
        Some(remote::PS_NOT_INSTALLED),
        "{}",
        text(&first.stderr)
    );
    let install = run(remote::ps_install(), remote::SCRIPT_PS.as_bytes());
    assert!(install.status.success(), "{}", text(&install.stderr));
    let again = run(remote::ps_invocation("ping", &[]), b"");
    assert_eq!(
        text(&again.stdout),
        "goway-remote ok\n",
        "{}",
        text(&again.stderr)
    );
    forget();
}

#[test]
fn every_argument_reaches_the_script_exactly() {
    let h = host!();
    // The attempt id is validated, so a hostile word shows up in the error
    // message exactly as sent: quotes, dollar signs, backticks, spaces.
    for hostile in [
        "a'b",
        "$env:PATH",
        "`n \"q\" ; & x",
        "it's \\\"x\\\"",
        "  lead trail  ",
    ] {
        let out = h.call(&["deletions", &h.root(), "r1/w1", hostile], b"");
        assert_eq!(out.status.code(), Some(125));
        assert!(
            text(&out.stderr).contains("deletions: bad id"),
            "{}",
            text(&out.stderr)
        );
    }
    // Words after `--` reach the job exactly.
    let job = [
        h.ps.to_string_lossy().into_owned(),
        "-NoProfile".to_owned(),
        "-NonInteractive".to_owned(),
        "-File".to_owned(),
    ];
    let echo = h.dir.path().join("echo.ps1");
    std::fs::write(
        &echo,
        "$o = [Console]::OpenStandardOutput(); foreach ($a in $args) { $b = [Text.Encoding]::UTF8.GetBytes($a + [string][char]0); $o.Write($b, 0, $b.Length) }; $o.Flush()\n",
    )
    .unwrap();
    let hostile = [
        "plain",
        "with space",
        "it's",
        "say \"hi\"",
        "$HOME `x`",
        "back\\slash",
        "a&b|c",
        "trail\\",
    ];
    let mut words: Vec<String> = job.to_vec();
    words.push(echo.to_string_lossy().into_owned());
    words.extend(hostile.iter().map(|s| (*s).to_owned()));
    let refs: Vec<&str> = words.iter().map(String::as_str).collect();
    let out = h.run("hostile1", false, &[], &refs);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let got: Vec<&str> = std::str::from_utf8(&out.stdout)
        .unwrap()
        .split('\0')
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(got, hostile);
}

// ---- the state root ----------------------------------------------------

#[test]
fn a_foreign_directory_is_never_adopted_and_purge_keeps_foreign_files() {
    let h = host!();
    let foreign = h.dir.path().join("precious");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("authorized_keys"), "key").unwrap();
    let out = h.call(&["manifest", &foreign.to_string_lossy(), "abc"], b"");
    assert_eq!(out.status.code(), Some(125), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("not goway state"));
    assert!(!foreign.join(".goway-root").exists());

    // A marked root: purge removes goway's entries and the marker only.
    h.ok(&["manifest", &h.root(), "abc"], b"");
    assert!(h.root.join(".goway-root").exists());
    std::fs::write(h.root.join("mine.txt"), "x").unwrap();
    assert_eq!(h.ok(&["purge", &h.root()], b""), "removed\n");
    assert!(h.root.join("mine.txt").exists(), "a foreign file survives");
    assert!(!h.root.join("seed").exists() && !h.root.join(".goway-root").exists());
    std::fs::remove_file(h.root.join("mine.txt")).unwrap();
    h.ok(&["manifest", &h.root(), "abc"], b"");
    assert_eq!(h.ok(&["purge", &h.root()], b""), "removed\n");
    assert!(!h.root.exists(), "an empty root goes too");
    assert_eq!(h.ok(&["purge", &h.root()], b""), "absent\n");
}

#[test]
fn purge_refuses_an_unmarked_root() {
    let h = host!();
    std::fs::create_dir(&h.root).unwrap();
    std::fs::write(h.root.join("x"), "1").unwrap();
    let out = h.call(&["purge", &h.root()], b"");
    assert_eq!(out.status.code(), Some(125));
    assert!(h.root.join("x").exists());
}

// ---- sync: manifest, receive, hashes, deletions -------------------------

#[test]
fn sync_round_trips_files_modes_and_hashes_like_remote_sh() {
    let h = host!();
    h.put("hello.txt", "hello\n", 1_700_000_000);
    h.put("src/run.sh", "echo hi\n", 1_700_000_001);
    let long = format!("deep/{}/leaf.txt", "d".repeat(130));
    h.put(&long, "leaf\n", 1_700_000_002);
    assert_eq!(
        h.ok(&["manifest", &h.root(), "r1/w1"], b""),
        "",
        "no tree yet"
    );
    let out = h.sync(&["hello.txt", "src/run.sh", &long], &[], None, false);
    assert!(out.status.success(), "{}", text(&out.stderr));

    let manifest = h.ok(&["manifest", &h.root(), "r1/w1"], b"");
    let entries = sync::parse_manifest(manifest.as_bytes());
    assert_eq!(entries.len(), 3, "{manifest:?}");
    let hello = &entries["hello.txt"];
    assert_eq!((hello.size, hello.mtime), (6, 1_700_000_000));
    assert!(matches!(hello.kind, Kind::File { exec: false }));
    assert!(
        matches!(entries["src/run.sh"].kind, Kind::File { exec: true }),
        "the execute bit survives"
    );
    assert!(entries.contains_key(long.as_str()), "a long path survives");
    assert!(manifest.starts_with("G\t0\t0\t0\t\t"), "{manifest:?}");

    // hashes: sha256 of the regular files named, nothing for missing ones.
    let hashes = h.ok(
        &["hashes", &h.root(), "r1/w1"],
        &nul(&["hello.txt", "missing"]),
    );
    assert_eq!(
        sync::parse_hashes(hashes.as_bytes())["hello.txt"],
        sync::file_sha256(&h.src.join("hello.txt")).unwrap()
    );
    assert_eq!(sync::parse_hashes(hashes.as_bytes()).len(), 1);

    // A later sync deletes and replaces by unlink and recreate.
    h.put("hello.txt", "HELLO!\n", 1_700_000_005);
    let out = h.sync(&["hello.txt"], &["src/run.sh"], None, false);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let entries = sync::parse_manifest(h.ok(&["manifest", &h.root(), "r1/w1"], b"").as_bytes());
    assert!(!entries.contains_key("src/run.sh"));
    assert_eq!(entries["hello.txt"].size, 7);
    let seed = h.root.join("seed/r1/w1");
    assert!(
        seed.join("changes/1").exists() && seed.join("changes/2").exists(),
        "the change log is kept"
    );
}

#[test]
fn a_stale_generation_is_refused_with_exit_75() {
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    assert!(h.sync(&["a.txt"], &[], None, false).status.success());
    let out = h.call(
        &[
            "receive",
            &h.root(),
            "r1/w1",
            &label("seed"),
            "not-the-generation",
            "",
            "",
            "0",
            "att-x",
        ],
        b"ignored",
    );
    assert_eq!(out.status.code(), Some(75), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("seed changed"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn locking_a_seed_survives_gc_removing_it_mid_acquire() {
    let h = host!();
    let seed = h.root.join("seed/repo/wt");
    let hook = format!("Remove-Item -Recurse -Force '{}'", seed.display());
    let out = h.call_env(
        &["manifest", &h.root(), "repo/wt"],
        &[("GOWAY_TEST_HOOK", &hook)],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        seed.join("lock").exists(),
        "the lock was retaken in a fresh dir"
    );
}

// ---- run: streaming, exit codes, slots ----------------------------------

#[test]
fn a_run_passes_output_and_the_exit_code_and_removes_its_work_dir() {
    let h = host!();
    h.put("hello.txt", "hello\n", 1_700_000_000);
    assert!(
        h.sync(&["hello.txt"], &[], Some("r-a"), false)
            .status
            .success()
    );
    let job = h.job("Get-Content hello.txt; [Console]::Error.WriteLine('to-stderr'); exit 7");
    let refs: Vec<&str> = job.iter().map(String::as_str).collect();
    let out = h.run_snapshot("r-a", false, &[], &refs, &[]);
    assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).trim(), "hello");
    assert!(text(&out.stderr).contains("to-stderr"));
    assert!(h.work_dirs().is_empty(), "{:?}", h.work_dirs());
    // Environment markers.
    let out = h.jobrun(
        "r-b",
        "Write-Output \"$env:GOWAY $env:GOWAY_RUN_ID\"; Write-Output $env:CARGO_TARGET_DIR",
    );
    let lines: Vec<String> = text(&out.stdout).lines().map(str::to_owned).collect();
    assert_eq!(lines[0], "1 r-b", "{}", text(&out.stderr));
    assert!(lines[1].ends_with("target-0"), "{lines:?}");
}

#[test]
fn a_missing_program_is_reported_as_not_found() {
    let h = host!();
    let out = h.run("r-nf", false, &[], &["goway-no-such-program-xyz"]);
    assert_ne!(out.status.code(), Some(0));
    assert!(h.work_dirs().is_empty());
}

#[test]
fn keep_copies_the_tree_out_and_the_slot_stays_usable() {
    let h = host!();
    h.put("hello.txt", "hello\n", 1_700_000_000);
    assert!(
        h.sync(&["hello.txt"], &[], Some("k1"), true)
            .status
            .success()
    );
    let job = h.job("Set-Content made.txt x");
    let refs: Vec<&str> = job.iter().map(String::as_str).collect();
    let out = h.run_snapshot("k1", true, &[], &refs, &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let kept = h.root.join("work/k1/tree");
    assert!(kept.join("hello.txt").is_file() && kept.join("made.txt").is_file());
    let out = h.jobrun("k2", "Get-ChildItem -Name");
    assert!(
        !text(&out.stdout).contains("made.txt"),
        "the slot lost the leftover"
    );
}

#[test]
fn a_slot_tree_persists_updates_in_place_and_removes_leftovers() {
    let h = host!();
    h.put("hello.txt", "hello\n", 1_700_000_000);
    h.put("other.txt", "other\n", 1_700_000_000);
    h.put("package.json", "{}\n", 1_700_000_000);
    assert!(
        h.sync(
            &["hello.txt", "other.txt", "package.json"],
            &[],
            None,
            false
        )
        .status
        .success()
    );
    let out = h.jobrun("p1", "New-Item -ItemType Directory -Force node_modules/x | Out-Null; Set-Content node_modules/x/i.js 1; Set-Content generated.txt 1");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(h.stats(0), "written=3 removed=0");

    // Second run: nothing is copied; dependency dirs stay; the leftover goes.
    let out = h.jobrun(
        "p2",
        "Test-Path node_modules/x/i.js; Test-Path generated.txt",
    );
    assert_eq!(
        text(&out.stdout).split_whitespace().collect::<Vec<_>>(),
        ["True", "False"],
        "{}",
        text(&out.stderr)
    );
    assert_eq!(h.stats(0), "written=0 removed=1");

    // An edit writes only that file; a deleted file disappears.
    h.put("hello.txt", "changed\n", 1_700_000_100);
    assert!(
        h.sync(&["hello.txt"], &["other.txt"], None, false)
            .status
            .success()
    );
    let out = h.jobrun("p3", "Get-Content hello.txt; Test-Path other.txt");
    assert_eq!(
        text(&out.stdout).split_whitespace().collect::<Vec<_>>(),
        ["changed", "False"]
    );
    assert_eq!(h.stats(0), "written=1 removed=1");
}

#[test]
fn same_size_edit_within_the_same_second_reaches_the_slot() {
    let h = host!();
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for content in ["aaaa\n", "bbbb\n", "cccc\n"] {
        h.put("f.txt", content, secs);
        assert!(h.sync(&["f.txt"], &[], None, false).status.success());
        let out = h.jobrun("same", "Get-Content f.txt");
        assert_eq!(
            text(&out.stdout).trim(),
            content.trim(),
            "{}",
            text(&out.stderr)
        );
    }
}

/// Build A in a slot, then run B whose changed source has an older mtime
/// than A's output: the build must still see B's source.
// The output is written fresh (Copy-Item would carry the source mtime over,
// which makes the warm check depend on timestamp precision, not on goway).
const STAMP_BUILD: &str = "if (-not (Test-Path out) -or ((Get-Item src.txt).LastWriteTimeUtc -gt (Get-Item out).LastWriteTimeUtc)) { Get-Content src.txt | Set-Content out; 'rebuilt' }; Get-Content out";

#[test]
fn changed_files_are_stamped_newer_than_the_slots_outputs() {
    let h = host!();
    h.put(".gitignore", "out\n", 1_700_000_000);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    h.put("src.txt", "from A\n", now);
    assert!(
        h.sync(&[".gitignore", "src.txt"], &[], None, false)
            .status
            .success()
    );
    let lines = |o: &Output| {
        text(&o.stdout)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(lines(&h.jobrun("s1", STAMP_BUILD)), ["rebuilt", "from A"]);
    // An older branch: different content, a much older mtime.
    h.put("src.txt", "from B\n", now - 3600);
    assert!(h.sync(&["src.txt"], &[], None, false).status.success());
    assert_eq!(
        lines(&h.jobrun("s2", STAMP_BUILD)),
        ["rebuilt", "from B"],
        "the build saw B's source"
    );
    // Nothing changed: the warm build stays warm.
    assert_eq!(lines(&h.jobrun("s3", STAMP_BUILD)), ["from B"]);
    assert_eq!(h.stats(0), "written=0 removed=0");
}

// frob:ticket 01M43CCF1E0CQW22RXDYS01CGE
// frob:tests crates/goway/src/remote.rs::SCRIPT_PS
#[test]
fn a_file_dated_an_hour_ahead_gets_the_hosts_time_and_never_rebuilds() {
    let h = host!();
    h.put(".gitignore", "out\n", 1_700_000_000);
    let ahead = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    h.put("src.txt", "from the laptop\n", ahead);
    assert!(
        h.sync(&[".gitignore", "src.txt"], &[], None, false)
            .status
            .success()
    );
    let lines = |o: &Output| {
        text(&o.stdout)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let first = h.jobrun("f1", STAMP_BUILD);
    assert_eq!(lines(&first), ["rebuilt", "from the laptop"]);
    // The slot's copy carries this host's time, not the laptop's future.
    let copy = h.cache_dir().join("tree-0").join("src.txt");
    let copied = std::fs::metadata(&copy).unwrap().modified().unwrap();
    assert!(
        copied < std::time::SystemTime::now() + Duration::from_secs(60),
        "the slot copy kept the laptop's future date"
    );
    let second = h.jobrun("f2", STAMP_BUILD);
    assert_eq!(
        lines(&second),
        ["from the laptop"],
        "the second run rebuilt"
    );
    assert_eq!(h.stats(0), "written=0 removed=0");
}

// frob:ticket 01M43CCF1E0CQW22RXDYS01CGE
// frob:tests crates/goway/src/remote.rs::SCRIPT_PS
#[cfg(unix)]
#[test]
fn a_work_dir_that_cannot_be_removed_never_changes_the_exit_code() {
    use std::os::unix::fs::PermissionsExt as _;
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    assert!(h.sync(&["a.txt"], &[], Some("rm1"), false).status.success());
    // The job leaves an unremovable directory in its own work dir.
    let root = h.root().replace('\'', "''");
    let job = h.job(&format!(
        "$w = '{root}/work/' + $env:GOWAY_RUN_ID; New-Item -ItemType Directory \"$w/d\" | Out-Null; Set-Content \"$w/d/f\" x; chmod 555 \"$w/d\"; exit 7"
    ));
    let refs: Vec<&str> = job.iter().map(String::as_str).collect();
    let out = h.run_snapshot("rm1", false, &[], &refs, &[]);
    for dir in h.work_dirs() {
        let stuck = h.root.join("work").join(dir).join("d");
        let _ = std::fs::set_permissions(stuck, std::fs::Permissions::from_mode(0o755));
    }
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(7), "{err}");
    assert!(err.contains("gc will collect it"), "{err}");
}

// frob:ticket 01M43ETRMKFS15MWTTVZHX18YW
// frob:tests crates/goway/src/run.rs::discard_work
#[test]
fn discard_removes_only_the_named_work_dir_and_refuses_a_bad_run_id() {
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    assert!(h.sync(&["a.txt"], &[], Some("d-a"), false).status.success());
    assert!(h.sync(&["a.txt"], &[], Some("d-b"), false).status.success());
    assert!(h.call(&["discard", &h.root(), "d-a"], b"").status.success());
    assert_eq!(h.work_dirs(), ["d-b"]);
    // A missing dir is fine; a path-shaped id is refused and removes nothing.
    assert!(h.call(&["discard", &h.root(), "d-a"], b"").status.success());
    assert!(
        !h.call(&["discard", &h.root(), "../work/d-b"], b"")
            .status
            .success()
    );
    assert_eq!(h.work_dirs(), ["d-b"]);
}

#[test]
fn concurrent_runs_use_different_slots_and_a_worktree_prefers_its_last_slot() {
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    assert!(h.sync(&["a.txt"], &[], Some("c1"), false).status.success());
    // The first run holds slot 0 until the test releases it.
    let release = h.dir.path().join("release");
    let job = h.job(&format!(
        "(Get-Location).Path; while (-not (Test-Path '{}')) {{ Start-Sleep -Milliseconds 100 }}",
        release.display()
    ));
    let refs: Vec<&str> = job.iter().map(String::as_str).collect();
    let words = h.run_words("c1", "0", &[], &refs);
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    let busy = h
        .command(&w)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(wait_for(60, || h.cache_dir().join("tree-0").is_dir()
        && h.root.join("work/c1/pid").exists()));
    let out = h.jobrun("c2", "(Get-Location).Path");
    assert!(
        text(&out.stdout).trim().ends_with("tree-1"),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    std::fs::write(&release, "go").unwrap();
    let first = busy.wait_with_output().unwrap();
    assert!(
        text(&first.stdout).trim().ends_with("tree-0"),
        "{}",
        text(&first.stdout)
    );
    // Slot 0 is free again, but this worktree last used slot 1.
    let out = h.jobrun("c3", "(Get-Location).Path");
    assert!(
        text(&out.stdout).trim().ends_with("tree-1"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn a_dropped_connection_stops_the_job_tree_and_releases_the_slot() {
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    assert!(h.sync(&["a.txt"], &[], Some("d1"), false).status.success());
    let pidfile = h.dir.path().join("jobpid");
    let job = h.job(&format!(
        "Set-Content '{}' $PID; Start-Sleep 120",
        pidfile.display()
    ));
    let refs: Vec<&str> = job.iter().map(String::as_str).collect();
    let words = h.run_words("d1", "0", &[], &refs);
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    // The "ssh session" is a PowerShell that runs the script as a child:
    // killing it is the connection dropping (the child outlives it).
    let inner = h.command(&w);
    let mut line = vec![inner.get_program().to_string_lossy().into_owned()];
    line.extend(inner.get_args().map(|a| a.to_string_lossy().into_owned()));
    let session_src = format!("{}; exit $LASTEXITCODE", goway::transport::ps_call(&line));
    let mut session = Command::new(h.ps)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &encoded(&session_src),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(wait_for(90, || pidfile.exists()
        && !std::fs::read_to_string(&pidfile)
            .unwrap_or_default()
            .trim()
            .is_empty()));
    let pid: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    session.kill().unwrap();
    session.wait().unwrap();
    let alive = |pid: u32| {
        if cfg!(windows) {
            Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}")])
                .output()
                .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
        } else {
            Path::new(&format!("/proc/{pid}")).exists()
        }
    };
    assert!(wait_for(30, || !alive(pid)), "the job outlived its session");
    assert!(
        wait_for(30, || h.work_dirs().is_empty()),
        "the run folder is removed: {:?}",
        h.work_dirs()
    );
    // The slot is free again.
    let out = h.jobrun("d2", "'again'");
    assert_eq!(text(&out.stdout).trim(), "again", "{}", text(&out.stderr));
}

// ---- copy integrity ----------------------------------------------------

impl Host {
    /// Wait for the claims of `phase` of run `run_id` (goway's control call).
    fn claims(&self, run_id: &str, phase: &str) -> Option<Vec<(String, sync::Claim)>> {
        for _ in 0..120 {
            let out = self.call(&["verify-wait", &self.root(), run_id, phase, "5"], b"");
            assert!(out.status.success(), "{}", text(&out.stderr));
            if let Some(rest) = out.stdout.strip_prefix(b"ready\n") {
                return Some(sync::parse_claims(rest).unwrap());
            }
            if out.stdout.starts_with(b"ended") {
                return None;
            }
        }
        panic!("no claims for phase {phase}");
    }

    fn verdict(&self, run_id: &str, phase: &str, v: &str) {
        self.ok(&["verify-verdict", &self.root(), run_id, phase, v], b"");
    }

    /// The laptop's files as a sync manifest, for comparing claims.
    fn manifest_of(&self, paths: &[&str]) -> sync::Manifest {
        sync::Manifest(
            paths
                .iter()
                .map(|p| {
                    let meta = std::fs::metadata(self.src.join(p)).unwrap();
                    let mtime = meta
                        .modified()
                        .unwrap()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    LocalFile {
                        path: (*p).to_owned(),
                        size: meta.len(),
                        mtime,
                        kind: Kind::File { exec: false },
                    }
                })
                .collect(),
        )
    }

    fn spawn_run(
        &self,
        run_id: &str,
        extra: &[&str],
        script: &str,
        env: &[(&str, &str)],
    ) -> std::thread::JoinHandle<Output> {
        let job = self.job(script);
        let refs: Vec<&str> = job.iter().map(String::as_str).collect();
        let words = self.run_words(run_id, "0", extra, &refs);
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        let mut c = self.command(&w);
        for (k, v) in env {
            c.env(k, v);
        }
        std::thread::spawn(move || run_with(c, b""))
    }
}

#[test]
fn the_helper_verifies_what_it_wrote_and_a_failure_is_verified_again() {
    let h = host!();
    h.put("hello.txt", "hello\n", 1_700_000_000);
    assert!(
        h.sync(&["hello.txt"], &[], Some("v1"), false)
            .status
            .success()
    );
    let run = h.spawn_run(
        "v1",
        &["verify:1:changed"],
        "Write-Output started; exit 3",
        &[],
    );
    let claims = h
        .claims("v1", "1")
        .expect("claims before the command starts");
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].0, "hello.txt");
    let cmp = sync::compare_claims(&h.src, &h.manifest_of(&["hello.txt"]), &claims);
    assert_eq!((cmp.checked, cmp.mismatches.len()), (1, 0), "{cmp:?}");
    h.verdict("v1", "1", "ok");
    // The command fails: every synced file it did not change is claimed again.
    let claims = h.claims("v1", "2").expect("claims after the failure");
    assert_eq!(claims.len(), 1);
    h.verdict("v1", "2", "ok");
    let out = run.join().unwrap();
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).trim(), "started");
}

#[test]
fn files_a_failing_command_changed_itself_are_not_claimed() {
    let h = host!();
    h.put("hello.txt", "hello\n", 1_700_000_000);
    h.put("other.txt", "other\n", 1_700_000_000);
    assert!(
        h.sync(&["hello.txt", "other.txt"], &[], Some("v2"), false)
            .status
            .success()
    );
    let run = h.spawn_run(
        "v2",
        &["verify:1:all"],
        "Add-Content hello.txt 'more'; exit 4",
        &[],
    );
    assert_eq!(
        h.claims("v2", "1").unwrap().len(),
        2,
        "distrusted: every file"
    );
    h.verdict("v2", "1", "ok");
    let claims = h.claims("v2", "2").unwrap();
    assert_eq!(
        claims.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
        ["other.txt"]
    );
    h.verdict("v2", "2", "ok");
    assert_eq!(run.join().unwrap().status.code(), Some(4));
}

#[test]
fn a_bad_verdict_stops_the_command_and_discards_the_slot_and_the_seed() {
    let h = host!();
    h.put("hello.txt", "hello\n", 1_700_000_000);
    assert!(
        h.sync(&["hello.txt"], &[], Some("v3"), false)
            .status
            .success()
    );
    let run = h.spawn_run(
        "v3",
        &["verify:1:changed"],
        "Write-Output ran",
        &[("GOWAY_TEST_CORRUPT", "1")],
    );
    let claims = h.claims("v3", "1").unwrap();
    let cmp = sync::compare_claims(&h.src, &h.manifest_of(&["hello.txt"]), &claims);
    assert_eq!(
        cmp.mismatches.len(),
        1,
        "the falsified hash is caught: {cmp:?}"
    );
    h.verdict("v3", "1", "bad");
    let out = run.join().unwrap();
    assert_eq!(out.status.code(), Some(125), "{}", text(&out.stderr));
    assert!(
        !text(&out.stdout).contains("ran"),
        "the command never started"
    );
    assert!(
        text(&out.stderr).contains("did not verify"),
        "{}",
        text(&out.stderr)
    );
    assert!(
        !h.cache_dir().join("tree-0").exists(),
        "the slot tree is gone"
    );
    assert!(
        h.root.join("seed/r1/w1/fresh").exists(),
        "the seed is rebuilt, never from a sibling"
    );
    assert!(h.work_dirs().is_empty());
    // The rerun (attempt 2) starts from scratch and passes.
    h.put("hello.txt", "hello\n", 1_700_000_000);
    assert!(
        h.sync(&["hello.txt"], &[], Some("v4"), false)
            .status
            .success()
    );
    let run = h.spawn_run("v4", &["verify:2:changed:fresh"], "Write-Output ran", &[]);
    assert_eq!(h.claims("v4", "1").unwrap().len(), 1);
    h.verdict("v4", "1", "ok");
    let out = run.join().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).trim(), "ran");
}

// ---- gc ----------------------------------------------------------------

fn backdate(path: &Path, days: u64) {
    let t = std::time::SystemTime::now() - Duration::from_secs(days * 86_400);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}

fn gc(h: &Host, mode: &str, repo: &str, older: &str) -> String {
    h.ok(
        &[
            "gc",
            &h.root(),
            &now(),
            "604800",
            "86400",
            "259200",
            mode,
            repo,
            older,
        ],
        b"",
    )
}

#[test]
fn gc_removes_expired_labelled_entries_and_keeps_busy_fresh_and_unlabelled_ones() {
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    // An expired kept work dir, an expired seed and cache, and a running job.
    assert!(
        h.sync(&["a.txt"], &[], Some("old-kept"), true)
            .status
            .success()
    );
    // (No run here: a run's detached gc would race the backdating below.)
    std::fs::create_dir_all(h.cache_dir()).unwrap();
    std::fs::write(
        h.cache_dir().join("meta.json"),
        text(
            &base64::engine::general_purpose::STANDARD
                .decode(label("cache"))
                .unwrap(),
        ),
    )
    .unwrap();
    // A run that has started removed its starting-run markers; this dir stands for one.
    for marker in ["creator", "born"] {
        let _ = std::fs::remove_file(h.root.join("work/old-kept").join(marker));
    }
    backdate(&h.root.join("work/old-kept/meta.json"), 4); // past kept_ttl (3d)
    backdate(&h.root.join("seed/r1/w1/meta.json"), 8); // past cache_ttl (7d)
    backdate(&h.cache_dir().join("meta.json"), 1); // fresh enough: kept

    // Dry run lists, removes nothing.
    let listing = gc(&h, "dry", "", "");
    assert!(
        listing.contains("remove\twork\t") && listing.contains("remove\tseed\t"),
        "{listing}"
    );
    assert!(h.root.join("work/old-kept").exists());
    // The repository filter matches by name or id.
    assert_eq!(gc(&h, "dry", "nope", ""), "");
    assert!(gc(&h, "dry", "proj", "").contains("proj\tr1\t"));

    // A running job: its work dir is old but locked.
    assert!(h.sync(&[], &[], Some("busy"), false).status.success());
    // The sync refreshed the seed; age it again.
    backdate(&h.root.join("seed/r1/w1/meta.json"), 8);
    let release = h.dir.path().join("release");
    let busy_job = h.job(&format!(
        "while (-not (Test-Path '{}')) {{ Start-Sleep -Milliseconds 100 }}",
        release.display()
    ));
    let busy_run = h.spawn_run_job("busy", &busy_job);
    assert!(wait_for(90, || h.root.join("work/busy/pid").exists()));
    backdate(&h.root.join("work/busy/meta.json"), 9);
    let applied = gc(&h, "apply", "", "");
    assert!(applied.contains("busy\twork\t"), "{applied}");
    assert!(
        !h.root.join("work/old-kept").exists(),
        "the expired kept dir is removed\n{applied}"
    );
    assert!(
        h.root.join("work/busy").exists(),
        "a locked dir is untouched"
    );
    assert!(h.cache_dir().exists(), "a fresh cache stays");
    std::fs::write(&release, "go").unwrap();
    assert!(busy_run.join().unwrap().status.success());
    assert!(
        !h.root.join("seed/r1/w1").exists(),
        "the expired seed is gone"
    );

    // --older-than replaces every TTL; --all style 0 removes the rest.
    let all = h.ok(
        &["gc", &h.root(), &now(), "0", "0", "0", "apply", "", ""],
        b"",
    );
    assert!(all.contains("remove\tcache\t"), "{all}");
    assert!(
        !h.cache_dir().exists(),
        "the idle cache goes with its slot tree"
    );
}

#[test]
fn gc_never_removes_unlabelled_entries_or_anything_under_an_unmarked_root() {
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    assert!(h.sync(&["a.txt"], &[], Some("lbl"), false).status.success());
    // An entry goway did not label as a work dir is left alone.
    let foreign = h.root.join("work/foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join("meta.json"), "{\"kind\":\"seed\"}").unwrap();
    backdate(&foreign.join("meta.json"), 30);
    let out = h.ok(
        &["gc", &h.root(), &now(), "0", "0", "0", "apply", "", ""],
        b"",
    );
    assert!(out.contains("unlabelled\twork\t"), "{out}");
    assert!(foreign.exists());
    // An unmarked root: nothing is removed.
    let other = h.dir.path().join("other-root");
    std::fs::create_dir_all(other.join("work/x")).unwrap();
    let out = h.call(
        &[
            "gc",
            &other.to_string_lossy(),
            &now(),
            "0",
            "0",
            "0",
            "apply",
            "",
            "",
        ],
        b"",
    );
    assert!(out.status.success());
    assert!(
        text(&out.stderr).contains("not marked as goway state"),
        "{}",
        text(&out.stderr)
    );
    assert!(other.join("work/x").exists());
}

#[test]
fn gc_never_removes_a_work_dir_created_moments_ago() {
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    assert!(
        h.sync(&["a.txt"], &[], Some("fresh-run"), false)
            .status
            .success()
    );
    // The run's lock does not exist yet; with every TTL at 0 the dir still stays.
    let out = h.ok(
        &["gc", &h.root(), &now(), "0", "0", "0", "apply", "", ""],
        b"",
    );
    assert!(out.contains("keep\twork\t"), "{out}");
    assert!(h.root.join("work/fresh-run").exists());
}

#[test]
fn every_run_triggers_a_detached_gc_of_expired_entries() {
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    assert!(
        h.sync(&["a.txt"], &[], Some("auto1"), false)
            .status
            .success()
    );
    // An expired labelled orphan work dir (a crashed run).
    let orphan = h.root.join("work/orphan");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(
        orphan.join("meta.json"),
        "{\"kind\":\"work\",\"repo\":\"x\",\"repo_id\":\"x1\"}",
    )
    .unwrap();
    std::fs::write(orphan.join("lock"), "").unwrap();
    backdate(&orphan.join("meta.json"), 3);
    let out = h.run_snapshot(
        "auto1",
        false,
        &[],
        &h.job("'x'").iter().map(String::as_str).collect::<Vec<_>>(),
        &[],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        wait_for(120, || !orphan.exists()),
        "the detached gc removes the expired orphan"
    );
}

impl Host {
    fn spawn_run_job(&self, run_id: &str, job: &[String]) -> std::thread::JoinHandle<Output> {
        let refs: Vec<&str> = job.iter().map(String::as_str).collect();
        let words = self.run_words(run_id, "0", &[], &refs);
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        let c = self.command(&w);
        std::thread::spawn(move || run_with(c, b""))
    }
}

// ---- facts -------------------------------------------------------------

#[test]
fn probe_reports_the_facts_goway_schedules_with() {
    let h = host!();
    let out = h.ok(&["probe", &h.root(), "disk", "static"], b"");
    let probe = goway::pool::parse_probe(&out).unwrap_or_else(|| panic!("{out}"));
    assert!(probe.cores >= 1 && !probe.arch.is_empty() && !probe.hostname.is_empty());
    assert!(probe.disk_free.is_some() && probe.disk_used.is_some());
    for key in [
        "mem_total=",
        "mem_avail=",
        "static=1",
        "cpu_flags=",
        "kvm=",
        "docker=",
        "wsl=",
        "winvideo=",
        "os=",
        "jobs=0",
    ] {
        assert!(out.contains(key), "{key} missing:\n{out}");
    }
    let os = if cfg!(windows) {
        "os=windows"
    } else {
        "os=linux"
    };
    assert!(out.contains(os), "{out}");
    // Without the optional words only the live facts come.
    let live = h.ok(&["probe", &h.root()], b"");
    assert!(
        !live.contains("static=1") && !live.contains("disk_used"),
        "{live}"
    );
}

#[test]
fn doctor_reports_tools_and_host_facts_as_key_value_lines() {
    let h = host!();
    let out = h.ok(&["doctor", &h.root()], b"");
    for key in [
        "cargo_env=",
        "tool.git=",
        "tool.cargo=",
        "tool.tar=",
        "os=",
        "arch=",
        "disk_free=",
        "password_auth=",
        "home=",
        "static=1",
        "cargo_home=",
    ] {
        assert!(out.contains(key), "{key} missing:\n{out}");
    }
    assert!(out.lines().all(|l| l.contains('=')), "{out}");
}

// ---- GPU slots, env, args ---------------------------------------------

#[test]
fn a_gpu_run_without_a_listed_gpu_warns_and_runs_anyway() {
    let h = host!();
    let out = h.run(
        "g1",
        false,
        &["gpu-slots:1"],
        &h.job("'ran'")
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).trim(), "ran");
    if !goway_has_gpu_tool() {
        assert!(
            text(&out.stderr).contains("no GPU tool lists one"),
            "{}",
            text(&out.stderr)
        );
    }
}

fn goway_has_gpu_tool() -> bool {
    Command::new("nvidia-smi")
        .arg("-L")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn env_values_and_long_command_lines_travel_through_files_not_arguments() {
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    assert!(h.sync(&["a.txt"], &[], Some("e1"), false).status.success());
    h.ok(
        &["envfile", &h.root(), "e1"],
        b"MY_KEY=my value $x\0OTHER=2\0",
    );
    // The command words come from the args file: no length limit on a command line.
    let job = h.job("Write-Output \"$env:MY_KEY|$env:OTHER\"");
    let mut bytes = Vec::new();
    for w in &job {
        bytes.extend_from_slice(w.as_bytes());
        bytes.push(0);
    }
    h.ok(&["argsfile", &h.root(), "e1"], &bytes);
    let words = h.run_words("e1", "0", &[], &["@args"]);
    let refs: Vec<&str> = words.iter().map(String::as_str).collect();
    let out = h.call(&refs, b"");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).trim(), "my value $x|2");
    assert!(
        !h.root.join("work/e1").exists(),
        "the env file went with the run"
    );
}

// ---- shard framework detection (unix, needs a C compiler) ---------------

#[cfg(unix)]
fn build_fixture(dir: &Path, name: &str, markers: &[&str], body: &str) -> PathBuf {
    let list = markers.iter().fold(String::new(), |mut a, m| {
        use std::fmt::Write as _;
        let _ = write!(a, "\"{m}\",");
        a
    });
    let src = format!(
        "#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\nconst char *const embedded[] = {{{list}0}};\n{body}"
    );
    let c = dir.join(format!("{name}.c"));
    std::fs::write(&c, src).unwrap();
    let exe = dir.join(name);
    let ok = Command::new("cc")
        .args(["-O0", "-s", "-o"])
        .arg(&exe)
        .arg(&c)
        .status();
    assert!(
        ok.is_ok_and(|s| s.success()),
        "a C compiler is needed for the fixtures"
    );
    exe
}

#[cfg(unix)]
#[test]
fn shard_detection_reads_binaries_and_reruns_catch2_at_most_once() {
    if Command::new("cc").arg("--version").output().is_err() {
        return;
    }
    let h = host!();
    h.put("a.txt", "a", 1_700_000_000);
    let gtest = build_fixture(
        h.dir.path(),
        "gt",
        &[
            "GTEST_SHARD_INDEX",
            "GTEST_TOTAL_SHARDS",
            "--gtest_list_tests",
            "--gtest_filter",
        ],
        "int main(void){const char*s=getenv(\"GTEST_SHARD_STATUS_FILE\"); if(s){FILE*f=fopen(s,\"w\"); if(f) fclose(f);} printf(\"GT %s/%s\\n\", getenv(\"GTEST_SHARD_INDEX\"), getenv(\"GTEST_TOTAL_SHARDS\")); return 0;}",
    );
    let catch2 = build_fixture(
        h.dir.path(),
        "c2",
        &[
            "--shard-count",
            "--shard-index",
            "--list-tests",
            "catch2-version",
        ],
        "int main(int argc,char**argv){for(int i=1;i<argc;i++) if(!strcmp(argv[i],\"--shard-count\")){fprintf(stderr,\"\\nError(s) in input:\\n  Unrecognised token: --shard-count\\n\"); return 1;} puts(\"ALL\"); return 0;}",
    );
    let both = build_fixture(
        h.dir.path(),
        "both",
        &[
            "GTEST_SHARD_INDEX",
            "GTEST_TOTAL_SHARDS",
            "--gtest_list_tests",
            "--gtest_filter",
            "--shard-count",
            "--shard-index",
            "--list-tests",
            "catch2-version",
        ],
        "int main(void){puts(\"BOTH\"); return 0;}",
    );
    let run = |id: &str, exe: &Path| {
        assert!(h.sync(&["a.txt"], &[], Some(id), false).status.success());
        let words = h.run_words(
            id,
            "0",
            &["shard-detect:1:2:ABC123"],
            &[&exe.to_string_lossy()],
        );
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        h.call(&refs, b"")
    };
    let out = run("sd1", &gtest);
    assert_eq!(text(&out.stdout).trim(), "GT 0/2", "{}", text(&out.stderr));
    assert!(
        text(&out.stderr)
            .contains("detected=gtest attempts=0 rerun=0 rejected=0 flagged=0 duplicated=0"),
        "{}",
        text(&out.stderr)
    );
    let out = run("sd2", &catch2);
    assert_eq!(text(&out.stdout).trim(), "ALL");
    let err = text(&out.stderr);
    assert!(
        err.contains("detected=catch2 attempts=1,0 rerun=1 rejected=0"),
        "{err}"
    );
    let out = run("sd3", &both);
    assert!(
        text(&out.stderr).contains("detected=none"),
        "both frameworks' markers count as neither"
    );
    let out = run("sd4", Path::new("a.txt"));
    assert!(text(&out.stderr).contains("detected=none") || out.status.code() != Some(0));
}

const MIB: u64 = 1 << 20;
const TEN_YEARS: &str = "315360000";

/// Set a file's mtime to `secs` seconds ago.
fn age(path: &Path, secs: u64) {
    let when = std::time::SystemTime::now() - Duration::from_secs(secs);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

/// A marked root with one repository cache per `(name, [slot ages])`; every slot holds a
/// 1 MiB tree. Returns the cache dir of each repository in order.
fn budget_root(h: &Host, repos: &[(&str, &[u64])]) {
    h.ok(&["manifest", &h.root(), "abc"], b"");
    for (name, slots) in repos {
        let cache = h.root.join("cache").join(name);
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(
            cache.join("meta.json"),
            format!(r#"{{"kind":"cache","repo":"{name}","repo_id":"id-{name}"}}"#),
        )
        .unwrap();
        for (k, secs) in slots.iter().enumerate() {
            std::fs::create_dir_all(cache.join(format!("tree-{k}"))).unwrap();
            let f = std::fs::File::create(cache.join(format!("tree-{k}/big"))).unwrap();
            f.set_len(MIB).unwrap();
            let lock = cache.join(format!("target-{k}.lock"));
            std::fs::write(&lock, "").unwrap();
            age(&lock, *secs);
        }
        age(&cache.join("meta.json"), *slots.iter().min().unwrap());
    }
}

fn budget_gc(h: &Host, mode: &str, max_disk: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    h.ok(
        &[
            "gc",
            &h.root(),
            &now,
            TEN_YEARS,
            TEN_YEARS,
            TEN_YEARS,
            mode,
            "",
            "",
            &max_disk.to_string(),
            "0",
        ],
        b"",
    )
}

fn evict_lines(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.starts_with("evict\t")).collect()
}

// frob:ticket 01M4262XD6M7F91AA2VMVTNZHA
// frob:tests crates/goway/src/remote.rs::SCRIPT_PS
#[test]
fn eviction_removes_the_least_recently_used_slot_first_and_reports_it() {
    let Some(h) = Host::new() else { return };
    // alpha's slot 1 is the oldest, then alpha 0, then beta 0; 3 MiB against 2.5 MiB.
    budget_root(&h, &[("alpha", &[3000, 5000]), ("beta", &[1000])]);
    let out = budget_gc(&h, "apply", 2 * MIB + MIB / 2);
    let evicted = evict_lines(&out);
    assert_eq!(evicted.len(), 1, "{out}");
    assert!(evicted[0].contains("\tslot\t"), "{out}");
    assert!(evicted[0].ends_with("tree-1"), "{out}");
    assert!(evicted[0].contains("\talpha\tid-alpha\t"), "{out}");
    assert!(!h.root.join("cache/alpha/tree-1").exists());
    assert!(h.root.join("cache/alpha/tree-0").exists());
    assert!(h.root.join("cache/beta/tree-0").exists());
}

// frob:ticket 01M4262XD6M7F91AA2VMVTNZHA
// frob:tests crates/goway/src/remote.rs::SCRIPT_PS
#[test]
fn a_dry_run_lists_what_eviction_would_remove_and_removes_nothing() {
    let Some(h) = Host::new() else { return };
    budget_root(&h, &[("alpha", &[3000, 5000])]);
    let out = budget_gc(&h, "dry", MIB + MIB / 2);
    assert_eq!(evict_lines(&out).len(), 1, "{out}");
    assert!(h.root.join("cache/alpha/tree-1").exists());
    // Within budget: nothing to evict.
    assert!(evict_lines(&budget_gc(&h, "dry", 100 * MIB)).is_empty());
}

// frob:ticket 01M4262XD6M7F91AA2VMVTNZHA
// frob:tests crates/goway/src/remote.rs::SCRIPT_PS
#[test]
fn the_automatic_gc_leaves_a_summary_and_probe_reports_the_budget() {
    let Some(h) = Host::new() else { return };
    budget_root(&h, &[("alpha", &[3000, 5000])]);
    // Nothing is expired by age (ten-year TTLs); the budget decides.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let now = now.as_str();
    h.ok(
        &[
            "auto-gc",
            &h.root(),
            now,
            TEN_YEARS,
            TEN_YEARS,
            TEN_YEARS,
            "apply",
            "",
            "",
            &(MIB + MIB / 2).to_string(),
            "0",
            "log",
        ],
        b"",
    );
    let log = std::fs::read_to_string(h.root.join("evicted.log")).unwrap();
    assert!(log.contains("evicted 1 entries, freed 1.0 MiB"), "{log}");
    let probe = h.ok(&["probe", &h.root(), "disk", "budget:1000:2000"], b"");
    assert!(probe.contains("disk_max=1000\n"), "{probe}");
    assert!(probe.contains("disk_min_free=2000\n"), "{probe}");
    // 0 means automatic: 20% of the disk, at most 50 GiB.
    let probe = h.ok(&["probe", &h.root(), "disk", "budget:0:2000"], b"");
    let max: u64 = probe
        .lines()
        .find_map(|l| l.strip_prefix("disk_max="))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(max > 0 && max <= 50 << 30, "{probe}");
}

// frob:ticket 01M43AS3TM0HGV1Q866V4A27S6
// frob:tests crates/goway/src/remote.rs::SCRIPT_PS
#[test]
fn probe_reports_the_clock_and_asked_for_tools() {
    let Some(h) = Host::new() else { return };
    let out = h.ok(
        &["probe", &h.root(), "tools:pwsh,no-such-tool-xyz,bad;name"],
        b"",
    );
    let epoch: u64 = out
        .lines()
        .find_map(|l| l.strip_prefix("epoch="))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(now.abs_diff(epoch) < 30, "{out}");
    assert!(out.contains("want.no-such-tool-xyz=\n"), "{out}");
    assert!(
        !out.contains("bad;name") && !out.contains("want.bad"),
        "{out}"
    );
}

/// A work dir for run `id` whose job (a `sleep` in its own session) is recorded in `pid`.
#[cfg(unix)]
fn work_with_job(h: &Host, id: &str) -> (PathBuf, std::process::Child) {
    let work = h.root.join("work").join(id);
    std::fs::create_dir_all(&work).unwrap();
    let child = Command::new("setsid")
        .args(["sleep", "60"])
        .spawn()
        .expect("setsid and sleep exist where pwsh does");
    std::fs::write(work.join("pid"), child.id().to_string()).unwrap();
    (work, child)
}

// frob:ticket 01M43AS3TM0HGV1Q866V4A27S6
// frob:tests crates/goway/src/remote.rs::SCRIPT_PS
#[cfg(unix)]
#[test]
fn the_lifeline_stops_the_job_when_the_client_goes_away_and_spares_a_finished_run() {
    let Some(h) = Host::new() else { return };
    h.ok(&["manifest", &h.root(), "abc"], b"");
    // The client closes its end: the job is stopped and the run marked lost.
    let (work, mut job) = work_with_job(&h, "run1");
    let out = h.call(&["lifeline", &h.root(), "run1"], b"");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(work.join("lost").exists());
    assert!(
        job.wait().is_ok_and(|s| !s.success()),
        "the job was stopped"
    );
    // A run that already finished is left alone.
    let (work, mut job) = work_with_job(&h, "run2");
    std::fs::write(work.join("done"), "").unwrap();
    let out = h.call(&["lifeline", &h.root(), "run2"], b"");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(!work.join("lost").exists());
    assert!(job.try_wait().unwrap().is_none(), "the job still runs");
    job.kill().unwrap();
    let _ = job.wait();
}

// frob:ticket 01M43AS3TM0HGV1Q866V4A27S6
// frob:tests crates/goway/src/remote.rs::SCRIPT_PS
#[test]
fn a_starting_runs_work_dir_survives_gc_by_liveness_not_by_age() {
    let Some(h) = Host::new() else { return };
    h.ok(&["manifest", &h.root(), "abc"], b"");
    let day = 86_400;
    let make = |name: &str, creator: Option<u32>| {
        let dir = h.root.join("work").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            r#"{"kind":"work","repo":"r","repo_id":"i"}"#,
        )
        .unwrap();
        age(&dir.join("meta.json"), day);
        if let Some(pid) = creator {
            std::fs::write(dir.join("creator"), format!("{pid}\n")).unwrap();
        }
    };
    // Both are a day old and have no lock yet; only the one whose creator lives is kept.
    make("alive", Some(std::process::id()));
    make("orphan", None);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let out = h.ok(
        &[
            "gc",
            &h.root(),
            &now,
            "604800",
            "3600",
            "259200",
            "apply",
            "",
            "",
        ],
        b"",
    );
    let line = |name: &str| {
        out.lines()
            .find(|l| l.ends_with(name))
            .unwrap_or("")
            .to_owned()
    };
    assert!(line("alive").starts_with("keep\t"), "{out}");
    assert!(line("orphan").starts_with("remove\t"), "{out}");
    assert!(h.root.join("work/alive").exists());
    assert!(!h.root.join("work/orphan").exists());
}

// frob:ticket 01M43AS3TM0HGV1Q866V4A27S6
// frob:tests crates/goway/src/remote.rs::SCRIPT_PS
#[test]
fn a_windows_job_holds_a_keep_awake_request_for_exactly_its_lifetime() {
    // The request is the job's own thread's, so it can only be checked in the source:
    // set just before the job starts and cleared in a `finally` right after it ends.
    let s = remote::SCRIPT_PS;
    assert!(s.contains("SetThreadExecutionState(on ? 0x80000001u : 0x80000000u)"));
    let on = s
        .find("[GowayNative]::KeepAwake($true)")
        .expect("set before the job");
    let job = s.find("else { $rc = Run-Job").expect("the job runs");
    let off = s
        .find("[GowayNative]::KeepAwake($false)")
        .expect("cleared after it");
    assert!(on < job && job < off);
    assert!(s[job..off].contains("finally"));
}
