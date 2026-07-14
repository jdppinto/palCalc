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
    use hyprland::data::{Clients, CursorPosition};
    use hyprland::dispatch::{Dispatch, DispatchType};
    use hyprland::shared::HyprData;
    use image::RgbaImage;
    use libwayshot::region::{LogicalRegion, Position, Region, Size};
    use libwayshot::WayshotConnection;

    pub struct HyprlandBackend {
        /// None when wlr-screencopy init failed — capture falls back to `grim`.
        wayshot: Option<WayshotConnection>,
    }

    impl HyprlandBackend {
        pub fn new() -> Result<Self, String> {
            Ok(Self {
                wayshot: WayshotConnection::new().ok(),
            })
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
            Ok(Clients::get()
                .map_err(|e| format!("hyprland clients: {e}"))?
                .iter()
                .map(|c| WindowInfo {
                    title: if c.title.is_empty() {
                        c.initial_title.clone()
                    } else {
                        c.title.clone()
                    },
                    x: c.at.0 as i32,
                    y: c.at.1 as i32,
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
            Dispatch::call(DispatchType::Custom("movecursor", &format!("{x} {y}")))
                .map_err(|e| format!("hyprland movecursor: {e}"))
        }

        fn cursor_pos(&mut self) -> Result<(i32, i32), String> {
            let p = CursorPosition::get().map_err(|e| format!("hyprland cursorpos: {e}"))?;
            Ok((p.x as i32, p.y as i32))
        }
    }
}
