//! Connection persistence in `~/Library/Application Support/DataCraft/connections.json`.
//! Passwords and secret driver properties are stored in the macOS Keychain (Windows Credential
//! Manager), never in the JSON.

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

/// Secret driver properties of a connection, as a JSON object `{name: value}`.
fn properties_entry(id: u64) -> Option<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, &format!("connection-{id}-properties")).ok()
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
    // Secrets an earlier build wrote to the file move to the keychain, and the file is rewritten.
    let mut leaked = false;
    for c in &mut list {
        c.migrate();
        if !c.secret_properties().is_empty() {
            leaked = true;
            let password = keychain_entry(c.id).and_then(|e| e.get_password().ok()).unwrap_or_default();
            store_secrets(&ConnectionConfig { password, ..c.clone() });
        }
        if let Some(pw) = keychain_entry(c.id).and_then(|e| e.get_password().ok()) {
            c.password = pw;
        }
        let secrets: std::collections::HashMap<String, String> = properties_entry(c.id)
            .and_then(|e| e.get_password().ok())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        fill_secret_properties(c, &secrets);
    }
    if leaked && let Err(e) = save_connections(&list) {
        eprintln!("could not rewrite connections.json without its secrets: {e:#}");
    }
    list
}

/// Puts the keychain's values back into their (blank) property rows.
fn fill_secret_properties(c: &mut ConnectionConfig, secrets: &std::collections::HashMap<String, String>) {
    for (k, v) in &mut c.properties {
        if let Some(secret) = secrets.get(k.trim()) {
            *v = secret.clone();
        }
    }
}

pub fn save_connections(list: &[ConnectionConfig]) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir())?;
    // Atomic write: quitting halfway doesn't leave a truncated file.
    let file = connections_file();
    let tmp = file.with_extension("json.tmp");
    let list: Vec<ConnectionConfig> = list.iter().map(ConnectionConfig::without_secrets).collect();
    std::fs::write(&tmp, serde_json::to_string_pretty(&list)?)?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

/// Saves a connection's password and secret driver properties to the keychain (an empty one
/// removes its entry).
pub fn store_secrets(cfg: &ConnectionConfig) {
    store(keychain_entry(cfg.id), &cfg.password);
    let secrets: std::collections::HashMap<String, String> =
        cfg.secret_properties().into_iter().map(|(k, v)| (k.trim().to_string(), v)).collect();
    let json = if secrets.is_empty() { String::new() } else { serde_json::to_string(&secrets).unwrap_or_default() };
    store(properties_entry(cfg.id), &json);
}

fn store(entry: Option<keyring::Entry>, value: &str) {
    if let Some(entry) = entry {
        if value.is_empty() {
            let _ = entry.delete_credential();
        } else {
            let _ = entry.set_password(value);
        }
    }
}

pub fn delete_secrets(id: u64) {
    for entry in [keychain_entry(id), properties_entry(id)].into_iter().flatten() {
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Driver;

    #[test]
    fn secret_properties_never_reach_the_file() {
        let mut cfg = ConnectionConfig::new(1, Driver::Snowflake);
        cfg.password = "PASSWORD-VALUE".into();
        cfg.set_property("private_key_file", "/k.p8");
        cfg.set_property("private_key_file_pwd", "PASSPHRASE-VALUE");
        cfg.set_property("token", "TOKEN-VALUE");
        let json = serde_json::to_string(&cfg.without_secrets()).unwrap();
        for secret in ["PASSPHRASE-VALUE", "TOKEN-VALUE", "PASSWORD-VALUE"] {
            assert!(!json.contains(secret), "{secret} written to the file: {json}");
        }
        assert!(json.contains("/k.p8"));

        // Loading puts the keychain's values back in their rows.
        let mut loaded: ConnectionConfig = serde_json::from_str(&json).unwrap();
        let secrets = cfg.secret_properties().into_iter().collect();
        fill_secret_properties(&mut loaded, &secrets);
        assert_eq!(loaded.properties, cfg.properties);
    }
}
