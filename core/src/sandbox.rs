//! Runs model-written commands in two layers (ADR 0004).
//!
//! Layer 1, bubblewrap, builds a fresh filesystem view: read-only system
//! folders, a throwaway home and `/tmp`, the worktree at `/work` and the
//! frozen checks read-only at `/checks`. Nothing else from the host is
//! visible. Layer 2, gVisor (`runsc do`), runs the command on its own
//! kernel with no network. If either layer is missing, nothing runs.

use crate::{Error, Result};
use std::path::PathBuf;
use std::time::Duration;

/// A command to run inside the sandbox, from `/work`.
#[derive(Debug, Clone)]
pub struct RunSpec {
    /// Run with `/bin/sh -c`.
    pub cmd: String,
    /// Mounted read-write at `/work`.
    pub workdir: PathBuf,
    /// Mounted read-only at `/checks`.
    pub checks_dir: Option<PathBuf>,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct RunOutput {
    /// 124 on timeout; 128 + signal when killed by a signal.
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    /// True when either stream passed `OUTPUT_CAP` and was cut.
    pub truncated: bool,
}

/// Bytes kept from each output stream.
pub const OUTPUT_CAP: usize = 1024 * 1024;

/// Proves both layers work: the command runs, on gVisor's kernel, cannot
/// read a planted secret outside its worktree, and has no network.
pub fn self_test() -> Result<()> {
    imp::self_test()
}

pub fn run(spec: &RunSpec) -> Result<RunOutput> {
    imp::run(spec)
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;

    const WHY: &str = "the sandbox needs Linux (bubblewrap and gVisor); run Cortex inside WSL2";

    pub fn self_test() -> Result<()> {
        Err(Error::Blocked(WHY.into()))
    }

    pub fn run(_spec: &RunSpec) -> Result<RunOutput> {
        Err(Error::Blocked(WHY.into()))
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Instant;

    const PASSWD: &str = "root:x:0:0:root:/home:/bin/sh\n";

    fn find(bin: &str) -> Result<PathBuf> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .map(|d| d.join(bin))
            .find(|p| p.is_file())
            .map(|p| p.canonicalize().unwrap_or(p))
            .ok_or_else(|| Error::Blocked(format!("`{bin}` is not installed")))
    }

    pub fn self_test() -> Result<()> {
        let work = scratch_dir("selftest-work")?;
        let outside = scratch_dir("selftest-secret")?;
        let secret = format!("cortex-secret-{}", std::process::id());
        let key = outside.join("key");
        std::fs::write(&key, &secret)?;
        let cmd = format!(
            "echo cortex-canary; cat /proc/version; cat {key} 2>&1; \
             python3 -c \"import socket; socket.create_connection(('1.1.1.1', 53), 3); print('NET OPEN')\" 2>&1",
            key = key.display()
        );
        let result = run(&RunSpec {
            cmd,
            workdir: work.clone(),
            checks_dir: None,
            timeout: Duration::from_secs(60),
        });
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&outside);
        let out = result?;
        let fail = |why: &str| Err(Error::Blocked(format!("sandbox self test: {why}")));
        if !out.stdout.contains("cortex-canary") {
            return fail(&format!("command did not run: {}", out.stderr.trim()));
        }
        if !out.stdout.contains("gvisor") {
            return fail("command did not run on gVisor's kernel");
        }
        if out.stdout.contains(&secret) {
            return fail("a file outside the worktree was readable");
        }
        if out.stdout.contains("NET OPEN") {
            return fail("the network was reachable");
        }
        Ok(())
    }

