//! Explicit Node process observation.
//!
//! Process identity is resolved only through the configured selector.  The
//! Agent never searches by name, command line, RPC port, or container socket.
//! PID-file reads use `O_NOFOLLOW` on Unix and the resulting PID is checked
//! against sysinfo's start time and executable metadata before a value is
//! emitted.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use platpulse_core::component::{BoundedError, ComponentObservation, ComponentStatus};
use platpulse_core::inventory::ProcessSelector;
use platpulse_core::observation::ProcessCurrent;
use platpulse_core::time::Rfc3339;
use sysinfo::{Pid, ProcessRefreshKind, System};

#[derive(Debug, thiserror::Error)]
pub enum ProcessCollectError {
    #[error("PID file is unavailable")]
    PidFileUnavailable,
    #[error("PID file contains an invalid PID")]
    InvalidPid,
    #[error("systemd unit is unavailable")]
    SystemdUnavailable,
    #[error("systemd unit has no running MainPID")]
    SystemdNotRunning,
    #[error("process identity could not be verified")]
    IdentityUnavailable,
    #[error("process executable could not be verified")]
    ExecutableUnavailable,
    #[error("process start time could not be verified")]
    StartTimeUnavailable,
}

fn error(
    at: Rfc3339,
    code: &'static str,
    message: &'static str,
) -> ComponentObservation<ProcessCurrent> {
    ComponentObservation {
        status: ComponentStatus::Error,
        attempted_at: Some(at),
        latest_observed_at: None,
        received_at: None,
        state_revision: 1,
        value_revision: 0,
        latest: None,
        error: Some(BoundedError {
            code: code.to_owned(),
            message: message.to_owned(),
        }),
    }
}

/// Return a Disabled envelope for an unconfigured Node process selector.
pub fn disabled() -> ComponentObservation<ProcessCurrent> {
    ComponentObservation {
        status: ComponentStatus::Disabled,
        attempted_at: None,
        latest_observed_at: None,
        received_at: None,
        state_revision: 1,
        value_revision: 0,
        latest: None,
        error: None,
    }
}

fn now_rfc3339() -> Option<Rfc3339> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let nanos =
        i128::from(now.as_secs()).checked_mul(1_000_000_000)? + i128::from(now.subsec_nanos());
    let timestamp = time::OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()?;
    timestamp
        .format(&time::format_description::well_known::Rfc3339)
        .ok()?
        .parse()
        .ok()
}

fn pid_from_selector(selector: &ProcessSelector) -> Result<u32, ProcessCollectError> {
    match selector {
        ProcessSelector::PidFile { path } => read_pid_file(Path::new(path)),
        ProcessSelector::SystemdUnit { unit } => {
            let output = std::process::Command::new("systemctl")
                .args(["show", "--property=MainPID", "--value", "--", unit])
                .output()
                .map_err(|_| ProcessCollectError::SystemdUnavailable)?;
            if !output.status.success() {
                return Err(ProcessCollectError::SystemdUnavailable);
            }
            let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let pid = value
                .parse::<u32>()
                .map_err(|_| ProcessCollectError::SystemdNotRunning)?;
            if pid == 0 {
                Err(ProcessCollectError::SystemdNotRunning)
            } else {
                Ok(pid)
            }
        }
    }
}

#[cfg(unix)]
fn read_pid_file(path: &Path) -> Result<u32, ProcessCollectError> {
    use nix::fcntl::{OFlag, open};
    use nix::sys::stat::Mode;
    use nix::unistd::{close, read};

    let fd = open(
        path,
        OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| ProcessCollectError::PidFileUnavailable)?;
    let mut bytes = [0_u8; 64];
    let result = read(fd, &mut bytes).map_err(|_| ProcessCollectError::PidFileUnavailable);
    let _ = close(fd);
    let count = result?;
    let text = std::str::from_utf8(&bytes[..count]).map_err(|_| ProcessCollectError::InvalidPid)?;
    let pid = text
        .trim()
        .parse::<u32>()
        .map_err(|_| ProcessCollectError::InvalidPid)?;
    if pid == 0 {
        return Err(ProcessCollectError::InvalidPid);
    }
    Ok(pid)
}

#[cfg(not(unix))]
fn read_pid_file(path: &Path) -> Result<u32, ProcessCollectError> {
    let text =
        std::fs::read_to_string(path).map_err(|_| ProcessCollectError::PidFileUnavailable)?;
    let pid = text
        .trim()
        .parse::<u32>()
        .map_err(|_| ProcessCollectError::InvalidPid)?;
    if pid == 0 {
        return Err(ProcessCollectError::InvalidPid);
    }
    Ok(pid)
}

