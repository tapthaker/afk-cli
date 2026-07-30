#![cfg(any(target_os = "linux", target_os = "macos"))]

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
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
        let path = Path::new("/tmp").join(format!(
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

    fn spawn_session(&self, script: &str) -> Result<Child, Box<dyn Error>> {
        Ok(Command::new(env!("CARGO_BIN_EXE_afk"))
            .args(["session", SESSION, "--", "/bin/sh", "-c", script])
            .env("HOME", &self.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?)
    }

    fn spawn_session_attachment(&self, creation_script: &str) -> Result<Child, Box<dyn Error>> {
        Ok(Command::new(env!("CARGO_BIN_EXE_afk"))
            .args(["session", SESSION, "--", "/bin/sh", "-c", creation_script])
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
fn trace_option_writes_bounded_lifecycle_events_without_terminal_data() -> Result<(), Box<dyn Error>>
{
    let home = TestHome::new()?;
    let secret = "synthetic-terminal-secret";
    let created = home.run(&[
        "session",
        SESSION,
        "--trace",
        "--",
        "/bin/sh",
        "-c",
        &format!("printf '{secret}'; exit 0"),
    ])?;
    assert!(created.status.success());
    home.wait_completed()?;

    let trace_path = home
        .path()
        .join(".afk/run")
        .join(format!("{SESSION}.trace"));
    let trace = fs::read(&trace_path)?;
    assert!(trace.len() <= 1024 * 1024);
    assert_eq!(
        fs::metadata(trace_path)?.permissions().mode() & 0o777,
        0o600
    );
    assert!(
        trace
            .windows(b"event=runner_started".len())
            .any(|part| part == b"event=runner_started")
    );
    assert!(
        trace
            .windows(b"event=child_exited".len())
            .any(|part| part == b"event=child_exited")
    );
    assert!(
        !trace
            .windows(secret.len())
            .any(|part| part == secret.as_bytes())
    );
    Ok(())
}

#[test]
fn completed_session_prints_retained_output_and_returns_child_status() -> Result<(), Box<dyn Error>>
{
    let home = TestHome::new()?;
    let created = home.run(&[
        "session",
        SESSION,
        "--",
        "/bin/sh",
        "-c",
        "printf 'synthetic-first\\n'; printf 'synthetic-last\\n'; exit 17",
    ])?;
    assert!(matches!(created.status.code(), Some(0) | Some(17)));
    assert!(created.stderr.is_empty());
    home.wait_completed()?;

    let attached = home.run(&["session", SESSION])?;
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
fn partial_and_delayed_output_flush_without_any_input() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let mut attached =
        home.spawn_session_attachment("printf 'a'; sleep 1; printf 'synthetic-delayed'; sleep 30")?;
    let mut stdout = attached.stdout.take().ok_or("missing attach stdout")?;

    let first_ready = {
        let mut descriptors = [PollFd::new(&stdout, PollFlags::IN)];
        poll(
            &mut descriptors,
            Some(&Timespec {
                tv_sec: 5,
                tv_nsec: 0,
            }),
        )?
    };
    assert_eq!(
        first_ready, 1,
        "first partial output timed out before user input"
    );
    let mut first = [0_u8; 1];
    stdout.read_exact(&mut first)?;
    assert_eq!(&first, b"a");

    let delayed_ready = {
        let mut descriptors = [PollFd::new(&stdout, PollFlags::IN)];
        poll(
            &mut descriptors,
            Some(&Timespec {
                tv_sec: 5,
                tv_nsec: 0,
            }),
        )?
    };
    assert_eq!(
        delayed_ready, 1,
        "delayed partial output timed out without user input"
    );
    let mut delayed = vec![0_u8; b"synthetic-delayed".len()];
    stdout.read_exact(&mut delayed)?;
    assert_eq!(delayed, b"synthetic-delayed");

    assert_eq!(home.run(&["stop", SESSION])?.status.code(), Some(0));
    let _ = attached.wait();
    Ok(())
}

#[test]
fn live_session_replays_history_before_new_output() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let created = home.run(&[
        "session",
        SESSION,
        "--",
        "/bin/sh",
        "-c",
        "printf 'synthetic-history\\n'; stty -echo; read line; printf 'synthetic-live\\n'; exit 19",
    ])?;
    assert_eq!(created.status.code(), Some(0));
    thread::sleep(Duration::from_millis(200));

    let mut attached = home.spawn_session_attachment("touch \"$HOME/unexpected-live-command\"")?;
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
    assert!(!home.path().join("unexpected-live-command").exists());
    Ok(())
}

#[test]
fn completed_output_is_truncated_to_the_final_256_kib() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let created = home.run(&[
        "session",
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

    let attached = home.run(&["session", SESSION])?;
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
fn concurrent_session_calls_create_only_one_runner() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let first = home.spawn_session("sleep 30")?;
    let second = home.spawn_session("sleep 30")?;
    let first = first.wait_with_output()?;
    let second = second.wait_with_output()?;
    assert_eq!(first.status.code(), Some(0), "first: {:?}", first.stderr);
    assert_eq!(second.status.code(), Some(0), "second: {:?}", second.stderr);

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

    let attempted = home.run(&["session", SESSION, "--", "/bin/sh", "-c", "exit 0"])?;
    assert_eq!(attempted.status.code(), Some(1));
    assert_eq!(fs::read(&target)?, b"unchanged");
    assert!(!runtime.join(format!("{SESSION}.sock")).exists());
    Ok(())
}

#[test]
fn unreachable_live_record_does_not_start_a_replacement() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let runtime = home.path().join(".afk/run");
    fs::create_dir_all(&runtime)?;
    let metadata = runtime.join(format!("{SESSION}.json"));
    fs::write(
        &metadata,
        format!(
            "{{\"state\":\"live\",\"session_id\":\"{SESSION}\",\"runner_pid\":1,\"child_pid\":2,\"started_at\":1,\"attached\":false}}"
        ),
    )?;
    fs::set_permissions(&metadata, fs::Permissions::from_mode(0o600))?;

    let marker = home.path().join("unexpected-replacement");
    let attempted = home.run(&[
        "session",
        SESSION,
        "--",
        "/bin/sh",
        "-c",
        "touch \"$HOME/unexpected-replacement\"",
    ])?;

    assert_eq!(attempted.status.code(), Some(1));
    assert_eq!(attempted.stderr, b"error: session unavailable\n");
    assert!(!marker.exists());
    assert!(!runtime.join(format!("{SESSION}.sock")).exists());
    Ok(())
}

#[test]
fn completed_id_returns_previous_exit_without_starting_again() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let created = home.run(&["session", SESSION, "--", "/bin/sh", "-c", "exit 23"])?;
    assert!(matches!(created.status.code(), Some(0) | Some(23)));
    home.wait_completed()?;

    let completion_path = home.path().join(".afk/run").join(format!("{SESSION}.json"));
    let completion = fs::read(&completion_path)?;
    assert!(
        completion
            .windows(b"\"value\":23".len())
            .any(|part| part == b"\"value\":23")
    );
    assert_eq!(fs::metadata(completion_path)?.mode() & 0o777, 0o600);

    let marker = home.path().join("unexpected-restart");
    let resumed = home.run(&[
        "session",
        SESSION,
        "--",
        "/bin/sh",
        "-c",
        "touch \"$HOME/unexpected-restart\"",
    ])?;
    assert_eq!(resumed.status.code(), Some(23));
    assert!(resumed.stderr.is_empty());
    assert!(!marker.exists());
    Ok(())
}
