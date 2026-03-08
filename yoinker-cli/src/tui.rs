use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use std::io::stdout;
use yoinker_common::{ClipboardEntry, Config};

/// Returns the selected entry's index in the original list, or None if cancelled.
pub async fn run(
    entries: Vec<ClipboardEntry>,
    _config: &Config,
) -> Result<Option<usize>, Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let result = run_loop(&mut terminal, entries);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    entries: Vec<ClipboardEntry>,
) -> Result<Option<usize>, Box<dyn std::error::Error>> {
    let matcher = SkimMatcherV2::default();
    let mut query = String::new();
    let mut list_state = ListState::default();
    list_state.select(Some(0));

    loop {
        // Filter entries based on query
        let filtered: Vec<(usize, &ClipboardEntry, i64)> = if query.is_empty() {
            entries
                .iter()
                .enumerate()
                .map(|(i, e)| (i, e, 0))
                .collect()
        } else {
            let mut scored: Vec<_> = entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| {
                    let preview = e.content.preview(200);
                    matcher
                        .fuzzy_match(&preview, &query)
                        .map(|score| (i, e, score))
                })
                .collect();
            scored.sort_by(|a, b| b.2.cmp(&a.2));
            scored
        };

        // Clamp selection
        if filtered.is_empty() {
            list_state.select(None);
        } else if let Some(sel) = list_state.selected() {
            if sel >= filtered.len() {
                list_state.select(Some(filtered.len() - 1));
            }
        } else {
            list_state.select(Some(0));
        }

        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // search bar
                    Constraint::Min(1),    // list
                    Constraint::Length(1), // help
                ])
                .split(frame.area());

            // Search bar
            let search = Paragraph::new(format!("> {}", query))
                .block(Block::default().borders(Borders::ALL).title(" Search "));
            frame.render_widget(search, chunks[0]);

            // Entry list
            let items: Vec<ListItem> = filtered
                .iter()
                .map(|(_, entry, _)| {
                    let pin = if entry.pinned { " [pinned]" } else { "" };
                    let preview = entry.content.preview(80);
                    ListItem::new(format!("{}{}", preview, pin))
                })
                .collect();

            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Clipboard History "))
                .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
                .highlight_symbol("▸ ");

            frame.render_stateful_widget(list, chunks[1], &mut list_state);

            // Help bar
            let help = Paragraph::new(" Enter: select  |  Esc/q: cancel  |  ↑↓: navigate  |  Type to search")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(help, chunks[2]);
        })?;

        // Handle input
        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Char('q') if query.is_empty() => return Ok(None),
                    KeyCode::Enter => {
                        if let Some(sel) = list_state.selected() {
                            if let Some((orig_idx, _, _)) = filtered.get(sel) {
                                return Ok(Some(*orig_idx));
                            }
                        }
                        return Ok(None);
                    }
                    KeyCode::Up => {
                        if let Some(sel) = list_state.selected() {
                            if sel > 0 {
                                list_state.select(Some(sel - 1));
                            }
                        }
                    }
                    KeyCode::Down => {
                        if let Some(sel) = list_state.selected() {
                            if sel + 1 < filtered.len() {
                                list_state.select(Some(sel + 1));
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        list_state.select(Some(0));
                    }
                    KeyCode::Char(c) => {
                        query.push(c);
                        list_state.select(Some(0));
                    }
                    _ => {}
                }
            }
        }
    }
}
