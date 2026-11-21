//! Draw the interactive Provider Router screens and handle terminal input.
use std::{
    env,
    future::Future,
    io::{self, Stdout},
};

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, Show},
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};

type Tty = Terminal<CrosstermBackend<Stdout>>;

const CANVAS: Color = Color::Rgb(16, 20, 27);
const PANEL: Color = Color::Rgb(23, 29, 39);
const RAISED: Color = Color::Rgb(32, 43, 59);
const EDGE: Color = Color::Rgb(52, 66, 87);
const INK: Color = Color::Rgb(231, 238, 248);
const MUTED: Color = Color::Rgb(169, 184, 205);
const SUBTLE: Color = Color::Rgb(126, 145, 172);
const ACCENT: Color = Color::Rgb(117, 231, 209);
const WARNING: Color = Color::Rgb(243, 201, 132);
const DANGER: Color = Color::Rgb(255, 153, 159);

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
}

impl Screen {
    pub fn open() -> Result<Self> {
        enable_raw_mode().context("Could not enable terminal input")?;
        let initialized = (|| -> Result<Tty> {
            execute!(
                io::stdout(),
                EnterAlternateScreen,
                Hide,
                EnableBracketedPaste
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
            let color = self.color;
            self.terminal.draw(|frame| {
                let Some(area) = shell(
                    frame,
                    color,
                    &[("Enter", "Continue"), ("Ctrl+U", "Clear"), ("Esc", "Back")],
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
        let color = self.color;
        self.terminal.draw(|frame| {
            let Some(area) = shell(
                frame,
                color,
                &[("↑ ↓", "Move"), ("Enter", "Select"), ("Esc", "Exit")],
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
                draw_menu(frame, color, left, "Settings", "", choices, selected);
                draw_overview(frame, color, right, info);
            } else {
                let label = dashboard_detail(info);
                draw_menu(frame, color, area, "Settings", &label, choices, selected);
            }
        })?;
        Ok(())
    }

    pub async fn event(&mut self) -> Result<Event> {
        loop {
            let event = self.events.next().await.ok_or(Exit)??;
            if let Event::Key(key) = &event {
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Err(Exit.into());
                }
            }
            return Ok(event);
        }
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
            let color = self.color;
            self.terminal.draw(|frame| {
                let Some(area) = shell(
                    frame,
                    color,
                    &[("↑ ↓", "Move"), ("Enter", "Select"), ("Esc", "Back")],
                ) else {
                    return;
                };
                let panel = top(area, choices.len().saturating_add(7));
                draw_menu(frame, color, panel, title, detail, choices, selected);
            })?;
            if let Event::Key(key) = self.event().await? {
                match navigate(key, choices, &mut selected) {
                    Navigation::Choose(index) => return Ok(Some(choices[index].value.clone())),
                    Navigation::Back => return Ok(None),
                    Navigation::Keep => {}
                }
            }
        }
    }

    pub async fn message(&mut self, title: &str, detail: &str) -> Result<()> {
        let mut offset = 0u16;
        loop {
            let color = self.color;
            self.terminal.draw(|frame| {
                let Some(area) = shell(
                    frame,
                    color,
                    &[("↑ ↓", "Scroll"), ("Enter", "Done"), ("Esc", "Back")],
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
        let color = self.color;
        self.terminal.draw(|frame| {
            let Some(area) = shell(frame, color, &[("Ctrl+C", "Exit")]) else {
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
        KeyCode::Enter => return Navigation::Choose(*selected),
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
        format!("API running  ·  {endpoint}")
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

fn draw_menu<T>(
    frame: &mut Frame<'_>,
    color: bool,
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
            Line::from(spans).style(fill(color, INK, if active { RAISED } else { PANEL })),
            y + offset as u16,
        );
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
        ("●  API running", ACCENT)
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
    y += u16::from(spaced);
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
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
}

fn shell(frame: &mut Frame<'_>, color: bool, keys: &[(&str, &str)]) -> Option<Rect> {
    let full = frame.area();
    frame.render_widget(Block::default().style(fill(color, INK, CANVAS)), full);
    if full.width < 27 || full.height < 8 {
        frame.render_widget(Paragraph::new("Resize to continue"), full);
        return None;
    }
    let width = full.width.saturating_sub(4).min(94);
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
    let mut spans = Vec::new();
    for (key, action) in keys {
        if !spans.is_empty() {
            spans.push(Span::raw(if width >= 44 { "   " } else { " " }));
        }
        if width >= 44 {
            spans.push(Span::styled(format!(" {key} "), fill(color, INK, RAISED)));
        } else {
            spans.push(Span::styled(*key, ink(color, INK)));
        }
        spans.push(Span::styled(format!(" {action}"), ink(color, SUBTLE)));
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
