//! Palbox scanner: hover-based (NEVER clicks pal slots — left-click picks the
//! pal up, right-click deploys it). Wayland/Hyprland first; Windows later.
//!
//! v1 identifies the species in each slot of the currently open box via icon
//! template matching. Passive-text OCR is deferred until real screenshots from
//! the gaming machine are available to calibrate against; scanned pals get
//! passives added manually in the UI for now.

pub mod matcher;
pub mod palbox;
pub mod platform;
