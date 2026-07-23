# Linux Runtime Spike Results

**Date:** 2026-07-23

**Status:** Completed for the initial implementation decisions

These disposable spikes were executed from an Apple Silicon macOS host inside Docker Desktop Linux containers. The test binary was cross-compiled as static musl for AArch64 and x86-64. Only synthetic terminal content was used.

## Environment

```text
Host                    macOS 15.7.7, arm64
Docker Desktop          4.58.1
LinuxKit kernel         6.12.65
Container               Alpine Linux 3.22.5
Rust                    1.85.0
rustix                  1.1.4
signal-hook              0.3.18
Native container arch   aarch64
Emulated container arch x86_64
```

The spike source was intentionally disposable and is not part of the AFK executable. The findings below are converted into implementation requirements and acceptance tests.

## 1. PTY setup and launcher survival

### Experiment

The launcher spawned a fresh hidden runner process and waited for readiness. The runner:

1. called safe `rustix::process::setsid`;
2. opened `/dev/ptmx` with `rustix::pty::openpt`;
3. called `grantpt` and `unlockpt`;
4. opened the slave with Linux `TIOCGPTPEER`;
5. passed duplicate slave descriptors as stdin, stdout, and stderr to a fresh hidden child-helper process;
6. closed its slave descriptors;
7. retained the master descriptor.

The child helper, running as a fresh executable rather than inside `pre_exec`:

1. called safe `setsid`;
2. called safe `ioctl_tiocsctty` on stdin;
3. executed Bash interactively with `CommandExt::exec`.

The launcher then exited.

### Result

On both architectures:

- the runner was reparented and remained alive;
- the runner had its own session and process group;
- the child was a separate session leader;
- the child owned `pts/0` as its controlling terminal;
- the child's foreground process group matched its process group;
- interactive Bash job control worked;
- the runner had exactly one thread;
- the shell PID, cwd, and inherited synthetic environment value remained intact after launcher exit.

Representative process state:

```text
runner: PID 14, PGID 14, SID 14, no controlling TTY
child:  PID 15, PGID 15, SID 15, TPGID 15, TTY pts/0
```

### Decision

Use fresh hidden executable modes for both runner startup and child terminal setup. Do not use `fork`, `forkpty`, or `CommandExt::pre_exec` in project code. This keeps project-owned PTY setup in safe Rust.

## 2. Safe API coverage

The spike confirmed safe APIs for:

- PTY master and slave creation;
- new Unix sessions;
- controlling-terminal assignment;
- terminal dimensions;
- descriptor duplication and ownership;
- nonblocking mode;
- polling;
- process and process-group signals.

`rustix` does not expose a safe high-level `signalfd` API. A `signal-hook` self-pipe was registered for `SIGWINCH`, raised, and observed through `rustix::event::poll`.

```text
poll_ready=1
wake_byte=88
tasks_before=1
tasks_after=1
```

### Decision

Use `rustix` for Linux PTY/process/poll operations and `signal-hook` for signal registration. The production attachment uses atomic signal flags with poll interruption and a bounded timeout; this needs no signal thread and no project-owned unsafe code.

## 3. Resize behavior

The runner changed the inner PTY from 24x80 to 50x120 with `tcsetwinsize`. The interactive shell received `SIGWINCH` from the kernel and read the updated dimensions:

```text
AFK_WINCH size=50 120
```

### Decision

The attachment reads the outer PTY dimensions after its signal wakeup and sends only rows and columns to the runner. The runner calls `TIOCSWINSZ` on the inner PTY. AFK does not forward `SIGWINCH` directly.

## 4. Inherited cwd and environment

The launcher was started in `/tmp/inherited-cwd` with a synthetic environment value. The inner shell reported:

```text
cwd=/tmp/inherited-cwd
var=inherited-ok
```

### Decision

The runner and child inherit cwd and environment from `afk stream`. AFK does not persist or log either. Explicit argv is executed without `sh -c`.

## 5. Threads and event loop

The runner remained at one task in `/proc/<pid>/task`. Pollable signal wakeup also remained at one task.

### Decision

Each AFK process has one thread:

```text
detached session: one runner process / one AFK thread
attached session: one runner process plus one attachment process / two AFK threads total
```

