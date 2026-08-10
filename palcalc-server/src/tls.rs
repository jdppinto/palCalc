//! TLS setup: load a PEM cert/key pair, or generate a self-signed one on first
//! run. The certificate's SHA-256 fingerprint is printed at startup so clients
//! can pin it (self-signed certs won't validate against a public CA).

use std::path::Path;

use axum_server::tls_rustls::RustlsConfig;
use sha2::{Digest, Sha256};

pub async fn load_or_generate(cert_path: &Path, key_path: &Path) -> anyhow::Result<RustlsConfig> {
    let (cert_pem, key_pem) = if cert_path.is_file() && key_path.is_file() {
        (std::fs::read(cert_path)?, std::fs::read(key_path)?)
    } else {
        tracing::warn!(
            "no TLS cert/key at {cert_path:?} / {key_path:?}; generating a self-signed pair"
        );
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .map_err(|e| anyhow::anyhow!("generating self-signed cert: {e}"))?;
        let cert_pem = certified.cert.pem().into_bytes();
        let key_pem = certified.key_pair.serialize_pem().into_bytes();
        // Key first, with restrictive perms, before the cert exists.
        write_private(key_path, &key_pem)?;
        std::fs::write(cert_path, &cert_pem)?;
        tracing::info!("wrote self-signed cert to {cert_path:?} and key to {key_path:?}");
        (cert_pem, key_pem)
    };

    print_fingerprint(&cert_pem);

    RustlsConfig::from_pem(cert_pem, key_pem)
        .await
        .map_err(|e| anyhow::anyhow!("building TLS config: {e}"))
}

/// Write a private file, creating it with restrictive permissions from the
/// start (no world-readable window). On unix the file is created `O_EXCL` with
/// mode 0600; if it already exists we refuse rather than risk writing a key
/// into a pre-existing, possibly loosely-permissioned file.
fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| anyhow::anyhow!("creating private key file {path:?}: {e}"))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

fn print_fingerprint(cert_pem: &[u8]) {
    let mut reader = std::io::BufReader::new(cert_pem);
    // Bind the first item to an owned value so the borrowing iterator is
    // dropped before the match body (the DER is 'static / owned).
    let first = rustls_pemfile::certs(&mut reader).next();
    match first {
        Some(Ok(der)) => {
            let digest = Sha256::digest(&der);
            let hex: Vec<String> = digest.iter().map(|b| format!("{b:02X}")).collect();
            tracing::info!("TLS certificate SHA-256 fingerprint: {}", hex.join(":"));
            tracing::info!("clients should pin this fingerprint (self-signed cert)");
        }
        _ => tracing::warn!("could not read certificate to compute fingerprint"),
    }
}
