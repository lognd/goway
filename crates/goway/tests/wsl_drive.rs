//! WSL helpers: `df` inside WSL shows the virtual disk, not the Windows drive that holds it, so
//! goway reads the drive through its drvfs mount, keeps a reserve on it, and sizes its default
//! disk budget from it. The fake host is a scratch `/proc` (version and mounts), a fake drive
//! directory holding an `ext4.vhdx`, and a `df` that answers per path.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// A fake WSL machine inside the world: a scratch proc, a drive mount and a path-aware `df`.
struct Wsl {
    proc: PathBuf,
    mnt: PathBuf,
}

fn exec(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Install the fake WSL: `/proc/version` says microsoft, `/proc/mounts` lists drive C as drvfs
/// at `mnt/c` (the 9p flavour of current WSL), `vhdx` puts an `ext4.vhdx` there, and `df`
/// reports `ext4` free/size for every path but the drive's, `drive` for the drive's.
fn wsl(w: &common::World, vhdx: bool, ext4: (u64, u64), drive: (u64, u64)) -> Wsl {
    let proc = w.root.join("proc");
    let mnt = w.root.join("mnt/c");
    std::fs::create_dir_all(&proc).unwrap();
    std::fs::create_dir_all(&mnt).unwrap();
    std::fs::write(
        proc.join("version"),
        "Linux version 6.6.87.1-microsoft-standard-WSL2 (root@build) (gcc)\n",
    )
    .unwrap();
    std::fs::write(
        proc.join("mounts"),
        format!(
            "/dev/sdd / ext4 rw,relatime,discard 0 0\nC:\\134 {} 9p rw,noatime,aname=drvfs;path=C:\\;uid=1000 0 0\n",
            mnt.display()
        ),
    )
    .unwrap();
    if vhdx {
        let dir = mnt.join("Users/u/AppData/Local/Packages/Canonical.Ubuntu/LocalState");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ext4.vhdx"), vec![0u8; 4096]).unwrap();
    }
    let df = w.bin.join("df");
    exec(
        &df,
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do last=$a; done\ncase \"$*\" in\n *--output=avail*) col=1 ;;\n *--output=size*) col=2 ;;\n *) exec /usr/bin/env -i PATH=/usr/bin:/bin df \"$@\" ;;\nesac\ncase \"$last\" in\n '{mnt}'*) a={da}; s={ds} ;;\n *) a={ea}; s={es} ;;\nesac\necho Header\nif [ $col = 1 ]; then echo $a; else echo $s; fi\n",
            mnt = mnt.display(),
            da = drive.0,
            ds = drive.1,
            ea = ext4.0,
            es = ext4.1
        ),
    );
    // On macOS remote.sh prefers Homebrew's tools over PATH, so the fake is also a function.
    std::fs::write(
        w.bin.join("fake-df.sh"),
        format!("df() {{ '{}' \"$@\"; }}\n", df.display()),
    )
    .unwrap();
    Wsl { proc, mnt }
}

