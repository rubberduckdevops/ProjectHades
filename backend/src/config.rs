//! Runtime configuration loaded from environment variables (see API_CONTRACT.md).

use thiserror::Error;

pub const DEFAULT_REGION: &str = "us-west-2";
pub const DEFAULT_CORS_ORIGINS: &str = "http://localhost:5173";
pub const DEFAULT_PORT: u16 = 3000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub table_name: String,
    pub notes_bucket: String,
    pub user_pool_id: String,
    pub client_id: String,
    pub region: String,
    pub cors_origins: Vec<String>,
    pub port: u16,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("required environment variable {0} is not set (or is empty)")]
    Missing(&'static str),
    #[error("environment variable {name} has invalid value {value:?}: {reason}")]
    Invalid {
        name: &'static str,
        value: String,
        reason: String,
    },
}

impl Config {
    /// Load configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Load configuration using an arbitrary lookup function (used by tests).
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let get = |k: &str| {
            lookup(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let required = |k: &'static str| get(k).ok_or(ConfigError::Missing(k));

        let table_name = required("TABLE_NAME")?;
        let notes_bucket = required("NOTES_BUCKET")?;
        let user_pool_id = required("COGNITO_USER_POOL_ID")?;
        let client_id = required("COGNITO_CLIENT_ID")?;
        let region = get("AWS_REGION").unwrap_or_else(|| DEFAULT_REGION.to_owned());

        let cors_origins = get("CORS_ORIGINS")
            .unwrap_or_else(|| DEFAULT_CORS_ORIGINS.to_owned())
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();

        let port = match get("PORT") {
            None => DEFAULT_PORT,
            Some(v) => v
                .parse()
                .map_err(|e: std::num::ParseIntError| ConfigError::Invalid {
                    name: "PORT",
                    value: v.clone(),
                    reason: e.to_string(),
                })?,
        };

        Ok(Config {
            table_name,
            notes_bucket,
            user_pool_id,
            client_id,
            region,
            cors_origins,
            port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn load(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|k| m.get(k).cloned())
    }

    const REQUIRED: [(&str, &str); 4] = [
        ("TABLE_NAME", "hades_journal"),
        ("NOTES_BUCKET", "hades-notes-bucket"),
        ("COGNITO_USER_POOL_ID", "us-west-2_abc"),
        ("COGNITO_CLIENT_ID", "client123"),
    ];

    #[test]
    fn defaults_applied() {
        let c = load(&REQUIRED).unwrap();
        assert_eq!(c.table_name, "hades_journal");
        assert_eq!(c.notes_bucket, "hades-notes-bucket");
        assert_eq!(c.region, "us-west-2");
        assert_eq!(c.cors_origins, vec!["http://localhost:5173"]);
        assert_eq!(c.port, 3000);
    }

    #[test]
    fn missing_required_fails() {
        let err = load(&REQUIRED[..3]).unwrap_err();
        assert_eq!(err, ConfigError::Missing("COGNITO_CLIENT_ID"));
    }

    #[test]
    fn notes_bucket_is_required() {
        let pairs: Vec<_> = REQUIRED
            .iter()
            .copied()
            .filter(|(k, _)| *k != "NOTES_BUCKET")
            .collect();
        assert_eq!(
            load(&pairs).unwrap_err(),
            ConfigError::Missing("NOTES_BUCKET")
        );
        let mut blank = pairs.clone();
        blank.push(("NOTES_BUCKET", "  "));
        assert_eq!(
            load(&blank).unwrap_err(),
            ConfigError::Missing("NOTES_BUCKET")
        );
    }

    #[test]
    fn parses_overrides() {
        let mut pairs = REQUIRED.to_vec();
        pairs.push(("AWS_REGION", "eu-west-1"));
        pairs.push(("CORS_ORIGINS", "http://a.test, https://b.test ,"));
        pairs.push(("PORT", "8080"));
        let c = load(&pairs).unwrap();
        assert_eq!(c.region, "eu-west-1");
        assert_eq!(c.cors_origins, vec!["http://a.test", "https://b.test"]);
        assert_eq!(c.port, 8080);
    }

    #[test]
    fn invalid_port_fails() {
        let mut pairs = REQUIRED.to_vec();
        pairs.push(("PORT", "nope"));
        assert!(matches!(load(&pairs), Err(ConfigError::Invalid { .. })));
    }
}
