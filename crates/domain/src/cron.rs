use chrono::{DateTime, Utc};
use croner::Cron;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("expression cron invalide: {0}")]
pub struct InvalidSchedule(pub String);

fn parse(schedule: &str) -> Result<Cron, InvalidSchedule> {
    schedule
        .parse()
        .map_err(|err: croner::errors::CronError| InvalidSchedule(err.to_string()))
}

pub fn validate(schedule: &str) -> Result<(), InvalidSchedule> {
    parse(schedule).map(|_| ())
}

// Prochaine occurrence UTC strictement après `after_secs`, en secondes Unix.
pub fn next_after(schedule: &str, after_secs: i64) -> Result<Option<i64>, InvalidSchedule> {
    let cron = parse(schedule)?;
    let Some(after) = DateTime::<Utc>::from_timestamp(after_secs, 0) else {
        return Ok(None);
    };
    Ok(cron
        .find_next_occurrence(&after, false)
        .ok()
        .map(|next| next.timestamp()))
}

// Dernière occurrence UTC dans l'intervalle ]after_secs, until_secs].
pub fn latest_between(
    schedule: &str,
    after_secs: i64,
    until_secs: i64,
) -> Result<Option<i64>, InvalidSchedule> {
    let mut latest = None;
    let mut cursor = after_secs;
    while let Some(next) = next_after(schedule, cursor)? {
        if next > until_secs {
            break;
        }
        latest = Some(next);
        cursor = next;
    }
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-11-13T10:00:00Z
    const T: i64 = 1_794_564_000;

    #[test]
    fn validates_five_field_expressions() {
        assert!(validate("*/5 * * * *").is_ok());
        assert!(validate("0 9 * * 1-5").is_ok());
        for bad in ["", "nope", "* * * *", "61 * * * *"] {
            assert!(validate(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn next_after_is_strictly_after() {
        assert_eq!(next_after("*/5 * * * *", T).unwrap(), Some(T + 300));
        assert_eq!(next_after("*/5 * * * *", T - 1).unwrap(), Some(T));
        assert_eq!(next_after("0 12 * * *", T).unwrap(), Some(T + 2 * 3600));
    }

    #[test]
    fn latest_between_returns_only_the_most_recent_missed_fire() {
        assert_eq!(
            latest_between("*/5 * * * *", T, T + 1000).unwrap(),
            Some(T + 900)
        );
        assert_eq!(latest_between("*/5 * * * *", T, T + 299).unwrap(), None);
        assert_eq!(
            latest_between("*/5 * * * *", T, T + 300).unwrap(),
            Some(T + 300)
        );
    }
}
