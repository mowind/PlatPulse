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

/// Default loopback address for the disabled-by-default metrics listener.
pub const DEFAULT_METRICS_LISTEN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 9090);

/// SQLite file name relative to the state directory.
pub const DEFAULT_DB_FILE: &str = "platpulse.db";

/// Pepper file name relative to the state directory when the config file
/// does not point at a dedicated secrets path (production deployments put
/// it under `/etc/platpulse/secrets/`, design §18.1).
pub const DEFAULT_PEPPER_FILE: &str = "server-pepper";

/// Default installed WebUI root for release packages.
pub const DEFAULT_WEB_ROOT: &str = "/usr/share/platpulse/web";

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
    /// Built WebUI root (`index.html` plus `assets/`); defaults to the
    /// release installation directory when not specified.
    pub web_root: Option<PathBuf>,
    /// Dedicated backup directory for Admin-triggered backup artifacts
    /// (design §20.1). Must never point at the Server state directory;
    /// when absent the backup surface reports NotConfigured.
    pub backup_dir: Option<PathBuf>,
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
    /// Explicit native Rustls HTTPS configuration. When present, both files
    /// are required and the Server starts only after the private key passes
    /// the strict sensitive-file checks.
    pub tls: Option<TlsSectionFile>,
    /// Optional GeoLite2 Country database configuration. The Server never
    /// downloads the file or stores MaxMind credentials.
    pub geo: Option<GeoSectionFile>,
    /// Optional Server-side Validator Provider configuration.
    pub validator_provider: Option<ValidatorProviderSectionFile>,
    /// Notification channel configuration (design §17.4). The TOML carries
    /// only the token file reference and the destination, never token
    /// contents.
    pub notifications: Option<NotificationsSectionFile>,
    /// Optional dedicated internal Prometheus metrics listener.
    pub metrics: Option<MetricsSectionFile>,
}

/// The optional `[metrics]` section. Presence enables metrics unless
/// explicitly disabled; its default address is loopback-only.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MetricsSectionFile {
    pub enabled: Option<bool>,
    pub listen: Option<SocketAddr>,
}

/// The optional `[geo]` section. Only an operator-provided database path is
/// accepted; credentials and downloader settings are intentionally absent.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GeoSectionFile {
    pub mmdb_path: Option<PathBuf>,
}

/// `[tls]` enables direct native Rustls HTTPS. Certificate automation and
/// in-process reload are intentionally outside the Server configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TlsSectionFile {
    /// PEM certificate chain presented by the Server.
    pub cert_chain_file: Option<PathBuf>,
    /// PEM private key. This path is validated as a private regular file.
    pub private_key_file: Option<PathBuf>,
}

/// `[validator_provider]` configures the Server-side Explorer adapter.
/// Provider credentials are intentionally not part of this section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ValidatorProviderSectionFile {
    pub base_url: Option<String>,
    pub refresh_seconds: Option<u64>,
    pub timeout_seconds: Option<u64>,
    /// IANA timezone used for Validator daily and calendar-month buckets.
    pub timezone: Option<String>,
}

/// `[notifications.telegram]`: the approved Telegram delivery path. The
/// provider token lives in a dedicated secret file referenced by
/// `token_file`; this file never holds the token itself (design §18.1).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TelegramChannelFile {
    pub enabled: Option<bool>,
    pub token_file: Option<PathBuf>,
    pub chat_id: Option<String>,
    /// Maximum automatic attempts (including the first) before a Delivery
    /// reaches DeadLetter; defaults to 5.
    pub max_attempts: Option<u32>,
    /// Exponential backoff base in seconds; defaults to 60.
    pub retry_base_seconds: Option<u32>,
}

/// The `[notifications]` section of `server.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NotificationsSectionFile {
    pub telegram: Option<TelegramChannelFile>,
}

