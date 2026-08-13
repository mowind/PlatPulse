//! Server configuration (design §18.1).
//!
//! `server.toml` carries paths and policy; it never holds plaintext
//! passwords or token contents. Every setting can be overridden from the
//! CLI, and the resolved configuration is the single source of truth for
//! startup, `init`, and `owner create`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ipnet::IpNet;

use serde::Deserialize;
use thiserror::Error;

/// Default listen address when neither the config file nor CLI flags set one.
pub const DEFAULT_LISTEN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8080);

/// SQLite file name relative to the state directory.
pub const DEFAULT_DB_FILE: &str = "platpulse.db";

/// Pepper file name relative to the state directory when the config file
/// does not point at a dedicated secrets path (production deployments put
/// it under `/etc/platpulse/secrets/`, design §18.1).
pub const DEFAULT_PEPPER_FILE: &str = "server-pepper";

/// Settings read from `server.toml`. All fields are optional: the CLI and
/// built-in defaults fill the gaps, so a minimal local config only needs
/// the paths `init` must create.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfigFile {
    /// Directory that holds Server state (the SQLite file by default).
    pub state_dir: Option<PathBuf>,
    /// SQLite database file; defaults to `<state_dir>/platpulse.db`.
    pub db_path: Option<PathBuf>,
    /// Standalone pepper secret file; defaults to `<state_dir>/server-pepper`.
    pub pepper_file: Option<PathBuf>,
    /// Built WebUI root (`index.html` plus `assets/`); optional.
    pub web_root: Option<PathBuf>,
    /// Address the HTTP listener binds to; defaults to `127.0.0.1:8080`.
    pub listen: Option<SocketAddr>,
    /// Origin the Server validates login requests against, e.g.
    /// `https://platpulse.example.com`. Defaults to `http://<listen>`.
    pub public_base_url: Option<String>,
    /// Explicit development mode: uses a separate non-`__Host-` cookie
    /// without `Secure` (design §19.1). Production cookies always use
    /// `__Host-platpulse_session` with `Secure`.
    pub development: Option<bool>,
    /// Explicitly trusted reverse-proxy source CIDRs. Empty means no proxy
    /// headers are trusted and non-loopback plaintext remains refused.
    pub trusted_proxy_cidrs: Option<Vec<String>>,
    /// Scheme asserted by a configured trusted proxy (`http` or `https`).
    pub trusted_proxy_scheme: Option<String>,
}

/// Per-setting overrides from the `serve` CLI flags.
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub db_path: Option<PathBuf>,
    pub pepper_file: Option<PathBuf>,
    pub web_root: Option<PathBuf>,
    pub listen: Option<SocketAddr>,
    pub base_url: Option<String>,
    pub development: bool,
}

/// Fully resolved Server settings.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Config file the settings were loaded from, when one was used.
    pub config_path: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub db_path: PathBuf,
    pub pepper_file: PathBuf,
    pub web_root: Option<PathBuf>,
    pub listen: SocketAddr,
    /// Exact origin used for strict login validation and cookie policy.
    pub public_base_url: String,
    pub development: bool,
    pub trusted_proxy_cidrs: Vec<IpNet>,
    pub trusted_proxy_scheme: Option<String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
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
    #[error("state_dir is required in {path} for `init`")]
    MissingStateDir { path: PathBuf },
    #[error("invalid public_base_url in {path}: {reason}")]
    InvalidBaseUrl { path: PathBuf, reason: String },
    #[error("invalid trusted proxy CIDR: {0}")]
    InvalidTrustedProxyCidr(String),
    #[error("invalid trusted proxy scheme: {0}")]
    InvalidTrustedProxyScheme(String),
}

impl ServerConfigFile {
    /// Parse a `server.toml` file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })
    }
}

/// Normalize a configured origin: `http(s)://host[:port]` with no path,
/// query, fragment, or credentials, and no trailing slash.
pub fn normalize_origin(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err("must be an absolute http:// or https:// origin".to_owned());
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
            "origin must be scheme://host[:port] with no path, query, fragment, or credentials"
                .to_owned(),
        );
    }
    Ok(format!("{scheme}://{rest}"))
}

