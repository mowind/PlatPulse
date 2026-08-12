//! Agent configuration (design §8.2).
//!
//! `agent.toml` carries the Server URL and file paths; it never holds
//! plaintext secrets. The Agent Credential lives in its own file (0600,
//! strict permission validation) and is never part of argv, URLs, logs, or
//! errors. Node connection configuration is added by the P1-04 ticket that
//! first needs it; unknown fields are rejected so a half-migrated config
//! can never be silently ignored.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Settings read from `agent.toml`. All fields are required: an Agent must
/// know where its Server, credential file, and state database are before
/// it can enroll or report.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigFile {
    /// Base origin of the Server, e.g. `https://monitor.example.com`.
    pub server_url: String,
    /// Path of the Agent Credential file (created by `enroll`, 0600).
    pub credential_file: PathBuf,
    /// Path of the Agent Store SQLite database.
    pub state_db: PathBuf,
}

/// Fully resolved Agent settings.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Config file the settings were loaded from.
    pub config_path: PathBuf,
    /// Server origin (normalized, no trailing slash).
    pub server_url: String,
    pub credential_file: PathBuf,
    pub state_db: PathBuf,
}

#[derive(Debug, Error)]
pub enum AgentConfigError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid server_url in {path}: {reason}")]
    InvalidServerUrl { path: PathBuf, reason: String },
}

impl AgentConfigFile {
    /// Parse an `agent.toml` file.
    pub fn load(path: &Path) -> Result<Self, AgentConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| AgentConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| AgentConfigError::Parse {
            path: path.to_owned(),
            source,
        })
    }
}

/// Normalize a configured Server URL: absolute `http(s)://host[:port]`
/// with no path, query, fragment, userinfo, or trailing slash. Credentials
/// in a URL are rejected outright — an RPC or Server URL must never carry
/// a secret (design §8.2, §12.6). Plaintext `http://` is only accepted for
/// loopback development endpoints: the Enrollment Token and every later
/// Agent auth exchange would otherwise travel in cleartext, so a
/// non-loopback `http://` URL is refused here (design §12.6/§19.2: Agent
/// auth is TLS-only off loopback).
pub fn normalize_server_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err("must be an absolute http:// or https:// URL".to_owned());
    };
    if !matches!(scheme, "http" | "https") {
        return Err("scheme must be http or https".to_owned());
    }
    if rest.is_empty()
        || rest.contains('/')
        || rest.contains('?')
        || rest.contains('#')
        || rest.contains('@')
    {
        return Err(
            "server URL must be scheme://host[:port] with no path, query, fragment, or credentials"
                .to_owned(),
        );
    }
    if scheme == "http" && !is_loopback_host(rest) {
        return Err(
            "plaintext http is only allowed for loopback development servers; use https otherwise"
                .to_owned(),
        );
    }
    Ok(format!("{scheme}://{rest}"))
}

/// Whether `host[:port]` names a loopback address. Literal checks only —
/// the Agent never resolves DNS for a security decision.
fn is_loopback_host(host_port: &str) -> bool {
    let host = host_port
        .rsplit_once(':')
        .map_or(host_port, |(host, _)| host);
    matches!(
        host.trim_matches(['[', ']']),
        "127.0.0.1" | "::1" | "localhost"
    )
}

impl AgentConfig {
    /// Load and resolve an `agent.toml`.
    pub fn resolve(config_path: &Path) -> Result<Self, AgentConfigError> {
        let file = AgentConfigFile::load(config_path)?;
        let server_url = match normalize_server_url(&file.server_url) {
            Ok(url) => url,
            Err(reason) => {
                return Err(AgentConfigError::InvalidServerUrl {
                    path: config_path.to_owned(),
                    reason,
                });
            }
        };
        Ok(Self {
            config_path: config_path.to_owned(),
            server_url,
            credential_file: file.credential_file,
            state_db: file.state_db,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn write_config(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("agent.toml");
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn full_config_resolves_and_normalizes_the_server_url() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            r#"
server_url = "https://monitor.example.com/"
credential_file = "/var/lib/platpulse-agent/credential"
state_db = "/var/lib/platpulse-agent/agent.db"
"#,
        );
        let config = AgentConfig::resolve(&path).unwrap();
        assert_eq!(config.server_url, "https://monitor.example.com");
        assert_eq!(
            config.credential_file,
            Path::new("/var/lib/platpulse-agent/credential")
        );
        assert_eq!(
            config.state_db,
            Path::new("/var/lib/platpulse-agent/agent.db")
        );
    }

    #[test]
    fn missing_required_fields_fail_parsing() {
        let dir = tempdir().unwrap();
        let path = write_config(dir.path(), "server_url = \"https://example.com\"\n");
        assert!(matches!(
            AgentConfig::resolve(&path).unwrap_err(),
            AgentConfigError::Parse { .. }
        ));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            r#"
server_url = "https://example.com"
credential_file = "/tmp/credential"
state_db = "/tmp/agent.db"
nodes = []
"#,
        );
        assert!(matches!(
            AgentConfig::resolve(&path).unwrap_err(),
            AgentConfigError::Parse { .. }
        ));
    }

    #[test]
    fn server_url_rejects_credentials_paths_and_bad_schemes() {
        for raw in [
            "http://user:pass@example.com",
            "https://example.com/path",
            "https://example.com?q=1",
            "ftp://example.com",
            "example.com",
            "https://",
        ] {
            assert!(
                normalize_server_url(raw).is_err(),
                "{raw} must be rejected as a server URL"
            );
        }
        assert_eq!(
            normalize_server_url(" http://127.0.0.1:8080/ ").unwrap(),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn plaintext_http_is_limited_to_loopback_servers() {
        // Loopback development endpoints may use http (design §19.1).
        for raw in [
            "http://127.0.0.1:8080",
            "http://localhost:4173",
            "http://[::1]:8080",
        ] {
            assert!(normalize_server_url(raw).is_ok(), "{raw} must be accepted");
        }
        // The Enrollment Token and Agent Credential are Bearer secrets;
        // sending them in cleartext to a remote host is refused (design
        // §12.6/§19.2: Agent auth is TLS-only off loopback).
        for raw in [
            "http://monitor.example.com",
            "http://192.168.1.10:8080",
            "http://10.0.0.5",
        ] {
            assert!(
                normalize_server_url(raw).is_err(),
                "{raw} must be rejected: plaintext Agent auth off loopback"
            );
        }
        // https is unrestricted.
        assert!(normalize_server_url("https://monitor.example.com").is_ok());
    }
}
