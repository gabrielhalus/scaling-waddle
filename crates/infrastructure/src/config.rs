use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub database_path: PathBuf,
    pub bind: SocketAddr,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("SW_BIND invalide: {0}")]
    Bind(String),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let bind = lookup("SW_BIND").unwrap_or_else(|| "127.0.0.1:8080".to_owned());
        Ok(Self {
            database_path: lookup("SW_DATABASE_PATH")
                .map_or_else(|| PathBuf::from("data/scaling-waddle.db"), PathBuf::from),
            bind: bind.parse().map_err(|_| ConfigError::Bind(bind))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_without_environment() {
        let config = Config::from_lookup(|_| None).unwrap();
        assert_eq!(config.bind, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(
            config.database_path,
            PathBuf::from("data/scaling-waddle.db")
        );
    }

    #[test]
    fn environment_overrides_defaults() {
        let config = Config::from_lookup(|key| match key {
            "SW_BIND" => Some("0.0.0.0:9000".to_owned()),
            "SW_DATABASE_PATH" => Some("/var/lib/sw.db".to_owned()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.bind.port(), 9000);
        assert_eq!(config.database_path, PathBuf::from("/var/lib/sw.db"));
    }

    #[test]
    fn invalid_bind_is_rejected() {
        let result = Config::from_lookup(|key| (key == "SW_BIND").then(|| "nope".to_owned()));
        assert!(matches!(result, Err(ConfigError::Bind(_))));
    }
}
