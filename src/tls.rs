use anyhow::{Context, Result};
use std::path::Path;
use tokio_postgres_rustls::MakeRustlsConnect;

/// TLS mode for PostgreSQL connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// No TLS (plaintext).
    Disable,
    /// Use TLS if server supports it, fall back to plaintext.
    Prefer,
    /// Require TLS, but don't verify server certificate.
    Require,
}

/// Build a TLS connector based on mode and optional CA cert.
pub fn make_tls_connector(
    mode: TlsMode,
    ca_cert_path: Option<&Path>,
) -> Result<Option<MakeRustlsConnect>> {
    match mode {
        TlsMode::Disable => Ok(None),
        TlsMode::Prefer | TlsMode::Require => {
            let config = if let Some(ca_path) = ca_cert_path {
                let cert_pem = std::fs::read(ca_path)
                    .with_context(|| format!("Failed to read CA cert: {}", ca_path.display()))?;
                let mut reader = std::io::BufReader::new(&cert_pem[..]);
                let certs: Vec<_> =
                    rustls_pemfile::certs(&mut reader).collect::<std::result::Result<_, _>>()?;
                let mut root_store = rustls::RootCertStore::empty();
                for cert in certs {
                    root_store.add(cert)?;
                }
                rustls::ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth()
            } else {
                // Use system CA certificates
                let mut root_store = rustls::RootCertStore::empty();
                let native_certs = rustls_native_certs::load_native_certs();
                for cert in native_certs.certs {
                    let _ = root_store.add(cert);
                }
                rustls::ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth()
            };
            Ok(Some(MakeRustlsConnect::new(config)))
        }
    }
}

/// Parse a TLS mode string from CLI.
pub fn parse_tls_mode(s: &str) -> Result<TlsMode> {
    match s.to_lowercase().as_str() {
        "disable" | "off" | "no" => Ok(TlsMode::Disable),
        "prefer" => Ok(TlsMode::Prefer),
        "require" => Ok(TlsMode::Require),
        other => anyhow::bail!(
            "Unknown TLS mode '{}'. Use: disable, prefer, require",
            other
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tls_mode() {
        assert_eq!(parse_tls_mode("disable").unwrap(), TlsMode::Disable);
        assert_eq!(parse_tls_mode("prefer").unwrap(), TlsMode::Prefer);
        assert_eq!(parse_tls_mode("require").unwrap(), TlsMode::Require);
        assert_eq!(parse_tls_mode("off").unwrap(), TlsMode::Disable);
        assert!(parse_tls_mode("invalid").is_err());
    }

    #[test]
    fn test_disable_returns_none() {
        let connector = make_tls_connector(TlsMode::Disable, None).unwrap();
        assert!(connector.is_none());
    }

    #[test]
    fn test_bad_ca_path_errors() {
        let result = make_tls_connector(TlsMode::Require, Some(Path::new("/nonexistent/ca.pem")));
        assert!(result.is_err());
    }
}
