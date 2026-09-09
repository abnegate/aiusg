use std::io::{IsTerminal, Write, stdout};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Stylize;
use crossterm::{cursor, event, execute, terminal};

use crate::app;
use crate::render::table;
use crate::store::Store;

const POLL: Duration = Duration::from_millis(250);
const MINIMUM_INTERVAL: u64 = 5;

pub async fn run(interval: u64) -> Result<()> {
    if !stdout().is_terminal() {
        bail!("`aiusg watch` needs a terminal; use `aiusg` or `aiusg --json` when piping");
    }

    let store = Store::open()?;
    let interval = Duration::from_secs(interval.max(MINIMUM_INTERVAL));

    terminal::enable_raw_mode()?;
    execute!(stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;

    let outcome = tick(&store, interval).await;

    execute!(stdout(), cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    outcome
}

async fn tick(store: &Store, interval: Duration) -> Result<()> {
    let mut due = Instant::now();

    loop {
        if Instant::now() >= due {
            let reports = app::collect(store, None).await?;
            let footer = format!(
                "  refreshing every {}s — r to refresh now, q to quit",
                interval.as_secs()
            );

            execute!(
                stdout(),
                terminal::Clear(terminal::ClearType::All),
                cursor::MoveTo(0, 0)
            )?;
            write!(
                stdout(),
                "{}{}",
                table::render(&reports).replace('\n', "\r\n"),
                footer.dark_grey()
            )?;
            stdout().flush()?;
            due = Instant::now() + interval;
        }

        if event::poll(POLL)?
            && let Event::Key(key) = event::read()?
        {
            match action_for(key) {
                Some(Action::Quit) => return Ok(()),
                Some(Action::Refresh) => due = Instant::now(),
                None => {}
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
        (KeyCode::Char('c' | 'd'), KeyModifiers::CONTROL) => Some(Action::Quit),
        (KeyCode::Char('r'), _) => Some(Action::Refresh),
        _ => None,
    }
}
