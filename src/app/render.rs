use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};
use unicode_width::UnicodeWidthChar;

use crate::json::{PathSegment, format_path, pretty_json_lines};

use super::{App, FormatMode, Mode, SourceEditMode};

impl App {
    /// Render one frame: header, tabs, body (main pane + optional outline), footer,
    /// and any active popup. Called once per event-loop tick.
    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(3),
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_tabs(frame, chunks[1]);

        let show_outline = self.should_show_outline();
        let body_chunks = if show_outline {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
                .split(chunks[2])
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(100)])
                .split(chunks[2])
        };

        self.draw_main(frame, body_chunks[0]);
        if show_outline && body_chunks.len() > 1 {
            self.draw_outline(frame, body_chunks[1]);
        }

        self.draw_footer(frame, chunks[3]);

        if matches!(self.mode, Mode::EditKey | Mode::EditValue | Mode::Search) {
            self.draw_edit_popup(frame, area);
        }
        if self.mode == Mode::Help {
            self.draw_help_popup(frame, area);
        }
    }

    /// Top two lines: app name + mode/format/row count, then a contextual key hint.
    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let json_state = if self.json.is_some() {
            format!("{} rows", self.rows.len())
        } else {
            "waiting for valid JSON".to_string()
        };
        let helper = if self.should_show_outline() {
            "outline"
        } else {
            "full"
        };
        let mode_label = if self.mode == Mode::Source {
            format!("Source {}", self.source_edit_mode.label())
        } else {
            self.mode.label().to_string()
        };
        let header = vec![
            Line::from(vec![
                Span::styled(
                    "jsontui",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(mode_label, Style::default().fg(Color::Cyan)),
                Span::raw("  "),
                Span::raw(self.format_mode.label()),
                Span::raw("  "),
                Span::styled(json_state, Style::default().fg(Color::DarkGray)),
                Span::raw("  context: "),
                Span::styled(helper, Style::default().fg(Color::Yellow)),
            ]),
            Line::from(self.key_help()),
        ];

        frame.render_widget(Paragraph::new(header), area);
    }

    /// Tab strip for [`FormatMode`] with the current one highlighted.
    fn draw_tabs(&self, frame: &mut Frame, area: Rect) {
        let titles = FormatMode::ALL
            .iter()
            .map(|mode| Line::from(Span::raw(mode.label())))
            .collect::<Vec<_>>();
        let tabs = Tabs::new(titles)
            .block(Block::default().borders(Borders::ALL).title("view"))
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .select(self.format_mode.index());
        frame.render_widget(tabs, area);
    }

    /// Central pane. Routes to source editor or the selected formatted JSON view.
    fn draw_main(&mut self, frame: &mut Frame, area: Rect) {
        if self.mode == Mode::Source || self.json.is_none() {
            self.draw_source(frame, area);
            return;
        }

        self.draw_json_text(frame, area);
    }

    /// Render the raw JSON text editor and place the terminal cursor on `source_cursor`.
    /// Adjusts `source_scroll` so the cursor line stays in view.
    fn draw_source(&mut self, frame: &mut Frame, area: Rect) {
        let inner_height = area.height.saturating_sub(2) as usize;
        let (line, col) = self.source_line_col();

        if inner_height > 0 {
            if line < self.source_scroll {
                self.source_scroll = line;
            } else if line >= self.source_scroll + inner_height {
                self.source_scroll = line.saturating_sub(inner_height.saturating_sub(1));
            }
        }

        let block = Block::default().borders(Borders::ALL).title("source json");
        let paragraph = Paragraph::new(self.source.as_str())
            .block(block)
            .scroll((self.source_scroll as u16, 0));
        frame.render_widget(paragraph, area);

        if self.mode == Mode::Source {
            let visible_line = line.saturating_sub(self.source_scroll) as u16;
            let max_col = area.width.saturating_sub(2);
            let cursor_col = (col as u16).min(max_col);
            let cursor_row = area.y + 1 + visible_line.min(area.height.saturating_sub(2));
            frame.set_cursor_position(Position {
                x: area.x + 1 + cursor_col,
                y: cursor_row,
            });
        }
    }

    /// Render `Pretty` / `Compact` / `Raw` views. Pretty has its own highlighter (see [`Self::draw_pretty_json`]).
    fn draw_json_text(&mut self, frame: &mut Frame, area: Rect) {
        if self.format_mode == FormatMode::Pretty {
            self.draw_pretty_json(frame, area);
            return;
        }

        let title = match self.format_mode {
            FormatMode::Compact => "compact json",
            FormatMode::Raw => "raw json",
            FormatMode::Pretty => "json",
        };
        let title = self.title_with_selected_path(title);
        let text = self.formatted_json();
        let paragraph = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false })
            .scroll((self.display_scroll, 0));
        frame.render_widget(paragraph, area);
    }

    /// Pretty-print JSON one line at a time, highlighting the line whose path matches
    /// the current selection. Scrolls automatically to keep that line in view.
    fn draw_pretty_json(&mut self, frame: &mut Frame, area: Rect) {
        let Some(json) = &self.json else {
            return;
        };
        let pretty_lines = pretty_json_lines(json);
        let selected_path = self.selected_row().map(|row| row.path.as_slice());
        let selected_line = selected_path.and_then(|path| {
            pretty_lines
                .iter()
                .position(|line| line.path.as_slice() == path)
        });

        if let Some(line_index) = selected_line {
            self.keep_pretty_line_visible(
                &pretty_lines,
                line_index,
                area.width.saturating_sub(2),
                area.height.saturating_sub(2) as usize,
            );
        }

        let lines = pretty_lines
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                if Some(index) == selected_line {
                    Line::styled(
                        line.text,
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    colorized_json_line(&line.text)
                }
            })
            .collect::<Vec<_>>();

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(self.title_with_selected_path("beautified json")),
            )
            .wrap(Wrap { trim: false })
            .scroll((self.display_scroll, 0));
        frame.render_widget(paragraph, area);
    }

    /// Append the selected row's path label to a pane title (e.g. `beautified json  $.items[0]`).
    fn title_with_selected_path(&self, title: &'static str) -> String {
        self.selected_row()
            .map(|row| format!("{title}  {}", row.path_label()))
            .unwrap_or_else(|| title.to_string())
    }

    /// Adjust `display_scroll` so the selected pretty-JSON line falls inside the viewport.
    ///
    /// The paragraph wraps long values, and Ratatui applies vertical scroll after wrapping. That
    /// means the scroll target must be measured in rendered rows rather than source line indexes.
    fn keep_pretty_line_visible(
        &mut self,
        lines: &[crate::json::PrettyJsonLine],
        line_index: usize,
        visible_width: u16,
        visible_height: usize,
    ) {
        if visible_height == 0 {
            return;
        }

        let line_start = lines
            .iter()
            .take(line_index)
            .map(|line| wrapped_line_height(&line.text, visible_width))
            .sum::<usize>();
        let line_height = lines
            .get(line_index)
            .map(|line| wrapped_line_height(&line.text, visible_width))
            .unwrap_or(1)
            .max(1);
        let line_end = line_start + line_height;
        let scroll = self.display_scroll as usize;
        if line_start < scroll {
            self.display_scroll = line_start as u16;
        } else if line_height > visible_height {
            if line_start >= scroll + visible_height {
                self.display_scroll = line_start.min(u16::MAX as usize) as u16;
            }
        } else if line_end > scroll + visible_height {
            self.display_scroll = line_end
                .saturating_sub(visible_height)
                .min(u16::MAX as usize) as u16;
        }
    }

    /// True when a formatted JSON view should carry a synchronized tree outline beside it.
    fn should_show_outline(&self) -> bool {
        self.outline_panel && self.json.is_some() && self.mode != Mode::Source
    }

    /// Right-side context pane: the same flattened tree used by navigation, kept in sync
    /// with the selected JSON path.
    fn draw_outline(&self, frame: &mut Frame, area: Rect) {
        let items = self
            .rows
            .iter()
            .map(|row| {
                let indent = "  ".repeat(row.depth);
                let name_style = match row.path.last() {
                    Some(PathSegment::Key(_)) => Style::default().fg(Color::Yellow),
                    Some(PathSegment::Index(_)) => Style::default().fg(Color::Magenta),
                    None => Style::default().fg(Color::White),
                };
                let line = Line::from(vec![
                    Span::raw(indent),
                    Span::styled(&row.name, name_style),
                    Span::raw("  "),
                    Span::styled(row.kind.label(), row.kind.style()),
                    Span::raw("  "),
                    Span::styled(&row.preview, Style::default().fg(Color::DarkGray)),
                ]);
                ListItem::new(line)
            })
            .collect::<Vec<_>>();

        let title = self.title_with_selected_path("tree context");
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(" > ");
        let mut state = ListState::default();
        state.select((!self.rows.is_empty()).then_some(self.selected));
        frame.render_stateful_widget(list, area, &mut state);
    }

    /// Bottom status bar. Shows `error` (red) when set, otherwise the latest `status` text.
    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let message = self.error.as_ref().unwrap_or(&self.status);
        let color = if self.error.is_some() {
            Color::Red
        } else {
            Color::Gray
        };
        let footer = Paragraph::new(message.as_str())
            .block(Block::default().borders(Borders::ALL).title("status"))
            .style(Style::default().fg(color))
            .wrap(Wrap { trim: false });
        frame.render_widget(footer, area);
    }

    /// Modal box used by `Search`, `EditKey`, and `EditValue`. Centers itself and
    /// places the terminal cursor on `edit_cursor` within the input line.
    fn draw_edit_popup(&self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 72, 5);
        let title = match self.mode {
            Mode::Search => "search",
            Mode::EditKey => "edit key",
            Mode::EditValue => "edit value as JSON",
            _ => "edit",
        };
        let target = match self.mode {
            Mode::Search => self.search_prompt_hint(),
            _ => format_path(&self.editing_path),
        };
        let input = match self.mode {
            Mode::Search => format!("/{}", self.edit_buffer),
            _ => self.edit_buffer.clone(),
        };
        let cursor_prefix_width = if self.mode == Mode::Search { 1 } else { 0 };
        let text = vec![
            Line::from(Span::styled(target, Style::default().fg(Color::DarkGray))),
            Line::from(input),
        ];
        let paragraph = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, popup);
        frame.render_widget(paragraph, popup);

        let cursor_x = popup.x
            + 1
            + ((self.edit_cursor + cursor_prefix_width) as u16).min(popup.width.saturating_sub(2));
        let cursor_y = popup.y + 2;
        frame.set_cursor_position(Position {
            x: cursor_x,
            y: cursor_y,
        });
    }

    /// Full-screen command reference shown while in [`Mode::Help`]. `?` or `Esc` closes it.
    fn draw_help_popup(&self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 70, area.height.saturating_sub(4).max(10));

        let section = |title: &'static str| {
            Line::from(Span::styled(
                title,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
        };
        let row = |keys: &'static str, desc: &'static str| {
            Line::from(vec![
                Span::styled(format!("  {keys:<14}"), Style::default().fg(Color::Cyan)),
                Span::raw(desc),
            ])
        };

        let lines = vec![
            section("Global"),
            row("Ctrl+C", "quit"),
            row("Ctrl+N", "new buffer"),
            row("?", "toggle this help"),
            Line::from(""),
            section("Source mode"),
            row("Esc", "normal mode"),
            row("i/a/I/A/o", "insert commands"),
            row("h/j/k/l", "move cursor"),
            row("0 / $", "line start / end"),
            row("gg / G", "buffer start / end"),
            row("x / X", "delete char"),
            row("dd", "delete line"),
            row("Ctrl+P", "parse JSON"),
            row("Ctrl+B", "parse + beautify"),
            row("Ctrl+M", "parse + compact"),
            row("Esc normal", "back to navigator (if parsed)"),
            Line::from(""),
            section("Navigate mode"),
            row("j / k", "next / previous row"),
            row("h / l", "parent / first child"),
            row("g / G", "top / bottom"),
            row("Ctrl+U/D", "page up / down (10 rows)"),
            row("b / m / r", "beautify / compact / raw view"),
            row("Tab / S-Tab", "cycle views"),
            row("i", "edit source text"),
            row("e / Enter", "edit selected value as JSON"),
            row("K", "rename selected object key"),
            row("/", "search"),
            row("n / N", "next / previous match"),
            row("yy", "copy current view"),
            row("Y / yv", "copy selected value"),
            row("yk", "copy \"key\": value pair"),
            row("q", "quit"),
            Line::from(""),
            section("Search / Edit popups"),
            row("Enter", "commit"),
            row("Esc", "cancel"),
        ];

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("help  (? or Esc to close)"),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, popup);
        frame.render_widget(paragraph, popup);
    }

    /// Contextual key cheat-sheet shown in the header, varies per [`Mode`].
    fn key_help(&self) -> Vec<Span<'static>> {
        match self.mode {
            Mode::Source => match self.source_edit_mode {
                SourceEditMode::Insert => vec![Span::raw(
                    "INSERT  Esc normal  Ctrl+P parse  Ctrl+B beautify  Ctrl+M compact  Ctrl+C quit",
                )],
                SourceEditMode::Normal => vec![Span::raw(
                    "NORMAL  i/a insert  h/j/k/l move  x delete  dd line  gg/G top/end  Esc navigate",
                )],
            },
            Mode::Navigate => vec![Span::raw(
                "j/k nodes  h/l parent/child  / search  yy copy view  Y value  yk pair  Ctrl+N new  ? help",
            )],
            Mode::Search => vec![Span::raw(
                "Enter search  Esc cancel  n/N repeat after search",
            )],
            Mode::EditKey | Mode::EditValue => {
                vec![Span::raw(
                    "Enter save  Esc cancel  value edits must be valid JSON",
                )]
            }
            Mode::Help => vec![Span::raw("? or Esc close help")],
        }
    }
}

