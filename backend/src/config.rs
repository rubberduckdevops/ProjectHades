//! Runtime configuration loaded from environment variables (see API_CONTRACT.md).

use thiserror::Error;

pub const DEFAULT_REGION: &str = "us-west-2";
pub const DEFAULT_CORS_ORIGINS: &str = "http://localhost:5173";
pub const DEFAULT_PORT: u16 = 3000;
pub const DEFAULT_EMBED_MODEL_ID: &str = "amazon.titan-embed-text-v2:0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub table_name: String,
    pub notes_bucket: String,
    pub vector_bucket: String,
    pub vector_index: String,
    pub embed_model_id: String,
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
        let env = Env(&lookup);
        let shared = SharedConfig::load(&env)?;
        let get = |k: &str| env.get(k);
        let required = |k: &'static str| env.required(k);

        let SharedConfig {
            table_name,
            notes_bucket,
            vector_bucket,
            vector_index,
            embed_model_id,
            region,
        } = shared;
        let user_pool_id = required("COGNITO_USER_POOL_ID")?;
        let client_id = required("COGNITO_CLIENT_ID")?;

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
            vector_bucket,
            vector_index,
            embed_model_id,
            user_pool_id,
            client_id,
            region,
            cors_origins,
            port,
        })
    }
}

/// Configuration of the search indexer Lambda (no Cognito or HTTP settings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerConfig {
    pub table_name: String,
    pub notes_bucket: String,
    pub vector_bucket: String,
    pub vector_index: String,
    pub embed_model_id: String,
    pub region: String,
}

impl IndexerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let SharedConfig {
            table_name,
            notes_bucket,
            vector_bucket,
            vector_index,
            embed_model_id,
            region,
        } = SharedConfig::load(&Env(&lookup))?;
        Ok(IndexerConfig {
            table_name,
            notes_bucket,
            vector_bucket,
            vector_index,
            embed_model_id,
            region,
        })
    }
}

/// Variable lookup that treats blank values as unset.
struct Env<'a, F: Fn(&str) -> Option<String>>(&'a F);

impl<F: Fn(&str) -> Option<String>> Env<'_, F> {
    fn get(&self, k: &str) -> Option<String> {
        (self.0)(k)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    }

    fn required(&self, k: &'static str) -> Result<String, ConfigError> {
        self.get(k).ok_or(ConfigError::Missing(k))
    }
}

/// Settings needed by both the API and the indexer.
struct SharedConfig {
    table_name: String,
    notes_bucket: String,
    vector_bucket: String,
    vector_index: String,
    embed_model_id: String,
    region: String,
}

impl SharedConfig {
    fn load<F: Fn(&str) -> Option<String>>(env: &Env<'_, F>) -> Result<Self, ConfigError> {
        Ok(SharedConfig {
            table_name: env.required("TABLE_NAME")?,
            notes_bucket: env.required("NOTES_BUCKET")?,
            vector_bucket: env.required("VECTOR_BUCKET")?,
            vector_index: env.required("VECTOR_INDEX")?,
            embed_model_id: env
                .get("EMBED_MODEL_ID")
                .unwrap_or_else(|| DEFAULT_EMBED_MODEL_ID.to_owned()),
            region: env
                .get("AWS_REGION")
                .unwrap_or_else(|| DEFAULT_REGION.to_owned()),
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

    const REQUIRED: [(&str, &str); 6] = [
        ("TABLE_NAME", "hades_journal"),
        ("NOTES_BUCKET", "hades-notes-bucket"),
        ("VECTOR_BUCKET", "hades-vectors"),
        ("VECTOR_INDEX", "notes"),
        ("COGNITO_USER_POOL_ID", "us-west-2_abc"),
        ("COGNITO_CLIENT_ID", "client123"),
    ];

    #[test]
    fn defaults_applied() {
        let c = load(&REQUIRED).unwrap();
        assert_eq!(c.table_name, "hades_journal");
        assert_eq!(c.notes_bucket, "hades-notes-bucket");
        assert_eq!(c.vector_bucket, "hades-vectors");
        assert_eq!(c.vector_index, "notes");
        assert_eq!(c.embed_model_id, "amazon.titan-embed-text-v2:0");
        assert_eq!(c.region, "us-west-2");
        assert_eq!(c.cors_origins, vec!["http://localhost:5173"]);
        assert_eq!(c.port, 3000);
    }

    #[test]
    fn missing_required_fails() {
        let err = load(&REQUIRED[..5]).unwrap_err();
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
        pairs.push(("EMBED_MODEL_ID", " other-model "));
        let c = load(&pairs).unwrap();
        assert_eq!(c.embed_model_id, "other-model");
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

    #[test]
    fn vector_settings_are_required() {
        for name in ["VECTOR_BUCKET", "VECTOR_INDEX"] {
            let pairs: Vec<_> = REQUIRED
                .iter()
                .copied()
                .filter(|(k, _)| *k != name)
                .collect();
            assert_eq!(load(&pairs).unwrap_err(), ConfigError::Missing(name));
        }
    }

    fn load_indexer(pairs: &[(&str, &str)]) -> Result<IndexerConfig, ConfigError> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        IndexerConfig::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn indexer_config_needs_no_cognito() {
        let pairs = [
            ("TABLE_NAME", "t"),
            ("NOTES_BUCKET", "b"),
            ("VECTOR_BUCKET", "vb"),
            ("VECTOR_INDEX", "notes"),
        ];
        let c = load_indexer(&pairs).unwrap();
        assert_eq!(
            c,
            IndexerConfig {
                table_name: "t".into(),
                notes_bucket: "b".into(),
                vector_bucket: "vb".into(),
                vector_index: "notes".into(),
                embed_model_id: DEFAULT_EMBED_MODEL_ID.into(),
                region: DEFAULT_REGION.into(),
            }
        );
        for (i, (name, _)) in pairs.iter().enumerate() {
            let mut missing = pairs.to_vec();
            missing.remove(i);
            assert_eq!(
                load_indexer(&missing).unwrap_err(),
                ConfigError::Missing(name)
            );
        }
    }
}
