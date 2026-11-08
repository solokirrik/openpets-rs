//! Where the app keeps its socket, installed pets and selection.

use std::path::PathBuf;

/// `~/Library/Application Support/openpets-rs`, or `$OPENPETS_RS_HOME` when set.
pub fn data_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("OPENPETS_RS_HOME") {
        return PathBuf::from(home);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/openpets-rs")
}

pub fn socket_path() -> PathBuf {
    data_dir().join("openpets.sock")
}

pub fn pets_dir() -> PathBuf {
    data_dir().join("pets")
}

pub fn selected_pet_file() -> PathBuf {
    data_dir().join("selected-pet")
}

/// State shared by hook runs: throttle times and the badges waiting on the user.
pub fn hook_state_file() -> PathBuf {
    data_dir().join("hook-state.json")
}