fn script_env(cmd: &mut std::process::Command, w: &common::World, f: &Wsl) {
    cmd.env("GOWAY_WSL_PROC", &f.proc)
        .env("BASH_ENV", w.bin.join("fake-df.sh"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                w.bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
}

/// The remote script's verb with the fake WSL, in a fresh home.
fn remote(w: &common::World, f: &Wsl, verb: &str, args: &[&str]) -> Output {
    let home = w.root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut cmd = std::process::Command::new("bash");
    cmd.args(["-c", include_str!("../src/remote.sh"), "goway", verb])
        .arg(&w.remote)
        .args(args)
        .env("HOME", &home);
    script_env(&mut cmd, w, f);
    cmd.output().unwrap()
}

fn probe(w: &common::World, f: &Wsl, args: &[&str]) -> String {
    let out = remote(w, f, "probe", args);
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn fact(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")).map(str::to_owned))
}

/// Set the `[defaults]` reserve of the world's config (the world switches it off).
fn set_reserve(w: &common::World, setting: &str) {
    let config = w.config.join("config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace(
            "win_reserve = \"off\"",
            &format!("win_reserve = \"{setting}\""),
        ),
    )
    .unwrap();
}

/// `goway <args>` against the world with the fake WSL passed through the fake ssh.
fn goway_run(w: &common::World, f: &Wsl, args: &[&str]) -> Output {
    let mut cmd = w.goway(args);
    let pass = cmd
        .get_envs()
        .find(|(k, _)| *k == "GOWAY_SSH_PASS_ENV")
        .and_then(|(_, v)| v)
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default();
    cmd.env(
        "GOWAY_SSH_PASS_ENV",
        format!("{pass},BASH_ENV,GOWAY_WSL_PROC"),
    )
    .env("GOWAY_WSL_PROC", &f.proc)
    .env("BASH_ENV", w.bin.join("fake-df.sh"))
    .output()
    .unwrap()
}

// frob:ticket 01M44WPJWSD12YH6MEZKZE0GWF
// frob:tests crates/goway/src/footprint.rs::parse_win
#[test]
fn free_space_is_the_smaller_of_the_ext4_and_the_windows_drive_and_both_are_reported() {
    let w = common::world();
    // ext4 says 700 GiB of 1 TiB free; the drive that holds the vhdx has 20 GiB of 500.
    let f = wsl(&w, true, (700 * GIB, 1024 * GIB), (20 * GIB, 500 * GIB));
    let text = probe(&w, &f, &["disk"]);
    assert_eq!(
        fact(&text, "disk_free"),
        Some((20 * GIB).to_string()),
        "{text}"
    );
    assert_eq!(fact(&text, "win_drive").as_deref(), Some("C"), "{text}");
    assert_eq!(
        fact(&text, "win_source").as_deref(),
        Some("found"),
        "{text}"
    );
    assert_eq!(fact(&text, "win_free"), Some((20 * GIB).to_string()));
    assert_eq!(fact(&text, "win_size"), Some((500 * GIB).to_string()));
    assert_eq!(fact(&text, "disk_free_fs"), Some((700 * GIB).to_string()));
    // The probe is parsed the way status reads it.
    let probe = goway::pool::parse_probe(&format!(
        "arch=x86_64\nhostname=h\ncores=4\nload1=0\nload5=0\nload15=0\njobs=0\n{text}"
    ))
    .unwrap();
    let cell = goway::status::free_cell(&probe);
    assert!(
        cell.contains("C: drive 20.0 GiB") && cell.contains("inside WSL 700.0 GiB"),
        "{cell}"
    );
    // When the ext4 is the tighter one, it wins.
    let f = wsl(&w, true, (5 * GIB, 100 * GIB), (300 * GIB, 500 * GIB));
    let text = probe_text(&w, &f);
    assert_eq!(
        fact(&text, "disk_free"),
        Some((5 * GIB).to_string()),
        "{text}"
    );
}

fn probe_text(w: &common::World, f: &Wsl) -> String {
    probe(w, f, &["disk"])
}

// frob:ticket 01M44WPJWSD12YH6MEZKZE0GWF
// frob:tests crates/goway/src/footprint.rs::parse_win
#[test]
fn a_drive_without_a_vhdx_is_assumed_to_be_c_and_the_probe_says_so() {
    let w = common::world();
    let f = wsl(&w, false, (700 * GIB, 1024 * GIB), (40 * GIB, 500 * GIB));
    let text = probe(&w, &f, &["disk"]);
    assert_eq!(fact(&text, "win_drive").as_deref(), Some("C"), "{text}");
    assert_eq!(
        fact(&text, "win_source").as_deref(),
        Some("assumed"),
        "{text}"
    );
    assert_eq!(fact(&text, "disk_free"), Some((40 * GIB).to_string()));
}

// frob:ticket 01M44WPJWSD12YH6MEZKZE0GWF
#[test]
fn off_wsl_the_probe_is_unchanged() {
    let w = common::world();
    let f = wsl(&w, true, (700 * GIB, 1024 * GIB), (20 * GIB, 500 * GIB));
    // Not WSL: the version file does not say microsoft.
    std::fs::write(
        f.proc.join("version"),
        "Linux version 6.8.0-generic (buildd)\n",
    )
    .unwrap();
    let text = probe(&w, &f, &["disk"]);
    assert_eq!(
        fact(&text, "disk_free"),
        Some((700 * GIB).to_string()),
        "{text}"
    );
    assert!(!text.contains("win_drive="), "{text}");
    assert!(f.mnt.exists());
}

// frob:ticket 01M44WPJWSD12YH6MEZKZE0GWF
// frob:tests crates/goway/src/footprint.rs::auto_reserve
#[test]
fn the_probe_reports_the_reserve_it_was_given() {
    let w = common::world();
    let f = wsl(&w, true, (700 * GIB, 1024 * GIB), (50 * GIB, 1000 * GIB));
    let auto = probe(&w, &f, &["disk", "reserve:auto"]);
    // 5% of 1000 GiB beats the 15 GiB floor.
    assert_eq!(
        fact(&auto, "win_reserve"),
        Some((50 * GIB).to_string()),
        "{auto}"
    );
    let small = wsl(&w, true, (700 * GIB, 1024 * GIB), (50 * GIB, 100 * GIB));
    let auto = probe(&w, &small, &["disk", "reserve:auto"]);
    assert_eq!(
        fact(&auto, "win_reserve"),
        Some((15 * GIB).to_string()),
        "{auto}"
    );
    let set = probe(&w, &small, &["disk", "reserve:7516192768"]);
    assert_eq!(fact(&set, "win_reserve"), Some((7 * GIB).to_string()));
    let off = probe(&w, &small, &["disk", "reserve:0"]);
    assert_eq!(fact(&off, "win_reserve").as_deref(), Some("0"));
}

// frob:ticket 01M44WPJWSD12YH6MEZKZE0GWF
#[test]
fn without_max_disk_the_budget_is_a_fraction_of_the_real_drive_not_a_fixed_size() {
    let w = common::world();
    // A 1000 GiB drive under a 2 TiB virtual disk: a fifth of the drive, past the 50 GiB cap
    // that a plain host gets.
    let f = wsl(&w, true, (700 * GIB, 2048 * GIB), (400 * GIB, 1000 * GIB));
    let text = probe(&w, &f, &["disk", "budget:0:10737418240"]);
    assert_eq!(
        fact(&text, "disk_max"),
        Some((200 * GIB).to_string()),
        "{text}"
    );
    // A smaller drive gives a smaller budget; an explicit max_disk still wins.
    let f = wsl(&w, true, (700 * GIB, 2048 * GIB), (80 * GIB, 250 * GIB));
    let text = probe(&w, &f, &["disk", "budget:0:10737418240"]);
    assert_eq!(
        fact(&text, "disk_max"),
        Some((50 * GIB).to_string()),
        "{text}"
    );
    let text = probe(&w, &f, &["disk", "budget:21474836480:10737418240"]);
    assert_eq!(
        fact(&text, "disk_max"),
        Some((20 * GIB).to_string()),
        "{text}"
    );
}

/// A marked root with one idle cache slot holding 1 MiB, old enough to be first in line.
fn idle_cache(w: &common::World) -> PathBuf {
    let root = w.remote.clone();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".goway-root"), "").unwrap();
    let cache = root.join("cache/idle");
    std::fs::create_dir_all(cache.join("tree-0")).unwrap();
    std::fs::write(
        cache.join("meta.json"),
        r#"{"kind":"cache","repo":"idle","repo_id":"id-idle"}"#,
    )
    .unwrap();
    std::fs::File::create(cache.join("tree-0/big"))
        .unwrap()
        .set_len(MIB)
        .unwrap();
    let lock = cache.join("target-0.lock");
    std::fs::write(&lock, "").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_mins(150);
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(old)
        .unwrap();
    cache
}

