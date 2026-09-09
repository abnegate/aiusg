use crossterm::style::Stylize;

use crate::model::Report;
use crate::render::{Severity, bar, counts, percent, resets, severity};

pub fn empty() {
    println!("No accounts yet.\n");
    println!("  aiusg import              adopt accounts already signed in to their CLI");
    println!("  aiusg login <provider>    sign in to claude, codex, gemini, copilot or grok");
}

pub fn print(reports: &[Report]) {
    print!("{}", render(reports));
}

pub fn render(reports: &[Report]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let width = reports
        .iter()
        .filter_map(|report| match report {
            Report::Ok(usage) => Some(usage),
            Report::Failed { .. } => None,
        })
        .flat_map(|usage| usage.windows.iter())
        .map(|window| window.name.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(10, 28);

    let _ = writeln!(out);
    for report in reports {
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
