//! Exact process control through pidfds.
//!
//! A pidfd names one process incarnation: once that process is gone, the
//! descriptor never reaches a process that later reuses its PID. Signals and
//! exit observation go through the descriptor, and a descriptor is only handed
//! out after the process's birth identity has been checked on both sides of the
//! open, so a recycled PID is never mistaken for the process that was claimed.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, poll};
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

use crate::control_plane::registry::ProcessVerifier;
use crate::model::ProcessIdentity;

/// Bounds one /proc read. A read cut short cannot describe the process, so it
/// never matches what was requested.
const MAX_PROC_BYTES: u64 = 1 << 20;

/// Whether the identity named by a claim is still that process.
pub enum Acquired {
    /// The descriptor names the claimed process, which is alive.
    Process(OwnedFd),
    /// No process with the claimed PID exists any longer: the claimed process has
    /// exited, and the absence is positive.
    Absent,
    /// The PID names another incarnation. Two live processes never share a birth
    /// identity, so the claimed process has exited; nothing was signalled.
    Replaced,
}

/// What the kernel reports a process was started with.
pub struct Effective {
    /// The command line as the kernel recorded it at exec.
    pub cmdline: Vec<String>,
    /// The working directory as the kernel names it.
    pub cwd: Option<PathBuf>,
    /// The environment the process was started with.
    pub env: BTreeMap<String, String>,
}

/// Reads what process `pid` was started with. The values are returned to the
/// caller and never formatted here.
pub fn effective(pid: i32) -> io::Result<Effective> {
    Ok(Effective {
        cmdline: read_fields(&format!("/proc/{pid}/cmdline"))?,
        cwd: fs::read_link(format!("/proc/{pid}/cwd")).ok(),
        env: read_fields(&format!("/proc/{pid}/environ"))?
            .into_iter()
            .filter_map(|entry| {
                entry
                    .split_once('=')
                    .map(|(name, value)| (name.to_owned(), value.to_owned()))
            })
            .collect(),
    })
}

/// A NUL-separated /proc file as its fields. Empty fields are kept: an empty
/// argument is part of the command line.
fn read_fields(path: &str) -> io::Result<Vec<String>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_PROC_BYTES)
        .read_to_end(&mut bytes)?;
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    Ok(bytes
        .split(|byte| *byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect())
}

/// Opens a pidfd for the claimed identity, proving the identity before and after
/// the open. `Err` is an identity that cannot be read; nothing may be signalled.
pub fn acquire(
    identity: &ProcessIdentity,
    verifier: &dyn ProcessVerifier,
) -> Result<Acquired, String> {
    match verifier.inspect(identity.pid)? {
        None => return Ok(Acquired::Absent),
        Some(observed) if observed != *identity => return Ok(Acquired::Replaced),
        Some(_) => {}
    }
    let fd = match open(identity.pid) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::SRCH) => return Ok(Acquired::Absent),
        Err(error) => return Err(format!("pidfd_open: {error}")),
    };
    match verifier.inspect(identity.pid)? {
        Some(observed) if observed == *identity => Ok(Acquired::Process(fd)),
        Some(_) => Ok(Acquired::Replaced),
        None => Ok(Acquired::Absent),
    }
}

/// Sends SIGTERM to the process the descriptor names, and nothing else.
pub fn terminate(fd: &OwnedFd) -> io::Result<()> {
    pidfd_send_signal(fd, Signal::TERM).map_err(io::Error::from)
}

/// Whether the process exits within `timeout`. A pidfd becomes readable when its
/// process terminates, so this waits on the descriptor, not on the PID.
pub fn exited_within(fd: &OwnedFd, timeout: Duration) -> bool {
    let timeout = rustix::time::Timespec {
        tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(timeout.subsec_nanos()),
    };
    let mut fds = [PollFd::new(fd, PollFlags::IN)];
    loop {
        match poll(&mut fds, Some(&timeout)) {
            Ok(ready) => return ready != 0 && fds[0].revents().contains(PollFlags::IN),
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return false,
        }
    }
}

fn open(pid: i32) -> Result<OwnedFd, rustix::io::Errno> {
    let pid = Pid::from_raw(pid).ok_or(rustix::io::Errno::INVAL)?;
    pidfd_open(pid, PidfdFlags::empty())
}
