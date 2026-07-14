//! Palbox scanner: hover-based (NEVER clicks pal slots — left-click picks the
//! pal up, right-click deploys it). Wayland/Hyprland only for now.
//!
//! Reads the currently open box in two passes: (1) one unhovered grid capture
//! classifies each slot empty/occupied (icons are an occupancy check only —
//! hover states distort them); (2) each occupied slot is hovered and its
//! hover panel read for species NAME, gender (color), and passives. Text is
//! read by `ocrs`/`rten` neural OCR with closed-vocabulary correction, backed
//! by learned crops (`textlib`) and a synthesized-glyph NCC fallback (`synth`).

pub mod matcher;
pub mod palbox;
pub mod panel;
pub mod platform;
pub mod ocr;
pub mod synth;
pub mod textlib;
