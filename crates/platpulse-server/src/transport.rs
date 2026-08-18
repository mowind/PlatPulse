//! Native Rustls transport configuration.
//!
//! The private key crosses this boundary only through a descriptor validated by
//! the Server's sensitive-file policy. Errors intentionally do not include
//! paths, parser details, or key material.

use std::io::Read;

use axum_server::tls_rustls::RustlsConfig;

use crate::config::NativeTlsConfig;

#[derive(thiserror::Error, PartialEq, Eq)]
pub enum NativeTlsError {
    #[error("native TLS certificate-chain material is invalid")]
    CertificateChain,
    #[error("native TLS private-key material is invalid or insecure")]
    PrivateKey,
    #[error("native TLS certificate and private-key material are incompatible")]
    Incompatible,
}

impl std::fmt::Debug for NativeTlsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.to_string())
    }
}

/// Load and validate native HTTPS material before the listener is bound.
///
/// Certificate chains are deployment inputs and may be readable by the
/// service. The private key must pass `open_readonly`, which rejects symlinks,
/// non-regular files, non-owned files, and any group/world permissions.
pub async fn load_rustls_config(tls: &NativeTlsConfig) -> Result<RustlsConfig, NativeTlsError> {
    // Reqwest selects ring for its own clients, but rustls 0.23 requires an
    // explicit process provider for the Server-side certificate parser.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certificate_chain =
        std::fs::read(&tls.cert_chain_file).map_err(|_| NativeTlsError::CertificateChain)?;
    let mut private_key_file = crate::file_security::open_readonly(&tls.private_key_file)
        .map_err(|_| NativeTlsError::PrivateKey)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if private_key_file
            .metadata()
            .map_err(|_| NativeTlsError::PrivateKey)?
            .mode()
            & 0o7777
            != 0o600
        {
            return Err(NativeTlsError::PrivateKey);
        }
    }
    let mut private_key = Vec::new();
    private_key_file
        .read_to_end(&mut private_key)
        .map_err(|_| NativeTlsError::PrivateKey)?;

    RustlsConfig::from_pem(certificate_chain, private_key)
        .await
        .map_err(|_| NativeTlsError::Incompatible)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn insecure_private_key_is_rejected_without_disclosing_path() {
        let directory = tempdir().unwrap();
        let cert = directory.path().join("chain.pem");
        let key = directory.path().join("private-key.pem");
        std::fs::write(&cert, b"not a certificate").unwrap();
        std::fs::write(&key, b"not a key").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o400)).unwrap();

        let error = load_rustls_config(&NativeTlsConfig {
            cert_chain_file: cert,
            private_key_file: key,
        })
        .await
        .unwrap_err();

        assert_eq!(error, NativeTlsError::PrivateKey);
        assert_eq!(
            error.to_string(),
            "native TLS private-key material is invalid or insecure"
        );
    }

    #[tokio::test]
    async fn symlinked_private_key_is_rejected() {
        let directory = tempdir().unwrap();
        let cert = directory.path().join("chain.pem");
        let key = directory.path().join("private-key.pem");
        let target = directory.path().join("real-key.pem");
        std::fs::write(&cert, b"not a certificate").unwrap();
        std::fs::write(&target, b"not a key").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, &key).unwrap();

        let error = load_rustls_config(&NativeTlsConfig {
            cert_chain_file: cert,
            private_key_file: key,
        })
        .await
        .unwrap_err();

        assert_eq!(error, NativeTlsError::PrivateKey);
    }
}
