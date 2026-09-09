use std::io::{Write, stdout};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Stylize;
use crossterm::{cursor, event, execute, terminal};

use crate::app;
use crate::render::table;
use crate::store::Store;

const POLL: Duration = Duration::from_millis(250);

pub async fn run(interval: u64) -> Result<()> {
    let store = Store::open()?;
    let interval = Duration::from_secs(interval.max(5));

    execute!(stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
    let outcome = loop_until_quit(&store, interval).await;
    execute!(stdout(), cursor::Show, terminal::LeaveAlternateScreen)?;
    outcome
}

async fn loop_until_quit(store: &Store, interval: Duration) -> Result<()> {
    let mut last = Instant::now() - interval;

    loop {
        if last.elapsed() >= interval {
            let reports = app::collect(store, None).await?;
            execute!(
                stdout(),
                terminal::Clear(terminal::ClearType::All),
                cursor::MoveTo(0, 0)
            )?;
            table::print(&reports);
            println!(
                "  {}",
                format!(
                    "refreshing every {}s — r to refresh now, q to quit",
                    interval.as_secs()
                )
                .dark_grey()
            );
            stdout().flush()?;
            last = Instant::now();
        }

        if event::poll(POLL)?
            && let Event::Key(key) = event::read()?
            && let Some(action) = action_for(key)
        {
            match action {
                Action::Quit => return Ok(()),
                Action::Refresh => last = Instant::now() - interval,
            }
        }
    }
}

enum Action {
    Quit,
    Refresh,
}

fn action_for(key: KeyEvent) -> Option<Action> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q') | KeyCode::Esc, _) => Some(Action::Quit),
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(Action::Quit),
        (KeyCode::Char('r'), _) => Some(Action::Refresh),
        _ => None,
    }
}
