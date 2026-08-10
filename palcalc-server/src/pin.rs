//! Certificate-pinning HTTPS client.
//!
//! palcalc-server uses a self-signed certificate, so ordinary CA validation
//! can't authenticate it — and the server may be reached at an arbitrary
//! host/IP whose name won't match the cert's SANs. Instead the client PINS the
//! server's certificate by its SHA-256 fingerprint (shared out-of-band by the
//! operator, printed at server startup).
//!
//! Security model — this is only MITM-safe because BOTH hold:
//!   1. The presented leaf certificate's SHA-256 must equal the pinned value
//!      (so we trust exactly one certificate, not "any self-signed cert").
//!   2. The TLS handshake signature is STILL verified against that certificate
//!      (so an on-path attacker who merely replays the public certificate —
//!      it's not secret — cannot complete the handshake without the private
//!      key). Skipping (2) would make pinning worthless.
//!
//! Hostname/CA checks are intentionally bypassed and REPLACED by (1)+(2).

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
        // (1) Pin the leaf certificate by SHA-256. The cert is public, so a
        // plain comparison is fine (nothing secret is compared here).
        let got = Sha256::digest(end_entity.as_ref());
        if got.as_slice() == self.pinned {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::General(
                "server certificate fingerprint does not match the pinned value".into(),
            ))
        }
    }

    // (2) Handshake-signature verification is delegated to the real crypto
    // provider, so possession of the private key is still proven.
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

/// Parse a SHA-256 fingerprint string (hex, optionally colon-separated, any
/// case) into 32 bytes.
pub fn parse_fingerprint(s: &str) -> Result<[u8; 32], String> {
    let hex: String = s.chars().filter(|c| !c.is_whitespace() && *c != ':').collect();
    // Guard non-ASCII before byte-slicing below (a stray multi-byte char would
    // otherwise panic on a char boundary rather than erroring).
    if !hex.is_ascii() {
        return Err("fingerprint contains non-hex characters".to_string());
    }
    if hex.len() != 64 {
        return Err(format!(
            "fingerprint must be 32 bytes (64 hex chars), got {} hex chars",
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

/// Build a reqwest client that trusts ONLY the certificate whose SHA-256 equals
/// `fingerprint`. HTTPS is enforced (no plaintext downgrade).
pub fn pinned_client(fingerprint: &str) -> anyhow::Result<reqwest::Client> {
    let pinned = parse_fingerprint(fingerprint).map_err(|e| anyhow::anyhow!(e))?;
    // Use an explicit ring provider so we don't depend on a process-wide
    // default being installed by the caller.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let algs = provider.signature_verification_algorithms;
    let verifier = Arc::new(PinnedVerifier { pinned, algs });

    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(rustls::DEFAULT_VERSIONS)
        .map_err(|e| anyhow::anyhow!("tls config: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    Ok(reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .https_only(true)
        .build()?)
}

/// Fetch a bearer-authenticated URL from a pinned server, returning the raw
/// response body on 2xx.
pub async fn fetch(url: &str, token: &str, fingerprint: &str) -> anyhow::Result<Vec<u8>> {
    let client = pinned_client(fingerprint)?;
    let resp = client.get(url).bearer_auth(token).send().await?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("server returned HTTP {status}");
    }
    Ok(resp.bytes().await?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::parse_fingerprint;

    #[test]
    fn fingerprint_parsing() {
        let colon = "91:F6:4F:13:09:74:1C:2F:9C:E3:42:AF:79:00:0A:B7:52:B7:0A:72:F3:14:9A:78:B3:69:80:39:54:3E:3C:74";
        let plain = "91f64f1309741c2f9ce342af79000ab752b70a72f3149a78b3698039543e3c74";
        assert_eq!(parse_fingerprint(colon).unwrap(), parse_fingerprint(plain).unwrap());
        assert!(parse_fingerprint("tooshort").is_err());
        assert!(parse_fingerprint(&"zz".repeat(32)).is_err());
        // Non-ASCII must error, not panic on a char boundary.
        assert!(parse_fingerprint(&"é".repeat(32)).is_err());
    }
}
