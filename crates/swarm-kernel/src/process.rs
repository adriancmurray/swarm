//! Small platform/process helpers shared by the CLI orchestration modules.

use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use sysinfo::Pid;

#[cfg(unix)]
pub fn terminate_pid(pid: u32) {
    let pgid = format!("-{pid}");
    let status = Command::new("/bin/kill")
        .arg("-TERM")
        .arg(pgid)
        .stderr(Stdio::null())
        .status();
    if !matches!(status, Ok(done) if done.success()) {
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(unix)]
pub fn force_terminate_pid(pid: u32) {
    terminate_pid(pid);
    let pgid = format!("-{pid}");
    let status = Command::new("/bin/kill")
        .arg("-KILL")
        .arg(pgid)
        .stderr(Stdio::null())
        .status();
    if !matches!(status, Ok(done) if done.success()) {
        let _ = Command::new("/bin/kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(not(unix))]
pub fn terminate_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .arg("/PID")
        .arg(pid.to_string())
        .arg("/T")
        .status();
}

#[cfg(not(unix))]
pub fn force_terminate_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .arg("/PID")
        .arg(pid.to_string())
        .arg("/T")
        .arg("/F")
        .status();
}

#[cfg(unix)]
pub fn process_is_alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn process_is_alive(pid: u32) -> bool {
    Command::new("tasklist")
        .arg("/FI")
        .arg(format!("PID eq {pid}"))
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

#[cfg(unix)]
pub fn detach_background_command(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub fn detach_background_command(_command: &mut Command) {}

#[cfg(unix)]
pub fn stdin_ready(timeout: Duration) -> bool {
    use std::os::unix::io::AsRawFd;

    let fd = std::io::stdin().as_raw_fd();
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    rc > 0 && (pfd.revents & (libc::POLLIN | libc::POLLHUP)) != 0
}

#[cfg(not(unix))]
pub fn stdin_ready(_timeout: Duration) -> bool {
    false
}

pub fn exit_code(status: Option<ExitStatus>) -> i32 {
    exit_code_impl(status)
}

#[cfg(unix)]
fn exit_code_impl(status: Option<ExitStatus>) -> i32 {
    use std::os::unix::process::ExitStatusExt;

    match status {
        Some(status) => status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(1),
        None => 1,
    }
}

#[cfg(not(unix))]
fn exit_code_impl(status: Option<ExitStatus>) -> i32 {
    status.and_then(|status| status.code()).unwrap_or(1)
}

/// Convert a sysinfo `Pid` to a `u32`, shared by the monitor sampler and the
/// runtime-process report. sysinfo's `Pid` has no direct `u32` accessor, so go
/// through its string form.
pub fn pid_to_u32(pid: Pid) -> Option<u32> {
    pid.to_string().parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_defaults_to_one_without_status() {
        assert_eq!(exit_code(None), 1);
    }

    #[cfg(unix)]
    #[test]
    fn exit_code_uses_process_code() {
        use std::os::unix::process::ExitStatusExt;

        assert_eq!(exit_code(Some(ExitStatus::from_raw(7 << 8))), 7);
    }

    #[cfg(unix)]
    #[test]
    fn exit_code_maps_signal_status() {
        use std::os::unix::process::ExitStatusExt;

        assert_eq!(exit_code(Some(ExitStatus::from_raw(15))), 143);
    }

    #[test]
    fn process_is_alive_detects_current_process() {
        assert!(process_is_alive(std::process::id()));
    }
}
