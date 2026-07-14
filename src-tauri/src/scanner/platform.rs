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
    /// Geometry of the currently focused monitor (x, y, w, h) — the canvas
    /// for freeze-frame zone calibration.
    fn focused_monitor_rect(&mut self) -> Result<(i32, i32, u32, u32), String>;
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

    #[derive(Deserialize)]
    struct HyprMonitor {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        #[serde(default)]
        focused: bool,
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
        /// Verified-working movecursor dispatch form. Hyprland 0.55+ with Lua
        /// config rewired `dispatch` through the Lua VM (hl.dsp namespace) and
        /// broke the classic syntax, so the form is discovered at first use.
        move_form: Option<usize>,
        /// Whether a working ydotool (uinput virtual mouse) is present —
        /// probed lazily. Real relative motion events are the most reliable
        /// way to trigger a game's hover detection; compositor warps teleport
        /// the cursor without the event stream games listen for.
        ydotool: Option<bool>,
    }

    /// Candidate movecursor forms, classic first. Which one a given Hyprland
    /// build accepts is verified by reading the cursor position back — replies
    /// alone can't be trusted across protocol generations.
    const MOVE_FORMS: &[fn(i32, i32) -> String] = &[
        |x, y| format!("dispatch movecursor {x} {y}"),
        |x, y| format!("dispatch hl.dsp.movecursor({{ x = {x}, y = {y} }})"),
        |x, y| format!("dispatch hl.dsp.movecursor({x}, {y})"),
        |x, y| format!("dispatch hl.dsp.movecursor(\"{x} {y}\")"),
        |x, y| format!("dispatch hl.dsp.cursor.move({{ x = {x}, y = {y} }})"),
    ];

    impl HyprlandBackend {
        pub fn new() -> Result<Self, String> {
            Ok(Self {
                socket: socket_path()?,
                wayshot: WayshotConnection::new().ok(),
                move_form: None,
                ydotool: None,
            })
        }

        fn ydotool_available(&mut self) -> bool {
            *self.ydotool.get_or_insert_with(|| {
                std::process::Command::new("ydotool")
                    .args(["mousemove", "-x", "0", "-y", "0"])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            })
        }

        /// Real relative mouse motion via uinput. Deltas are sent as SMALL
        /// chunks at a mouse-like pace: pointer acceleration is ~1:1 for slow
        /// motion, so chunks land where they claim — one big delta gets
        /// amplified, overshoots, and the corrections oscillate across the
        /// target (observed in-game as the hover flickering between the
        /// neighboring slots). Never "fixed" with a warp: warps are invisible
        /// to the game, which is the whole reason ydotool is here.
        fn move_relative_closed_loop(&mut self, x: i32, y: i32) -> Result<(), String> {
            const TOLERANCE: i32 = 4;
            const CHUNK_PX: f64 = 16.0;
            for _ in 0..5 {
                let (cx, cy) = self.query_cursor()?;
                let (dx, dy) = (x - cx, y - cy);
                if dx.abs() <= TOLERANCE && dy.abs() <= TOLERANCE {
                    return Ok(());
                }
                let dist = ((dx * dx + dy * dy) as f64).sqrt();
                let steps = (dist / CHUNK_PX).ceil().max(1.0) as i32;
                let (mut sent_x, mut sent_y) = (0, 0);
                for i in 1..=steps {
                    let tx = (dx as f64 * i as f64 / steps as f64).round() as i32;
                    let ty = (dy as f64 * i as f64 / steps as f64).round() as i32;
                    let (mx, my) = (tx - sent_x, ty - sent_y);
                    (sent_x, sent_y) = (tx, ty);
                    if mx == 0 && my == 0 {
                        continue;
                    }
                    let status = std::process::Command::new("ydotool")
                        .args(["mousemove", "-x", &mx.to_string(), "-y", &my.to_string()])
                        .status()
                        .map_err(|e| format!("ydotool mousemove: {e}"))?;
                    if !status.success() {
                        return Err("ydotool mousemove failed".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                // Let the events propagate before measuring, or the correction
                // works from stale coordinates.
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            Err(
                "cursor did not settle on the target slot — pointer acceleration may be \
                 too aggressive; consider a flat accel profile in Hyprland input config"
                    .into(),
            )
        }

        /// Warp in small steps along the path (plus a final wiggle inside the
        /// target) so clients that only react to motion events still see the
        /// cursor "travel" — a single teleport doesn't trigger game hover.
        fn move_interpolated(&mut self, x: i32, y: i32) -> Result<(), String> {
            let (sx, sy) = self.query_cursor().unwrap_or((x, y));
            let dist = (((x - sx).pow(2) + (y - sy).pow(2)) as f64).sqrt();
            let steps = (dist / 40.0).clamp(3.0, 12.0) as i32;
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                self.warp(
                    sx + ((x - sx) as f64 * t).round() as i32,
                    sy + ((y - sy) as f64 * t).round() as i32,
                )?;
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
            self.warp(x + 2, y)?;
            std::thread::sleep(std::time::Duration::from_millis(8));
            self.warp(x, y)
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

        fn query_cursor(&self) -> Result<(i32, i32), String> {
            let json = self.request("j/cursorpos")?;
            let p: HyprCursor = serde_json::from_str(&json)
                .map_err(|e| format!("hyprland cursorpos parse: {e}"))?;
            Ok((p.x as i32, p.y as i32))
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

        /// Compositor-side cursor placement (teleport) via dispatch, with
        /// dispatch-form discovery for Lua-config Hyprland builds.
        fn warp(&mut self, x: i32, y: i32) -> Result<(), String> {
            if let Some(i) = self.move_form {
                let reply = self.request(&MOVE_FORMS[i](x, y))?;
                return if reply.to_lowercase().contains("error") {
                    Err(format!("hyprland movecursor: {reply}"))
                } else {
                    Ok(())
                };
            }
            // First use: discover which form this Hyprland build accepts by
            // checking whether the cursor actually arrived.
            let mut replies = Vec::new();
            for (i, form) in MOVE_FORMS.iter().enumerate() {
                let msg = form(x, y);
                let reply = self.request(&msg)?;
                if let Ok((cx, cy)) = self.query_cursor() {
                    if (cx - x).abs() <= 2 && (cy - y).abs() <= 2 {
                        self.move_form = Some(i);
                        return Ok(());
                    }
                }
                replies.push(format!("`{msg}` -> {}", reply.trim()));
            }
            // Nothing moved the cursor: gather what the Lua API actually
            // exposes so the error is diagnosable.
            let dsp = self
                .request(
                    "eval 'local t={} for k,_ in pairs(hl.dsp) do t[#t+1]=tostring(k) end \
                     table.sort(t) return table.concat(t, \", \")'",
                )
                .unwrap_or_else(|e| format!("(eval failed: {e})"));
            Err(format!(
                "no movecursor form accepted by this Hyprland build.\nTried:\n{}\navailable hl.dsp entries: {dsp}",
                replies.join("\n")
            ))
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
            // Games update hover state from motion EVENTS, not cursor
            // position: a bare compositor warp teleports the pointer without
            // the event stream they listen for. Prefer real uinput motion
            // (ydotool) when present; otherwise emulate travel with
            // interpolated warps.
            if self.ydotool_available() {
                self.move_relative_closed_loop(x, y)
            } else {
                self.move_interpolated(x, y)
            }
        }

        fn cursor_pos(&mut self) -> Result<(i32, i32), String> {
            self.query_cursor()
        }

        fn focused_monitor_rect(&mut self) -> Result<(i32, i32, u32, u32), String> {
            let json = self.request("j/monitors")?;
            let monitors: Vec<HyprMonitor> = serde_json::from_str(&json)
                .map_err(|e| format!("hyprland monitors parse: {e}"))?;
            monitors
                .iter()
                .find(|m| m.focused)
                .or(monitors.first())
                .map(|m| (m.x, m.y, m.width, m.height))
                .ok_or_else(|| "no monitors reported by Hyprland".into())
        }
    }
}
