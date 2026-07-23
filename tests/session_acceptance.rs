#![cfg(target_os = "linux")]

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_HOME: AtomicU64 = AtomicU64::new(0);
const SESSION: &str = "00112233445566778899aabbccddeeff";

struct TestHome {
    path: PathBuf,
}

impl TestHome {
    fn new() -> Result<Self, Box<dyn Error>> {
        let path = std::env::temp_dir().join(format!(
            "afk-session-acceptance-{}-{}",
            std::process::id(),
            NEXT_HOME.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn run(&self, arguments: &[&str]) -> Result<Output, Box<dyn Error>> {
        Ok(Command::new(env!("CARGO_BIN_EXE_afk"))
            .args(arguments)
            .env("HOME", &self.path)
            .stdin(Stdio::null())
            .output()?)
    }

    fn spawn_stream(&self, script: &str) -> Result<Child, Box<dyn Error>> {
        Ok(Command::new(env!("CARGO_BIN_EXE_afk"))
            .args(["stream", SESSION, "--", "/bin/sh", "-c", script])
            .env("HOME", &self.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?)
    }

    fn spawn_attach(&self) -> Result<Child, Box<dyn Error>> {
        Ok(Command::new(env!("CARGO_BIN_EXE_afk"))
            .args(["attach", SESSION])
            .env("HOME", &self.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?)
    }

    fn wait_completed(&self) -> Result<(), Box<dyn Error>> {
        let metadata = self.path.join(".afk/run").join(format!("{SESSION}.json"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if fs::read(&metadata).is_ok_and(|bytes| {
                bytes
                    .windows(b"completed".len())
                    .any(|part| part == b"completed")
            }) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        Err("session did not complete".into())
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_afk"))
            .args(["stop", SESSION])
            .env("HOME", &self.path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn completed_attach_prints_retained_output_and_returns_child_status() -> Result<(), Box<dyn Error>>
{
    let home = TestHome::new()?;
    let created = home.run(&[
        "stream",
        SESSION,
        "--",
        "/bin/sh",
        "-c",
        "printf 'synthetic-first\\n'; printf 'synthetic-last\\n'; exit 17",
    ])?;
    assert_eq!(created.status.code(), Some(0));
    assert!(created.stderr.is_empty());
    home.wait_completed()?;

    let attached = home.run(&["attach", SESSION])?;
    assert_eq!(attached.status.code(), Some(17));
    assert!(
        attached
            .stdout
            .windows(b"synthetic-first".len())
            .any(|part| part == b"synthetic-first")
    );
    assert!(
        attached
            .stdout
            .windows(b"synthetic-last".len())
            .any(|part| part == b"synthetic-last")
    );
    assert!(
        attached
            .stdout
            .windows(b"process exited with code 17".len())
            .any(|part| part == b"process exited with code 17")
    );
    assert!(attached.stderr.is_empty());

    let output_path = home.path().join(".afk/run").join(format!("{SESSION}.out"));
    let metadata = fs::metadata(output_path)?;
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert!(metadata.len() <= 256 * 1024);
    Ok(())
}

#[test]
fn live_attach_replays_history_before_new_output() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let created = home.run(&[
        "stream",
        SESSION,
        "--",
        "/bin/sh",
        "-c",
        "printf 'synthetic-history\\n'; stty -echo; read line; printf 'synthetic-live\\n'; exit 19",
    ])?;
    assert_eq!(created.status.code(), Some(0));
    thread::sleep(Duration::from_millis(200));

    let mut attached = home.spawn_attach()?;
    let mut stdout = attached.stdout.take().ok_or("missing attach stdout")?;
    let ready = {
        let mut descriptors = [PollFd::new(&stdout, PollFlags::IN)];
        poll(
            &mut descriptors,
            Some(&Timespec {
                tv_sec: 5,
                tv_nsec: 0,
            }),
        )?
    };
    assert_eq!(ready, 1);
    let mut replay = vec![0_u8; b"synthetic-history\r\n".len()];
    stdout.read_exact(&mut replay)?;
    assert!(
        replay == b"synthetic-history\r\n",
        "replayed output mismatch"
    );

    let mut attach_input = attached.stdin.take().ok_or("missing attach stdin")?;
    attach_input.write_all(b"continue\n")?;
    let live_ready = {
        let mut descriptors = [PollFd::new(&stdout, PollFlags::IN)];
        poll(
            &mut descriptors,
            Some(&Timespec {
                tv_sec: 5,
                tv_nsec: 0,
            }),
        )?
    };
    assert_eq!(live_ready, 1);
    let mut live = vec![0_u8; b"synthetic-live\r\n".len()];
    stdout.read_exact(&mut live)?;
    assert!(live == b"synthetic-live\r\n", "live output mismatch");

    assert_eq!(attached.wait()?.code(), Some(19));
    drop(attach_input);
    home.wait_completed()?;
    Ok(())
}

#[test]
fn completed_output_is_truncated_to_the_final_256_kib() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let created = home.run(&[
        "stream",
        SESSION,
        "--",
        "/bin/sh",
        "-c",
        "dd if=/dev/zero bs=1024 count=300 2>/dev/null | tr '\\000' x; printf END",
    ])?;
    assert_eq!(created.status.code(), Some(0));
    home.wait_completed()?;

    let output_path = home.path().join(".afk/run").join(format!("{SESSION}.out"));
    let retained = fs::read(output_path)?;
    assert_eq!(retained.len(), 256 * 1024);
    assert!(retained.ends_with(b"END"));

    let attached = home.run(&["attach", SESSION])?;
    assert_eq!(attached.status.code(), Some(0));
    assert!(
        attached
            .stdout
            .starts_with(b"\r\n[afk: earlier terminal output was truncated]\r\n")
    );
    assert!(
        attached
            .stdout
            .windows(b"END".len())
            .any(|part| part == b"END")
    );
    Ok(())
}

#[test]
fn concurrent_stream_creates_only_one_runner() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let mut first = home.spawn_stream("sleep 30")?;
    let mut second = home.spawn_stream("sleep 30")?;
    let mut statuses = [first.wait()?.code(), second.wait()?.code()];
    statuses.sort_unstable();
    assert_eq!(statuses, [Some(0), Some(1)]);

    let listing = home.run(&["sessions", "--json"])?;
    let id_count = listing
        .stdout
        .windows(SESSION.len())
        .filter(|part| *part == SESSION.as_bytes())
        .count();
    assert_eq!(id_count, 1);
    assert_eq!(home.run(&["stop", SESSION])?.status.code(), Some(0));
    Ok(())
}

#[test]
fn symlinked_lock_is_rejected_without_modifying_its_target() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let runtime = home.path().join(".afk/run");
    fs::create_dir_all(&runtime)?;
    let target = home.path().join("synthetic-target");
    fs::write(&target, b"unchanged")?;
    symlink(&target, runtime.join(format!("{SESSION}.lock")))?;

    let attempted = home.run(&["stream", SESSION, "--", "/bin/sh", "-c", "exit 0"])?;
    assert_eq!(attempted.status.code(), Some(1));
    assert_eq!(fs::read(&target)?, b"unchanged");
    assert!(!runtime.join(format!("{SESSION}.sock")).exists());
    Ok(())
}

#[test]
fn stream_rejects_a_retained_completed_id() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let created = home.run(&["stream", SESSION, "--", "/bin/sh", "-c", "exit 0"])?;
    assert_eq!(created.status.code(), Some(0));
    home.wait_completed()?;

    let duplicate = home.run(&["stream", SESSION, "--", "/bin/sh", "-c", "exit 0"])?;
    assert_eq!(duplicate.status.code(), Some(1));
    assert_eq!(duplicate.stderr, b"error: session already exists\n");
    Ok(())
}
