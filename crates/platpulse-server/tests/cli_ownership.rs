#![cfg(unix)]
//! Subprocess regression coverage for the Server database ownership guard
//! (issue #160, follow-up to #137).
//!
//! Every offline CLI command must refuse a database owned by a running
//! Server *before* SQLite opens, must not modify DB/WAL/SHM or create a
//! replacement database file, and must release the guard on success and
//! failure. These tests run the real `platpulse-server` binary as a child
//! process against real temporary SQLite files in non-development
//! configurations.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// A temporary non-development Server deployment with a private state
/// directory, backup directory, and `server.toml`.
struct Deployment {
    _dir: TempDir,
    state: PathBuf,
    config: PathBuf,
    db: PathBuf,
    backups: PathBuf,
}

impl Deployment {
    fn new() -> Self {
        Self::with_db_name("server.db")
    }

    fn with_db_name(name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let backups = dir.path().join("backups");
        for path in [&state, &backups] {
            fs::create_dir_all(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let db = state.join(name);
        let config = state.join("server.toml");
        fs::write(
            &config,
            format!(
                "state_dir = \"{state}\"\ndb_path = \"{db}\"\npepper_file = \"{pepper}\"\nbackup_dir = \"{backups}\"\n",
                state = state.display(),
                db = db.display(),
                pepper = state.join("server-pepper").display(),
                backups = backups.display(),
            ),
        )
        .unwrap();
        Self {
            _dir: dir,
            state,
            config,
            db,
            backups,
        }
    }

    fn append_listen(&self, port: u16) {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&self.config)
            .unwrap();
        writeln!(file, "listen = \"127.0.0.1:{port}\"").unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with_stdin(args, None)
    }

    fn run_with_stdin(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_platpulse-server"));
        command
            .args(args)
            .arg("--config")
            .arg(&self.config)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if stdin.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        let mut child = command.spawn().unwrap();
        if let Some(input) = stdin {
            let mut handle = child.stdin.take().unwrap();
            handle.write_all(input.as_bytes()).unwrap();
            drop(handle);
        }
        child.wait_with_output().unwrap()
    }
}

/// SHA-256 of a file's bytes when it exists, so an unchanged fingerprint is a
/// true byte-for-byte comparison.
fn fingerprint(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|bytes| {
        Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    })
}

fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Content fingerprint of the database and both WAL sidecars.
fn database_fingerprint(db: &Path) -> [Option<String>; 3] {
    [
        fingerprint(db),
        fingerprint(&sidecar(db, "-wal")),
        fingerprint(&sidecar(db, "-shm")),
    ]
}

/// The refusal must name the stopped-Server requirement. Restore keeps its
/// own specialized wording; every other offline command uses the ownership
/// guard wording.
fn assert_refused(output: &Output, context: &str) {
    assert!(!output.status.success(), "expected refusal for {context}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("running PlatPulse Server owns")
            || stderr.contains("exclusive stopped Server"),
        "unexpected stderr for {context}: {stderr}"
    );
}

/// A running Server (simulated here by holding the same guard) refuses every
/// offline CLI path before SQLite opens. The database files are byte-for-byte
/// untouched and no backup artifact is created.
#[test]
fn a_running_server_owner_blocks_every_offline_cli_command_before_sqlite() {
    let deployment = Deployment::new();
    assert!(
        deployment.run(&["init"]).status.success(),
        "init must succeed while no Server owns the database"
    );

    let _guard = platpulse_server::ownership::acquire(&deployment.db).unwrap();
    let before = database_fingerprint(&deployment.db);

    let long_hex = format!("0x{}", "1".repeat(64));
    let cases: Vec<(Vec<String>, Option<String>)> = vec![
        (vec!["backup".into()], None),
        (vec!["init".into()], None),
        (
            vec![
                "owner".into(),
                "create".into(),
                "--username".into(),
                "admin".into(),
            ],
            Some("correct-horse-battery\n".into()),
        ),
        (
            vec![
                "viewer".into(),
                "create".into(),
                "--username".into(),
                "viewer".into(),
            ],
            Some("correct-horse-battery\n".into()),
        ),
        (
            vec![
                "restore".into(),
                "--artifact-id".into(),
                "missing-artifact".into(),
                "--yes".into(),
            ],
            None,
        ),
        (
            vec![
                "agent".into(),
                "create-enrollment-token".into(),
                "--expires-in".into(),
                "0".into(),
            ],
            None,
        ),
        (
            vec![
                "network".into(),
                "create".into(),
                "--key".into(),
                "platon-mainnet".into(),
                "--display-name".into(),
                "PlatON Mainnet".into(),
                "--genesis-hash".into(),
                long_hex,
                "--chain-id".into(),
                "210425".into(),
                "--p2p-network-id".into(),
                "210425".into(),
                "--address-hrp".into(),
                "lat".into(),
            ],
            None,
        ),
    ];
    for (args, stdin) in &cases {
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = deployment.run_with_stdin(&borrowed, stdin.as_deref());
        assert_refused(&output, &format!("{args:?}"));
        assert_eq!(
            database_fingerprint(&deployment.db),
            before,
            "database changed for {args:?}"
        );
    }

    assert_eq!(
        fs::read_dir(&deployment.backups).unwrap().count(),
        0,
        "a refused command must not create a backup artifact"
    );
}

