//! The connection dialog's URL field: DataGrip's JDBC URL for each driver, built from the
//! General tab fields and parsed back into them when the URL is edited.

use super::{ConnectionConfig, Driver, snowflake};

/// The URL DataGrip shows for these settings (default URL templates of its drivers).
pub fn jdbc_url(cfg: &ConnectionConfig) -> String {
    let host = cfg.host.trim();
    let db = cfg.database.trim();
    let authority = if cfg.port == 0 { host.to_string() } else { format!("{host}:{}", cfg.port) };
    let slash_db = if db.is_empty() { String::new() } else { format!("/{db}") };
    match cfg.driver {
        Driver::Postgres => format!("jdbc:postgresql://{authority}{slash_db}"),
        Driver::MySql => format!("jdbc:mysql://{authority}{slash_db}"),
        Driver::MariaDb => format!("jdbc:mariadb://{authority}{slash_db}"),
        Driver::MsSql if db.is_empty() => format!("jdbc:sqlserver://{authority}"),
        Driver::MsSql => format!("jdbc:sqlserver://{authority};database={db}"),
        Driver::Oracle => format!("jdbc:oracle:thin:@//{authority}{slash_db}"),
        Driver::Sqlite => format!("jdbc:sqlite:{}", cfg.file_path.trim()),
        Driver::Snowflake => snowflake::jdbc_url(cfg),
    }
}

/// Sets the fields the URL carries. Returns false, changing nothing, when the URL is not one of
/// this driver's (the field then just keeps the text until it is fixed).
pub fn apply_jdbc_url(cfg: &mut ConnectionConfig, url: &str) -> bool {
    let url = url.trim();
    let prefix = match cfg.driver {
        Driver::Postgres => "jdbc:postgresql://",
        Driver::MySql => "jdbc:mysql://",
        Driver::MariaDb => "jdbc:mariadb://",
        Driver::MsSql => "jdbc:sqlserver://",
        Driver::Oracle => "jdbc:oracle:thin:@//",
        Driver::Sqlite => {
            let Some(path) = url.strip_prefix("jdbc:sqlite:") else { return false };
            cfg.file_path = path.to_string();
            return true;
        }
        Driver::Snowflake => return snowflake::apply_jdbc_url(cfg, url),
    };
    let Some(rest) = url.strip_prefix(prefix) else { return false };
    let default_port = cfg.driver.default_port();
    if cfg.driver == Driver::MsSql {
        // host:port;database=db;other=…
        let mut parts = rest.split(';');
        set_authority(cfg, parts.next().unwrap_or(""), default_port);
        cfg.database = parts
            .filter_map(|p| p.split_once('='))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("database") || k.trim().eq_ignore_ascii_case("databaseName"))
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_default();
        return true;
    }
    // host:port/db?params
    let rest = rest.split(['?', ';']).next().unwrap_or(rest);
    let (authority, db) = rest.split_once('/').unwrap_or((rest, ""));
    set_authority(cfg, authority, default_port);
    cfg.database = db.to_string();
    true
}

/// `host[:port]`; a missing port is the driver's default, as the JDBC drivers read it.
pub(crate) fn set_authority(cfg: &mut ConnectionConfig, authority: &str, default_port: u16) {
    match authority.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => {
            cfg.host = h.to_string();
            cfg.port = p.parse().unwrap_or(default_port);
        }
        _ => {
            cfg.host = authority.to_string();
            cfg.port = default_port;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_driver() {
        for driver in Driver::ALL {
            let mut cfg = ConnectionConfig::new(1, driver);
            cfg.host = "db.example.com".into();
            cfg.port = 5555;
            cfg.database = if driver == Driver::Snowflake { "ANALYTICS".into() } else { "sales".into() };
            cfg.file_path = "/tmp/x.db".into();
            let url = jdbc_url(&cfg);
            let mut back = ConnectionConfig::new(2, driver);
            assert!(apply_jdbc_url(&mut back, &url), "{url}");
            assert_eq!(jdbc_url(&back), url);
        }
    }

    #[test]
    fn parses_the_usual_forms() {
        let mut cfg = ConnectionConfig::new(1, Driver::Postgres);
        assert!(apply_jdbc_url(&mut cfg, "jdbc:postgresql://localhost/app?sslmode=require"));
        assert_eq!((cfg.host.as_str(), cfg.port, cfg.database.as_str()), ("localhost", 5432, "app"));
        assert!(!apply_jdbc_url(&mut cfg, "jdbc:mysql://localhost:3306/app"));

        let mut ms = ConnectionConfig::new(1, Driver::MsSql);
        assert!(apply_jdbc_url(&mut ms, "jdbc:sqlserver://sql:14330;encrypt=true;databaseName=erp"));
        assert_eq!((ms.host.as_str(), ms.port, ms.database.as_str()), ("sql", 14330, "erp"));
    }
}
