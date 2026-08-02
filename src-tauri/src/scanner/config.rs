use std::path::PathBuf;

/// Root directory for all palcalc persistent data.
///
/// - **Windows:** `<exe_dir>/palcalc/` — keeps everything next to the bundled
///   executable so portable users don't have to hunt through `%APPDATA%`.
/// - **Linux / macOS:** `$XDG_CONFIG_HOME/palcalc` or `~/.config/palcalc`
///   following the XDG Base Directory specification.
pub fn palcalc_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::current_exe()
            .expect("failed to get current exe path")
            .parent()
            .expect("exe has no parent directory")
            .join("palcalc")
    }
    #[cfg(not(target_os = "windows"))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            });
        base.join("palcalc")
    }
}
