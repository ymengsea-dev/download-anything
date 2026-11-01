//! Spawning external tools (yt-dlp, ffmpeg) with line streaming and cancellation.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};

pub type Result<T> = std::result::Result<T, String>;

/// Receives `true` once the job has been cancelled.
pub type Cancel = watch::Receiver<bool>;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // yt-dlp (a Python program) otherwise prints in the console code page on Windows.
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .kill_on_drop(true);
    // Own process group, so cancelling also stops grandchildren: the PyInstaller build of
    // yt-dlp runs the real work in a child process, and yt-dlp starts ffmpeg.
    #[cfg(unix)]
    cmd.process_group(0);
    // Don't flash a console window for every spawned tool.
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// The most useful error line from a failed run.
    pub fn error_message(&self) -> String {
        self.stderr
            .lines()
            .rev()
            .find(|l| l.contains("ERROR") || l.contains("rror:"))
            .or_else(|| self.stderr.lines().rev().find(|l| !l.trim().is_empty()))
            .unwrap_or("process failed")
            .trim()
            .trim_start_matches("ERROR: ")
            .to_string()
    }
}

/// Waits until `cancel` flips to true. Never resolves without a token.
pub async fn cancelled(cancel: Option<&Cancel>) {
    match cancel {
        Some(rx) => {
            let mut rx = rx.clone();
            if rx.wait_for(|c| *c).await.is_err() {
                std::future::pending::<()>().await;
            }
        }
        None => std::future::pending().await,
    }
}

/// Runs `program args...` to completion, feeding every stdout/stderr line to `on_line`.
pub async fn run<I, S>(
    program: &Path,
    args: I,
    cancel: Option<&Cancel>,
    on_line: impl FnMut(&str),
) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_with_timeout(program, args, cancel, None, on_line).await
}

/// Like [`run`], but kills the tool and fails if it takes longer than `timeout`.
pub async fn run_with_timeout<I, S>(
    program: &Path,
    args: I,
    cancel: Option<&Cancel>,
    timeout: Option<Duration>,
    on_line: impl FnMut(&str),
) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = command(program);
    cmd.args(args);
    run_command(cmd, cancel, timeout, on_line).await
}

/// Runs a prepared [`command`] (e.g. with extra environment variables); see [`run_with_timeout`].
pub async fn run_command(
    mut cmd: Command,
    cancel: Option<&Cancel>,
    timeout: Option<Duration>,
    mut on_line: impl FnMut(&str),
) -> Result<Output> {
    let program = Path::new(cmd.as_std().get_program()).to_path_buf();
    let name = program.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let mut child = cmd.spawn().map_err(|e| format!("failed to start {}: {e}", program.display()))?;

    let (tx, mut rx) = mpsc::unbounded_channel::<(bool, String)>();
    forward_lines(child.stdout.take().unwrap(), true, tx.clone());
    forward_lines(child.stderr.take().unwrap(), false, tx);

    let deadline = async {
        match timeout {
            Some(t) => tokio::time::sleep(t).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(deadline);

    let (mut stdout, mut stderr) = (String::new(), String::new());
    loop {
        tokio::select! {
            line = rx.recv() => match line {
                Some((is_stdout, line)) => {
                    on_line(&line);
                    let buf = if is_stdout { &mut stdout } else { &mut stderr };
                    buf.push_str(&line);
                    buf.push('\n');
                }
                None => break,
            },
            _ = cancelled(cancel) => {
                kill_tree(&mut child).await;
                return Err("Cancelled".into());
            }
            _ = &mut deadline => {
                kill_tree(&mut child).await;
                return Err(format!("{name} timed out"));
            }
        }
    }

    let status = child.wait().await.map_err(err)?;
    Ok(Output { success: status.success(), stdout, stderr })
}

/// Kills the child and everything it started.
async fn kill_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        #[cfg(unix)]
        let mut killer = {
            let mut c = Command::new("kill");
            c.args(["-KILL", "--", &format!("-{pid}")]); // negative pid = the whole group
            c
        };
        #[cfg(windows)]
        let mut killer = {
            let mut c = Command::new("taskkill");
            c.args(["/T", "/F", "/PID", &pid.to_string()]).creation_flags(CREATE_NO_WINDOW);
            c
        };
        let _ = killer.stdout(Stdio::null()).stderr(Stdio::null()).status().await;
    }
    let _ = child.kill().await;
}

/// Sends each line, decoding lossily: one non-UTF-8 byte mustn't drop the rest of the output.
fn forward_lines(
    stream: impl AsyncRead + Unpin + Send + 'static,
    is_stdout: bool,
    tx: mpsc::UnboundedSender<(bool, String)>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        while let Ok(n) = reader.read_until(b'\n', &mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf);
            let line = line.trim_end_matches(['\n', '\r']).to_string();
            buf.clear();
            if tx.send((is_stdout, line)).is_err() {
                break;
            }
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh() -> &'static Path {
        Path::new("/bin/sh")
    }

    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    #[tokio::test]
    async fn cancel_kills_grandchildren() {
        let (tx, cancel) = watch::channel(false);
        let grandchild = std::sync::Mutex::new(String::new());
        let run = run(sh(), ["-c", "sleep 30 & echo $!; wait"], Some(&cancel), |line| {
            *grandchild.lock().unwrap() = line.to_string();
            let _ = tx.send(true);
        });
        assert_eq!(run.await.err().as_deref(), Some("Cancelled"));
        let pid = grandchild.lock().unwrap().clone();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!alive(&pid), "grandchild {pid} survived cancel");
    }

    #[tokio::test]
    async fn timeout_kills_and_reports() {
        let out = run_with_timeout(sh(), ["-c", "sleep 30"], None, Some(Duration::from_millis(200)), |_| {});
        assert_eq!(out.await.err().as_deref(), Some("sh timed out"));
    }

    #[tokio::test]
    async fn invalid_utf8_does_not_drop_later_lines() {
        let out = run(sh(), ["-c", r"printf 'caf\351\nafter\n'"], None, |_| {}).await.unwrap();
        assert_eq!(out.stdout, "caf\u{FFFD}\nafter\n");
    }
}
