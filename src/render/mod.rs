pub mod table;
pub mod watch;

use chrono::{DateTime, Utc};

use crate::model::Window;

pub const BAR_WIDTH: usize = 20;

pub fn bar(used_percent: Option<f64>) -> String {
    let Some(percent) = used_percent else {
        return "─".repeat(BAR_WIDTH);
    };
    let filled = ((percent / 100.0).clamp(0.0, 1.0) * BAR_WIDTH as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled))
}

pub fn percent(window: &Window) -> String {
    match window.used_percent {
        Some(value) => format!("{value:>3.0}%"),
        None => "  ?%".to_owned(),
    }
}

pub fn counts(window: &Window) -> String {
    match (window.used, window.limit) {
        (Some(used), Some(limit)) => format!("{used}/{limit}"),
        _ => String::new(),
    }
}

pub fn resets(at: Option<DateTime<Utc>>) -> String {
    resets_from(at, Utc::now())
}

pub fn resets_from(at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(at) = at else {
        return String::new();
    };
    let remaining = at.signed_duration_since(now);
    if remaining.num_seconds() <= 0 {
        return "resets now".to_owned();
    }

    let days = remaining.num_days();
    let hours = remaining.num_hours() % 24;
    let minutes = remaining.num_minutes() % 60;

    if days > 0 {
        format!("resets in {days}d {hours}h")
    } else if hours > 0 {
        format!("resets in {hours}h {minutes}m")
    } else {
        format!("resets in {minutes}m")
    }
}

pub fn severity(used_percent: Option<f64>) -> Severity {
    match used_percent {
        Some(value) if value >= 100.0 => Severity::Exhausted,
        Some(value) if value >= 90.0 => Severity::Critical,
        Some(value) if value >= 75.0 => Severity::Warning,
        Some(_) => Severity::Fine,
        None => Severity::Unknown,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Fine,
    Warning,
    Critical,
    Exhausted,
    Unknown,
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    #[test]
    fn a_full_bar_is_all_blocks() {
        assert_eq!(bar(Some(100.0)), "█".repeat(BAR_WIDTH));
        assert_eq!(bar(Some(0.0)), "░".repeat(BAR_WIDTH));
    }

    #[test]
    fn overage_does_not_overflow_the_bar() {
        let rendered = bar(Some(150.0));
        assert_eq!(rendered.chars().count(), BAR_WIDTH);
    }

    #[test]
    fn an_unknown_percentage_renders_a_dashed_bar() {
        assert_eq!(bar(None).chars().count(), BAR_WIDTH);
    }

    fn at(offset: Duration) -> (Option<DateTime<Utc>>, DateTime<Utc>) {
        let now = DateTime::parse_from_rfc3339("2026-09-09T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        (Some(now + offset), now)
    }

    #[test]
    fn reset_times_read_as_durations() {
        let (reset, now) = at(Duration::minutes(42));
        assert_eq!(resets_from(reset, now), "resets in 42m");

        let (reset, now) = at(Duration::hours(3));
        assert_eq!(resets_from(reset, now), "resets in 3h 0m");

        let (reset, now) = at(Duration::hours(50));
        assert_eq!(resets_from(reset, now), "resets in 2d 2h");
    }

    #[test]
    fn a_past_reset_reads_as_now() {
        let (reset, now) = at(Duration::hours(-1));
        assert_eq!(resets_from(reset, now), "resets now");
        assert_eq!(resets(None), "");
    }

    #[test]
    fn severity_escalates_with_usage() {
        assert_eq!(severity(Some(10.0)), Severity::Fine);
        assert_eq!(severity(Some(80.0)), Severity::Warning);
        assert_eq!(severity(Some(95.0)), Severity::Critical);
        assert_eq!(severity(Some(100.0)), Severity::Exhausted);
        assert_eq!(severity(None), Severity::Unknown);
    }
}