/// Per-setting overrides from the `serve` CLI flags.
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub db_path: Option<PathBuf>,
    pub pepper_file: Option<PathBuf>,
    pub web_root: Option<PathBuf>,
    pub backup_dir: Option<PathBuf>,
    pub geo_mmdb: Option<PathBuf>,
    pub listen: Option<SocketAddr>,
    pub base_url: Option<String>,
    pub development: bool,
    pub tls_cert_chain_file: Option<PathBuf>,
    pub tls_private_key_file: Option<PathBuf>,
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
    pub backup_dir: Option<PathBuf>,
    pub listen: SocketAddr,
    /// Exact origin used for strict login validation and cookie policy.
    pub public_base_url: String,
    pub development: bool,
    /// Native Rustls HTTPS files, when configured.
    pub tls: Option<NativeTlsConfig>,
    pub trusted_proxy_cidrs: Vec<IpNet>,
    pub trusted_proxy_scheme: Option<String>,
    /// Resolved optional GeoLite2 Country database path.
    pub geo: Option<PathBuf>,
    /// Resolved optional Validator Provider settings.
    pub validator_provider: Option<ValidatorProviderConfig>,
    /// Resolved notification channels (design §17.4).
    pub notifications: NotificationChannels,
    /// Dedicated internal metrics listener policy.
    pub metrics: MetricsConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricsConfig {
    pub enabled: bool,
    pub listen: SocketAddr,
}

#[derive(Debug, Clone)]
pub struct NativeTlsConfig {
    pub cert_chain_file: PathBuf,
    pub private_key_file: PathBuf,
}
#[derive(Debug, Clone)]
pub struct ValidatorProviderConfig {
    pub base_url: String,
    pub refresh_seconds: u64,
    pub timeout_seconds: u64,
    pub timezone: String,
}

/// Resolved Telegram channel policy. Present only when the channel is
/// configured in `server.toml`.
#[derive(Debug, Clone)]
pub struct TelegramChannel {
    pub enabled: bool,
    /// Path to the secret file holding the Bot API token. The WebUI, logs,
    /// Audit rows, and DTOs only ever see a redacted reference to this path.
    pub token_file: PathBuf,
    /// Destination chat identifier; DTOs only ever see a redacted summary.
    pub chat_id: String,
    pub max_attempts: u32,
    pub retry_base_seconds: u32,
}

/// Resolved notification channels. A channel is present only when it is
/// configured in `server.toml`; unconfigured channels create no
/// Deliveries. Provider tokens never enter the WebUI, logs, or Audit.
#[derive(Debug, Clone, Default)]
pub struct NotificationChannels {
    pub telegram: Option<TelegramChannel>,
}

impl NotificationChannels {
    /// The configured Telegram channel, if any.
    pub fn telegram(&self) -> Option<&TelegramChannel> {
        self.telegram.as_ref()
    }
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
    #[error("invalid native TLS configuration in {path}: {reason}")]
    InvalidTlsConfiguration { path: PathBuf, reason: String },
    #[error("invalid Validator analytics IANA timezone in {path}: {timezone}")]
    InvalidValidatorTimezone { path: PathBuf, timezone: String },
    #[error("notifications.telegram.token_file is required in {path}")]
    MissingTelegramTokenFile { path: PathBuf },
    #[error("notifications.telegram.chat_id is required in {path}")]
    MissingTelegramChatId { path: PathBuf },
    #[error("invalid notification policy in {path}: {reason}")]
    InvalidNotificationPolicy { path: PathBuf, reason: String },
    #[error("backup_dir must not point at the Server state directory in {path} (design §20.1)")]
    InvalidBackupDir { path: PathBuf },
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
        let web_root = Some(
            cli.web_root
                .clone()
                .or_else(|| file.and_then(|file| file.web_root.clone()))
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WEB_ROOT)),
        );
        let backup_dir = cli
            .backup_dir
            .clone()
            .or_else(|| file.and_then(|file| file.backup_dir.clone()));
        if let Some(dir) = &backup_dir {
            if dir == &state_dir {
                return Err(ConfigError::InvalidBackupDir {
                    path: config_path
                        .map(Path::to_owned)
                        .unwrap_or_else(|| PathBuf::from("<cli>")),
                });
            }
        }
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
        let tls = resolve_tls(file, cli, config_path, development, &public_base_url)?;
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

        let config_path = config_path.map(Path::to_owned);
        let geo = cli.geo_mmdb.clone().or_else(|| resolve_geo_path(file));
        let validator_provider = resolve_validator_provider(file, config_path.as_deref())?;
        let notifications = resolve_notification_channels(file, &config_path)?;
        let metrics_section = file.and_then(|value| value.metrics.as_ref());
        let metrics = MetricsConfig {
            enabled: metrics_section
                .and_then(|section| section.enabled)
                .unwrap_or(metrics_section.is_some()),
            listen: metrics_section
                .and_then(|section| section.listen)
                .unwrap_or(DEFAULT_METRICS_LISTEN),
        };

        Ok(Self {
            config_path,
            state_dir,
            db_path,
            pepper_file,
            web_root,
            backup_dir,
            listen,
            public_base_url,
            development,
            tls,
            trusted_proxy_cidrs,
            trusted_proxy_scheme,
            geo,
            validator_provider,
            notifications,
            metrics,
        })
    }
}

