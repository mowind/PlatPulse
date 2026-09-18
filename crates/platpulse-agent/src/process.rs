//! Explicit Node process observation.
//!
//! Process identity is resolved only through the configured selector.  The
//! Agent never searches by name, command line, RPC port, or container socket.
//! Every selector resolves its PID through one injectable
//! [`ProcessSelectorRunner`]; production wiring uses [`SystemSelectorRunner`],
//! which reads real PID files with `O_NOFOLLOW` on Unix and executes real
//! system commands, while tests inject deterministic fakes.  The resolved PID
//! is checked against sysinfo's start time and executable metadata before a
//! value is emitted.

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

/// Captured result of one selector command.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Whether the command exited with a success status.
    pub success: bool,
    /// Captured standard output.
    pub stdout: Vec<u8>,
}

/// Injectable runner that resolves a [`ProcessSelector`] to a PID.
///
/// PID-file selectors call [`ProcessSelectorRunner::read_pid_file`];
/// command-backed selectors (today `systemd_unit`, later a supervisor
/// selector) call [`ProcessSelectorRunner::run`].  Production wires
/// [`SystemSelectorRunner`]; tests inject fakes so no real systemd or
/// supervisord is required.
pub trait ProcessSelectorRunner {
    /// Read the raw text of a PID file. Unreadable files map to
    /// [`ProcessCollectError::PidFileUnavailable`] and non-UTF-8 content to
    /// [`ProcessCollectError::InvalidPid`].
    fn read_pid_file(&self, path: &Path) -> Result<String, ProcessCollectError>;

    /// Execute `program` with `args`, capturing its exit status and stdout.
    /// `Err` means the command itself could not be executed; a non-zero exit
    /// is reported through [`CommandOutput::success`].
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutput>;
}

/// Production runner backed by the real filesystem and real system commands.
#[derive(Debug, Clone, Copy)]
pub struct SystemSelectorRunner;

impl ProcessSelectorRunner for SystemSelectorRunner {
    fn read_pid_file(&self, path: &Path) -> Result<String, ProcessCollectError> {
        read_pid_file_raw(path)
    }

    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutput> {
        let output = std::process::Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: output.stdout,
        })
    }
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

/// Parse a strictly positive PID from selector output. `None` means the text
/// was not a positive integer; each selector maps that to its own typed error.
fn parse_positive_pid(text: &str) -> Option<u32> {
    let pid = text.trim().parse::<u32>().ok()?;
    (pid != 0).then_some(pid)
}

fn pid_from_selector<R: ProcessSelectorRunner + ?Sized>(
    runner: &R,
    selector: &ProcessSelector,
) -> Result<u32, ProcessCollectError> {
    match selector {
        ProcessSelector::PidFile { path } => {
            let text = runner.read_pid_file(Path::new(path))?;
            parse_positive_pid(&text).ok_or(ProcessCollectError::InvalidPid)
        }
        ProcessSelector::SystemdUnit { unit } => {
            let output = runner
                .run(
                    "systemctl",
                    &["show", "--property=MainPID", "--value", "--", unit.as_str()],
                )
                .map_err(|_| ProcessCollectError::SystemdUnavailable)?;
            if !output.success {
                return Err(ProcessCollectError::SystemdUnavailable);
            }
            parse_positive_pid(&String::from_utf8_lossy(&output.stdout))
                .ok_or(ProcessCollectError::SystemdNotRunning)
        }
    }
}

#[cfg(unix)]
fn read_pid_file_raw(path: &Path) -> Result<String, ProcessCollectError> {
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
    std::str::from_utf8(&bytes[..count])
        .map(str::to_owned)
        .map_err(|_| ProcessCollectError::InvalidPid)
}

#[cfg(not(unix))]
fn read_pid_file_raw(path: &Path) -> Result<String, ProcessCollectError> {
    std::fs::read_to_string(path).map_err(|_| ProcessCollectError::PidFileUnavailable)
}