impl ServerConfig {
    /// Resolve CLI overrides on top of an optional config file.
    pub fn resolve(config_path: Option<&Path>, cli: &CliOverrides) -> Result<Self, ConfigError> {
        let file = match config_path {
            Some(path) => Some(ServerConfigFile::load(path)?),
            None => None,
        };
        Self::resolve_from(config_path, file.as_ref(), cli, None)
    }

    /// Resolve settings for `init`: the config file must name an explicit
    /// `state_dir`, because `init` creates that directory and a silent `.`
    /// fallback would scatter state into the working directory.
    pub fn resolve_init(config_path: &Path) -> Result<Self, ConfigError> {
        let file = ServerConfigFile::load(config_path)?;
        let state_dir = file
            .state_dir
            .clone()
            .ok_or_else(|| ConfigError::MissingStateDir {
                path: config_path.to_owned(),
            })?;
        Self::resolve_from(
            Some(config_path),
            Some(&file),
            &CliOverrides::default(),
            Some(state_dir),
        )
    }

    fn resolve_from(
        config_path: Option<&Path>,
        file: Option<&ServerConfigFile>,
        cli: &CliOverrides,
        pinned_state_dir: Option<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let state_dir = pinned_state_dir
            .or_else(|| file.and_then(|file| file.state_dir.clone()))
            .unwrap_or_else(|| PathBuf::from("."));
        let db_path = cli
            .db_path
            .clone()
            .or_else(|| file.and_then(|file| file.db_path.clone()))
            .unwrap_or_else(|| state_dir.join(DEFAULT_DB_FILE));
        let pepper_file = cli
            .pepper_file
            .clone()
            .or_else(|| file.and_then(|file| file.pepper_file.clone()))
            .unwrap_or_else(|| state_dir.join(DEFAULT_PEPPER_FILE));
        let web_root = cli
            .web_root
            .clone()
            .or_else(|| file.and_then(|file| file.web_root.clone()));
        let listen = cli
            .listen
            .or_else(|| file.and_then(|file| file.listen))
            .unwrap_or(DEFAULT_LISTEN);

        let raw_base_url = cli
            .base_url
            .clone()
            .or_else(|| file.and_then(|file| file.public_base_url.clone()))
            .unwrap_or_else(|| format!("http://{listen}"));
        let public_base_url =
            normalize_origin(&raw_base_url).map_err(|reason| ConfigError::InvalidBaseUrl {
                path: config_path
                    .map(Path::to_owned)
                    .unwrap_or_else(|| PathBuf::from("<cli>")),
                reason,
            })?;

        let development =
            cli.development || file.and_then(|file| file.development).unwrap_or(false);
        let trusted_proxy_cidrs = file
            .and_then(|value| value.trusted_proxy_cidrs.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| ConfigError::InvalidTrustedProxyCidr(value))
            })
            .collect::<Result<Vec<IpNet>, _>>()?;
        let trusted_proxy_scheme = file
            .and_then(|value| value.trusted_proxy_scheme.clone())
            .map(|value| value.to_ascii_lowercase());
        if let Some(scheme) = &trusted_proxy_scheme {
            if !matches!(scheme.as_str(), "http" | "https") {
                return Err(ConfigError::InvalidTrustedProxyScheme(scheme.clone()));
            }
        }

        Ok(Self {
            config_path: config_path.map(Path::to_owned),
            state_dir,
            db_path,
            pepper_file,
            web_root,
            listen,
            public_base_url,
            development,
            trusted_proxy_cidrs,
            trusted_proxy_scheme,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn write_config(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("server.toml");
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn full_config_resolves_with_cli_override() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            r#"
state_dir = "/var/lib/platpulse"
db_path = "/var/lib/platpulse/platpulse.db"
pepper_file = "/etc/platpulse/secrets/server-pepper"
web_root = "/usr/share/platpulse/web"
listen = "127.0.0.1:8443"
public_base_url = "https://platpulse.example.com"
development = false
"#,
        );
        let config = ServerConfig::resolve(
            Some(&path),
            &CliOverrides {
                db_path: Some("/tmp/override.db".into()),
                ..CliOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(config.state_dir, Path::new("/var/lib/platpulse"));
        assert_eq!(config.db_path, Path::new("/tmp/override.db"));
        assert_eq!(
            config.pepper_file,
            Path::new("/etc/platpulse/secrets/server-pepper")
        );
        assert_eq!(
            config.web_root.as_deref(),
            Some(Path::new("/usr/share/platpulse/web"))
        );
        assert_eq!(config.listen, "127.0.0.1:8443".parse().unwrap());
        assert_eq!(config.public_base_url, "https://platpulse.example.com");
        assert!(!config.development);
    }

    #[test]
    fn minimal_config_uses_state_dir_defaults() {
        let dir = tempdir().unwrap();
        let path = write_config(dir.path(), "state_dir = \"/srv/platpulse\"\n");
        let config = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap();
        assert_eq!(config.db_path, Path::new("/srv/platpulse/platpulse.db"));
        assert_eq!(
            config.pepper_file,
            Path::new("/srv/platpulse/server-pepper")
        );
        assert_eq!(config.listen, DEFAULT_LISTEN);
        assert_eq!(config.public_base_url, "http://127.0.0.1:8080");
        assert!(!config.development);
    }

    #[test]
    fn trusted_proxy_requires_explicit_cidr_and_scheme() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\nlisten = \"0.0.0.0:8080\"\ntrusted_proxy_cidrs = [\"10.0.0.0/8\"]\ntrusted_proxy_scheme = \"https\"\n",
        );
        let config = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap();
        assert_eq!(config.trusted_proxy_cidrs.len(), 1);
        assert_eq!(config.trusted_proxy_scheme.as_deref(), Some("https"));
        assert!(
            crate::validate_listen_address_with_proxy(
                config.listen,
                &config.trusted_proxy_cidrs,
                config.trusted_proxy_scheme.as_deref()
            )
            .is_ok()
        );
    }
    #[test]
    fn cli_base_url_and_dev_flag_override_config() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\npublic_base_url = \"https://prod.example.com\"\n",
        );
        let config = ServerConfig::resolve(
            Some(&path),
            &CliOverrides {
                base_url: Some("http://127.0.0.1:4173/".into()),
                development: true,
                ..CliOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(config.public_base_url, "http://127.0.0.1:4173");
        assert!(config.development);
    }

    #[test]
    fn without_config_legacy_flags_resolve_with_working_directory_defaults() {
        let config = ServerConfig::resolve(
            None,
            &CliOverrides {
                db_path: Some("custom.db".into()),
                ..CliOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(config.db_path, Path::new("custom.db"));
        assert_eq!(config.pepper_file, Path::new("./server-pepper"));
        assert_eq!(config.state_dir, Path::new("."));
    }

    #[test]
    fn init_requires_state_dir() {
        let dir = tempdir().unwrap();
        let path = write_config(dir.path(), "db_path = \"/tmp/x.db\"\n");
        let error = ServerConfig::resolve_init(&path).unwrap_err();
        assert!(matches!(error, ConfigError::MissingStateDir { .. }));
    }

    #[test]
    fn init_anchors_defaults_to_state_dir() {
        let dir = tempdir().unwrap();
        let path = write_config(dir.path(), "state_dir = \"/srv/platpulse\"\n");
        let config = ServerConfig::resolve_init(&path).unwrap();
        assert_eq!(config.state_dir, Path::new("/srv/platpulse"));
        assert_eq!(config.db_path, Path::new("/srv/platpulse/platpulse.db"));
        assert_eq!(
            config.pepper_file,
            Path::new("/srv/platpulse/server-pepper")
        );
    }

    #[test]
    fn origin_normalization_rejects_paths_and_queries() {
        for (raw, expected) in [
            (
                "https://platpulse.example.com",
                "https://platpulse.example.com",
            ),
            (
                "https://platpulse.example.com/",
                "https://platpulse.example.com",
            ),
            (" http://127.0.0.1:8080 ", "http://127.0.0.1:8080"),
        ] {
            assert_eq!(normalize_origin(raw).unwrap(), expected);
        }
        for raw in [
            "https://platpulse.example.com/path",
            "http://example.com?q=1",
            "ftp://example.com",
            "example.com",
            "http://user@example.com",
            "http://",
        ] {
            assert!(
                normalize_origin(raw).is_err(),
                "{raw} must be rejected as an origin"
            );
        }
    }

    #[test]
    fn unknown_config_fields_are_rejected() {
        let dir = tempdir().unwrap();
        let path = write_config(dir.path(), "state_dir = \"/srv/x\"\nunknown_key = 1\n");
        let error = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }));
    }
}