/// A refused init must not create the database file or the pepper file, even
/// though the state directory itself is prepared first.
#[test]
fn a_refused_init_does_not_create_a_replacement_database() {
    let deployment = Deployment::with_db_name("fresh.db");
    let _guard = platpulse_server::ownership::acquire(&deployment.db).unwrap();

    let output = deployment.run(&["init"]);
    assert_refused(&output, "init on a fresh path");
    assert!(
        !deployment.db.exists(),
        "a refused init must not create the database file"
    );
    assert!(
        !deployment.state.join("server-pepper").exists(),
        "a refused init must not create the pepper file"
    );
}

/// The ownership refusal precedes any SQLite open: the same command on a
/// non-SQLite file reports the stopped-Server requirement while the guard is
/// held, and only reaches SQLite once the guard is released.
#[test]
fn ownership_refusal_precedes_sqlite_open() {
    let deployment = Deployment::new();
    fs::write(&deployment.db, b"this is not a sqlite database").unwrap();
    fs::set_permissions(&deployment.db, fs::Permissions::from_mode(0o600)).unwrap();

    let guard = platpulse_server::ownership::acquire(&deployment.db).unwrap();
    let refused = deployment.run(&["init"]);
    assert_refused(&refused, "init over a non-SQLite file");
    drop(guard);

    let released = deployment.run(&["init"]);
    assert!(!released.status.success());
    let stderr = String::from_utf8_lossy(&released.stderr);
    assert!(
        !stderr.contains("running PlatPulse Server owns"),
        "ownership error leaked after the guard was released: {stderr}"
    );
    assert!(
        stderr.contains("SQLite"),
        "expected a SQLite-stage failure after the guard was released: {stderr}"
    );
    assert!(
        platpulse_server::ownership::acquire(&deployment.db).is_ok(),
        "a command that failed at the SQLite stage did not release the guard"
    );
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A real non-development Server subprocess (exclusive SQLite locking) owns
/// the database for its whole lifetime: the offline CLI is refused before it
/// opens SQLite, and both the guard and the lock are released when the Server
/// stops.
#[test]
fn a_real_production_server_subprocess_owns_the_database_until_it_stops() {
    let deployment = Deployment::new();
    assert!(deployment.run(&["init"]).status.success());
    let port = free_port();
    deployment.append_listen(port);

    let log_path = deployment.state.join("serve.log");
    let log = fs::File::create(&log_path).unwrap();
    let log_err = log.try_clone().unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_platpulse-server"))
        .args(["serve", "--config"])
        .arg(&deployment.config)
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut listening = false;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            listening = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        listening,
        "production Server never listened; log:\n{}",
        fs::read_to_string(&log_path).unwrap_or_default()
    );

    // The offline CLI is refused before it opens SQLite. This is the flock
    // ownership guard, not SQLite locking: the issue records that SQLite's
    // own EXCLUSIVE locking is not a substitute for rejecting a second
    // process before SQLite open.
    let refused = deployment.run(&["backup"]);
    assert_refused(&refused, "backup while a production Server runs");

    server.kill().unwrap();
    server.wait().unwrap();

    // Stopped Server: the same CLI command works and releases the guard.
    let succeeded = deployment.run(&["backup"]);
    assert!(
        succeeded.status.success(),
        "backup failed after the Server stopped: {}",
        String::from_utf8_lossy(&succeeded.stderr)
    );
    assert_eq!(fs::read_dir(&deployment.backups).unwrap().count(), 1);
    assert!(
        platpulse_server::ownership::acquire(&deployment.db).is_ok(),
        "the CLI did not release the ownership guard"
    );
}
