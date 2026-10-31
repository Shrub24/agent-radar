//! The control-mode observer: tmux says something changed, the sampler reads.
//!
//! One no-output control client per server. A notification is only a hint to
//! read sooner; it never establishes focus, placement or an exit, and a missing
//! one costs one poll interval. The observer therefore never queues, parses
//! beyond the notification name, or keeps pane output.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{TmuxConfig, server_args};
use crate::runtime::{ChangeSource, Wake};

/// Longest notification line kept; the rest of a longer one is discarded.
const MAX_LINE: usize = 4096;
const FIRST_RETRY: Duration = Duration::from_millis(500);
const LAST_RETRY: Duration = Duration::from_secs(5);
/// An attachment that lasted this long was a working one, so the backoff restarts.
const STABLE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);

/// Notifications that can mean focus or topology moved. Everything else, and
/// anything unknown, is ignored.
const HINTS: &[&str] = &[
    "%client-session-changed",
    "%client-detached",
    "%session-changed",
    "%session-window-changed",
    "%window-pane-changed",
    "%layout-change",
    "%window-add",
    "%window-close",
    "%window-renamed",
    "%unlinked-window-add",
    "%unlinked-window-close",
    "%unlinked-window-renamed",
    "%session-renamed",
    "%sessions-changed",
];

/// Whether the observer is currently attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    /// Not attached; `reason` is why the last attempt ended.
    Lost {
        reason: String,
    },
    Attached,
}

/// The running observer.
pub struct Observer {
    health: Arc<Mutex<Health>>,
    worker: Option<JoinHandle<()>>,
}

impl Observer {
    /// Starts supervising one control client for `config`'s server until `stop`
    /// is set. Each attach and each loss wakes the sampler once, so a change
    /// around either is read rather than lost.
    pub fn start(config: TmuxConfig, wake: Wake, stop: Arc<AtomicBool>) -> Self {
        let health = Arc::new(Mutex::new(Health::Lost {
            reason: "not attached yet".into(),
        }));
        let worker = {
            let health = Arc::clone(&health);
            thread::Builder::new()
                .name("radar-tmux-observer".into())
                .spawn(move || supervise(&config, &wake, &stop, &health))
                .ok()
        };
        Self { health, worker }
    }
}

impl ChangeSource for Observer {
    fn lost(&self) -> Option<String> {
        match &*self
            .health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            Health::Lost { reason } => Some(reason.clone()),
            Health::Attached => None,
        }
    }

    fn join(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn supervise(config: &TmuxConfig, wake: &Wake, stop: &AtomicBool, health: &Mutex<Health>) {
    let mut retry = FIRST_RETRY;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        let reason = match attach(config, wake, stop, health) {
            Ok(()) => return,
            Err(reason) => reason,
        };
        *health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Health::Lost { reason };
        wake();
        if started.elapsed() >= STABLE {
            retry = FIRST_RETRY;
        }
        sleep_unless_stopped(retry, stop);
        retry = (retry * 2).min(LAST_RETRY);
    }
}

fn sleep_unless_stopped(duration: Duration, stop: &AtomicBool) {
    let end = Instant::now() + duration;
    while !stop.load(Ordering::SeqCst) && Instant::now() < end {
        thread::sleep(POLL);
    }
}

/// Runs one control client to its end. `Ok` means the daemon is stopping;
/// `Err` says why the client ended.
fn attach(
    config: &TmuxConfig,
    wake: &Wake,
    stop: &AtomicBool,
    health: &Mutex<Health>,
) -> Result<(), String> {
    let mut child = Command::new(&config.executable)
        .args(server_args(
            &config.server,
            &["-C", "attach-session", "-f", "ignore-size,no-output"],
        ))
        .env_remove("TMUX")
        // A control client ends when its input closes, so the pipe stays open.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not run {}: {error}", config.executable.display()))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout is piped");
    let (hints, lines) = mpsc::channel();
    let reader = thread::spawn(move || read_hints(stdout, &hints));

    let mut attached = false;
    let outcome = loop {
        match lines.recv_timeout(POLL) {
            Ok(Line::Hint) if attached => wake(),
            Ok(Line::Hint) => {}
            Ok(Line::Ready) => {
                attached = true;
                *health
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Health::Attached;
                wake();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                break Err(if attached {
                    "the control client ended".to_string()
                } else {
                    "tmux has no session to attach to".to_string()
                });
            }
        }
        if stop.load(Ordering::SeqCst) {
            break Ok(());
        }
    };
    end(&mut child, stdin);
    let _ = reader.join();
    outcome
}

fn end(child: &mut Child, stdin: Option<std::process::ChildStdin>) {
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
}

enum Line {
    /// The first reply block: the client is attached.
    Ready,
    Hint,
}

/// Reads bounded notification lines until the stream ends.
fn read_hints(stdout: impl Read, out: &mpsc::Sender<Line>) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    let mut ready = false;
    loop {
        line.clear();
        let Ok(read) = (&mut reader)
            .take(MAX_LINE as u64)
            .read_until(b'\n', &mut line)
        else {
            return;
        };
        if read == 0 {
            return;
        }
        if line.last() != Some(&b'\n') && read == MAX_LINE {
            // An oversized line is not a notification; skip to its end.
            let mut rest = Vec::new();
            loop {
                rest.clear();
                match (&mut reader)
                    .take(MAX_LINE as u64)
                    .read_until(b'\n', &mut rest)
                {
                    Ok(0) | Err(_) => return,
                    Ok(_) if rest.last() == Some(&b'\n') => break,
                    Ok(_) => {}
                }
            }
            continue;
        }
        let sent = if is_hint(&line) {
            out.send(Line::Hint)
        } else if !ready && line.starts_with(b"%end ") {
            ready = true;
            out.send(Line::Ready)
        } else {
            Ok(())
        };
        if sent.is_err() {
            return;
        }
    }
}

fn is_hint(line: &[u8]) -> bool {
    let name = line
        .split(|b| b.is_ascii_whitespace())
        .next()
        .unwrap_or(&[]);
    HINTS.iter().any(|hint| hint.as_bytes() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(input: &[u8]) -> Vec<&'static str> {
        let (sender, receiver) = mpsc::channel();
        read_hints(input, &sender);
        drop(sender);
        receiver
            .iter()
            .map(|line| match line {
                Line::Ready => "ready",
                Line::Hint => "hint",
            })
            .collect()
    }

    #[test]
    fn only_listed_notifications_are_hints_and_the_first_reply_is_ready() {
        let input =
            b"%begin 1 1 0\n%end 1 1 0\n%session-changed $0 a\n%window-pane-changed @0 %1\n\
            %output %1 secret\n%exit\n%begin 2 2 0\n%end 2 2 0\n%made-up-in-a-later-tmux x\n\
            %layout-change @0 a b 1\n";
        assert_eq!(read(input), ["ready", "hint", "hint", "hint"]);
    }

    #[test]
    fn an_oversized_line_is_skipped_whole() {
        let mut input = b"%session-renamed ".to_vec();
        input.extend(std::iter::repeat_n(b'x', MAX_LINE * 3));
        input.extend(b"\n%sessions-changed\n");
        assert_eq!(read(&input), ["hint"], "only the line after it is read");
    }
}
