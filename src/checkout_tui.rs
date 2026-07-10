use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use std::io;
use std::path::PathBuf;

use crate::cmd::checkout::{CloneAction, UnmappedProject};

struct TuiItem {
    remote_url: String,
    normalized_remote: String,
    session_count: usize,
    checked: bool,
    clone_path: String,
}

struct TuiState {
    items: Vec<TuiItem>,
    cursor: usize,
    list_state: ListState,
    editing: Option<usize>,
    edit_buffer: String,
    edit_cursor: usize,
}

impl TuiState {
    fn from_projects(projects: &[UnmappedProject]) -> Self {
        let items: Vec<TuiItem> = projects
            .iter()
            .map(|p| TuiItem {
                remote_url: p.remote_url.clone(),
                normalized_remote: p.normalized_remote.clone(),
                session_count: p.session_count,
                checked: false,
                clone_path: p.suggested_clone_path.to_string_lossy().to_string(),
            })
            .collect();
        let mut list_state = ListState::default();
        if !items.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            items,
            cursor: 0,
            list_state,
            editing: None,
            edit_buffer: String::new(),
            edit_cursor: 0,
        }
    }

    fn toggle_current(&mut self) {
        if let Some(item) = self.items.get_mut(self.cursor) {
            item.checked = !item.checked;
        }
    }

    fn start_edit(&mut self) {
        if let Some(item) = self.items.get(self.cursor) {
            self.edit_buffer = item.clone_path.clone();
            self.edit_cursor = self.edit_buffer.len();
            self.editing = Some(self.cursor);
        }
    }

    fn finish_edit(&mut self) {
        if let Some(idx) = self.editing {
            if let Some(item) = self.items.get_mut(idx) {
                item.clone_path = self.edit_buffer.clone();
            }
            self.editing = None;
        }
    }

    fn cancel_edit(&mut self) {
        self.editing = None;
    }

    fn selected_actions(&self) -> Vec<CloneAction> {
        self.items
            .iter()
            .filter(|i| i.checked)
            .map(|i| CloneAction {
                remote_url: i.remote_url.clone(),
                clone_path: PathBuf::from(&i.clone_path),
            })
            .collect()
    }
}

fn prev_char_boundary(s: &str, from: usize) -> usize {
    let mut pos = from.saturating_sub(1);
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

fn next_char_boundary(s: &str, from: usize) -> usize {
    let mut pos = from + 1;
    while pos < s.len() && !s.is_char_boundary(pos) {
        pos += 1;
    }
    pos
}

pub fn run_tui(projects: &[UnmappedProject]) -> Result<Vec<CloneAction>> {
    enable_raw_mode()?;
    let result = run_tui_inner(projects);
    let _ = disable_raw_mode();
    let _ = Terminal::new(CrosstermBackend::new(io::stdout())).and_then(|mut t| t.clear());
    result
}

fn run_tui_inner(projects: &[UnmappedProject]) -> Result<Vec<CloneAction>> {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let mut state = TuiState::from_projects(projects);
    run_loop(&mut terminal, &mut state)
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut TuiState,
) -> Result<Vec<CloneAction>> {
    loop {
        terminal.draw(|frame| draw(frame, &mut *state))?;

        if let Event::Key(key) = event::read()? {
            if state.editing.is_some() {
                match key.code {
                    KeyCode::Enter => state.finish_edit(),
                    KeyCode::Esc => state.cancel_edit(),
                    KeyCode::Backspace => {
                        if state.edit_cursor > 0 {
                            let prev = prev_char_boundary(&state.edit_buffer, state.edit_cursor);
                            state.edit_buffer.drain(prev..state.edit_cursor);
                            state.edit_cursor = prev;
                        }
                    }
                    KeyCode::Left => {
                        if state.edit_cursor > 0 {
                            state.edit_cursor =
                                prev_char_boundary(&state.edit_buffer, state.edit_cursor);
                        }
                    }
                    KeyCode::Right => {
                        if state.edit_cursor < state.edit_buffer.len() {
                            state.edit_cursor =
                                next_char_boundary(&state.edit_buffer, state.edit_cursor);
                        }
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(Vec::new());
                    }
                    KeyCode::Char(c) => {
                        state.edit_buffer.insert(state.edit_cursor, c);
                        state.edit_cursor += c.len_utf8();
                    }
                    _ => {}
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(Vec::new()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(Vec::new());
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if state.cursor > 0 {
                        state.cursor -= 1;
                        state.list_state.select(Some(state.cursor));
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if state.cursor + 1 < state.items.len() {
                        state.cursor += 1;
                        state.list_state.select(Some(state.cursor));
                    }
                }
                KeyCode::Char(' ') => state.toggle_current(),
                KeyCode::Char('e') => state.start_edit(),
                KeyCode::Char('a') => {
                    let all_checked = state.items.iter().all(|i| i.checked);
                    for item in &mut state.items {
                        item.checked = !all_checked;
                    }
                }
                KeyCode::Enter => {
                    let actions = state.selected_actions();
                    if actions.is_empty() {
                        state.toggle_current();
                        let actions = state.selected_actions();
                        return Ok(actions);
                    }
                    return Ok(actions);
                }
                _ => {}
            }
        }
    }
}

fn draw(frame: &mut ratatui::Frame, state: &mut TuiState) {
    let area = frame.area();

    let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(area);

    let items: Vec<ListItem> = state
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let check = if item.checked { "[x]" } else { "[ ]" };
            let is_current = i == state.cursor;
            let is_editing = state.editing == Some(i);

            let remote_style = if is_current {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let sessions = format!(
                " ({} session{})",
                item.session_count,
                if item.session_count == 1 { "" } else { "s" }
            );

            let path_line = if is_editing {
                let before = &state.edit_buffer[..state.edit_cursor];
                let rest = &state.edit_buffer[state.edit_cursor..];
                let mut chars = rest.chars();
                let cursor_char = chars
                    .next()
                    .map(|c| c.to_string())
                    .unwrap_or(" ".to_string());
                let after = chars.as_str();
                Line::from(vec![
                    Span::raw("    -> "),
                    Span::raw(before.to_string()),
                    Span::styled(
                        cursor_char,
                        Style::default().bg(Color::White).fg(Color::Black),
                    ),
                    Span::raw(after.to_string()),
                ])
            } else {
                Line::from(vec![
                    Span::raw("    -> "),
                    Span::styled(
                        item.clone_path.clone(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            };

            let lines = vec![
                Line::from(vec![
                    Span::raw(format!(" {check} ")),
                    Span::styled(item.normalized_remote.clone(), remote_style),
                    Span::styled(sessions, Style::default().fg(Color::DarkGray)),
                ]),
                path_line,
            ];

            ListItem::new(lines)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" clync checkout ")
                .borders(Borders::ALL),
        )
        .highlight_style(Style::default());
    frame.render_stateful_widget(list, chunks[0], &mut state.list_state);

    let help = if state.editing.is_some() {
        " ENTER: save | ESC: cancel "
    } else {
        " SPACE: toggle | a: all | e: edit path | ENTER: clone | q: quit "
    };
    let help_widget = Paragraph::new(help).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(help_widget, chunks[1]);
}
