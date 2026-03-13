use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
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
use std::time::{SystemTime, UNIX_EPOCH};
use yoinker_common::{ClipboardEntry, Config, Request, Response};

pub enum TuiAction {
    Select(usize),
}

/// Returns the selected action or None if cancelled.
pub async fn run(
    entries: Vec<ClipboardEntry>,
    config: &Config,
) -> Result<Option<TuiAction>, Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let result = run_loop(&mut terminal, entries, config).await;

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

fn relative_time(timestamp: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let diff = now.saturating_sub(timestamp);
    if diff < 60 {
        format!("{}s", diff)
    } else if diff < 3600 {
        format!("{}m", diff / 60)
    } else if diff < 86400 {
        format!("{}h", diff / 3600)
    } else {
        format!("{}d", diff / 86400)
    }
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    mut entries: Vec<ClipboardEntry>,
    config: &Config,
) -> Result<Option<TuiAction>, Box<dyn std::error::Error>> {
    let matcher = SkimMatcherV2::default();
    let mut query = String::new();
    let mut list_state = ListState::default();
    list_state.select(Some(0));

    loop {
        // Filter entries based on query, pinned always first
        let filtered: Vec<(usize, &ClipboardEntry, i64)> = {
            let mut result: Vec<_> = if query.is_empty() {
                entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| (i, e, 0i64))
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
            // Stable partition: pinned first, preserve order within each group
            result.sort_by_key(|(_, e, _)| if e.pinned { 0 } else { 1 });
            result
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

        let visible_height = terminal.size()?.height.saturating_sub(6) as usize; // approx list area

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

            // Entry list with relative timestamps and pin icons
            let items: Vec<ListItem> = filtered
                .iter()
                .map(|(_, entry, _)| {
                    let time = relative_time(entry.timestamp);
                    let preview = entry.content.preview(60);
                    if entry.pinned {
                        ListItem::new(format!("{:>4} | {} [pin]", time, preview))
                            .style(Style::default().fg(Color::Cyan).bold())
                    } else {
                        ListItem::new(format!("{:>4} | {}", time, preview))
                    }
                })
                .collect();

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Clipboard History "),
                )
                .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
                .highlight_symbol("▸ ");

            frame.render_stateful_widget(list, chunks[1], &mut list_state);

            // Help bar
            let help = Paragraph::new(
                " Enter: select | Esc/q: cancel | ↑↓: navigate | C-d/C-u: page | C-p: pin | C-x: delete",
            )
            .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(help, chunks[2]);
        })?;

        // Handle input
        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Char('q') if query.is_empty() => return Ok(None),
                    KeyCode::Enter => {
                        if let Some(sel) = list_state.selected() {
                            if let Some((orig_idx, _, _)) = filtered.get(sel) {
                                return Ok(Some(TuiAction::Select(*orig_idx)));
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
                    // Ctrl+D: page down (half page)
                    KeyCode::Char('d') if ctrl => {
                        if let Some(sel) = list_state.selected() {
                            let half = visible_height / 2;
                            let new_sel = (sel + half).min(filtered.len().saturating_sub(1));
                            list_state.select(Some(new_sel));
                        }
                    }
                    // Ctrl+U: page up (half page)
                    KeyCode::Char('u') if ctrl => {
                        if let Some(sel) = list_state.selected() {
                            let half = visible_height / 2;
                            let new_sel = sel.saturating_sub(half);
                            list_state.select(Some(new_sel));
                        }
                    }
                    // Ctrl+P: toggle pin
                    KeyCode::Char('p') if ctrl => {
                        if let Some(sel) = list_state.selected() {
                            if let Some((orig_idx, _, _)) = filtered.get(sel) {
                                let idx = *orig_idx;
                                let pinned = entries[idx].pinned;
                                let req = if pinned {
                                    Request::Unpin { index: idx }
                                } else {
                                    Request::Pin { index: idx }
                                };
                                if let Ok(Response::Ok) = crate::ipc::send(config, req).await {
                                    entries[idx].pinned = !pinned;
                                }
                            }
                        }
                    }
                    // Ctrl+X: delete entry
                    KeyCode::Char('x') if ctrl => {
                        if let Some(sel) = list_state.selected() {
                            if let Some((orig_idx, _, _)) = filtered.get(sel) {
                                let idx = *orig_idx;
                                // Send delete to daemon
                                if let Ok(Response::Ok) =
                                    crate::ipc::send(config, Request::Delete { index: idx }).await
                                {
                                    entries.remove(idx);
                                    if entries.is_empty() {
                                        return Ok(None);
                                    }
                                    // Let the clamp logic at loop top fix selection
                                }
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