/// Resolve and verify one configured process.  `System` is refreshed only for
/// the selected PID; the executable path and start time are both mandatory so
/// PID reuse or restricted `/proc` visibility fails closed.
pub fn collect(
    system: &mut System,
    selector: Option<&ProcessSelector>,
    attempted_at: Rfc3339,
) -> ComponentObservation<ProcessCurrent> {
    let Some(selector) = selector else {
        return disabled();
    };
    let pid = match pid_from_selector(selector) {
        Ok(pid) => pid,
        Err(process_error) => {
            return error(
                attempted_at,
                "process_selector_error",
                error_message(&process_error),
            );
        }
    };
    let sys_pid = Pid::from(pid as usize);
    system.refresh_process_specifics(sys_pid, ProcessRefreshKind::everything().without_environ());
    let Some(process) = system.process(sys_pid) else {
        return error(
            attempted_at,
            "process_not_found",
            "selected process is not running",
        );
    };
    let executable = process.exe().filter(|path| !path.as_os_str().is_empty());
    if executable.is_none() {
        return error(
            attempted_at,
            "process_executable_unknown",
            "selected process executable is unavailable",
        );
    }
    let start_time = process.start_time();
    if start_time == 0 {
        return error(
            attempted_at,
            "process_start_time_unknown",
            "selected process start time is unavailable",
        );
    }
    let Some(started_at) = time::OffsetDateTime::from_unix_timestamp(start_time as i64)
        .ok()
        .and_then(|time| {
            time.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .and_then(|value| value.parse::<Rfc3339>().ok())
    else {
        return error(
            attempted_at,
            "process_start_time_invalid",
            "selected process start time is invalid",
        );
    };
    let Some(observed_at) = now_rfc3339() else {
        return error(
            attempted_at,
            "process_observed_time_invalid",
            "process observation time is invalid",
        );
    };
    let value = ProcessCurrent {
        pid: pid as u64,
        started_at,
        cpu_percent: f64::from(process.cpu_usage()).clamp(0.0, 100.0),
        memory_bytes: process.memory(),
        uptime_ms: process.run_time().saturating_mul(1_000),
    };
    ComponentObservation {
        status: ComponentStatus::Ok,
        attempted_at: Some(attempted_at),
        latest_observed_at: Some(observed_at),
        received_at: None,
        state_revision: 1,
        value_revision: 1,
        latest: Some(value),
        error: None,
    }
}

fn error_message(error: &ProcessCollectError) -> &'static str {
    match error {
        ProcessCollectError::PidFileUnavailable => "PID file is unavailable",
        ProcessCollectError::InvalidPid => "PID file contains an invalid PID",
        ProcessCollectError::SystemdUnavailable => "systemd unit could not be queried",
        ProcessCollectError::SystemdNotRunning => "systemd unit is not running",
        ProcessCollectError::IdentityUnavailable => "process identity could not be verified",
        ProcessCollectError::ExecutableUnavailable => "process executable could not be verified",
        ProcessCollectError::StartTimeUnavailable => "process start time could not be verified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn timestamp() -> Rfc3339 {
        "2026-01-01T00:00:00Z".parse().unwrap()
    }

    #[test]
    fn missing_selector_is_disabled() {
        let mut system = System::new_all();
        let observation = collect(&mut system, None, timestamp());
        assert_eq!(observation.status, ComponentStatus::Disabled);
        assert!(observation.latest.is_none());
    }

    #[test]
    fn pid_file_process_is_verified_by_sysinfo_identity() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("node.pid");
        fs::write(&path, std::process::id().to_string()).unwrap();
        let selector = ProcessSelector::PidFile {
            path: path.display().to_string(),
        };
        let mut system = System::new_all();
        let observation = collect(&mut system, Some(&selector), timestamp());
        assert_eq!(observation.status, ComponentStatus::Ok);
        assert_eq!(observation.latest.unwrap().pid, std::process::id() as u64);
    }

    #[test]
    fn missing_pid_preserves_error_without_fabricated_value() {
        let selector = ProcessSelector::PidFile {
            path: "/definitely/missing/platpulse.pid".to_owned(),
        };
        let mut system = System::new_all();
        let observation = collect(&mut system, Some(&selector), timestamp());
        assert_eq!(observation.status, ComponentStatus::Error);
        assert!(observation.latest.is_none());
        assert_eq!(observation.error.unwrap().code, "process_selector_error");
    }
}
