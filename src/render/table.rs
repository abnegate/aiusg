use crossterm::style::Stylize;

use crate::model::Report;
use crate::render::{Severity, bar, counts, percent, resets, severity};

pub fn empty() {
    println!("No accounts yet.\n");
    println!("  aiusg import              adopt accounts already signed in to their CLI");
    println!("  aiusg login <provider>    sign in to claude, codex, gemini, copilot or grok");
}

pub fn print(reports: &[Report], all: bool) {
    print!("{}", render(reports, all));
}

pub fn render(reports: &[Report], all: bool) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let shown: Vec<&Report> = reports
        .iter()
        .filter(|report| all || !report.is_signed_out())
        .collect();
    let hidden = reports.len() - shown.len();

    let width = shown
        .iter()
        .filter_map(|report| match report {
            Report::Ok(usage) => Some(usage),
            _ => None,
        })
        .flat_map(|usage| usage.windows.iter())
        .map(|window| window.name.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(10, 28);

    let _ = writeln!(out);
    for report in shown {
        match report {
            Report::Ok(usage) => {
                let plan = usage.plan.as_deref().unwrap_or("-");
                let _ = writeln!(
                    out,
                    "  {}  {}  {}",
                    format!("{:<8}", usage.provider.display()).bold(),
                    usage.label.as_str().grey(),
                    plan.dark_grey()
                );

                if usage.windows.is_empty() {
                    let _ = writeln!(out, "    {}", "no limits reported".dark_grey());
                }

                for window in &usage.windows {
                    let painted = paint(&bar(window.used_percent), severity(window.used_percent));
                    let _ = writeln!(
                        out,
                        "    {:<width$} {} {}  {:<11} {}",
                        truncate(&window.name, width),
                        painted,
                        percent(window),
                        counts(window),
                        resets(window.resets_at).dark_grey()
                    );
                }
                let _ = writeln!(out);
            }
            Report::SignedOut {
                provider, label, ..
            } => {
                let _ = writeln!(
                    out,
                    "  {}  {}  {}",
                    format!("{:<8}", provider.display()).bold(),
                    label.as_str().grey(),
                    "signed out".dark_grey()
                );
                let _ = writeln!(out);
            }
            Report::Failed {
                provider,
                label,
                message,
                ..
            } => {
                let _ = writeln!(
                    out,
                    "  {}  {}  {}",
                    format!("{:<8}", provider.display()).bold(),
                    label.as_str().grey(),
                    "failed".red()
                );
                let _ = writeln!(out, "    {}\n", message.as_str().dark_red());
            }
        }
    }

    if hidden > 0 {
        let plural = if hidden == 1 { "account" } else { "accounts" };
        let _ = writeln!(
            out,
            "  {}",
            format!("{hidden} signed-out {plural} hidden — aiusg --all to show").dark_grey()
        );
        let _ = writeln!(out);
    }
    out
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    let kept: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

fn paint(bar: &str, severity: Severity) -> String {
    let styled = match severity {
        Severity::Fine => bar.green(),
        Severity::Warning => bar.yellow(),
        Severity::Critical => bar.magenta(),
        Severity::Exhausted => bar.red(),
        Severity::Unknown => bar.dark_grey(),
    };
    styled.to_string()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::model::{AccountId, Provider, Usage, Window};

    fn signed_out() -> Report {
        Report::SignedOut {
            account: AccountId::new(Provider::Gemini, "a@example.com"),
            provider: Provider::Gemini,
            label: "a@example.com".to_owned(),
        }
    }

    fn healthy() -> Report {
        Report::Ok(Usage {
            account: AccountId::new(Provider::Claude, "b@example.com"),
            provider: Provider::Claude,
            label: "b@example.com".to_owned(),
            plan: Some("max".to_owned()),
            windows: vec![Window::from_percent("7d", 42.0)],
            fetched_at: Utc::now(),
        })
    }

    #[test]
    fn signed_out_accounts_are_hidden_by_default() {
        let rendered = render(&[healthy(), signed_out()], false);
        assert!(
            !rendered.contains("Gemini"),
            "a signed-out account must not be listed"
        );
        assert!(rendered.contains("Claude"));
        assert!(
            rendered.contains("1 signed-out account hidden"),
            "the count must still be reported"
        );
    }

    #[test]
    fn all_shows_them_again() {
        let rendered = render(&[healthy(), signed_out()], true);
        assert!(rendered.contains("Gemini"));
        assert!(rendered.contains("signed out"));
        assert!(!rendered.contains("hidden"));
    }

    #[test]
    fn nothing_is_reported_when_every_account_is_healthy() {
        let rendered = render(&[healthy()], false);
        assert!(!rendered.contains("hidden"));
    }

    #[test]
    fn the_plural_matches_the_count() {
        let rendered = render(&[signed_out(), signed_out()], false);
        assert!(
            rendered.contains("2 signed-out accounts hidden"),
            "got: {rendered}"
        );
    }
}