/// Build a `Rect` of `percent_x` width and fixed `height`, centered inside `area`.
fn centered_rect(area: Rect, percent_x: u16, height: u16) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn colorized_json_line(text: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut cursor = 0;

    while cursor < text.len() {
        let Some(relative_quote) = text[cursor..].find('"') else {
            push_json_syntax_spans(&mut spans, &text[cursor..]);
            break;
        };
        let quote = cursor + relative_quote;
        push_json_syntax_spans(&mut spans, &text[cursor..quote]);

        let end = string_literal_end(text, quote);
        let style = if is_object_key(text, end) {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        spans.push(Span::styled(text[quote..end].to_string(), style));
        cursor = end;
    }

    Line::from(spans)
}

fn push_json_syntax_spans(spans: &mut Vec<Span<'static>>, text: &str) {
    let mut token = String::new();
    let mut token_kind = JsonSyntaxKind::Whitespace;

    for char in text.chars() {
        let kind = JsonSyntaxKind::from_char(char);
        if !token.is_empty() && kind != token_kind {
            push_json_syntax_span(spans, std::mem::take(&mut token), token_kind);
        }
        token_kind = kind;
        token.push(char);
    }

    if !token.is_empty() {
        push_json_syntax_span(spans, token, token_kind);
    }
}

fn push_json_syntax_span(
    spans: &mut Vec<Span<'static>>,
    token: String,
    token_kind: JsonSyntaxKind,
) {
    let style = match token_kind {
        JsonSyntaxKind::Whitespace => Style::default(),
        JsonSyntaxKind::Punctuation => Style::default().fg(Color::DarkGray),
        JsonSyntaxKind::Literal => json_literal_style(&token),
    };
    spans.push(Span::styled(token, style));
}

fn json_literal_style(token: &str) -> Style {
    match token {
        "true" | "false" => Style::default().fg(Color::Cyan),
        "null" => Style::default().fg(Color::DarkGray),
        _ if is_json_number_token(token) => Style::default().fg(Color::Yellow),
        _ => Style::default(),
    }
}

fn is_json_number_token(token: &str) -> bool {
    token
        .chars()
        .next()
        .is_some_and(|char| char == '-' || char.is_ascii_digit())
        && token
            .chars()
            .all(|char| char.is_ascii_digit() || matches!(char, '-' | '+' | '.' | 'e' | 'E'))
}

fn string_literal_end(text: &str, start: usize) -> usize {
    let mut escaped = false;
    for (offset, char) in text[start + 1..].char_indices() {
        if escaped {
            escaped = false;
        } else if char == '\\' {
            escaped = true;
        } else if char == '"' {
            return start + 1 + offset + char.len_utf8();
        }
    }
    text.len()
}

fn is_object_key(text: &str, string_end: usize) -> bool {
    text[string_end..]
        .chars()
        .find(|char| !char.is_whitespace())
        .is_some_and(|char| char == ':')
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum JsonSyntaxKind {
    Whitespace,
    Punctuation,
    Literal,
}

impl JsonSyntaxKind {
    fn from_char(char: char) -> Self {
        if char.is_whitespace() {
            Self::Whitespace
        } else if matches!(char, '{' | '}' | '[' | ']' | ':' | ',') {
            Self::Punctuation
        } else {
            Self::Literal
        }
    }
}

fn wrapped_line_height(text: &str, width: u16) -> usize {
    if width == 0 {
        return 0;
    }

    let width = width as usize;
    let mut lines = 1;
    let mut line_width = 0;
    let mut pending_whitespace = 0;

    for (is_whitespace, token_width) in wrap_tokens(text) {
        if is_whitespace {
            pending_whitespace += token_width;
            continue;
        }

        if line_width == 0 {
            if pending_whitespace > 0 {
                let whitespace_lines = pending_whitespace.div_ceil(width);
                lines += whitespace_lines.saturating_sub(1);
                line_width = pending_whitespace % width;
                if line_width == 0 {
                    line_width = width;
                }
                pending_whitespace = 0;
            }
        }

        if token_width > width {
            if line_width > 0 {
                lines += 1;
            }
            let token_lines = token_width.div_ceil(width);
            lines += token_lines.saturating_sub(1);
            line_width = token_width % width;
            if line_width == 0 {
                line_width = width;
            }
        } else if line_width + pending_whitespace + token_width <= width {
            line_width += pending_whitespace + token_width;
        } else {
            lines += 1;
            line_width = token_width;
        }
        pending_whitespace = 0;
    }

    if pending_whitespace > 0 {
        let available = width.saturating_sub(line_width);
        pending_whitespace = pending_whitespace.saturating_sub(available);
        if pending_whitespace > 0 {
            lines += pending_whitespace.div_ceil(width);
        }
    }

    lines
}

fn wrap_tokens(text: &str) -> Vec<(bool, usize)> {
    let mut tokens = Vec::new();
    let mut current_kind = None;
    let mut current_width = 0;

    for char in text.chars() {
        let is_whitespace = char.is_whitespace();
        if current_kind.is_some_and(|kind| kind != is_whitespace) {
            tokens.push((current_kind.unwrap(), current_width));
            current_width = 0;
        }

        current_kind = Some(is_whitespace);
        current_width += char.width().unwrap_or(0);
    }

    if let Some(kind) = current_kind {
        tokens.push((kind, current_width));
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::PrettyJsonLine;

    fn line(text: &str) -> PrettyJsonLine {
        PrettyJsonLine {
            text: text.to_string(),
            path: Vec::new(),
        }
    }

    #[test]
    fn pretty_scroll_accounts_for_wrapped_rows_before_selection() {
        let mut app = App::new();
        let lines = vec![line(&"x".repeat(50)), line("ok")];

        app.keep_pretty_line_visible(&lines, 1, 10, 3);

        assert_eq!(app.display_scroll, 3);
    }

    #[test]
    fn pretty_scroll_keeps_start_of_oversized_selected_line_visible() {
        let mut app = App::new();
        let lines = vec![line("short"), line(&"x".repeat(50))];

        app.keep_pretty_line_visible(&lines, 1, 10, 3);

        assert_eq!(app.display_scroll, 0);
    }

    #[test]
    fn pretty_scroll_jumps_to_start_of_oversized_selected_line_below_viewport() {
        let mut app = App::new();
        let lines = vec![line(&"x".repeat(30)), line(&"y".repeat(50))];

        app.keep_pretty_line_visible(&lines, 1, 10, 3);

        assert_eq!(app.display_scroll, 3);
    }

    #[test]
    fn colorizes_pretty_json_keys_and_string_values() {
        let line = colorized_json_line(r#"  "name": "Ada","#);

        assert_eq!(
            span_style(&line, r#""name""#),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(
            span_style(&line, r#""Ada""#),
            Style::default().fg(Color::Green)
        );
    }

    #[test]
    fn colorizes_pretty_json_literals() {
        let number = colorized_json_line(r#"  "age": -42.5,"#);
        let boolean = colorized_json_line(r#"  "active": true,"#);
        let null = colorized_json_line(r#"  "missing": null"#);

        assert_eq!(
            span_style(&number, "-42.5"),
            Style::default().fg(Color::Yellow)
        );
        assert_eq!(
            span_style(&boolean, "true"),
            Style::default().fg(Color::Cyan)
        );
        assert_eq!(
            span_style(&null, "null"),
            Style::default().fg(Color::DarkGray)
        );
    }

    fn span_style(line: &Line<'_>, content: &str) -> Style {
        line.spans
            .iter()
            .find(|span| span.content.as_ref() == content)
            .map(|span| span.style)
            .unwrap_or_else(|| panic!("span not found: {content}"))
    }
}
