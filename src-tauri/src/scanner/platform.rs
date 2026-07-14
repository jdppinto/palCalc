//! Capture + input + window geometry behind one trait, selected at runtime.

use image::RgbaImage;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct WindowInfo {
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

pub trait Backend: Send {
    fn name(&self) -> &'static str;
    fn list_windows(&mut self) -> Result<Vec<WindowInfo>, String>;
    fn capture_region(&mut self, x: i32, y: i32, w: u32, h: u32) -> Result<RgbaImage, String>;
    /// Cursor placement only — the scan flow hovers, it never clicks slots.
    fn move_cursor(&mut self, x: i32, y: i32) -> Result<(), String>;
    fn cursor_pos(&mut self) -> Result<(i32, i32), String>;
}

pub fn detect() -> Result<Box<dyn Backend>, String> {
    #[cfg(target_os = "linux")]
    {
        if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
            return Ok(Box::new(linux::HyprlandBackend::new()?));
        }
        Err(
            "no supported compositor: the scanner currently needs Hyprland (wlr-screencopy). \
             This desktop's compositor doesn't expose it."
                .into(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("scanner backend for this OS is not implemented yet (Windows planned)".into())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Backend, WindowInfo};
    use image::RgbaImage;
    use libwayshot::region::{LogicalRegion, Position, Region, Size};
    use libwayshot::WayshotConnection;
    use serde::Deserialize;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;

    // Hyprland IPC is spoken directly over its UNIX socket. The hyprland crate
    // was dropped after field testing: it still looks for the pre-0.40 socket
    // path (/tmp/hypr) and PANICS on socket errors — inside a webkit callback
    // that can't unwind, that aborts the entire app.

    #[derive(Deserialize)]
    struct HyprClient {
        at: (i32, i32),
        size: (i32, i32),
        #[serde(default)]
        title: String,
        #[serde(default, rename = "initialTitle")]
        initial_title: String,
    }

    #[derive(Deserialize)]
    struct HyprCursor {
        x: i64,
        y: i64,
    }

    fn socket_path() -> Result<PathBuf, String> {
        let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
            .map_err(|_| "HYPRLAND_INSTANCE_SIGNATURE not set — not in a Hyprland session?")?;
        let mut candidates = Vec::new();
        if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
            candidates.push(
                PathBuf::from(runtime)
                    .join("hypr")
                    .join(&sig)
                    .join(".socket.sock"),
            );
        }
        // Pre-Hyprland-0.40 location.
        candidates.push(PathBuf::from("/tmp/hypr").join(&sig).join(".socket.sock"));
        candidates.into_iter().find(|p| p.exists()).ok_or_else(|| {
            "Hyprland IPC socket not found (checked $XDG_RUNTIME_DIR/hypr and /tmp/hypr)".into()
        })
    }

    pub struct HyprlandBackend {
        socket: PathBuf,
        /// None when wlr-screencopy init failed — capture falls back to `grim`.
        wayshot: Option<WayshotConnection>,
    }

    impl HyprlandBackend {
        pub fn new() -> Result<Self, String> {
            Ok(Self {
                socket: socket_path()?,
                wayshot: WayshotConnection::new().ok(),
            })
        }

        /// One request/response over the IPC socket ("j/<cmd>" returns JSON,
        /// "dispatch <cmd>" returns "ok"). Hyprland closes after replying.
        fn request(&self, msg: &str) -> Result<String, String> {
            let mut s = UnixStream::connect(&self.socket).map_err(|e| {
                format!("hyprland socket connect ({}): {e}", self.socket.display())
            })?;
            s.write_all(msg.as_bytes())
                .map_err(|e| format!("hyprland socket write: {e}"))?;
            let mut out = String::new();
            s.read_to_string(&mut out)
                .map_err(|e| format!("hyprland socket read: {e}"))?;
            Ok(out)
        }

        fn capture_grim(x: i32, y: i32, w: u32, h: u32) -> Result<RgbaImage, String> {
            let out = std::process::Command::new("grim")
                .args(["-g", &format!("{x},{y} {w}x{h}"), "-"])
                .output()
                .map_err(|e| format!("grim not runnable: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "grim failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            Ok(image::load_from_memory(&out.stdout)
                .map_err(|e| format!("grim png decode: {e}"))?
                .to_rgba8())
        }
    }

    impl Backend for HyprlandBackend {
        fn name(&self) -> &'static str {
            if self.wayshot.is_some() {
                "hyprland (libwayshot)"
            } else {
                "hyprland (grim fallback)"
            }
        }

        fn list_windows(&mut self) -> Result<Vec<WindowInfo>, String> {
            let json = self.request("j/clients")?;
            let clients: Vec<HyprClient> = serde_json::from_str(&json)
                .map_err(|e| format!("hyprland clients parse: {e}"))?;
            Ok(clients
                .into_iter()
                .map(|c| WindowInfo {
                    title: if c.title.is_empty() {
                        c.initial_title
                    } else {
                        c.title
                    },
                    x: c.at.0,
                    y: c.at.1,
                    w: c.size.0.max(0) as u32,
                    h: c.size.1.max(0) as u32,
                })
                .collect())
        }

        fn capture_region(&mut self, x: i32, y: i32, w: u32, h: u32) -> Result<RgbaImage, String> {
            if let Some(ws) = &self.wayshot {
                let region = LogicalRegion {
                    inner: Region {
                        position: Position { x, y },
                        size: Size {
                            width: w,
                            height: h,
                        },
                    },
                };
                match ws.screenshot(region, false) {
                    Ok(img) => return Ok(img.to_rgba8()),
                    Err(e) => {
                        eprintln!("libwayshot capture failed ({e}), falling back to grim");
                    }
                }
            }
            Self::capture_grim(x, y, w, h)
        }

        fn move_cursor(&mut self, x: i32, y: i32) -> Result<(), String> {
            // Compositor-side placement — exact, unlike ydotool --absolute
            let reply = self.request(&format!("dispatch movecursor {x} {y}"))?;
            if reply.trim() == "ok" {
                Ok(())
            } else {
                Err(format!("hyprland movecursor: {reply}"))
            }
        }

        fn cursor_pos(&mut self) -> Result<(i32, i32), String> {
            let json = self.request("j/cursorpos")?;
            let p: HyprCursor = serde_json::from_str(&json)
                .map_err(|e| format!("hyprland cursorpos parse: {e}"))?;
            Ok((p.x as i32, p.y as i32))
        }
    }
}