// frob:ticket 01M44WPJWSD12YH6MEZKZE0GWF
#[test]
fn a_run_evicts_idle_caches_first_then_is_refused_naming_the_drive_and_its_free_space() {
    let w = common::world();
    // 10 GiB free on a 100 GiB drive: under the 15 GiB automatic reserve, and eviction inside
    // WSL cannot raise the fake drive's figure.
    let f = wsl(&w, true, (700 * GIB, 1024 * GIB), (10 * GIB, 100 * GIB));
    set_reserve(&w, "auto");
    let cache = idle_cache(&w);
    let out = goway_run(&w, &f, &["run", "--", "true"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(125), "{out:?}");
    assert!(
        err.contains("refusing to run")
            && err.contains("Windows drive C:")
            && err.contains("10.0 GiB free"),
        "{err}"
    );
    assert!(err.contains("reserve of 15.0 GiB"), "{err}");
    assert!(
        !cache.join("tree-0").exists(),
        "idle caches went first: {err}"
    );
}

// frob:ticket 01M44WPJWSD12YH6MEZKZE0GWF
#[test]
fn a_drive_above_its_reserve_runs_and_the_host_can_override_or_switch_the_reserve_off() {
    let w = common::world();
    set_reserve(&w, "auto");
    let f = wsl(&w, true, (700 * GIB, 1024 * GIB), (30 * GIB, 100 * GIB));
    let out = goway_run(&w, &f, &["run", "--", "true"]);
    assert!(out.status.success(), "{out:?}");
    // Below the automatic 15 GiB, but the host asks for no reserve.
    let f = wsl(&w, true, (700 * GIB, 1024 * GIB), (10 * GIB, 100 * GIB));
    let config = w.config.join("config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    let host = |setting: &str| {
        text.replace(
            "max_jobs = 64",
            &format!("max_jobs = 64\nwin_reserve = \"{setting}\""),
        )
    };
    std::fs::write(&config, host("off")).unwrap();
    let out = goway_run(&w, &f, &["run", "--", "true"]);
    assert!(out.status.success(), "{out:?}");
    // An explicit reserve above what is free refuses again.
    std::fs::write(&config, host("12G")).unwrap();
    let out = goway_run(&w, &f, &["run", "--", "true"]);
    assert_eq!(out.status.code(), Some(125), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("reserve of 12.0 GiB"));
}

// frob:ticket 01M44WPJWSD12YH6MEZKZE0GWF
#[test]
fn eviction_hands_freed_blocks_back_with_fstrim_when_it_may_and_never_otherwise() {
    let w = common::world();
    let f = wsl(&w, true, (700 * GIB, 1024 * GIB), (10 * GIB, 100 * GIB));
    let log = w.root.join("fstrim.log");
    exec(
        &w.bin.join("fstrim"),
        &format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display()),
    );
    exec(&w.bin.join("id"), "#!/bin/sh\necho 0\n");
    let gc = |f: &Wsl| {
        remote(
            &w,
            f,
            "gc",
            &[
                &common_now(),
                "315360000",
                "315360000",
                "315360000",
                "apply",
                "",
                "",
                "0",
                "0",
                "",
                "reserve:auto",
            ],
        )
    };
    idle_cache(&w);
    let out = gc(&f);
    assert!(out.status.success(), "{out:?}");
    assert!(log.exists(), "fstrim ran after the eviction as root");
    // As an ordinary user fstrim is never attempted.
    std::fs::remove_file(&log).unwrap();
    exec(&w.bin.join("id"), "#!/bin/sh\necho 1000\n");
    idle_cache(&w);
    assert!(gc(&f).status.success());
    assert!(!log.exists(), "no fstrim without root");
}

fn common_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}
