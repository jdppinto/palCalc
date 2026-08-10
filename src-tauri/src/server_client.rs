//! Pinned HTTPS client for palCalc "server mode".
//!
//! palcalc-server serves TLS with a self-signed cert at an arbitrary
//! port-forwarded host, so ordinary CA/hostname validation can't authenticate
//! it. We PIN the certificate by its SHA-256 fingerprint (shared out-of-band)
//! and STILL verify the handshake signature — so an on-path attacker can't
//! replay the public cert without the private key. This mirrors
//! `palcalc-server/src/pin.rs`; see `palcalc-server/docs/CLIENT-PINNING.md`.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as TlsError, SignatureScheme};
use sha2::{Digest, Sha256};

#[derive(Debug)]
struct PinnedVerifier {
    pinned: [u8; 32],
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let got = Sha256::digest(end_entity.as_ref());
        if got.as_slice() == self.pinned {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::General(
                "server certificate fingerprint does not match the pinned value".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

fn parse_fingerprint(s: &str) -> Result<[u8; 32], String> {
    let hex: String = s.chars().filter(|c| !c.is_whitespace() && *c != ':').collect();
    // Guard non-ASCII before byte-slicing below (a stray multi-byte char from a
    // copy-paste would otherwise panic on a char boundary).
    if !hex.is_ascii() {
        return Err("fingerprint contains non-hex characters".into());
    }
    if hex.len() != 64 {
        return Err(format!(
            "fingerprint must be 32 bytes (64 hex chars), got {}",
            hex.len()
        ));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| "fingerprint contains non-hex characters".to_string())?;
    }
    Ok(out)
}

fn pinned_client(fingerprint: &str) -> Result<reqwest::Client, String> {
    let pinned = parse_fingerprint(fingerprint)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let algs = provider.signature_verification_algorithms;
    let verifier = Arc::new(PinnedVerifier { pinned, algs });

    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(rustls::DEFAULT_VERSIONS)
        .map_err(|e| format!("tls config: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .https_only(true)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("building client: {e}"))
}

/// Fetch the roster JSON from a pinned palcalc-server. Returns the raw response
/// body on success; the caller (frontend) parses it.
pub async fn fetch_roster(url: &str, token: &str, fingerprint: &str) -> Result<String, String> {
    // Refuse anything that isn't HTTPS up front (defense in depth; the client
    // also enforces https_only).
    if !url.trim_start().to_ascii_lowercase().starts_with("https://") {
        return Err("server URL must start with https://".into());
    }
    let client = pinned_client(fingerprint)?;
    let resp = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("server returned HTTP {status}"));
    }
    resp.text().await.map_err(|e| format!("reading response: {e}"))
}
