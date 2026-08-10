// End-to-end test of the certificate-pinning client against a real TLS
// handshake. Run with: cargo test --features client --test pinning
#![cfg(feature = "client")]

use std::net::SocketAddr;

use axum::{routing::get, Router};
use axum_server::tls_rustls::RustlsConfig;
use sha2::{Digest, Sha256};

fn fingerprint_of(cert_pem: &[u8]) -> String {
    let mut r = std::io::BufReader::new(cert_pem);
    let der = rustls_pemfile::certs(&mut r).next().unwrap().unwrap();
    Sha256::digest(&der).iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn pinned_client_accepts_matching_and_rejects_wrong_cert() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // A self-signed cert for the in-process test server.
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_pem = certified.cert.pem().into_bytes();
    let key_pem = certified.key_pair.serialize_pem().into_bytes();
    let fp = fingerprint_of(&cert_pem);

    let tls = RustlsConfig::from_pem(cert_pem, key_pem).await.unwrap();
    let app = Router::new().route("/roster", get(|| async { "ROSTER-OK" }));

    let handle = axum_server::Handle::new();
    let serve_handle = handle.clone();
    tokio::spawn(async move {
        axum_server::bind_rustls(SocketAddr::from(([127, 0, 0, 1], 0)), tls)
            .handle(serve_handle)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    let addr = handle.listening().await.expect("test server failed to bind");
    let url = format!("https://{addr}/roster");

    // Correct fingerprint -> handshake trusted, body returned.
    let body = palcalc_server::pin::fetch(&url, "tok", &fp)
        .await
        .expect("pinned fetch with the matching fingerprint should succeed");
    assert_eq!(body, b"ROSTER-OK");

    // Wrong fingerprint -> the pin rejects the cert, handshake fails.
    let mut wrong = fp.clone();
    let last = wrong.pop().unwrap();
    wrong.push(if last == '0' { '1' } else { '0' });
    assert!(
        palcalc_server::pin::fetch(&url, "tok", &wrong).await.is_err(),
        "pinned fetch must reject a certificate whose fingerprint doesn't match"
    );

    handle.shutdown();
}