fn resolve_tls(
    file: Option<&ServerConfigFile>,
    cli: &CliOverrides,
    config_path: Option<&Path>,
    development: bool,
    public_base_url: &str,
) -> Result<Option<NativeTlsConfig>, ConfigError> {
    let section = file.and_then(|value| value.tls.as_ref());
    let cert_chain_file = cli
        .tls_cert_chain_file
        .clone()
        .or_else(|| section.and_then(|value| value.cert_chain_file.clone()));
    let private_key_file = cli
        .tls_private_key_file
        .clone()
        .or_else(|| section.and_then(|value| value.private_key_file.clone()));
    if cert_chain_file.is_none() && private_key_file.is_none() {
        if section.is_some() {
            return Err(ConfigError::InvalidTlsConfiguration {
                path: config_path
                    .map(Path::to_owned)
                    .unwrap_or_else(|| PathBuf::from("<cli>")),
                reason: "cert_chain_file and private_key_file must be configured together"
                    .to_owned(),
            });
        }
        return Ok(None);
    }
    let path = config_path
        .map(Path::to_owned)
        .unwrap_or_else(|| PathBuf::from("<cli>"));
    if development {
        return Err(ConfigError::InvalidTlsConfiguration {
            path,
            reason: "native TLS cannot be combined with development mode".to_owned(),
        });
    }
    if !public_base_url.starts_with("https://") {
        return Err(ConfigError::InvalidTlsConfiguration {
            path,
            reason: "native TLS requires an https public_base_url".to_owned(),
        });
    }
    let (Some(cert_chain_file), Some(private_key_file)) = (cert_chain_file, private_key_file)
    else {
        return Err(ConfigError::InvalidTlsConfiguration {
            path,
            reason: "cert_chain_file and private_key_file must be configured together".to_owned(),
        });
    };
    Ok(Some(NativeTlsConfig {
        cert_chain_file,
        private_key_file,
    }))
}
fn resolve_validator_provider(
    file: Option<&ServerConfigFile>,
    config_path: Option<&Path>,
) -> Result<Option<ValidatorProviderConfig>, ConfigError> {
    let Some(section) = file.and_then(|value| value.validator_provider.as_ref()) else {
        return Ok(None);
    };
    let Some(base_url) = section.base_url.clone() else {
        return Ok(None);
    };
    let timezone = section.timezone.clone().unwrap_or_else(|| "UTC".to_owned());
    if timezone.parse::<chrono_tz::Tz>().is_err() {
        return Err(ConfigError::InvalidValidatorTimezone {
            path: config_path
                .map(Path::to_owned)
                .unwrap_or_else(|| PathBuf::from("<config>")),
            timezone,
        });
    }
    Ok(Some(ValidatorProviderConfig {
        base_url,
        refresh_seconds: section.refresh_seconds.unwrap_or(60).clamp(1, 86_400),
        timeout_seconds: section.timeout_seconds.unwrap_or(10).clamp(1, 300),
        timezone,
    }))
}

