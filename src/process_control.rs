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
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::Duration;

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
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => return Ok(Acquired::Absent),
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
    // SAFETY: the arguments are a valid pidfd, a signal number and a null siginfo
    // (as pidfd_send_signal(2) defines it for a plain signal). No memory is
    // written through them.
    let sent = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            libc::SIGTERM,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether the process exits within `timeout`. A pidfd becomes readable when its
/// process terminates, so this waits on the descriptor, not on the PID.
pub fn exited_within(fd: &OwnedFd, timeout: Duration) -> bool {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    loop {
        // SAFETY: one valid pollfd is passed with its count.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        match ready {
            1.. => return poll.revents & libc::POLLIN != 0,
            0 => return false,
            _ if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => continue,
            _ => return false,
        }
    }
}

fn open(pid: i32) -> io::Result<OwnedFd> {
    // SAFETY: pidfd_open takes a PID and flags, and returns a new descriptor the
    // caller owns.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the kernel returned a fresh descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}