    fn scratch_dir(tag: &str) -> Result<PathBuf> {
        let dir =
            std::env::temp_dir().join(format!("cortex-{tag}-{}-{}", std::process::id(), nanos()));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    pub fn run(spec: &RunSpec) -> Result<RunOutput> {
        let bwrap = find("bwrap")?;
        let runsc = find("runsc")?;
        if !runsc.starts_with("/usr") {
            return Err(Error::Blocked(format!(
                "runsc must live under /usr to be visible in the sandbox; found {}",
                runsc.display()
            )));
        }
        let workdir = spec.workdir.canonicalize()?;
        let etc = scratch_dir("etc")?;
        std::fs::write(etc.join("passwd"), PASSWD)?;

        let mut cmd = Command::new(bwrap);
        cmd.args(["--unshare-all", "--die-with-parent", "--new-session"])
            .args(["--clearenv", "--setenv", "PATH", "/usr/bin:/bin"])
            .args(["--setenv", "HOME", "/home", "--setenv", "LANG", "C.UTF-8"])
            .args(["--ro-bind", "/usr", "/usr"]);
        for dir in ["/bin", "/sbin", "/lib", "/lib64", "/lib32"] {
            cmd.args(["--ro-bind-try", dir, dir]);
        }
        for file in ["/etc/alternatives", "/etc/ld.so.cache", "/etc/ld.so.conf"] {
            cmd.args(["--ro-bind-try", file, file]);
        }
        cmd.arg("--ro-bind")
            .arg(etc.join("passwd"))
            .arg("/etc/passwd")
            .args(["--proc", "/proc", "--dev", "/dev"])
            .args(["--tmpfs", "/tmp", "--tmpfs", "/home"])
            .arg("--bind")
            .arg(&workdir)
            .arg("/work");
        if let Some(checks) = &spec.checks_dir {
            cmd.arg("--ro-bind")
                .arg(checks.canonicalize()?)
                .arg("/checks");
        }
        cmd.args(["--chdir", "/work"])
            .arg(&runsc)
            .args(["--rootless", "--network=none", "--ignore-cgroups"])
            .args(["--root=/tmp/runsc", "do", "-quiet", "-cwd", "/work"])
            // Without this, `runsc do` keeps writes in memory and drops
            // them; bubblewrap already limits what is writable.
            .arg("-force-overlay=false")
            .args(["/bin/sh", "-c", &spec.cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);

        let result = supervise(cmd, spec.timeout);
        let _ = std::fs::remove_dir_all(&etc);
        result
    }

    struct Capture {
        buf: Arc<Mutex<Vec<u8>>>,
        truncated: Arc<Mutex<bool>>,
        done: thread::JoinHandle<()>,
    }

    fn capture(mut stream: impl Read + Send + 'static) -> Capture {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let truncated = Arc::new(Mutex::new(false));
        let (b, t) = (buf.clone(), truncated.clone());
        let done = thread::spawn(move || {
            let mut chunk = [0u8; 64 * 1024];
            // Keep reading past the cap so the writer never blocks.
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut buf = b.lock().unwrap();
                let room = OUTPUT_CAP.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..n.min(room)]);
                if n > room {
                    *t.lock().unwrap() = true;
                }
            }
        });
        Capture {
            buf,
            truncated,
            done,
        }
    }

    impl Capture {
        /// Waits a short while for the stream to close, then takes what
        /// was captured either way: a leftover process holding the pipe
        /// must not hang the caller.
        fn finish(self) -> (String, bool) {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !self.done.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            let buf = self.buf.lock().unwrap();
            let text = String::from_utf8_lossy(&buf).into_owned();
            (text, *self.truncated.lock().unwrap())
        }
    }

    fn supervise(mut cmd: Command, timeout: Duration) -> Result<RunOutput> {
        let mut child = cmd.spawn()?;
        let out = capture(child.stdout.take().expect("piped stdout"));
        let err = capture(child.stderr.take().expect("piped stderr"));
        let pgid = child.id() as i32;
        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                // SAFETY: plain kill(2) on the process group we created.
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
                break child.wait()?;
            }
            thread::sleep(Duration::from_millis(20));
        };
        let (stdout, t1) = out.finish();
        let (stderr, t2) = err.finish();
        let exit_code = if timed_out {
            124
        } else {
            use std::os::unix::process::ExitStatusExt;
            status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
        };
        Ok(RunOutput {
            exit_code,
            stdout,
            stderr,
            timed_out,
            truncated: t1 || t2,
        })
    }
}