fn resolve_geo_path(file: Option<&ServerConfigFile>) -> Option<PathBuf> {
    file.and_then(|value| value.geo.as_ref())
        .and_then(|section| section.mmdb_path.clone())
}

fn resolve_notification_channels(
    file: Option<&ServerConfigFile>,
    config_path: &Option<PathBuf>,
) -> Result<NotificationChannels, ConfigError> {
    let Some(section) = file.and_then(|value| value.notifications.as_ref()) else {
        return Ok(NotificationChannels::default());
    };
    let Some(telegram) = section.telegram.as_ref() else {
        return Ok(NotificationChannels::default());
    };
    let path = config_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("<cli>"));
    let token_file = telegram
        .token_file
        .clone()
        .ok_or_else(|| ConfigError::MissingTelegramTokenFile { path: path.clone() })?;
    let chat_id = telegram
        .chat_id
        .clone()
        .ok_or_else(|| ConfigError::MissingTelegramChatId { path: path.clone() })?;
    if chat_id.trim().is_empty() {
        return Err(ConfigError::InvalidNotificationPolicy {
            path,
            reason: "notifications.telegram.chat_id must not be empty".to_owned(),
        });
    }
    let max_attempts = telegram.max_attempts.unwrap_or(5);
    if !(1..=20).contains(&max_attempts) {
        return Err(ConfigError::InvalidNotificationPolicy {
            path: path.clone(),
            reason: "notifications.telegram.max_attempts must be between 1 and 20".to_owned(),
        });
    }
    let retry_base_seconds = telegram.retry_base_seconds.unwrap_or(60);
    if !(1..=3600).contains(&retry_base_seconds) {
        return Err(ConfigError::InvalidNotificationPolicy {
            path: path.clone(),
            reason: "notifications.telegram.retry_base_seconds must be between 1 and 3600"
                .to_owned(),
        });
    }
    Ok(NotificationChannels {
        telegram: Some(TelegramChannel {
            enabled: telegram.enabled.unwrap_or(true),
            token_file,
            chat_id,
            max_attempts,
            retry_base_seconds,
        }),
    })
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
    fn geo_path_is_optional_and_cli_override_wins() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\n[geo]\nmmdb_path = \"/etc/platpulse/GeoLite2-Country.mmdb\"\n",
        );
        let config = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap();
        assert_eq!(
            config.geo,
            Some(PathBuf::from("/etc/platpulse/GeoLite2-Country.mmdb"))
        );
        let config = ServerConfig::resolve(
            Some(&path),
            &CliOverrides {
                geo_mmdb: Some(PathBuf::from("/tmp/test.mmdb")),
                ..CliOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(config.geo, Some(PathBuf::from("/tmp/test.mmdb")));
    }
    #[test]
    fn metrics_are_disabled_without_a_metrics_section() {
        let dir = tempdir().unwrap();
        let path = write_config(dir.path(), "state_dir = \"/srv/platpulse\"\n");
        let config = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap();
        assert!(!config.metrics.enabled);
        assert_eq!(config.metrics.listen, DEFAULT_METRICS_LISTEN);
    }

    #[test]
    fn metrics_section_defaults_to_loopback_and_can_be_enabled() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\n[metrics]\nenabled = true\n",
        );
        let config = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap();
        assert!(config.metrics.enabled);
        assert!(config.metrics.listen.ip().is_loopback());
    }

    #[test]
    fn metrics_non_loopback_requires_transport_policy() {
        let addr = "0.0.0.0:9090".parse().unwrap();
        assert!(crate::validate_listen_address_with_transport(addr, false, &[], None).is_err());
        assert!(crate::validate_listen_address_with_transport(addr, true, &[], None).is_ok());
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
        assert_eq!(config.web_root, Some(PathBuf::from(DEFAULT_WEB_ROOT)));
        assert!(!config.development);
    }

    #[test]
    fn web_root_cli_override_beats_config_and_default() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\nweb_root = \"/config/web\"\n",
        );
        let config = ServerConfig::resolve(
            Some(&path),
            &CliOverrides {
                web_root: Some(PathBuf::from("/cli/web")),
                ..CliOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(config.web_root, Some(PathBuf::from("/cli/web")));
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
    fn notification_telegram_section_resolves_with_defaults_and_validates() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\n[notifications.telegram]\ntoken_file = \"/etc/platpulse/secrets/telegram-token\"\nchat_id = \"123456789\"\n",
        );
        let config = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap();
        let telegram = config
            .notifications
            .telegram()
            .expect("telegram configured");
        assert!(telegram.enabled);
        assert_eq!(
            telegram.token_file,
            Path::new("/etc/platpulse/secrets/telegram-token")
        );
        assert_eq!(telegram.chat_id, "123456789");
        assert_eq!(telegram.max_attempts, 5);
        assert_eq!(telegram.retry_base_seconds, 60);

        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\n[notifications.telegram]\nchat_id = \"1\"\nmax_attempts = 2\nretry_base_seconds = 10\n",
        );
        let error = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::MissingTelegramTokenFile { .. }
        ));

        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\n[notifications.telegram]\ntoken_file = \"/tmp/t\"\nchat_id = \"1\"\nmax_attempts = 0\n",
        );
        let error = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::InvalidNotificationPolicy { .. }
        ));
    }

    #[test]
    fn native_tls_config_requires_both_files_and_is_separate_from_development() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\npublic_base_url = \"https://platpulse.example.com\"\n[tls]\ncert_chain_file = \"/etc/platpulse/tls/fullchain.pem\"\nprivate_key_file = \"/etc/platpulse/tls/privkey.pem\"\n",
        );
        let config = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap();
        let tls = config.tls.unwrap();
        assert_eq!(
            tls.cert_chain_file,
            Path::new("/etc/platpulse/tls/fullchain.pem")
        );
        assert_eq!(
            tls.private_key_file,
            Path::new("/etc/platpulse/tls/privkey.pem")
        );

        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\npublic_base_url = \"https://platpulse.example.com\"\n[tls]\ncert_chain_file = \"/etc/platpulse/tls/fullchain.pem\"\n",
        );
        assert!(matches!(
            ServerConfig::resolve(Some(&path), &CliOverrides::default()),
            Err(ConfigError::InvalidTlsConfiguration { .. })
        ));

        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\ndevelopment = true\n[tls]\ncert_chain_file = \"chain.pem\"\nprivate_key_file = \"key.pem\"\n",
        );
        assert!(matches!(
            ServerConfig::resolve(Some(&path), &CliOverrides::default()),
            Err(ConfigError::InvalidTlsConfiguration { .. })
        ));
    }

    #[test]
    fn native_tls_cli_overrides_config_files() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "state_dir = \"/srv/platpulse\"\npublic_base_url = \"https://platpulse.example.com\"\n[tls]\ncert_chain_file = \"config-chain.pem\"\nprivate_key_file = \"config-key.pem\"\n",
        );
        let config = ServerConfig::resolve(
            Some(&path),
            &CliOverrides {
                tls_cert_chain_file: Some(PathBuf::from("cli-chain.pem")),
                tls_private_key_file: Some(PathBuf::from("cli-key.pem")),
                ..CliOverrides::default()
            },
        )
        .unwrap();
        let tls = config.tls.unwrap();
        assert_eq!(tls.cert_chain_file, Path::new("cli-chain.pem"));
        assert_eq!(tls.private_key_file, Path::new("cli-key.pem"));
    }
    #[test]
    fn unknown_config_fields_are_rejected() {
        let dir = tempdir().unwrap();
        let path = write_config(dir.path(), "state_dir = \"/srv/x\"\nunknown_key = 1\n");
        let error = ServerConfig::resolve(Some(&path), &CliOverrides::default()).unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }));
    }
}
