use chrono::{Datelike, Timelike, Utc};

use kelvin_core::{KelvinError, KelvinResult};

use super::MINUTE_MS;

const MAX_CRON_SCAN_MINUTES: usize = 1_051_200;

#[derive(Debug, Clone)]
pub(crate) struct CronSchedule {
    raw: String,
    minutes: Vec<bool>,
    hours: Vec<bool>,
    month_days: Vec<bool>,
    months: Vec<bool>,
    week_days: Vec<bool>,
    month_days_wild: bool,
    week_days_wild: bool,
}

impl CronSchedule {
    pub(crate) fn parse(raw: &str) -> KelvinResult<Self> {
        let trimmed = raw.trim();
        let parts = trimmed.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 5 {
            return Err(KelvinError::InvalidInput(
                "cron must have exactly 5 fields".to_string(),
            ));
        }
        Ok(Self {
            raw: trimmed.to_string(),
            minutes: parse_field(parts[0], 0, 59, false)?,
            hours: parse_field(parts[1], 0, 23, false)?,
            month_days: parse_field(parts[2], 1, 31, false)?,
            months: parse_field(parts[3], 1, 12, false)?,
            week_days: parse_field(parts[4], 0, 7, true)?,
            month_days_wild: parts[2].trim() == "*",
            week_days_wild: parts[4].trim() == "*",
        })
    }

    pub(crate) fn raw(&self) -> &str {
        &self.raw
    }

    pub(crate) fn first_slot_at_or_after(&self, start_ms: u128) -> KelvinResult<u128> {
        let mut candidate = start_ms;
        for _ in 0..MAX_CRON_SCAN_MINUTES {
            if self.matches(candidate)? {
                return Ok(candidate);
            }
            candidate = candidate.saturating_add(MINUTE_MS);
        }
        Err(KelvinError::InvalidInput(format!(
            "cron '{}' produced no matching slot within scan window",
            self.raw
        )))
    }

    pub(crate) fn next_slot_after(&self, slot_ms: u128) -> KelvinResult<u128> {
        self.first_slot_at_or_after(slot_ms.saturating_add(MINUTE_MS))
    }

    fn matches(&self, slot_ms: u128) -> KelvinResult<bool> {
        let date_time =
            chrono::DateTime::<Utc>::from_timestamp_millis(slot_ms.min(i64::MAX as u128) as i64)
                .ok_or_else(|| {
                    KelvinError::InvalidInput("invalid scheduler timestamp".to_string())
                })?;

        let minute = date_time.minute() as usize;
        let hour = date_time.hour() as usize;
        let day = date_time.day() as usize;
        let month = date_time.month() as usize;
        let weekday = date_time.weekday().num_days_from_sunday() as usize;

        if !self.minutes[minute] || !self.hours[hour] || !self.months[month] {
            return Ok(false);
        }

        let month_day_match = self.month_days[day];
        let week_day_match = self.week_days[weekday];
        let day_ok = match (self.month_days_wild, self.week_days_wild) {
            (true, true) => true,
            (true, false) => week_day_match,
            (false, true) => month_day_match,
            (false, false) => month_day_match || week_day_match,
        };

        Ok(day_ok)
    }
}

fn parse_field(raw: &str, min: usize, max: usize, sunday_alias: bool) -> KelvinResult<Vec<bool>> {
    if raw.trim().is_empty() {
        return Err(KelvinError::InvalidInput(
            "cron contains empty field".to_string(),
        ));
    }
    let mut field = vec![false; max.saturating_add(1)];
    for item in raw.split(',') {
        let (range_raw, step) = match item.split_once('/') {
            Some((range, step)) => {
                let parsed = step.trim().parse::<usize>().map_err(|_| {
                    KelvinError::InvalidInput(format!("invalid cron step '{}'", step.trim()))
                })?;
                if parsed == 0 {
                    return Err(KelvinError::InvalidInput(
                        "cron step must be >= 1".to_string(),
                    ));
                }
                (range.trim(), parsed)
            }
            None => (item.trim(), 1),
        };
        let (start, end) = if range_raw == "*" {
            (min, max)
        } else if let Some((start, end)) = range_raw.split_once('-') {
            (
                parse_value(start.trim(), min, max, sunday_alias)?,
                parse_value(end.trim(), min, max, sunday_alias)?,
            )
        } else {
            let value = parse_value(range_raw, min, max, sunday_alias)?;
            (value, value)
        };
        if start > end {
            return Err(KelvinError::InvalidInput(format!(
                "invalid cron range '{}'",
                range_raw
            )));
        }

        let mut value = start;
        while value <= end {
            field[value] = true;
            value = value.saturating_add(step);
            if value == 0 {
                break;
            }
        }
    }
    Ok(field)
}

fn parse_value(raw: &str, min: usize, max: usize, sunday_alias: bool) -> KelvinResult<usize> {
    let mut value = raw
        .parse::<usize>()
        .map_err(|_| KelvinError::InvalidInput(format!("invalid cron value '{}'", raw)))?;
    if sunday_alias && value == 7 {
        value = 0;
    }
    if value < min || value > max {
        return Err(KelvinError::InvalidInput(format!(
            "cron value '{}' must be between {} and {}",
            raw, min, max
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::CronSchedule;

    #[test]
    fn cron_parser_accepts_common_patterns() {
        CronSchedule::parse("* * * * *").expect("wildcard cron");
        CronSchedule::parse("*/5 1,2 1-5 * 1-3").expect("mixed cron");
        assert!(CronSchedule::parse("61 * * * *").is_err());
        assert!(CronSchedule::parse("* * *").is_err());
    }
}
