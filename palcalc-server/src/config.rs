//! Server configuration, loaded from a TOML file that is never committed
//! (it holds the access tokens). See `palcalc-server.example.toml`.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

fn default_bind() -> SocketAddr {
    // Safe default: loopback only. The operator must explicitly set a
    // non-loopback bind to expose the server. Port 8123 (not 8122) to steer
    // clear of a common Palworld game port — use a port distinct from the
    // game's.
    "127.0.0.1:8123".parse().unwrap()
}

fn default_request_timeout() -> u64 {
    30
}
fn default_max_inflight() -> usize {
    64
}
fn default_rate_burst() -> u32 {
    30
}
fn default_rate_per_second() -> u64 {
    5
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The save world directory: the folder holding `Level.sav` and `Players/`.
    pub save_dir: PathBuf,

    /// Address to bind. Defaults to loopback; set to `0.0.0.0:8123` to expose
    /// (use a port distinct from your Palworld game port, forwarded as TCP).
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,

    /// Access tokens. Each is an opaque bearer secret that grants read access
    /// to the whole roster. Generate long random values (see the README).
    /// Never commit these.
    #[serde(default)]
    pub tokens: Vec<String>,

    /// PEM cert + key paths. If either is missing, a self-signed pair is
    /// generated at these paths on first run (fingerprint printed at startup).
    #[serde(default = "default_cert_path")]
    pub tls_cert: PathBuf,
    #[serde(default = "default_key_path")]
    pub tls_key: PathBuf,

    /// Per-request timeout (seconds).
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,

    /// Global cap on concurrently in-flight requests (load-shed beyond this).
    #[serde(default = "default_max_inflight")]
    pub max_inflight: usize,

    /// Per-IP rate limit: sustained requests/second and burst size.
    #[serde(default = "default_rate_per_second")]
    pub rate_per_second: u64,
    #[serde(default = "default_rate_burst")]
    pub rate_burst: u32,
}

fn default_cert_path() -> PathBuf {
    PathBuf::from("cert.pem")
}
fn default_key_path() -> PathBuf {
    PathBuf::from("key.pem")
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Config> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {path}: {e}"))?;
        let cfg: Config = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("parsing config {path}: {e}"))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.tokens.is_empty() {
            anyhow::bail!("config has no tokens — the server would be unauthenticated; add at least one");
        }
        // Reject weak/empty tokens so a typo can't create a guessable secret.
        for (i, t) in self.tokens.iter().enumerate() {
            if t.trim().len() < 24 {
                anyhow::bail!("token #{i} is shorter than 24 chars — use a long random secret");
            }
        }
        if !self.save_dir.join("Level.sav").is_file() {
            anyhow::bail!(
                "save_dir {:?} has no Level.sav — point it at the world folder",
                self.save_dir
            );
        }
        Ok(())
    }
}
