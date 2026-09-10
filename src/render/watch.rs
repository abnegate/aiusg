use std::io::{IsTerminal, Write, stdout};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Stylize;
use crossterm::{cursor, event, execute, terminal};

use crate::app;
use crate::cli::{Sort, StatusArgs};
use crate::model::Report;
use crate::render::table;
use crate::store::Store;

const POLL: Duration = Duration::from_millis(250);
const MINIMUM_INTERVAL: u64 = 5;

pub async fn run(interval: u64, store: &Store, args: &StatusArgs) -> Result<()> {
    if !stdout().is_terminal() {
        bail!("`aiusg --watch` needs a terminal; use `aiusg` or `aiusg --json` when piping");
    }

    let interval = Duration::from_secs(interval.max(MINIMUM_INTERVAL));

    terminal::enable_raw_mode()?;
    execute!(stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;

    let outcome = tick(store, interval, args).await;

    execute!(stdout(), cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    outcome
}

async fn tick(store: &Store, interval: Duration, args: &StatusArgs) -> Result<()> {
    let mut sort = args.sort;
    let mut reports = Vec::new();
    let mut due = Instant::now();

    loop {
        if Instant::now() >= due {
            reports = app::collect(store, args.provider).await?;
            due = Instant::now() + interval;
            draw(&reports, args.all, sort, interval)?;
        }

        if event::poll(POLL)?
            && let Event::Key(key) = event::read()?
        {
            match action_for(key) {
                Some(Action::Quit) => return Ok(()),
                Some(Action::Refresh) => due = Instant::now(),
                Some(Action::Reorder) => {
                    sort = match sort {
                        Sort::Provider => Sort::Usable,
                        Sort::Usable => Sort::Provider,
                    };
                    draw(&reports, args.all, sort, interval)?;
                }
                None => {}
            }
        }
    }
}

fn draw(reports: &[Report], all: bool, sort: Sort, interval: Duration) -> Result<()> {
    let mut ordered = reports.to_vec();
    app::order(&mut ordered, sort);

    execute!(
        stdout(),
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0)
    )?;
    write!(
        stdout(),
        "{}{}",
        table::render(&ordered, all).replace('\n', "\r\n"),
        footer(sort, interval).dark_grey()
    )?;
    stdout().flush()?;
    Ok(())
}

fn footer(sort: Sort, interval: Duration) -> String {
    let order = match sort {
        Sort::Provider => "by provider",
        Sort::Usable => "most usable first",
    };
    format!(
        "  every {}s · {order} — r refresh, s reorder, q quit",
        interval.as_secs()
    )
}

enum Action {
    Quit,
    Refresh,
    Reorder,
}

fn action_for(key: KeyEvent) -> Option<Action> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q') | KeyCode::Esc, _) => Some(Action::Quit),
        (KeyCode::Char('c' | 'd'), KeyModifiers::CONTROL) => Some(Action::Quit),
        (KeyCode::Char('r'), _) => Some(Action::Refresh),
        (KeyCode::Char('s'), _) => Some(Action::Reorder),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(character: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
    }

    #[test]
    fn s_reorders_the_table() {
        assert!(matches!(action_for(press('s')), Some(Action::Reorder)));
        assert!(matches!(action_for(press('r')), Some(Action::Refresh)));
        assert!(matches!(action_for(press('q')), Some(Action::Quit)));
        assert!(action_for(press('x')).is_none());
    }

    #[test]
    fn the_footer_names_the_order_it_is_showing() {
        let usable = footer(Sort::Usable, Duration::from_secs(30));
        assert!(usable.contains("every 30s"), "got: {usable}");
        assert!(usable.contains("most usable first"), "got: {usable}");
        assert!(usable.contains("s reorder"), "got: {usable}");

        let provider = footer(Sort::Provider, Duration::from_secs(5));
        assert!(provider.contains("by provider"), "got: {provider}");
    }
}