/// Resolve and verify one configured process.  `System` is refreshed only for
/// the selected PID; the executable path and start time are both mandatory so
/// PID reuse or restricted `/proc` visibility fails closed.
pub fn collect<R: ProcessSelectorRunner + ?Sized>(
    runner: &R,
    system: &mut System,
    selector: Option<&ProcessSelector>,
    attempted_at: Rfc3339,
) -> ComponentObservation<ProcessCurrent> {
    let Some(selector) = selector else {
        return disabled();
    };
    let pid = match pid_from_selector(runner, selector) {
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

    /// Scripted runner outcome used to exercise command-backed selectors
    /// without a real systemd (or, later, supervisord) installation.
    #[derive(Debug, Clone)]
    enum FakeOutcome {
        PidFile(String),
        Unavailable,
        Exited { success: bool, stdout: Vec<u8> },
    }

    #[derive(Debug, Clone)]
    struct FakeRunner {
        outcome: FakeOutcome,
    }

    impl ProcessSelectorRunner for FakeRunner {
        fn read_pid_file(&self, _path: &Path) -> Result<String, ProcessCollectError> {
            match &self.outcome {
                FakeOutcome::PidFile(text) => Ok(text.clone()),
                _ => Err(ProcessCollectError::PidFileUnavailable),
            }
        }

        fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutput> {
            // Pin the exact command contract so an argv regression fails here.
            assert_eq!(program, "systemctl");
            assert_eq!(
                args,
                [
                    "show",
                    "--property=MainPID",
                    "--value",
                    "--",
                    "platon-validator-a.service"
                ]
                .as_slice()
            );
            match &self.outcome {
                FakeOutcome::Unavailable => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "systemctl is unavailable",
                )),
                FakeOutcome::Exited { success, stdout } => Ok(CommandOutput {
                    success: *success,
                    stdout: stdout.clone(),
                }),
                FakeOutcome::PidFile(_) => {
                    panic!("run must not be called for a PID-file selector")
                }
            }
        }
    }

    fn systemd_selector() -> ProcessSelector {
        ProcessSelector::SystemdUnit {
            unit: "platon-validator-a.service".to_owned(),
        }
    }

    fn collect_systemd(outcome: FakeOutcome) -> ComponentObservation<ProcessCurrent> {
        let runner = FakeRunner { outcome };
        let selector = systemd_selector();
        let mut system = System::new_all();
        collect(&runner, &mut system, Some(&selector), timestamp())
    }

    fn collect_pid_file(text: &str) -> ComponentObservation<ProcessCurrent> {
        let runner = FakeRunner {
            outcome: FakeOutcome::PidFile(text.to_owned()),
        };
        let selector = ProcessSelector::PidFile {
            path: "/run/platon-validator-a.pid".to_owned(),
        };
        let mut system = System::new_all();
        collect(&runner, &mut system, Some(&selector), timestamp())
    }

    fn assert_selector_error(observation: &ComponentObservation<ProcessCurrent>, message: &str) {
        assert_eq!(observation.status, ComponentStatus::Error);
        assert!(observation.latest.is_none());
        assert_eq!(observation.value_revision, 0);
        let error = observation
            .error
            .as_ref()
            .expect("selector error is recorded");
        assert_eq!(error.code, "process_selector_error");
        assert_eq!(error.message, message);
    }

    #[test]
    fn missing_selector_is_disabled() {
        let mut system = System::new_all();
        let observation = collect(&SystemSelectorRunner, &mut system, None, timestamp());
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
        let observation = collect(
            &SystemSelectorRunner,
            &mut system,
            Some(&selector),
            timestamp(),
        );
        assert_eq!(observation.status, ComponentStatus::Ok);
        assert_eq!(observation.latest.unwrap().pid, std::process::id() as u64);
    }

    #[test]
    fn missing_pid_preserves_error_without_fabricated_value() {
        let selector = ProcessSelector::PidFile {
            path: "/definitely/missing/platpulse.pid".to_owned(),
        };
        let mut system = System::new_all();
        let observation = collect(
            &SystemSelectorRunner,
            &mut system,
            Some(&selector),
            timestamp(),
        );
        assert_selector_error(&observation, "PID file is unavailable");
    }

    #[test]
    fn pid_file_pid_resolves_through_the_injected_runner() {
        let own_pid = std::process::id();
        let observation = collect_pid_file(&format!("{own_pid}\n"));
        assert_eq!(observation.status, ComponentStatus::Ok);
        assert_eq!(observation.latest.unwrap().pid, own_pid as u64);
    }

    #[test]
    fn pid_file_zero_pid_is_rejected_by_the_injected_runner() {
        let observation = collect_pid_file("0\n");
        assert_selector_error(&observation, "PID file contains an invalid PID");
    }

    #[test]
    fn pid_file_non_numeric_content_is_rejected_by_the_injected_runner() {
        let observation = collect_pid_file("not-a-pid\n");
        assert_selector_error(&observation, "PID file contains an invalid PID");
    }

    #[test]
    fn systemd_command_unavailable_is_a_typed_error() {
        let observation = collect_systemd(FakeOutcome::Unavailable);
        assert_selector_error(&observation, "systemd unit could not be queried");
    }

    #[test]
    fn systemd_nonzero_exit_is_a_typed_error() {
        let observation = collect_systemd(FakeOutcome::Exited {
            success: false,
            stdout: b"1234\n".to_vec(),
        });
        assert_selector_error(&observation, "systemd unit could not be queried");
    }

    #[test]
    fn systemd_main_pid_zero_is_not_a_running_unit() {
        let observation = collect_systemd(FakeOutcome::Exited {
            success: true,
            stdout: b"0\n".to_vec(),
        });
        assert_selector_error(&observation, "systemd unit is not running");
    }

    #[test]
    fn systemd_valid_main_pid_is_observed() {
        let own_pid = std::process::id();
        let observation = collect_systemd(FakeOutcome::Exited {
            success: true,
            stdout: own_pid.to_string().into_bytes(),
        });
        assert_eq!(observation.status, ComponentStatus::Ok);
        assert_eq!(observation.latest.unwrap().pid, own_pid as u64);
        assert!(observation.error.is_none());
    }
}
