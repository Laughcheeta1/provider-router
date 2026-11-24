//! Draw the interactive Provider Router screens and handle terminal input.
use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, Show},
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};
use std::{
    env,
    future::Future,
    io::{self, Stdout},
};
type Tty = Terminal<CrosstermBackend<Stdout>>;
// Dracula palette: background and current-line surfaces, foreground text, comment for hints,
// purple for focus, and the green, orange, and red accents for status.
const CANVAS: Color = Color::Rgb(40, 42, 54);
const PANEL: Color = Color::Rgb(40, 42, 54);
const RAISED: Color = Color::Rgb(68, 71, 90);
const SELECTED: Color = Color::Rgb(68, 71, 90);
const EDGE: Color = Color::Rgb(98, 114, 164);
const INK: Color = Color::Rgb(248, 248, 242);
const MUTED: Color = Color::Rgb(248, 248, 242);
const SUBTLE: Color = Color::Rgb(98, 114, 164);
const ACCENT: Color = Color::Rgb(189, 147, 249);
const SUCCESS: Color = Color::Rgb(80, 250, 123);
const WARNING: Color = Color::Rgb(255, 184, 108);
const DANGER: Color = Color::Rgb(255, 85, 85);
pub struct Choice<T> {
    pub label: String,
    pub value: T,
    pub shortcut: Option<char>,
    pub description: &'static str,
}
impl<T> Choice<T> {
    pub fn new(label: impl Into<String>, value: T) -> Self {
        Self {
            label: label.into(),
            value,
            shortcut: None,
            description: "",
        }
    }
    pub fn action(mut self, shortcut: char, description: &'static str) -> Self {
        self.shortcut = Some(shortcut);
        self.description = description;
        self
    }
}
/// One row of a multi-select list. Rows with a depth belong to the closest shallower row above.
pub struct Check {
    pub label: String,
    pub note: String,
    pub checked: bool,
    pub depth: u16,
}
/// A multi-select list toggled with Space or a click; callers save each toggle. Typing filters
/// the top-level rows.
pub struct Checklist<'a> {
    pub title: &'a str,
    pub detail: &'a str,
    pub items: &'a [Check],
    pub searchable: bool,
}
/// What the user did on a checklist. Indexes refer to `Checklist::items`.
pub enum Pick {
    Toggle(usize),
    Back,
}
/// A footer hint: the key label, its action, and the key a click on it sends.
type Hint<'a> = (&'a str, &'a str, Option<KeyCode>);
const MENU_HINTS: [Hint<'static>; 3] = [
    ("↑ ↓", "Move", None),
    ("Enter", "Select", Some(KeyCode::Enter)),
    ("Esc", "Back", Some(KeyCode::Esc)),
];
/// Clickable regions recorded while drawing the latest frame.
#[derive(Default)]
struct Hits {
    keys: Vec<(Rect, KeyCode)>,
    rows: Vec<(Rect, usize)>,
}
impl Hits {
    fn key(&self, position: Position) -> Option<KeyCode> {
        self.keys
            .iter()
            .find(|(area, _)| area.contains(position))
            .map(|(_, code)| *code)
    }
    fn row(&self, position: Position) -> Option<usize> {
        self.rows
            .iter()
            .find(|(area, _)| area.contains(position))
            .map(|(_, index)| *index)
    }
}
/// One row of a settings form, shown with its current value.
pub struct Entry {
    pub label: String,
    pub kind: EntryKind,
    pub hint: &'static str,
}

pub enum EntryKind {
    /// Edited in place. The placeholder shows while the value is empty, and always for a secret,
    /// whose editor starts empty.
    Text {
        value: String,
        placeholder: &'static str,
        secret: bool,
    },
    Toggle(bool),
    /// Opens another screen; the text summarizes what it holds.
    Open(String),
    /// A command such as removal, shown as its label.
    Action,
}

/// What the user did on a form. Indexes refer to the entries.
pub enum FormPick {
    Text(usize, String),
    Toggle(usize),
    Open(usize),
    Back,
}