The runner and attachment each use one nonblocking `poll` loop. No async runtime, worker pool, or signal thread is needed.

## 6. Stop behavior

An interactive shell created:

- a normal background `sleep` in a separate process group;
- a background process that ignored `SIGHUP` and `SIGTERM`.

Sending `SIGTERM` only to the shell's process group did not terminate the interactive shell or either background group. Closing the PTY master ended the shell and normal background process, but the signal-ignoring background process survived.

### Finding

A process-group-only stop promise is incorrect for an interactive shell with job control. Closing the PTY is sufficient for ordinary jobs but cannot guarantee termination of a process that deliberately ignores hangup or leaves the terminal session.

### Decision

Use a documented best-effort stop: close the PTY, signal the verified child session leader, and escalate that process after the grace period. Escaped or signal-ignoring descendants may survive.

Do not add `/proc` scanning and PID-race controls. Even a session scan cannot stop a process that calls `setsid`, and AFK does not claim process isolation.

## 7. Unix socket path length

Linux filesystem Unix sockets accepted a 107-byte path and rejected a 108-byte path:

```text
path bytes 107: success
path bytes 108: InvalidInput, path must be shorter than SUN_LEN
```

### Decision

Calculate the complete socket path before creating a session. Use `~/.afk/run` and return a bounded error if the encoded path exceeds 107 bytes. Do not add abstract sockets or fallback directories initially.

## 8. OpenSSH and host process policy

An Alpine OpenSSH fixture started the launcher through an allocated SSH PTY. After runner readiness, the local SSH client was killed without a graceful channel close. Two seconds later, the runner and interactive child were alive with the expected independent sessions and controlling terminal.

```text
OPENSSH_ABRUPT_CLIENT_DEATH_PASSED
runner: SID 32, no controlling TTY
child:  SID 33, TTY pts/1, TPGID 33
```

The runner also survived ordinary launcher exit in Alpine containers on both architectures. Docker containers do not reproduce every `systemd-logind`, hosting-provider, or SSH cgroup policy.

### Decision

AFK supports hosts that permit user processes to remain after an SSH process exits. A host policy that kills the entire login cgroup is an explicit unsupported environment unless AFK later gains an optional user-service mode.

This is a host capability, not something additional threads, signals, or daemonization can bypass.

## 9. Artifact impact

The unstripped synthetic spike included `rustix` and `signal-hook` and remained small:

```text
AArch64 musl   732,256 bytes; 310,392 bytes gzip -9
x86-64 musl    717,184 bytes; 308,174 bytes gzip -9
```

The dependency tree contained `rustix`, `signal-hook`, and their small permissively licensed support crates. Production artifact size will continue to be measured independently.

## 10. Product decisions confirmed separately

- `stream` is create-only and returns `SessionExists` for a live or retained completed ID.
- default-shell and explicit-command sessions inherit cwd and environment;
- child exit code or signal is sent to an active attachment and persisted as bounded completion metadata;
- attaching to retained completed metadata reports the outcome without creating a process;
- a later product decision added a 256 KiB in-memory raw PTY output tail, replayed on every live attachment and persisted only after observed process completion;
- attaching to a completed session prints that raw tail, a truncation marker when needed, and the completion summary;
- no separate terminal-input stream is persisted.

Completed output and metadata retention is initially 24 hours and is cleaned lazily by later AFK commands.

## 11. macOS implementation validation

The shared Unix implementation was subsequently validated natively on Apple Silicon macOS:

- safe PTY allocation through `rustix-openpty`;
- independent runner and child Unix sessions;
- controlling-terminal setup and interactive `/bin/sh` operation;
- launcher detach followed by reattach to the same shell PID, cwd, and synthetic environment value;
- raw-tail replay before new output;
- completion output retention and exact exit status;
- concurrent-create exclusion and symlink rejection;
- Intel and Apple Silicon Mach-O cross-builds.

macOS `poll` does not reliably report terminal stdin readiness. The shared attachment loop therefore sets stdin nonblocking and probes it once per bounded iteration on both supported hosts while continuing to poll the Unix socket. This keeps one implementation, adds at most the 100 ms bounded poll interval to idle input delivery, and requires no worker thread or unsafe project code.
