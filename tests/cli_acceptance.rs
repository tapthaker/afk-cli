use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestHome {
    path: PathBuf,
}

impl TestHome {
    fn new() -> Result<Self, Box<dyn Error>> {
        let directory_id = NEXT_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "afk-cli-acceptance-{}-{directory_id}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _result = fs::remove_dir_all(&self.path);
    }
}

fn run_afk(home: &TestHome, arguments: &[&str]) -> Result<Output, Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_afk"))
        .args(arguments)
        .env("HOME", home.path())
        .output()?;
    Ok(output)
}

#[test]
fn cli_001_version_is_side_effect_free() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let output = run_afk(&home, &["--version"])?;

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"afk 0.1.0\n");
    assert!(output.stderr.is_empty());
    assert!(!home.path().join(".afk").exists());
    assert_eq!(fs::read_dir(home.path())?.count(), 0);
    Ok(())
}

#[test]
fn cli_002_invalid_arguments_are_bounded_and_redacted() -> Result<(), Box<dyn Error>> {
    const SENTINEL: &str = "synthetic-sensitive-argument";

    let home = TestHome::new()?;
    let output = run_afk(&home, &[SENTINEL])?;

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.len() <= 128);
    assert!(
        !output
            .stderr
            .windows(SENTINEL.len())
            .any(|part| part == SENTINEL.as_bytes())
    );
    assert_eq!(
        output.stderr,
        b"error: unsupported command or option\nTry 'afk --help' for usage.\n"
    );
    assert!(!home.path().join(".afk").exists());
    Ok(())
}

#[test]
fn help_describes_only_available_behavior() -> Result<(), Box<dyn Error>> {
    let home = TestHome::new()?;
    let output = run_afk(&home, &["--help"])?;

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert!(output.stdout.starts_with(b"AFK CLI"));
    assert!(
        output
            .stdout
            .windows(b"afk --version".len())
            .any(|part| part == b"afk --version")
    );
    assert!(
        output
            .stdout
            .windows(b"not implemented".len())
            .any(|part| part == b"not implemented")
    );
    Ok(())
}
