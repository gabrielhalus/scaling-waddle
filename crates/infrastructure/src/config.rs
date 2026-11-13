use std::net::SocketAddr;
use std::path::PathBuf;

use crate::crypto::{CryptoError, Keyring};

const DEFAULT_MAX_CONCURRENT_RUNS: usize = 4;
const DEFAULT_RETENTION_DAYS: u32 = 30;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_path: PathBuf,
    pub bind: SocketAddr,
    pub jwt_secret: String,
    pub allowed_internal_networks: String,
    pub max_concurrent_runs: usize,
    pub retention_days: u32,
    pub keyring: Keyring,
    pub web_dir: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("SW_BIND invalide: {0}")]
    Bind(String),
    #[error("SW_MAX_CONCURRENT_RUNS doit être un entier entre 1 et 256")]
    MaxConcurrentRuns,
    #[error("SW_RETENTION_DAYS doit être un entier positif (0 pour tout conserver)")]
    RetentionDays,
    #[error("clés de secrets invalides: {0}")]
    SecretKeys(#[from] CryptoError),
    #[error("SW_JWT_SECRET absent ou de moins de 32 caractères")]
    JwtSecret,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    // Le worker ne signe aucun jeton : le secret JWT reste optionnel pour lui.
    pub fn worker_from_env() -> Result<Self, ConfigError> {
        Self::build(|key| std::env::var(key).ok(), false)
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        Self::build(lookup, true)
    }

    fn build(
        lookup: impl Fn(&str) -> Option<String>,
        require_jwt_secret: bool,
    ) -> Result<Self, ConfigError> {
        let bind = lookup("SW_BIND").unwrap_or_else(|| "127.0.0.1:8080".to_owned());
        let jwt_secret = match lookup("SW_JWT_SECRET").filter(|secret| secret.len() >= 32) {
            Some(secret) => secret,
            None if require_jwt_secret => return Err(ConfigError::JwtSecret),
            None => String::new(),
        };
        let max_concurrent_runs = match lookup("SW_MAX_CONCURRENT_RUNS") {
            None => DEFAULT_MAX_CONCURRENT_RUNS,
            Some(text) => text
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=256).contains(n))
                .ok_or(ConfigError::MaxConcurrentRuns)?,
        };
        let retention_days = match lookup("SW_RETENTION_DAYS") {
            None => DEFAULT_RETENTION_DAYS,
            Some(text) => text
                .trim()
                .parse::<u32>()
                .map_err(|_| ConfigError::RetentionDays)?,
        };
        let keyring = Keyring::parse(
            &lookup("SW_SECRET_KEYS").unwrap_or_default(),
            lookup("SW_SECRET_ACTIVE_KEY")
                .filter(|id| !id.trim().is_empty())
                .as_deref(),
        )?;
        Ok(Self {
            database_path: lookup("SW_DATABASE_PATH")
                .map_or_else(|| PathBuf::from("data/scaling-waddle.db"), PathBuf::from),
            bind: bind.parse().map_err(|_| ConfigError::Bind(bind))?,
            jwt_secret,
            allowed_internal_networks: lookup("SW_ALLOWED_INTERNAL_NETWORKS").unwrap_or_default(),
            max_concurrent_runs,
            retention_days,
            keyring,
            web_dir: lookup("SW_WEB_DIR")
                .filter(|dir| !dir.trim().is_empty())
                .map(PathBuf::from),
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

    #[test]
    fn retention_defaults_to_30_days_and_zero_disables_it() {
        let with = |value: Option<&'static str>| {
            Config::from_lookup(move |key| match key {
                "SW_JWT_SECRET" => Some(SECRET.to_owned()),
                "SW_RETENTION_DAYS" => value.map(str::to_owned),
                _ => None,
            })
        };
        assert_eq!(with(None).unwrap().retention_days, 30);
        assert_eq!(with(Some("0")).unwrap().retention_days, 0);
        assert_eq!(with(Some("90")).unwrap().retention_days, 90);
        for bad in ["-1", "x", ""] {
            assert!(
                matches!(with(Some(bad)), Err(ConfigError::RetentionDays)),
                "{bad}"
            );
        }
    }

    #[test]
    fn secret_keys_are_optional_and_validated() {
        let with = |keys: Option<String>, active: Option<&'static str>| {
            Config::from_lookup(move |key| match key {
                "SW_JWT_SECRET" => Some(SECRET.to_owned()),
                "SW_SECRET_KEYS" => keys.clone(),
                "SW_SECRET_ACTIVE_KEY" => active.map(str::to_owned),
                _ => None,
            })
        };
        assert!(!with(None, None).unwrap().keyring.is_enabled());
        let key = hex::encode([7u8; 32]);
        let config = with(Some(format!("k1={key}")), Some("k1")).unwrap();
        assert_eq!(config.keyring.active_id(), Some("k1"));
        assert!(matches!(
            with(Some(format!("k1={key}")), None),
            Err(ConfigError::SecretKeys(_))
        ));
    }

    #[test]
    fn empty_secret_variables_mean_disabled() {
        let config = Config::from_lookup(|key| match key {
            "SW_JWT_SECRET" => Some(SECRET.to_owned()),
            "SW_SECRET_KEYS" | "SW_SECRET_ACTIVE_KEY" => Some(String::new()),
            _ => None,
        })
        .unwrap();
        assert!(!config.keyring.is_enabled());
    }

    #[test]
    fn the_worker_does_not_need_a_jwt_secret() {
        let lookup = |_: &str| None;
        assert!(matches!(
            Config::from_lookup(lookup),
            Err(ConfigError::JwtSecret)
        ));
        let config = Config::build(lookup, false).unwrap();
        assert!(config.jwt_secret.is_empty());
        assert_eq!(config.max_concurrent_runs, 4);
    }

    #[test]
    fn web_dir_is_optional() {
        let with = |value: Option<&'static str>| {
            Config::from_lookup(move |key| match key {
                "SW_JWT_SECRET" => Some(SECRET.to_owned()),
                "SW_WEB_DIR" => value.map(str::to_owned),
                _ => None,
            })
            .unwrap()
            .web_dir
        };
        assert_eq!(with(None), None);
        assert_eq!(with(Some("")), None);
        assert_eq!(with(Some("/srv/web")), Some(PathBuf::from("/srv/web")));
    }
}
