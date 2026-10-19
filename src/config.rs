//! Connection persistence in `~/Library/Application Support/DataCraft/connections.json`.
//! Passwords are stored in the macOS Keychain, never in the JSON.

use crate::db::ConnectionConfig;
use std::path::PathBuf;

const KEYCHAIN_SERVICE: &str = "DataCraft";

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("DataCraft")
}

fn connections_file() -> PathBuf {
    data_dir().join("connections.json")
}

fn keychain_entry(id: u64) -> Option<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, &format!("connection-{id}")).ok()
}

pub fn load_connections() -> Vec<ConnectionConfig> {
    let file = connections_file();
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Vec::new();
    };
    let mut list: Vec<ConnectionConfig> = match serde_json::from_str(&text) {
        Ok(list) => list,
        Err(e) => {
            // Don't lose the connections: set the unreadable file aside before it gets overwritten.
            let backup = file.with_extension("json.bak");
            let _ = std::fs::copy(&file, &backup);
            eprintln!("unreadable connections.json ({e}); copy at {}", backup.display());
            Vec::new()
        }
    };
    for c in &mut list {
        c.migrate();
        if let Some(pw) = keychain_entry(c.id).and_then(|e| e.get_password().ok()) {
            c.password = pw;
        }
    }
    list
}

pub fn save_connections(list: &[ConnectionConfig]) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir())?;
    // Atomic write: quitting halfway doesn't leave a truncated file.
    let file = connections_file();
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(list)?)?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

pub fn store_password(id: u64, password: &str) {
    if let Some(entry) = keychain_entry(id) {
        if password.is_empty() {
            let _ = entry.delete_credential();
        } else {
            let _ = entry.set_password(password);
        }
    }
}

pub fn delete_password(id: u64) {
    if let Some(entry) = keychain_entry(id) {
        let _ = entry.delete_credential();
    }
}
