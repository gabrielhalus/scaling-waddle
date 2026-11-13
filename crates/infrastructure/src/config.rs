use std::net::SocketAddr;
use std::path::PathBuf;

const DEFAULT_MAX_CONCURRENT_RUNS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub database_path: PathBuf,
    pub bind: SocketAddr,
    pub jwt_secret: String,
    pub allowed_internal_networks: String,
    pub max_concurrent_runs: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("SW_BIND invalide: {0}")]
    Bind(String),
    #[error("SW_MAX_CONCURRENT_RUNS doit être un entier entre 1 et 256")]
    MaxConcurrentRuns,
    #[error("SW_JWT_SECRET absent ou de moins de 32 caractères")]
    JwtSecret,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let bind = lookup("SW_BIND").unwrap_or_else(|| "127.0.0.1:8080".to_owned());
        let jwt_secret = lookup("SW_JWT_SECRET")
            .filter(|secret| secret.len() >= 32)
            .ok_or(ConfigError::JwtSecret)?;
        let max_concurrent_runs = match lookup("SW_MAX_CONCURRENT_RUNS") {
            None => DEFAULT_MAX_CONCURRENT_RUNS,
            Some(text) => text
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=256).contains(n))
                .ok_or(ConfigError::MaxConcurrentRuns)?,
        };
        Ok(Self {
            database_path: lookup("SW_DATABASE_PATH")
                .map_or_else(|| PathBuf::from("data/scaling-waddle.db"), PathBuf::from),
            bind: bind.parse().map_err(|_| ConfigError::Bind(bind))?,
            jwt_secret,
            allowed_internal_networks: lookup("SW_ALLOWED_INTERNAL_NETWORKS").unwrap_or_default(),
            max_concurrent_runs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    fn only_secret(key: &str) -> Option<String> {
        (key == "SW_JWT_SECRET").then(|| SECRET.to_owned())
    }

    #[test]
    fn defaults_apply_with_only_the_secret() {
        let config = Config::from_lookup(only_secret).unwrap();
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
            other => only_secret(other),
        })
        .unwrap();
        assert_eq!(config.bind.port(), 9000);
        assert_eq!(config.database_path, PathBuf::from("/var/lib/sw.db"));
    }

    #[test]
    fn invalid_bind_is_rejected() {
        let result = Config::from_lookup(|key| match key {
            "SW_BIND" => Some("nope".to_owned()),
            other => only_secret(other),
        });
        assert!(matches!(result, Err(ConfigError::Bind(_))));
    }

    #[test]
    fn allowed_internal_networks_default_to_empty_and_can_be_set() {
        assert_eq!(
            Config::from_lookup(only_secret)
                .unwrap()
                .allowed_internal_networks,
            ""
        );
        let config = Config::from_lookup(|key| match key {
            "SW_ALLOWED_INTERNAL_NETWORKS" => Some("10.0.0.0/8".to_owned()),
            other => only_secret(other),
        })
        .unwrap();
        assert_eq!(config.allowed_internal_networks, "10.0.0.0/8");
    }

    #[test]
    fn missing_secret_is_rejected() {
        assert!(matches!(
            Config::from_lookup(|_| None),
            Err(ConfigError::JwtSecret)
        ));
    }

    #[test]
    fn short_secret_is_rejected() {
        let result =
            Config::from_lookup(|key| (key == "SW_JWT_SECRET").then(|| "court".to_owned()));
        assert!(matches!(result, Err(ConfigError::JwtSecret)));
    }

    #[test]
    fn max_concurrent_runs_defaults_to_4_and_is_bounded() {
        let with = |value: Option<&'static str>| {
            Config::from_lookup(move |key| match key {
                "SW_JWT_SECRET" => Some(SECRET.to_owned()),
                "SW_MAX_CONCURRENT_RUNS" => value.map(str::to_owned),
                _ => None,
            })
        };
        assert_eq!(with(None).unwrap().max_concurrent_runs, 4);
        assert_eq!(with(Some("8")).unwrap().max_concurrent_runs, 8);
        for bad in ["0", "257", "x", "-1"] {
            assert!(
                matches!(with(Some(bad)), Err(ConfigError::MaxConcurrentRuns)),
                "{bad}"
            );
        }
    }
}
