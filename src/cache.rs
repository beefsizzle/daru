use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn xdg(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback))
}

/// the shell's settings: ~/.config/amane-shell (or under $XDG_CONFIG_HOME)
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("amane-shell")
}

/// what can be made again: ~/.cache/amane-shell (or under $XDG_CACHE_HOME)
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("amane-shell")
}

/// what the shell remembers between runs, which nobody edits and a cache cleaner mustn't wipe:
/// ~/.local/state/amane-shell (or under $XDG_STATE_HOME)
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("amane-shell")
}

/// a stable 64-bit hash (FNV-1a), for naming cache files after what they hold
pub fn fnv(data: &[u8]) -> u64 {
    let mut hash: u64 = 1469598103934665603;

    for byte in data {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(1099511628211);
    }

    hash
}

/*
 * where short-lived files go (clipboard thumbnails, screenshots being taken).
 * $XDG_RUNTIME_DIR belongs to the user, mode 0700, and is cleared at logout. a fixed name in /tmp would be
 * world-writable ground where another user could plant a symlink at the name we're about to write
 */
pub fn runtime_dir() -> Option<&'static Path> {
    static DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();

    DIRECTORY
        .get_or_init(|| {
            let me = fs::metadata("/proc/self").ok()?.uid();

            let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join(format!("amane-shell-{me}")));

            let directory = if std::env::var_os("XDG_RUNTIME_DIR").is_some() { base.join("amane-shell") } else { base };

            fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory).ok()?;

            // someone else's directory, or a symlink to one, is not ours to write into
            let metadata = fs::symlink_metadata(&directory).ok()?;

            (metadata.is_dir() && metadata.uid() == me).then_some(directory)
        })
        .as_deref()
}

/// deletes the files in the runtime folder that start with `prefix` and aren't in `keep`
pub fn prune(prefix: &str, keep: &HashSet<PathBuf>) {
    let Some(directory) = runtime_dir() else { return };
    let Ok(entries) = fs::read_dir(directory) else { return };

    for entry in entries.flatten() {
        let path = entry.path();

        let named = path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with(prefix));

        if named && !keep.contains(&path) {
            let _ = fs::remove_file(path);
        }
    }
}