pub struct Dashboard<'a> {
    pub endpoint: Option<&'a str>,
    pub error: Option<&'a str>,
    pub startup: &'a str,
    pub provider_count: usize,
    pub enabled_count: usize,
}
#[derive(Debug)]
pub struct Exit;
impl std::fmt::Display for Exit {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("Terminal closed")
    }
}
impl std::error::Error for Exit {}
pub struct Screen {
    terminal: Tty,
    events: EventStream,
    color: bool,
    hits: Hits,
}
impl Screen {
    pub fn open() -> Result<Self> {
        enable_raw_mode().context("Could not enable terminal input")?;
        let initialized = (|| -> Result<Tty> {
            execute!(
                io::stdout(),
                EnterAlternateScreen,
                Hide,
                EnableBracketedPaste,
                EnableMouseCapture
            )?;
            Ok(Terminal::new(CrosstermBackend::new(io::stdout()))?)
        })();
        let terminal = match initialized {
            Ok(terminal) => terminal,
            Err(error) => {
                restore_terminal();
                return Err(error);
            }
        };
        Ok(Self {
            terminal,
            color: env::var_os("NO_COLOR").is_none()
                && env::var_os("TERM").is_none_or(|term| term != "dumb"),
            events: EventStream::new(),
            hits: Hits::default(),
        })
    }
    pub async fn ask(
        &mut self,
        title: &str,
        initial: &str,
        hint: &str,
        secret: bool,
    ) -> Result<Option<String>> {
        let text = plain(initial);
        let mut field = Field {
            cursor: text.len(),
            text,
            secret,
        };
        loop {
            self.draw(|frame, color, hits| {
                let Some(area) = shell(
                    frame,
                    color,
                    &[
                        ("Enter", "Continue", Some(KeyCode::Enter)),
                        ("Ctrl+U", "Clear", None),
                        ("Esc", "Back", Some(KeyCode::Esc)),
                    ],
                    hits,
                ) else {
                    return;
                };
                let panel = top(area, 7);
                let inside = draw_panel(frame, color, panel, title);
                if !hint.is_empty() {
                    draw_text(frame, inside, plain(hint), ink(color, MUTED), 2, 0);
                }
                let line = field.line(color, inside.width);
                draw_line(frame, inside, line, 3);
            })?;
            match self.event().await? {
                Event::Key(key) if key.code == KeyCode::Esc => return Ok(None),
                Event::Key(key) if key.code == KeyCode::Enter => return Ok(Some(field.text)),
                Event::Key(key) => field.handle(key),
                Event::Paste(text) => field.insert(&plain(&text)),
                _ => {}
            }
        }
    }
    pub fn dashboard<T>(
        &mut self,
        info: &Dashboard<'_>,
        choices: &[Choice<T>],
        selected: usize,
    ) -> Result<()> {
        self.draw(|frame, color, hits| {
            let Some(area) = shell(
                frame,
                color,
                &[
                    ("↑ ↓", "Move", None),
                    ("Enter", "Select", Some(KeyCode::Enter)),
                    ("Esc", "Exit", Some(KeyCode::Esc)),
                ],
                hits,
            ) else {
                return;
            };
            if area.width >= 62 && area.height >= 8 {
                let panels = top(area, if area.height >= 12 { 12 } else { 8 });
                let left_width = (panels.width - 3) * 52 / 100;
                let left = Rect::new(panels.x, panels.y, left_width, panels.height);
                let right = Rect::new(
                    panels.x + left_width + 3,
                    panels.y,
                    panels.width - left_width - 3,
                    panels.height,
                );
                draw_menu(frame, color, hits, left, "Settings", "", choices, selected);
                draw_overview(frame, color, right, info);
            } else {
                let label = dashboard_detail(info);
                draw_menu(
                    frame, color, hits, area, "Settings", &label, choices, selected,
                );
            }
        })?;
        Ok(())
    }
    /// Read the next input. Wheel scrolling arrives as arrow keys and a click on a footer hint
    /// arrives as its key, so every screen supports them without extra handling.
    pub async fn event(&mut self) -> Result<Event> {
        loop {
            let event = self.events.next().await.ok_or(Exit)??;
            match &event {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        return Err(Exit.into());
                    }
                }
                Event::Mouse(mouse) => {
                    let position = Position::new(mouse.column, mouse.row);
                    let code = match mouse.kind {
                        MouseEventKind::ScrollUp => KeyCode::Up,
                        MouseEventKind::ScrollDown => KeyCode::Down,
                        MouseEventKind::Down(MouseButton::Left) => match self.hits.key(position) {
                            Some(code) => code,
                            None if self.hits.row(position).is_some() => return Ok(event),
                            None => continue,
                        },
                        _ => continue,
                    };
                    return Ok(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
                }
                _ => {}
            }
            return Ok(event);
        }
    }
    /// The row index a left click landed on in the latest frame.
    pub fn clicked(&self, event: &Event) -> Option<usize> {
        match event {
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                self.hits.row(Position::new(mouse.column, mouse.row))
            }
            _ => None,
        }
    }
    pub async fn checklist(
        &mut self,
        list: &Checklist<'_>,
        cursor: &mut usize,
        filter: &mut String,
    ) -> Result<Pick> {
        loop {
            let terms = filter.to_lowercase();
            let mut shown = true;
            let visible: Vec<usize> = (0..list.items.len())
                .filter(|index| {
                    let item = &list.items[*index];
                    if item.depth == 0 {
                        shown = item.label.to_lowercase().contains(&terms);
                    }
                    shown
                })
                .collect();
            *cursor = (*cursor).min(visible.len().saturating_sub(1));
            let hints = [
                ("↑ ↓", "Move", None),
                ("Space", "Toggle", Some(KeyCode::Char(' '))),
                ("Enter", "Done", Some(KeyCode::Enter)),
                ("Esc", "Back", Some(KeyCode::Esc)),
            ];
            let current = *cursor;
            self.draw(|frame, color, hits| {
                let Some(area) = shell(frame, color, &hints, hits) else {
                    return;
                };
                draw_checklist(frame, color, hits, area, list, &visible, current, filter);
            })?;
            let event = self.event().await?;
            if let Some(position) = self.clicked(&event) {
                *cursor = position;
                return Ok(Pick::Toggle(visible[position]));
            }
            let Event::Key(key) = event else {
                if let Event::Paste(text) = event
                    && list.searchable
                {
                    filter.push_str(&plain(&text).replace(' ', ""));
                }
                continue;
            };
            let last = visible.len().saturating_sub(1);
            match key.code {
                KeyCode::Esc if !filter.is_empty() => filter.clear(),
                KeyCode::Esc | KeyCode::Enter => return Ok(Pick::Back),
                KeyCode::Char(' ') => {
                    if let Some(index) = visible.get(*cursor) {
                        return Ok(Pick::Toggle(*index));
                    }
                }
                KeyCode::Up | KeyCode::BackTab => *cursor = cursor.saturating_sub(1),
                KeyCode::Down | KeyCode::Tab => *cursor = (*cursor + 1).min(last),
                KeyCode::PageUp => *cursor = cursor.saturating_sub(8),
                KeyCode::PageDown => *cursor = (*cursor + 8).min(last),
                KeyCode::Home => *cursor = 0,
                KeyCode::End => *cursor = last,
                KeyCode::Backspace if list.searchable => {
                    filter.pop();
                }
                KeyCode::Char(ch)
                    if list.searchable
                        && !ch.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    filter.push(ch);
                    *cursor = 0;
                }
                _ => {}
            }
        }
    }
    /// A settings page: every entry is visible, and Enter, Space or a click edits one in place.
    pub async fn form(
        &mut self,
        title: &str,
        entries: &[Entry],
        cursor: &mut usize,
    ) -> Result<FormPick> {
        let mut editing: Option<Field> = None;
        loop {
            *cursor = (*cursor).min(entries.len().saturating_sub(1));
            let hints: [Hint<'_>; 3] = if editing.is_some() {
                [
                    ("Enter", "Save", Some(KeyCode::Enter)),
                    ("Ctrl+U", "Clear", None),
                    ("Esc", "Cancel", Some(KeyCode::Esc)),
                ]
            } else {
                [
                    ("↑ ↓", "Move", None),
                    ("Enter", "Edit", Some(KeyCode::Enter)),
                    ("Esc", "Back", Some(KeyCode::Esc)),
                ]
            };
            let current = *cursor;
            self.draw(|frame, color, hits| {
                let Some(area) = shell(frame, color, &hints, hits) else {
                    return;
                };
                draw_form(
                    frame,
                    color,
                    hits,
                    area,
                    title,
                    entries,
                    current,
                    editing.as_ref(),
                );
            })?;
            let event = self.event().await?;
            if let Some(mut field) = editing.take() {
                match event {
                    Event::Key(key) if key.code == KeyCode::Esc => {}
                    Event::Key(key) if key.code == KeyCode::Enter => {
                        return Ok(FormPick::Text(current, field.text));
                    }
                    Event::Key(key) => {
                        field.handle(key);
                        editing = Some(field);
                    }
                    Event::Paste(text) => {
                        field.insert(&plain(&text));
                        editing = Some(field);
                    }
                    _ => editing = Some(field),
                }
                continue;
            }
            let activate = if let Some(row) = self.clicked(&event) {
                *cursor = row;
                true
            } else if let Event::Key(key) = event {
                let last = entries.len().saturating_sub(1);
                match key.code {
                    KeyCode::Esc => return Ok(FormPick::Back),
                    KeyCode::Enter | KeyCode::Char(' ') => true,
                    KeyCode::Up | KeyCode::BackTab => {
                        *cursor = cursor.saturating_sub(1);
                        false
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        *cursor = (*cursor + 1).min(last);
                        false
                    }
                    KeyCode::Home => {
                        *cursor = 0;
                        false
                    }
                    KeyCode::End => {
                        *cursor = last;
                        false
                    }
                    _ => false,
                }
            } else {
                false
            };
            if !activate {
                continue;
            }
            match &entries[*cursor].kind {
                EntryKind::Text { value, secret, .. } => {
                    let text = if *secret { String::new() } else { plain(value) };
                    editing = Some(Field {
                        cursor: text.len(),
                        text,
                        secret: *secret,
                    });
                }
                EntryKind::Toggle(_) => return Ok(FormPick::Toggle(*cursor)),
                EntryKind::Open(_) | EntryKind::Action => return Ok(FormPick::Open(*cursor)),
            }
        }
    }

    fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>, bool, &mut Hits)) -> Result<()> {
        let color = self.color;
        let mut hits = Hits::default();
        self.terminal
            .draw(|frame| render(frame, color, &mut hits))?;
        self.hits = hits;
        Ok(())
    }
    pub async fn menu<T: Clone>(
        &mut self,
        title: &str,
        detail: &str,
        choices: &[Choice<T>],
        mut selected: usize,
    ) -> Result<Option<T>> {
        if choices.is_empty() {
            return Ok(None);
        }
        selected = selected.min(choices.len() - 1);
        loop {
            self.draw(|frame, color, hits| {
                let Some(area) = shell(frame, color, &MENU_HINTS, hits) else {
                    return;
                };
                let panel = top(area, choices.len().saturating_add(7));
                draw_menu(frame, color, hits, panel, title, detail, choices, selected);
            })?;
            let event = self.event().await?;
            if let Some(index) = self.clicked(&event) {
                return Ok(Some(choices[index].value.clone()));
            }
            if let Event::Key(key) = event {
                match navigate(key, choices, &mut selected) {
                    Navigation::Choose(index) => return Ok(Some(choices[index].value.clone())),
                    Navigation::Back => return Ok(None),
                    Navigation::Keep => {}
                }
            }
        }
    }
    /// Show text with mouse capture released, so the terminal can select and copy it.
    pub async fn message(&mut self, title: &str, detail: &str) -> Result<()> {
        execute!(io::stdout(), DisableMouseCapture)?;
        let shown = self.read_message(title, detail).await;
        execute!(io::stdout(), EnableMouseCapture)?;
        shown
    }
    async fn read_message(&mut self, title: &str, detail: &str) -> Result<()> {
        let mut offset = 0u16;
        loop {
            self.draw(|frame, color, hits| {
                let Some(area) = shell(
                    frame,
                    color,
                    &[
                        ("↑ ↓", "Scroll", None),
                        ("Enter", "Done", None),
                        ("Esc", "Back", None),
                    ],
                    hits,
                ) else {
                    return;
                };
                let inside = draw_panel(frame, color, top(area, 9), title);
                let body = Paragraph::new(plain_lines(detail))
                    .style(ink(color, MUTED))
                    .wrap(Wrap { trim: true })
                    .scroll((offset, 0));
                frame.render_widget(body, inside);
            })?;
            if let Event::Key(key) = self.event().await? {
                match key.code {
                    KeyCode::Enter | KeyCode::Esc => return Ok(()),
                    KeyCode::Up => offset = offset.saturating_sub(1),
                    KeyCode::Down => offset = offset.saturating_add(1),
                    KeyCode::Home => offset = 0,
                    _ => {}
                }
            }
        }
    }
    pub async fn loading<T>(
        &mut self,
        label: &str,
        operation: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        self.progress(label)?;
        tokio::pin!(operation);
        loop {
            tokio::select! {
                result = &mut operation => return result,
                event = self.event() => {
                    if matches!(event?, Event::Resize(..)) {
                        self.progress(label)?;
                    }
                }
            }
        }
    }
    fn progress(&mut self, label: &str) -> Result<()> {
        self.draw(|frame, color, hits| {
            let Some(area) = shell(frame, color, &[("Ctrl+C", "Exit", None)], hits) else {
                return;
            };
            draw_panel(frame, color, top(area, 4), label);
        })?;
        Ok(())
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        restore_terminal();
    }
}
pub enum Navigation {
    Keep,
    Back,
    Choose(usize),
}
pub fn navigate<T>(key: KeyEvent, choices: &[Choice<T>], selected: &mut usize) -> Navigation {
    if choices.is_empty() {
        return Navigation::Back;
    }
    match key.code {
        KeyCode::Esc => return Navigation::Back,
        KeyCode::Enter | KeyCode::Char(' ') => return Navigation::Choose(*selected),
        KeyCode::Up | KeyCode::BackTab => *selected = selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Tab => *selected = (*selected + 1).min(choices.len() - 1),
        KeyCode::PageUp => *selected = selected.saturating_sub(8),
        KeyCode::PageDown => *selected = (*selected + 8).min(choices.len() - 1),
        KeyCode::Home => *selected = 0,
        KeyCode::End => *selected = choices.len() - 1,
        KeyCode::Char(ch)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            if let Some(index) = choices.iter().position(|choice| {
                choice
                    .shortcut
                    .is_some_and(|shortcut| shortcut.eq_ignore_ascii_case(&ch))
            }) {
                return Navigation::Choose(index);
            }
        }
        _ => {}
    }
    Navigation::Keep
}
struct Field {
    text: String,
    cursor: usize,
    secret: bool,
}
impl Field {
    fn handle(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Left => self.cursor = self.previous(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Backspace => {
                let from = self.previous();
                self.text.drain(from..self.cursor);
                self.cursor = from;
            }
            KeyCode::Delete => {
                let end = self.next();
                self.text.drain(self.cursor..end);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.clear();
                self.cursor = 0;
            }
            KeyCode::Char(ch)
                if !ch.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert(&ch.to_string());
            }
            _ => {}
        }
    }
    fn insert(&mut self, text: &str) {
        if self.text.len().saturating_add(text.len()) <= 16_384 {
            self.text.insert_str(self.cursor, text);
            self.cursor += text.len();
        }
    }
    fn line(&self, color: bool, width: u16) -> Line<'static> {
        let before = &self.text[..self.cursor];
        let after = &self.text[self.cursor..];
        let masked = |value: &str| -> String {
            if self.secret {
                "•".repeat(value.chars().count())
            } else {
                value.to_owned()
            }
        };
        let left = masked(before);
        let budget = usize::from(width.saturating_sub(2));
        let start = left
            .char_indices()
            .rev()
            .find(|(index, _)| Line::raw(&left[*index..]).width() > budget)
            .map_or(0, |(index, character)| index + character.len_utf8());
        Line::from(vec![
            Span::raw(" "),
            Span::raw(left[start..].to_owned()),
            Span::styled("▏", emphasis(color, ACCENT)),
            Span::raw(masked(after)),
        ])
        .style(fill(color, INK, RAISED))
    }
    fn next(&self) -> usize {
        self.cursor
            + self.text[self.cursor..]
                .chars()
                .next()
                .map_or(0, char::len_utf8)
    }
    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }
}
fn dashboard_detail(info: &Dashboard<'_>) -> String {
    if let Some(endpoint) = info.endpoint {
        if info.error.is_some() {
            "API running  ·  Docker bridge unavailable".to_owned()
        } else {
            format!("API running  ·  {endpoint}")
        }
    } else if let Some(error) = info.error {
        format!("API stopped  ·  {error}")
    } else {
        "API stopped".to_owned()
    }
}
fn draw_line(frame: &mut Frame<'_>, area: Rect, line: Line<'_>, y: u16) {
    if y < area.height {
        let style = line.style;
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
    }
}
#[allow(clippy::too_many_arguments)]
fn draw_form(
    frame: &mut Frame<'_>,
    color: bool,
    hits: &mut Hits,
    area: Rect,
    title: &str,
    entries: &[Entry],
    cursor: usize,
    editing: Option<&Field>,
) {
    let panel = top(area, entries.len().saturating_add(7));
    let inside = draw_panel(frame, color, panel, title);
    let column = entries
        .iter()
        .filter(|entry| !matches!(entry.kind, EntryKind::Action))
        .map(|entry| Line::raw(entry.label.as_str()).width())
        .max()
        .unwrap_or(0) as u16
        + 7;
    for (index, entry) in entries.iter().enumerate() {
        let y = index as u16 + 1;
        if y >= inside.height {
            break;
        }
        let active = index == cursor;
        let background = if active { SELECTED } else { PANEL };
        let label_style = match (&entry.kind, active) {
            (EntryKind::Action, _) => emphasis(color, DANGER),
            (_, true) => emphasis(color, INK),
            (_, false) => ink(color, MUTED),
        };
        let row = Line::from(vec![
            Span::styled(if active { "▌  " } else { "   " }, emphasis(color, ACCENT)),
            Span::styled(entry.label.as_str(), label_style),
        ]);
        draw_line(frame, inside, row.style(fill(color, INK, background)), y);
        let bounds = Rect::new(inside.x, inside.y + y, inside.width, 1);
        hits.rows.push((bounds, index));
        let value_area = Rect::new(
            inside.x + column.min(inside.width),
            inside.y,
            inside.width.saturating_sub(column),
            inside.height,
        );
        let value = match &entry.kind {
            EntryKind::Text { .. } if active && editing.is_some() => {
                let field = editing.expect("an open editor");
                // The editor line starts with a padding cell; keep its text in the value column.
                let editor = Rect::new(
                    value_area.x.saturating_sub(1),
                    value_area.y,
                    value_area.width + 1,
                    value_area.height,
                );
                draw_line(frame, editor, field.line(color, editor.width), y);
                continue;
            }
            EntryKind::Text {
                value,
                placeholder,
                secret,
            } => {
                if value.is_empty() || *secret {
                    Line::styled(*placeholder, ink(color, SUBTLE))
                } else {
                    Line::styled(plain(value), ink(color, INK))
                }
            }
            EntryKind::Toggle(true) => Line::styled("●  On", emphasis(color, ACCENT)),
            EntryKind::Toggle(false) => Line::styled("○  Off", ink(color, SUBTLE)),
            EntryKind::Open(summary) => Line::from(vec![
                Span::styled(plain(summary), ink(color, INK)),
                Span::styled("  ›", emphasis(color, ACCENT)),
            ]),
            EntryKind::Action => continue,
        };
        draw_line(
            frame,
            value_area,
            value.style(fill(color, INK, background)),
            y,
        );
    }
    let editing_secret = editing.is_some()
        && entries
            .get(cursor)
            .is_some_and(|entry| matches!(entry.kind, EntryKind::Text { secret: true, .. }));
    let hint = if editing_secret {
        "Paste the new value. Leave it empty to keep the saved one."
    } else {
        entries.get(cursor).map_or("", |entry| entry.hint)
    };
    let below = entries.len() as u16 + 2;
    if !hint.is_empty() && below < inside.height {
        draw_text(frame, inside, hint.to_owned(), ink(color, MUTED), 2, below);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_checklist(
    frame: &mut Frame<'_>,
    color: bool,
    hits: &mut Hits,
    area: Rect,
    list: &Checklist<'_>,
    visible: &[usize],
    cursor: usize,
    filter: &str,
) {
    let header = 2 + u16::from(list.searchable) * 2;
    let panel = top(
        area,
        list.items.len().saturating_add(usize::from(header) + 3),
    );
    let inside = draw_panel(frame, color, panel, list.title);
    let mut y = draw_text(frame, inside, plain(list.detail), ink(color, MUTED), 1, 0);
    if list.searchable {
        let search = if filter.is_empty() {
            Line::from(vec![
                Span::styled(" ⌕ ", ink(color, SUBTLE)),
                Span::styled("Type to filter", ink(color, SUBTLE)),
            ])
        } else {
            Line::from(vec![
                Span::styled(" ⌕ ", emphasis(color, ACCENT)),
                Span::styled(plain(filter), ink(color, INK)),
                Span::styled("▏", emphasis(color, ACCENT)),
            ])
        };
        draw_line(frame, inside, search.style(fill(color, INK, RAISED)), y + 1);
        y += 2;
    }
    y += 1;
    if visible.is_empty() {
        let empty = Line::styled("   No matches", ink(color, SUBTLE));
        draw_line(frame, inside, empty, y);
        return;
    }
    let available = usize::from(inside.height.saturating_sub(y));
    let start = cursor
        .saturating_sub(available.saturating_sub(1) / 2)
        .min(visible.len().saturating_sub(available));
    for (offset, &index) in visible.iter().skip(start).take(available).enumerate() {
        let item = &list.items[index];
        let active = start + offset == cursor;
        let mark = if item.checked { "[✓] " } else { "[ ] " };
        let label = plain(&item.label);
        let note = plain(&item.note);
        let indent = "    ".repeat(usize::from(item.depth));
        let label_style = if active {
            emphasis(color, INK)
        } else {
            ink(color, MUTED)
        };
        let mark_style = emphasis(color, if item.checked { ACCENT } else { SUBTLE });
        let row = Line::from(vec![
            Span::styled(if active { "▌  " } else { "   " }, emphasis(color, ACCENT)),
            Span::raw(indent),
            Span::styled(mark, mark_style),
            Span::styled(label, label_style),
            Span::raw("  "),
            Span::styled(note, ink(color, SUBTLE)),
        ]);
        let line_y = y + offset as u16;
        let background = if active { SELECTED } else { PANEL };
        draw_line(
            frame,
            inside,
            row.style(fill(color, INK, background)),
            line_y,
        );
        let bounds = Rect::new(inside.x, inside.y + line_y, inside.width, 1);
        hits.rows.push((bounds, start + offset));
    }
}
#[allow(clippy::too_many_arguments)]
fn draw_menu<T>(
    frame: &mut Frame<'_>,
    color: bool,
    hits: &mut Hits,
    panel: Rect,
    title: &str,
    detail: &str,
    choices: &[Choice<T>],
    selected: usize,
) {
    let area = draw_panel(frame, color, panel, title);
    let mut y = 0;
    if !detail.is_empty() && area.height >= 3 {
        y = draw_text(
            frame,
            area,
            plain_lines(detail),
            ink(color, MUTED),
            area.height.saturating_sub(2).min(3),
            0,
        );
    }
    let rows = usize::from(area.height.saturating_sub(y));
    let description = choices.get(selected).map_or("", |item| item.description);
    let show_description = !description.is_empty() && rows >= choices.len() + 2;
    let available = rows.saturating_sub(usize::from(show_description) * 2);
    let start = selected
        .saturating_sub(available.saturating_sub(1) / 2)
        .min(choices.len().saturating_sub(available));
    for (offset, choice) in choices.iter().skip(start).take(available).enumerate() {
        let active = start + offset == selected;
        let mut label = plain(&choice.label);
        let mut spans = vec![Span::styled(
            if active { "▌  " } else { "   " },
            emphasis(color, ACCENT),
        )];
        let label_budget = usize::from(area.width).saturating_sub(3);
        if let Some(key) = choice.shortcut {
            let label_width = Line::raw(label.as_str()).width();
            let shortcut_width = 3;
            let target = label_budget.saturating_sub(shortcut_width);
            if label_width <= target {
                label.push_str(&" ".repeat(target - label_width));
                spans.push(Span::styled(
                    label,
                    if active {
                        emphasis(color, INK)
                    } else {
                        ink(color, MUTED)
                    },
                ));
                spans.push(Span::styled(
                    format!(" {key} "),
                    emphasis(color, if active { ACCENT } else { SUBTLE }),
                ));
            } else {
                spans.push(Span::raw(label));
            }
        } else {
            spans.push(Span::styled(
                label,
                if active {
                    emphasis(color, INK)
                } else {
                    ink(color, MUTED)
                },
            ));
        }
        draw_line(
            frame,
            area,
            Line::from(spans).style(fill(color, INK, if active { SELECTED } else { PANEL })),
            y + offset as u16,
        );
        let bounds = Rect::new(area.x, area.y + y + offset as u16, area.width, 1);
        hits.rows.push((bounds, start + offset));
    }
    if show_description {
        draw_text(
            frame,
            area,
            description.to_owned(),
            ink(color, MUTED),
            2,
            area.height.saturating_sub(2),
        );
    }
}
fn draw_overview(frame: &mut Frame<'_>, color: bool, panel: Rect, info: &Dashboard<'_>) {
    let area = draw_panel(frame, color, panel, "Router status");
    let (label, state_color) = if info.endpoint.is_some() {
        ("●  API running", SUCCESS)
    } else if info.error.is_some() {
        ("○  API stopped", DANGER)
    } else {
        ("○  API stopped", WARNING)
    };
    let spaced = area.height >= 9;
    let mut y = u16::from(spaced);
    draw_line(
        frame,
        area,
        Line::styled(label, emphasis(color, state_color)),
        y,
    );
    y += 1;
    let address = info.endpoint.or(info.error).unwrap_or("");
    if !address.is_empty() {
        y += draw_text(
            frame,
            area,
            plain(address),
            ink(color, MUTED),
            if spaced { 2 } else { 1 },
            y,
        );
    }
    if info.endpoint.is_some() && info.error.is_some() {
        draw_line(
            frame,
            area,
            Line::styled("Docker bridge unavailable", ink(color, WARNING)),
            y,
        );
        y += 1;
    } else {
        y += u16::from(spaced);
    }
    draw_line(
        frame,
        area,
        Line::styled("Startup behavior", ink(color, SUBTLE)),
        y,
    );
    draw_line(
        frame,
        area,
        Line::styled(info.startup, ink(color, INK)),
        y + 1,
    );
    draw_line(
        frame,
        area,
        Line::styled(
            format!(
                "{} enabled · {} configured",
                info.enabled_count, info.provider_count
            ),
            ink(color, MUTED),
        ),
        y + if spaced { 3 } else { 2 },
    );
}
fn draw_panel(frame: &mut Frame<'_>, color: bool, area: Rect, title: &str) -> Rect {
    let panel = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(ink(color, EDGE))
        .style(fill(color, INK, PANEL));
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    let inside = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    draw_line(
        frame,
        inside,
        Line::styled(plain(title).to_uppercase(), emphasis(color, SUBTLE)),
        0,
    );
    Rect::new(
        inside.x,
        inside.y + 1,
        inside.width,
        inside.height.saturating_sub(1),
    )
}
fn draw_text(
    frame: &mut Frame<'_>,
    area: Rect,
    text: String,
    style: Style,
    maximum: u16,
    y: u16,
) -> u16 {
    if area.width == 0 || y >= area.height {
        return 0;
    }
    let limit = maximum.min(area.height - y);
    let mut end = 0;
    for line in text.lines() {
        let width = usize::from(area.width);
        let words = line.split_whitespace();
        let mut used = 0;
        end += 1;
        for word in words {
            let cells = Line::raw(word).width();
            if used != 0 && used + 1 + cells > width {
                end += 1;
                used = 0;
            }
            end += cells.saturating_sub(1) / width;
            used = if cells > width {
                (cells - 1) % width + 1
            } else {
                used + cells + usize::from(used != 0)
            };
        }
        if end >= usize::from(limit) {
            break;
        }
    }
    let height = limit.min(u16::try_from(end).unwrap_or(u16::MAX));
    let paragraph = Paragraph::new(text).style(style).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, Rect::new(area.x, area.y + y, area.width, height));
    height
}
fn emphasis(color: bool, foreground: Color) -> Style {
    ink(color, foreground).add_modifier(Modifier::BOLD)
}
fn fill(color: bool, foreground: Color, background: Color) -> Style {
    if color {
        Style::default().fg(foreground).bg(background)
    } else {
        Style::default()
    }
}
fn ink(color: bool, foreground: Color) -> Style {
    if color {
        Style::default().fg(foreground)
    } else {
        Style::default()
    }
}
fn plain(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_control()).collect()
}
fn plain_lines(text: &str) -> String {
    text.split('\n').map(plain).collect::<Vec<_>>().join("\n")
}
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        Show,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
}
fn shell(frame: &mut Frame<'_>, color: bool, keys: &[Hint<'_>], hits: &mut Hits) -> Option<Rect> {
    let full = frame.area();
    frame.render_widget(Block::default().style(fill(color, INK, CANVAS)), full);
    if full.width < 27 || full.height < 8 {
        frame.render_widget(Paragraph::new("Resize to continue"), full);
        return None;
    }
    let width = full.width.saturating_sub(4);
    let x = full.x + (full.width - width) / 2;
    let header = Rect::new(x, full.y + 1, width, 2);
    draw_line(
        frame,
        header,
        Line::from(vec![
            Span::styled("◈  ", emphasis(color, ACCENT)),
            Span::styled("Provider Router", emphasis(color, INK)),
        ]),
        0,
    );
    draw_line(
        frame,
        header,
        Line::styled("─".repeat(usize::from(width)), ink(color, EDGE)),
        1,
    );
    let footer = Rect::new(x, full.bottom() - 2, width, 1);
    let mut spans: Vec<Span<'_>> = Vec::new();
    let column =
        |spans: &[Span<'_>]| footer.x + spans.iter().map(Span::width).sum::<usize>() as u16;
    for (key, action, code) in keys {
        if !spans.is_empty() {
            spans.push(Span::raw(if width >= 44 { "   " } else { " " }));
        }
        let start = column(&spans);
        if width >= 44 {
            spans.push(Span::styled(format!(" {key} "), fill(color, INK, RAISED)));
        } else {
            spans.push(Span::styled(*key, ink(color, INK)));
        }
        spans.push(Span::styled(format!(" {action}"), ink(color, SUBTLE)));
        if let Some(code) = code {
            let end = column(&spans).min(footer.right());
            let bounds = Rect::new(start, footer.y, end.saturating_sub(start), 1);
            hits.keys.push((bounds, *code));
        }
    }
    draw_line(frame, footer, Line::from(spans), 0);
    Some(Rect::new(x, full.y + 4, width, full.height - 7))
}
fn top(area: Rect, height: usize) -> Rect {
    Rect::new(
        area.x,
        area.y,
        area.width,
        area.height.min(height.min(u16::MAX as usize) as u16),
    )
}
