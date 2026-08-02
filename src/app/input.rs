use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::json::{cursor_for_line_col, next_char_boundary, pretty_json_lines, row_matches_query};

use super::{AddEntryField, App, FormatMode, Mode, SourceEditMode};

impl App {
    /// Entry point for every key event. Clears errors, handles global shortcuts
    /// (`Ctrl+C`, `Ctrl+N`, pending `y`), then dispatches by [`Mode`].
    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        self.error = None;

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('n') {
            self.begin_new_json();
            return;
        }
        if self.mode == Mode::Navigate && self.pending_yank {
            self.handle_yank_key(key);
            return;
        }
        if self.mode == Mode::Navigate && self.pending_delete {
            self.handle_delete_key(key);
            return;
        }

        match self.mode {
            Mode::Source => self.handle_source_key(key),
            Mode::Navigate => self.handle_navigate_key(key),
            Mode::LineSelect => self.handle_line_select_key(key),
            Mode::Search => self.handle_search_key(key),
            Mode::AddEntry => self.handle_add_entry_key(key),
            Mode::EditKey | Mode::EditValue => self.handle_edit_key(key),
            Mode::Help => self.handle_help_key(key),
        }
    }

    /// Handle a bracketed-paste event. In `Navigate` mode, a paste replaces the buffer;
    /// in `Source` it appends and auto-parses; in edit/search modes it inserts as text.
    pub(crate) fn handle_paste(&mut self, text: &str) {
        self.error = None;
        match self.mode {
            Mode::Source => {
                self.insert_source(text);
                if self.parse_source().is_ok() {
                    self.beautify_source();
                    self.mode = Mode::Navigate;
                    self.format_mode = FormatMode::Pretty;
                    self.status = "Pasted, parsed, and beautified. Use j/k or arrows to navigate."
                        .to_string();
                }
            }
            Mode::AddEntry => self.insert_add_entry(text),
            Mode::Search | Mode::EditKey | Mode::EditValue => self.insert_edit(text),
            Mode::Help => {}
            Mode::Navigate | Mode::LineSelect => {
                self.clear_line_selection();
                self.mode = Mode::Source;
                self.source.clear();
                self.source_cursor = 0;
                self.insert_source(text);
                if self.parse_source().is_ok() {
                    self.beautify_source();
                    self.mode = Mode::Navigate;
                    self.format_mode = FormatMode::Pretty;
                    self.status = "Replaced JSON from paste.".to_string();
                }
            }
        }
    }

    /// Keymap for `Source` mode: Vim-style normal/insert editing plus
    /// `Ctrl+P/B/M` to parse/beautify/compact.
    fn handle_source_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.source_pending_g = false;
            self.source_pending_d = false;
            match key.code {
                KeyCode::Char('p') => {
                    let _ = self.parse_source();
                }
                KeyCode::Char('b') => self.parse_and_beautify_source(),
                KeyCode::Char('m') => self.parse_and_compact_source(),
                _ => {}
            }
            return;
        }

        match self.source_edit_mode {
            SourceEditMode::Insert => self.handle_source_insert_key(key),
            SourceEditMode::Normal => self.handle_source_normal_key(key),
        }
    }

    /// Insert-mode source editing. This preserves the original text-entry behavior;
    /// Esc switches to source normal mode instead of leaving the source editor.
    fn handle_source_insert_key(&mut self, key: KeyEvent) {
        self.source_pending_g = false;
        self.source_pending_d = false;

        match key.code {
            KeyCode::Esc => {
                self.move_source_left();
                self.source_edit_mode = SourceEditMode::Normal;
                self.status =
                    "Source normal mode. i/a insert, h/j/k/l move, dd delete line.".to_string();
            }
            KeyCode::Enter => self.insert_source("\n"),
            KeyCode::Tab => self.insert_source("  "),
            KeyCode::Backspace => self.delete_source_before_cursor(),
            KeyCode::Delete => self.delete_source_at_cursor(),
            KeyCode::Left => self.move_source_left(),
            KeyCode::Right => self.move_source_right(),
            KeyCode::Up => self.move_source_up(),
            KeyCode::Down => self.move_source_down(),
            KeyCode::Home => self.move_source_line_start(),
            KeyCode::End => self.move_source_line_end(),
            KeyCode::Char(ch) => self.insert_source(&ch.to_string()),
            _ => {}
        }
    }

    /// Vim-style normal mode inside the source editor. This is intentionally small:
    /// it covers common movement/edit commands without trying to emulate full Vim.
    fn handle_source_normal_key(&mut self, key: KeyEvent) {
        if self.source_pending_g {
            self.source_pending_g = false;
            if key.code == KeyCode::Char('g') {
                self.move_source_start();
                return;
            }
        }

        if self.source_pending_d {
            self.source_pending_d = false;
            if key.code == KeyCode::Char('d') {
                self.delete_current_source_line();
                return;
            }
        }

        match key.code {
            KeyCode::Esc => self.return_to_navigate_if_parsed(),
            KeyCode::Char('i') => self.enter_source_insert_mode(),
            KeyCode::Char('a') => {
                self.move_source_right();
                self.enter_source_insert_mode();
            }
            KeyCode::Char('I') => {
                self.move_source_line_start();
                self.enter_source_insert_mode();
            }
            KeyCode::Char('A') => {
                self.move_source_line_end();
                self.enter_source_insert_mode();
            }
            KeyCode::Char('o') => {
                self.move_source_line_end();
                self.insert_source("\n");
                self.enter_source_insert_mode();
            }
            KeyCode::Char('h') | KeyCode::Left => self.move_source_left(),
            KeyCode::Char('j') | KeyCode::Down => self.move_source_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_source_up(),
            KeyCode::Char('l') | KeyCode::Right => self.move_source_right(),
            KeyCode::Char('0') | KeyCode::Home => self.move_source_line_start(),
            KeyCode::Char('$') | KeyCode::End => self.move_source_line_end(),
            KeyCode::Char('g') => self.source_pending_g = true,
            KeyCode::Char('G') => self.move_source_end(),
            KeyCode::Char('d') => self.source_pending_d = true,
            KeyCode::Char('x') | KeyCode::Delete => self.delete_source_at_cursor(),
            KeyCode::Char('X') | KeyCode::Backspace => self.delete_source_before_cursor(),
            _ => {}
        }
    }

    fn enter_source_insert_mode(&mut self) {
        self.source_edit_mode = SourceEditMode::Insert;
        self.source_pending_g = false;
        self.source_pending_d = false;
        self.status = "Source insert mode.".to_string();
    }

    /// Keymap for `Navigate` mode: vim-style movement, view switches, search, yank, edit.
    fn handle_navigate_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u') => self.page_selection_up(),
                KeyCode::Char('d') => self.page_selection_down(),
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.open_help(),
            KeyCode::Char('i') => {
                self.mode = Mode::Source;
                self.source_edit_mode = SourceEditMode::Insert;
                self.source_pending_g = false;
                self.source_pending_d = false;
                if self.source.is_empty() {
                    self.source = self.formatted_json();
                    self.source_cursor = self.source.len();
                }
                self.status = "Editing source. Ctrl+P parses back into the navigator.".to_string();
            }
            KeyCode::Char('b') => {
                self.format_mode = FormatMode::Pretty;
                self.display_scroll = 0;
            }
            KeyCode::Char('m') => {
                self.format_mode = FormatMode::Compact;
                self.display_scroll = 0;
            }
            KeyCode::Char('r') => {
                self.format_mode = FormatMode::Raw;
                self.display_scroll = 0;
            }
            KeyCode::Tab => self.next_format_mode(),
            KeyCode::BackTab => self.previous_format_mode(),
            KeyCode::Up => self.move_view_up(),
            KeyCode::Down => self.move_view_down(),
            KeyCode::Char('k') => self.move_selection_up(),
            KeyCode::Char('j') => self.move_selection_down(),
            KeyCode::Left | KeyCode::Char('h') => self.move_to_parent_or_previous_view(),
            KeyCode::Right | KeyCode::Char('l') => self.move_to_child_or_next_view(),
            KeyCode::PageUp => self.page_view_up(),
            KeyCode::PageDown => self.page_view_down(),
            KeyCode::Home | KeyCode::Char('g') => self.move_to_top(),
            KeyCode::End | KeyCode::Char('G') => self.move_to_bottom(),
            KeyCode::Char('/') => self.begin_search(),
            KeyCode::Char('n') => self.repeat_search(true),
            KeyCode::Char('N') => self.repeat_search(false),
            KeyCode::Char('V') => self.begin_line_select(),
            KeyCode::Char('y') => self.begin_yank(),
            KeyCode::Char('Y') => self.yank_selected_value(),
            KeyCode::Char('e') | KeyCode::Enter => self.begin_value_edit(),
            KeyCode::Char('o') => self.begin_entry_add(),
            KeyCode::Char('d') => self.begin_pair_delete(),
            KeyCode::Char('u') => self.undo_last_add_delete(),
            KeyCode::Char('K') => self.begin_key_edit(),
            _ => {}
        }
    }

    /// `V` in `Navigate`: enter Vim-like visual-line mode for the pretty JSON view.
    fn begin_line_select(&mut self) {
        if self.format_mode != FormatMode::Pretty {
            self.error =
                Some("Line selection is only available in the beautified view.".to_string());
            return;
        }

        let Some(line_index) = self.selected_pretty_line_index() else {
            self.error = Some("No pretty JSON line to select.".to_string());
            return;
        };

        self.mode = Mode::LineSelect;
        self.line_selection_anchor = Some(line_index);
        self.line_selection_cursor = Some(line_index);
        self.update_line_selection_status();
    }

    /// Keymap for visual-line mode: extend with j/k, copy with y, cancel with Esc.
    fn handle_line_select_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.clear_line_selection();
                self.mode = Mode::Navigate;
                self.status = "Line selection cancelled.".to_string();
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_line_selection_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_line_selection_up(),
            KeyCode::Char('g') | KeyCode::Home => self.move_line_selection_start(),
            KeyCode::Char('G') | KeyCode::End => self.move_line_selection_end(),
            KeyCode::Char('y') => self.yank_selected_lines(),
            _ => {}
        }
    }

    /// `?` in `Navigate`: open the full command reference popup.
    fn open_help(&mut self) {
        self.mode = Mode::Help;
        self.status = "Help: press ? or Esc to close.".to_string();
    }

    /// Keymap for `Help` mode. Any key (`?` / `Esc` / other) returns to `Navigate`.
    fn handle_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => {
                self.mode = Mode::Navigate;
                self.status = "Help closed.".to_string();
            }
            _ => {}
        }
    }

    /// First `y` press. Arms `pending_yank` so the next key picks what to copy.
    fn begin_yank(&mut self) {
        self.pending_yank = true;
        self.pending_delete = false;
        self.status =
            "Yank: y copies current view, v copies selected value, k copies key/value pair."
                .to_string();
    }

    /// Second key after `y`. `y`=view, `v`/`Y`=value, `k`=key:value pair, anything else cancels.
    fn handle_yank_key(&mut self, key: KeyEvent) {
        self.pending_yank = false;

        match key.code {
            KeyCode::Char('y') => self.yank_current_view(),
            KeyCode::Char('v') | KeyCode::Char('Y') => self.yank_selected_value(),
            KeyCode::Char('k') => self.yank_selected_key_value(),
            KeyCode::Esc => self.status = "Yank cancelled.".to_string(),
            _ => self.status = "Yank cancelled.".to_string(),
        }
    }

    /// Second key after `d`. `d` confirms deletion, Esc cancels, anything else cancels
    /// and is handled as a normal Navigate key.
    fn handle_delete_key(&mut self, key: KeyEvent) {
        self.pending_delete = false;

        match key.code {
            KeyCode::Char('d') if key.modifiers.is_empty() => self.delete_selected_pair(),
            KeyCode::Esc => self.status = "Delete cancelled.".to_string(),
            _ => {
                self.status = "Delete cancelled.".to_string();
                self.handle_navigate_key(key);
            }
        }
    }

    /// Keymap for the `Search` popup. Enter commits the query, Esc cancels.
    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Navigate;
                self.status = "Search cancelled.".to_string();
            }
            KeyCode::Enter => self.commit_search(),
            KeyCode::Backspace => self.delete_edit_before_cursor(),
            KeyCode::Delete => self.delete_edit_at_cursor(),
            KeyCode::Left => self.move_edit_left(),
            KeyCode::Right => self.move_edit_right(),
            KeyCode::Home => self.edit_cursor = 0,
            KeyCode::End => self.edit_cursor = self.edit_buffer.len(),
            KeyCode::Up => self.recall_older_search(),
            KeyCode::Down => self.recall_newer_search(),
            KeyCode::Char(ch) => self.insert_edit(&ch.to_string()),
            _ => {}
        }
    }

    /// Keymap for `AddEntry`: two fields, Tab switches focus, Enter commits.
    fn handle_add_entry_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.clear_add_entry_state();
                self.mode = Mode::Navigate;
                self.status = "Add cancelled.".to_string();
            }
            KeyCode::Enter => self.commit_edit(),
            KeyCode::Tab | KeyCode::BackTab => self.toggle_add_entry_field(),
            KeyCode::Backspace => self.delete_add_entry_before_cursor(),
            KeyCode::Delete => self.delete_add_entry_at_cursor(),
            KeyCode::Left => self.move_add_entry_left(),
            KeyCode::Right => self.move_add_entry_right(),
            KeyCode::Home => self.set_add_entry_cursor_start(),
            KeyCode::End => self.set_add_entry_cursor_end(),
            KeyCode::Char(ch) => self.insert_add_entry(&ch.to_string()),
            _ => {}
        }
    }

    /// Keymap for `EditKey` / `EditValue` popups.
    fn handle_edit_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Navigate;
                self.status = "Edit cancelled.".to_string();
            }
            KeyCode::Enter => self.commit_edit(),
            KeyCode::Backspace => self.delete_edit_before_cursor(),
            KeyCode::Delete => self.delete_edit_at_cursor(),
            KeyCode::Left => self.move_edit_left(),
            KeyCode::Right => self.move_edit_right(),
            KeyCode::Home => self.edit_cursor = 0,
            KeyCode::End => self.edit_cursor = self.edit_buffer.len(),
            KeyCode::Char(ch) => self.insert_edit(&ch.to_string()),
            _ => {}
        }
    }

    /// `/` in `Navigate`: open an empty search popup.
    fn begin_search(&mut self) {
        self.edit_buffer.clear();
        self.edit_cursor = 0;
        self.search_history_index = None;
        self.search_history_draft.clear();
        self.mode = Mode::Search;
        self.status = "Type a search pattern. Enter jumps to the next match.".to_string();
    }

    /// `Enter` in search: save the query, recompute matches, jump to the first one.
    fn commit_search(&mut self) {
        self.search_query.clone_from(&self.edit_buffer);
        if !self.search_query.is_empty() && self.search_history.last() != Some(&self.search_query) {
            self.search_history.push(self.search_query.clone());
        }
        self.search_history_index = None;
        self.search_history_draft.clear();
        self.mode = Mode::Navigate;

        if self.search_query.is_empty() {
            self.search_matches.clear();
            self.search_match_index = None;
            self.status = "Search cleared.".to_string();
            return;
        }

        self.refresh_search_matches();
        if self.search_matches.is_empty() {
            self.error = Some(format!("Pattern not found: {}", self.search_query));
            return;
        }

        self.select_search_match_from_current(true);
    }

    /// `Up` in search: walk backward through committed queries.
    fn recall_older_search(&mut self) {
        if self.search_history.is_empty() {
            return;
        }

        let index = match self.search_history_index {
            Some(0) => 0,
            Some(index) => index - 1,
            None => {
                self.search_history_draft.clone_from(&self.edit_buffer);
                self.search_history.len() - 1
            }
        };

        self.search_history_index = Some(index);
        self.edit_buffer.clone_from(&self.search_history[index]);
        self.edit_cursor = self.edit_buffer.len();
    }

    /// `Down` in search: walk forward, then restore the draft from before history browsing.
    fn recall_newer_search(&mut self) {
        let Some(index) = self.search_history_index else {
            self.edit_buffer.clear();
            self.edit_cursor = 0;
            return;
        };

        if index + 1 < self.search_history.len() {
            let next_index = index + 1;
            self.search_history_index = Some(next_index);
            self.edit_buffer
                .clone_from(&self.search_history[next_index]);
        } else {
            self.search_history_index = None;
            self.edit_buffer.clone_from(&self.search_history_draft);
        }
        self.edit_cursor = self.edit_buffer.len();
    }

    /// `n` / `N`: move to next/previous match for the last query. Errors if no prior search.
    fn repeat_search(&mut self, forward: bool) {
        if self.search_query.is_empty() {
            self.error = Some("No previous search. Use / to search.".to_string());
            return;
        }

        self.refresh_search_matches();
        if self.search_matches.is_empty() {
            self.error = Some(format!("Pattern not found: {}", self.search_query));
            return;
        }

        self.select_search_match_from_current(forward);
    }

    /// Pick the next/previous match relative to `selected`, wrapping at the ends.
    /// If `selected` is itself a match, step past it; otherwise jump to the nearest in `forward`.
    fn select_search_match_from_current(&mut self, forward: bool) {
        let match_index = if let Some(current_index) = self
            .search_matches
            .iter()
            .position(|row_index| *row_index == self.selected)
        {
            if forward {
                (current_index + 1) % self.search_matches.len()
            } else if current_index == 0 {
                self.search_matches.len() - 1
            } else {
                current_index - 1
            }
        } else if forward {
            self.search_matches
                .iter()
                .position(|row_index| *row_index > self.selected)
                .unwrap_or(0)
        } else {
            self.search_matches
                .iter()
                .rposition(|row_index| *row_index < self.selected)
                .unwrap_or_else(|| self.search_matches.len() - 1)
        };

        self.move_to_search_match(match_index);
    }

    /// Focus the match at `search_matches[match_index]` and update the status line.
    fn move_to_search_match(&mut self, match_index: usize) {
        let Some(row_index) = self.search_matches.get(match_index).copied() else {
            return;
        };

        self.selected = row_index;
        self.search_match_index = Some(match_index);
        self.status = self.search_status();
    }

    /// Recompute `search_matches` against current `rows`. Called after every row rebuild.
    pub(super) fn refresh_search_matches(&mut self) {
        if self.search_query.is_empty() {
            self.search_matches.clear();
            self.search_match_index = None;
            return;
        }

        self.search_matches = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| row_matches_query(row, &self.search_query).then_some(index))
            .collect();
        self.search_match_index = self
            .search_matches
            .iter()
            .position(|row_index| *row_index == self.selected);
    }

    /// Status line text after a search, e.g. `match 2 of 7 for /name`.
    fn search_status(&self) -> String {
        let Some(match_index) = self.search_match_index else {
            return format!(
                "{} matches for /{}",
                self.search_matches.len(),
                self.search_query
            );
        };

        format!(
            "match {} of {} for /{}",
            match_index + 1,
            self.search_matches.len(),
            self.search_query
        )
    }

    /// Hint shown above the search input. Reflects empty / no-match / live counts.
    pub(super) fn search_prompt_hint(&self) -> String {
        if self.search_query.is_empty() {
            "Enter search  Esc cancel".to_string()
        } else if self.search_matches.is_empty() {
            format!("previous: /{}", self.search_query)
        } else {
            self.search_status()
        }
    }

    /// `Tab`: advance one position in [`FormatMode::ALL`], wrapping.
    fn next_format_mode(&mut self) {
        let index = (self.format_mode.index() + 1) % FormatMode::ALL.len();
        self.format_mode = FormatMode::ALL[index];
        self.display_scroll = 0;
    }

    /// `Shift+Tab`: step back one position in [`FormatMode::ALL`], wrapping.
    fn previous_format_mode(&mut self) {
        let index = self.format_mode.index();
        let next = if index == 0 {
            FormatMode::ALL.len() - 1
        } else {
            index - 1
        };
        self.format_mode = FormatMode::ALL[next];
        self.display_scroll = 0;
    }

    /// `k`: move outline selection up by one row.
    fn move_selection_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// `j`: move outline selection down by one row.
    fn move_selection_down(&mut self) {
        self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
    }

    /// Number of lines in the pretty JSON view.
    fn pretty_line_count(&self) -> usize {
        self.json
            .as_ref()
            .map(|json| pretty_json_lines(json).len())
            .unwrap_or(0)
    }

    /// `j` / `Down` in line-select mode: extend the range down by one pretty line.
    fn move_line_selection_down(&mut self) {
        let count = self.pretty_line_count();
        if count == 0 {
            return;
        }

        let cursor = self.line_selection_cursor.unwrap_or(0);
        self.line_selection_cursor = Some((cursor + 1).min(count - 1));
        self.update_line_selection_status();
    }

    /// `k` / `Up` in line-select mode: extend the range up by one pretty line.
    fn move_line_selection_up(&mut self) {
        let cursor = self.line_selection_cursor.unwrap_or(0);
        self.line_selection_cursor = Some(cursor.saturating_sub(1));
        self.update_line_selection_status();
    }

    /// `g` / `Home` in line-select mode: move the range cursor to the first pretty line.
    fn move_line_selection_start(&mut self) {
        if self.pretty_line_count() == 0 {
            return;
        }

        self.line_selection_cursor = Some(0);
        self.update_line_selection_status();
    }

    /// `G` / `End` in line-select mode: move the range cursor to the last pretty line.
    fn move_line_selection_end(&mut self) {
        let count = self.pretty_line_count();
        if count == 0 {
            return;
        }

        self.line_selection_cursor = Some(count - 1);
        self.update_line_selection_status();
    }

    fn update_line_selection_status(&mut self) {
        let Some(range) = self.line_selection_range() else {
            return;
        };
        let selected_lines = range.end() - range.start() + 1;
        self.status =
            format!("Line select: {selected_lines} lines. j/k extend, y copy, Esc cancel.");
    }

    /// `Up`: scroll the JSON text viewport.
    fn move_view_up(&mut self) {
        self.display_scroll = self.display_scroll.saturating_sub(1);
    }

    /// `Down`: scroll the JSON text viewport.
    fn move_view_down(&mut self) {
        self.display_scroll = self.display_scroll.saturating_add(1);
    }

    /// `h` / `Left`: jump to the row representing the parent of the selection.
    fn move_to_parent_or_previous_view(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let parent_len = row.path.len().saturating_sub(1);
        let parent_path = row.path[..parent_len].to_vec();

        if let Some(index) = self.rows.iter().position(|row| row.path == parent_path) {
            self.selected = index;
        }
    }

    /// `l` / `Right`: descend into the first child if the selected node has any.
    fn move_to_child_or_next_view(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let selected_depth = row.depth;
        let child_index = self.selected + 1;

        if self
            .rows
            .get(child_index)
            .is_some_and(|row| row.depth > selected_depth)
        {
            self.selected = child_index;
        }
    }

    /// `g` / `Home`: jump to the first row and top of the view.
    fn move_to_top(&mut self) {
        self.selected = 0;
        self.display_scroll = 0;
    }

    /// `G` / `End`: jump to the last row and bottom of the view.
    fn move_to_bottom(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
        self.display_scroll = u16::MAX;
    }

    /// `Ctrl+U`: move selection up by 10 rows.
    fn page_selection_up(&mut self) {
        self.selected = self.selected.saturating_sub(10);
    }

    /// `Ctrl+D`: move selection down by 10 rows.
    fn page_selection_down(&mut self) {
        self.selected = (self.selected + 10).min(self.rows.len().saturating_sub(1));
    }

    /// `PageUp`: page the JSON text viewport up.
    fn page_view_up(&mut self) {
        self.display_scroll = self.display_scroll.saturating_sub(10);
    }

    /// `PageDown`: page the JSON text viewport down.
    fn page_view_down(&mut self) {
        self.display_scroll = self.display_scroll.saturating_add(10);
    }

    /// Insert `text` at `source_cursor`, advancing the cursor by `text.len()` bytes.
    fn insert_source(&mut self, text: &str) {
        self.source.insert_str(self.source_cursor, text);
        self.source_cursor += text.len();
    }

    /// `Backspace`: remove the char before `source_cursor`.
    fn delete_source_before_cursor(&mut self) {
        if self.source_cursor == 0 {
            return;
        }
        if let Some((index, _)) = self.source[..self.source_cursor].char_indices().last() {
            self.source.drain(index..self.source_cursor);
            self.source_cursor = index;
        }
    }

    /// `Delete`: remove the char at `source_cursor`.
    fn delete_source_at_cursor(&mut self) {
        if self.source_cursor >= self.source.len() {
            return;
        }
        let next = next_char_boundary(&self.source, self.source_cursor);
        self.source.drain(self.source_cursor..next);
    }

    /// `Left`: step the source cursor back one UTF-8 char.
    fn move_source_left(&mut self) {
        if let Some((index, _)) = self.source[..self.source_cursor].char_indices().last() {
            self.source_cursor = index;
        }
    }

    /// `Right`: step the source cursor forward one UTF-8 char.
    fn move_source_right(&mut self) {
        self.source_cursor = next_char_boundary(&self.source, self.source_cursor);
    }

    /// `Up`: move source cursor one line up, preserving column.
    fn move_source_up(&mut self) {
        let (line, col) = self.source_line_col();
        if line == 0 {
            return;
        }
        self.source_cursor = cursor_for_line_col(&self.source, line - 1, col);
    }

    /// `Down`: move source cursor one line down, preserving column.
    fn move_source_down(&mut self) {
        let (line, col) = self.source_line_col();
        self.source_cursor = cursor_for_line_col(&self.source, line + 1, col);
    }

    /// `Home`: move source cursor to column 0 of the current line.
    fn move_source_line_start(&mut self) {
        let (line, _) = self.source_line_col();
        self.source_cursor = cursor_for_line_col(&self.source, line, 0);
    }

    /// `End`: move source cursor to the last column of the current line.
    fn move_source_line_end(&mut self) {
        let (line, _) = self.source_line_col();
        let line_len = self
            .source
            .lines()
            .nth(line)
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or(0);
        self.source_cursor = cursor_for_line_col(&self.source, line, line_len);
    }

    /// `gg`: move source cursor to the start of the buffer.
    fn move_source_start(&mut self) {
        self.source_cursor = 0;
    }

    /// `G`: move source cursor to the end of the buffer.
    fn move_source_end(&mut self) {
        self.source_cursor = self.source.len();
    }

    /// `dd`: delete the current source line, including one surrounding newline when possible.
    fn delete_current_source_line(&mut self) {
        if self.source.is_empty() {
            return;
        }

        let start = self.source[..self.source_cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let line_end = self.source[self.source_cursor..]
            .find('\n')
            .map(|offset| self.source_cursor + offset)
            .unwrap_or(self.source.len());

        let (delete_start, delete_end, new_cursor) = if line_end < self.source.len() {
            (start, line_end + 1, start)
        } else if start > 0 {
            (start - 1, line_end, start - 1)
        } else {
            (start, line_end, 0)
        };

        self.source.drain(delete_start..delete_end);
        self.source_cursor = new_cursor.min(self.source.len());
    }

    /// Insert `text` into `edit_buffer` at `edit_cursor`. Used by search and edit popups.
    fn insert_edit(&mut self, text: &str) {
        self.edit_buffer.insert_str(self.edit_cursor, text);
        self.edit_cursor += text.len();
    }

    /// `Backspace` in an edit popup: remove the char before `edit_cursor`.
    fn delete_edit_before_cursor(&mut self) {
        if self.edit_cursor == 0 {
            return;
        }
        if let Some((index, _)) = self.edit_buffer[..self.edit_cursor].char_indices().last() {
            self.edit_buffer.drain(index..self.edit_cursor);
            self.edit_cursor = index;
        }
    }

    /// `Delete` in an edit popup: remove the char at `edit_cursor`.
    fn delete_edit_at_cursor(&mut self) {
        if self.edit_cursor >= self.edit_buffer.len() {
            return;
        }
        let next = next_char_boundary(&self.edit_buffer, self.edit_cursor);
        self.edit_buffer.drain(self.edit_cursor..next);
    }

    /// `Left` in an edit popup: step `edit_cursor` back one UTF-8 char.
    fn move_edit_left(&mut self) {
        if let Some((index, _)) = self.edit_buffer[..self.edit_cursor].char_indices().last() {
            self.edit_cursor = index;
        }
    }

    /// `Right` in an edit popup: step `edit_cursor` forward one UTF-8 char.
    fn move_edit_right(&mut self) {
        self.edit_cursor = next_char_boundary(&self.edit_buffer, self.edit_cursor);
    }

    fn toggle_add_entry_field(&mut self) {
        self.add_entry_field = match self.add_entry_field {
            AddEntryField::Key => AddEntryField::Value,
            AddEntryField::Value => AddEntryField::Key,
        };
    }

    fn active_add_entry_buffer_mut(&mut self) -> (&mut String, &mut usize) {
        match self.add_entry_field {
            AddEntryField::Key => (&mut self.add_key_buffer, &mut self.add_key_cursor),
            AddEntryField::Value => (&mut self.add_value_buffer, &mut self.add_value_cursor),
        }
    }

    fn insert_add_entry(&mut self, text: &str) {
        let (buffer, cursor) = self.active_add_entry_buffer_mut();
        buffer.insert_str(*cursor, text);
        *cursor += text.len();
    }

    fn delete_add_entry_before_cursor(&mut self) {
        let (buffer, cursor) = self.active_add_entry_buffer_mut();
        if *cursor == 0 {
            return;
        }
        if let Some((index, _)) = buffer[..*cursor].char_indices().last() {
            buffer.drain(index..*cursor);
            *cursor = index;
        }
    }

    fn delete_add_entry_at_cursor(&mut self) {
        let (buffer, cursor) = self.active_add_entry_buffer_mut();
        if *cursor >= buffer.len() {
            return;
        }
        let next = next_char_boundary(buffer, *cursor);
        buffer.drain(*cursor..next);
    }

    fn move_add_entry_left(&mut self) {
        let (buffer, cursor) = self.active_add_entry_buffer_mut();
        if let Some((index, _)) = buffer[..*cursor].char_indices().last() {
            *cursor = index;
        }
    }

    fn move_add_entry_right(&mut self) {
        let (buffer, cursor) = self.active_add_entry_buffer_mut();
        *cursor = next_char_boundary(buffer, *cursor);
    }

    fn set_add_entry_cursor_start(&mut self) {
        let (_, cursor) = self.active_add_entry_buffer_mut();
        *cursor = 0;
    }

    fn set_add_entry_cursor_end(&mut self) {
        let (buffer, cursor) = self.active_add_entry_buffer_mut();
        *cursor = buffer.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn select_path(app: &mut App, path: &str) {
        app.selected = app
            .rows
            .iter()
            .position(|row| row.path_label() == path)
            .unwrap();
    }

    fn type_text(app: &mut App, text: &str) {
        for char in text.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(char), KeyModifiers::NONE));
        }
    }

    #[test]
    fn moves_between_parent_and_first_child_in_outline() {
        let mut app = App::new();
        app.set_json(json!({"items": [{"name": "Ada"}]}), None);

        app.format_mode = FormatMode::Pretty;
        app.move_to_child_or_next_view();
        assert_eq!(app.selected_row().unwrap().path_label(), "$.items");

        app.move_to_child_or_next_view();
        assert_eq!(app.selected_row().unwrap().path_label(), "$.items[0]");

        app.move_to_parent_or_previous_view();
        assert_eq!(app.selected_row().unwrap().path_label(), "$.items");
    }

    #[test]
    fn repeats_search_forward_and_backward_with_wrapping() {
        let mut app = App::new();
        app.set_json(
            json!({
                "items": [
                    {"name": "Ada"},
                    {"name": "Grace"}
                ],
                "owner": "Ada"
            }),
            None,
        );
        app.search_query = "ada".to_string();
        app.refresh_search_matches();

        app.repeat_search(true);
        assert_eq!(app.selected_row().unwrap().path_label(), "$.items[0].name");
        assert_eq!(app.search_match_index, Some(0));

        app.repeat_search(true);
        assert_eq!(app.selected_row().unwrap().path_label(), "$.owner");
        assert_eq!(app.search_match_index, Some(1));

        app.repeat_search(true);
        assert_eq!(app.selected_row().unwrap().path_label(), "$.items[0].name");
        assert_eq!(app.search_match_index, Some(0));

        app.repeat_search(false);
        assert_eq!(app.selected_row().unwrap().path_label(), "$.owner");
        assert_eq!(app.search_match_index, Some(1));
    }

    #[test]
    fn new_search_starts_empty_and_arrows_navigate_query_history() {
        let mut app = App::new();
        app.set_json(json!({"names": ["Ada", "Grace"]}), None);

        for query in ["ada", "grace", "grace"] {
            app.begin_search();
            app.edit_buffer = query.to_string();
            app.edit_cursor = app.edit_buffer.len();
            app.commit_search();
        }

        assert_eq!(app.search_history, ["ada", "grace"]);
        app.edit_buffer = "stale edit".to_string();

        app.begin_search();

        assert_eq!(app.mode, Mode::Search);
        assert!(app.edit_buffer.is_empty());
        assert_eq!(app.edit_cursor, 0);
        type_text(&mut app, "draft");

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

        assert_eq!(app.edit_buffer, "grace");
        assert_eq!(app.edit_cursor, 5);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

        assert_eq!(app.edit_buffer, "ada");
        assert_eq!(app.edit_cursor, 3);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

        assert_eq!(app.edit_buffer, "ada");

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.edit_buffer, "grace");

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.edit_buffer, "draft");
        assert_eq!(app.edit_cursor, 5);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert!(app.edit_buffer.is_empty());
        assert_eq!(app.edit_cursor, 0);
    }

    #[test]
    fn ctrl_n_starts_a_fresh_json_buffer() {
        let mut app = App::new();
        app.set_json(json!({"old": true}), None);
        app.source = "{\"old\":true}".to_string();
        app.raw_source = app.source.clone();
        app.mode = Mode::Navigate;
        app.search_history = vec!["old".to_string()];
        app.search_history_index = Some(0);
        app.search_history_draft = "draft".to_string();

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));

        assert_eq!(app.mode, Mode::Source);
        assert!(app.source.is_empty());
        assert!(app.raw_source.is_empty());
        assert!(app.json.is_none());
        assert!(app.rows.is_empty());
        assert!(app.search_query.is_empty());
        assert!(app.search_history.is_empty());
        assert_eq!(app.search_history_index, None);
        assert!(app.search_history_draft.is_empty());
        assert_eq!(app.source_edit_mode, SourceEditMode::Insert);
    }

    #[test]
    fn source_starts_in_insert_mode_and_esc_enters_normal_mode() {
        let mut app = App::new();

        app.handle_key(KeyEvent::new(KeyCode::Char('{'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(app.source_edit_mode, SourceEditMode::Normal);
        assert!(app.source.is_empty());
    }

    #[test]
    fn source_normal_mode_supports_vim_movement_and_insert_commands() {
        let mut app = App::new();
        app.source = "one\ntwo".to_string();
        app.source_cursor = app.source.len();
        app.source_edit_mode = SourceEditMode::Normal;

        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(app.source_cursor, 0);

        app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert_eq!(app.source_cursor, app.source.len());

        app.handle_key(KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE));

        assert_eq!(app.source, "one\n>two");
        assert_eq!(app.source_edit_mode, SourceEditMode::Insert);
    }

    #[test]
    fn source_normal_mode_deletes_current_line_with_dd() {
        let mut app = App::new();
        app.source = "one\ntwo\nthree".to_string();
        app.source_cursor = cursor_for_line_col(&app.source, 1, 1);
        app.source_edit_mode = SourceEditMode::Normal;

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        assert_eq!(app.source, "one\nthree");
        assert_eq!(app.source_cursor, "one\n".len());
    }

    #[test]
    fn double_y_yanks_current_view() {
        let mut app = App::new();
        app.set_json(json!({"a": 1}), None);
        app.mode = Mode::Navigate;
        app.format_mode = FormatMode::Compact;

        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(app.take_clipboard().is_none());

        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        assert_eq!(app.take_clipboard(), Some(r#"{"a":1}"#.to_string()));
        assert!(!app.pending_yank);
    }

    #[test]
    fn capital_y_yanks_selected_value() {
        let mut app = App::new();
        app.set_json(json!({"items": [{"name": "Ada"}]}), None);
        app.mode = Mode::Navigate;
        app.selected = app
            .rows
            .iter()
            .position(|row| row.path_label() == "$.items")
            .unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT));

        assert_eq!(
            app.take_clipboard(),
            Some("[\n  {\n    \"name\": \"Ada\"\n  }\n]".to_string())
        );
    }

    #[test]
    fn yk_yanks_selected_key_value_pair() {
        let mut app = App::new();
        app.set_json(json!({"name": "Ada", "age": 36}), None);
        app.mode = Mode::Navigate;
        app.selected = app
            .rows
            .iter()
            .position(|row| row.path_label() == "$.age")
            .unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));

        assert_eq!(app.take_clipboard(), Some(r#""age": 36"#.to_string()));
    }

    #[test]
    fn yanks_value_after_editing_it() {
        let mut app = App::new();
        app.set_json(json!({"name": "Ada", "age": 36}), None);
        app.mode = Mode::Navigate;
        select_path(&mut app, "$.age");

        app.begin_value_edit();
        app.edit_buffer = "37".to_string();
        app.commit_edit();
        app.handle_key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT));

        assert_eq!(app.take_clipboard(), Some("37".to_string()));
    }

    #[test]
    fn o_adds_key_value_after_selected_object_entry() {
        let mut app = App::new();
        app.set_json(json!({"a": 1, "b": 2}), None);
        app.mode = Mode::Navigate;
        select_path(&mut app, "$.a");

        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));

        assert_eq!(app.mode, Mode::AddEntry);
        assert_eq!(app.editing_path, Vec::<crate::json::PathSegment>::new());
        assert_eq!(app.entry_insert_index, Some(1));
        assert_eq!(app.add_entry_field, AddEntryField::Key);
        assert!(app.add_key_buffer.is_empty());
        assert!(app.add_value_buffer.is_empty());

        type_text(&mut app, "name");
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.add_entry_field, AddEntryField::Value);
        type_text(&mut app, "Ada");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"a":1,"name":"Ada","b":2}"#
        );
        assert_eq!(app.selected_row().unwrap().path_label(), "$.name");
        assert!(serde_json::from_str::<serde_json::Value>(&app.source).is_ok());
        assert_eq!(app.source, app.raw_source);
    }

    #[test]
    fn o_appends_key_value_inside_selected_object() {
        let mut app = App::new();
        app.set_json(json!({"obj": {"a": 1}, "b": 2}), None);
        app.mode = Mode::Navigate;
        select_path(&mut app, "$.obj");

        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));

        assert_eq!(app.mode, Mode::AddEntry);
        assert_eq!(
            app.editing_path,
            vec![crate::json::PathSegment::Key("obj".to_string())]
        );
        assert_eq!(app.entry_insert_index, Some(1));

        app.add_key_buffer = "z".to_string();
        app.add_key_cursor = app.add_key_buffer.len();
        app.add_value_buffer = "false".to_string();
        app.add_value_cursor = app.add_value_buffer.len();
        app.add_entry_field = AddEntryField::Value;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"obj":{"a":1,"z":false},"b":2}"#
        );
        assert_eq!(app.selected_row().unwrap().path_label(), "$.obj.z");
    }

    #[test]
    fn o_rejects_duplicate_key_without_changing_json() {
        let mut app = App::new();
        app.set_json(json!({"a": 1, "b": 2}), None);
        app.mode = Mode::Navigate;
        select_path(&mut app, "$.a");

        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        app.add_key_buffer = "b".to_string();
        app.add_key_cursor = app.add_key_buffer.len();
        app.add_value_buffer = "3".to_string();
        app.add_value_cursor = app.add_value_buffer.len();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.mode, Mode::AddEntry);
        assert_eq!(app.add_entry_field, AddEntryField::Key);
        assert!(app.error.as_ref().is_some_and(|error| error.contains("b")));
        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"a":1,"b":2}"#
        );
    }

    #[test]
    fn d_deletes_selected_object_key_value_pair() {
        let mut app = App::new();
        app.set_json(json!({"a": 1, "b": 2, "c": 3}), None);
        app.mode = Mode::Navigate;
        select_path(&mut app, "$.b");

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        assert!(app.pending_delete);
        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"a":1,"b":2,"c":3}"#
        );

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"a":1,"c":3}"#
        );
        assert_eq!(app.selected_row().unwrap().path_label(), "$.c");
        assert!(serde_json::from_str::<serde_json::Value>(&app.source).is_ok());
        assert_eq!(app.source, app.raw_source);
    }

    #[test]
    fn d_deletes_selected_object_subtree_pair() {
        let mut app = App::new();
        app.set_json(json!({"obj": {"a": 1}, "b": 2}), None);
        app.mode = Mode::Navigate;
        select_path(&mut app, "$.obj");

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"b":2}"#
        );
        assert_eq!(app.selected_row().unwrap().path_label(), "$.b");
    }

    #[test]
    fn d_rejects_array_items_and_root_without_changing_json() {
        let mut app = App::new();
        app.set_json(json!({"items": [1, 2]}), None);
        app.mode = Mode::Navigate;
        select_path(&mut app, "$.items[0]");

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        assert!(!app.pending_delete);
        assert!(app.error.is_some());
        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"items":[1,2]}"#
        );

        select_path(&mut app, "$");
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        assert!(!app.pending_delete);
        assert!(app.error.is_some());
        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"items":[1,2]}"#
        );
    }

    #[test]
    fn esc_cancels_pending_pair_delete() {
        let mut app = App::new();
        app.set_json(json!({"a": 1, "b": 2}), None);
        app.mode = Mode::Navigate;
        select_path(&mut app, "$.a");

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(!app.pending_delete);
        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"a":1,"b":2}"#
        );
        assert_eq!(app.status, "Delete cancelled.");
    }

    #[test]
    fn u_undoes_last_key_value_add() {
        let mut app = App::new();
        app.set_json(json!({"a": 1, "b": 2}), None);
        app.mode = Mode::Navigate;
        select_path(&mut app, "$.a");

        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        type_text(&mut app, "name");
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        type_text(&mut app, "Ada");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"a":1,"name":"Ada","b":2}"#
        );

        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));

        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"a":1,"b":2}"#
        );
        assert_eq!(app.selected_row().unwrap().path_label(), "$.a");
        assert!(app.undo_snapshot.is_none());
    }

    #[test]
    fn u_undoes_last_key_value_delete() {
        let mut app = App::new();
        app.set_json(json!({"a": 1, "b": 2, "c": 3}), None);
        app.mode = Mode::Navigate;
        select_path(&mut app, "$.b");

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"a":1,"c":3}"#
        );

        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));

        assert_eq!(
            serde_json::to_string(app.json.as_ref().unwrap()).unwrap(),
            r#"{"a":1,"b":2,"c":3}"#
        );
        assert_eq!(app.selected_row().unwrap().path_label(), "$.b");
        assert!(app.undo_snapshot.is_none());
    }

    #[test]
    fn capital_v_enters_line_select_mode_on_selected_pretty_line() {
        let mut app = App::new();
        app.set_json(json!({"a": 1, "b": 2}), None);
        app.mode = Mode::Navigate;
        app.format_mode = FormatMode::Pretty;
        select_path(&mut app, "$.a");

        app.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT));

        assert_eq!(app.mode, Mode::LineSelect);
        assert_eq!(app.line_selection_anchor, Some(1));
        assert_eq!(app.line_selection_cursor, Some(1));
    }

    #[test]
    fn line_select_j_and_k_extend_the_line_range() {
        let mut app = App::new();
        app.set_json(json!({"a": 1, "b": 2}), None);
        app.mode = Mode::Navigate;
        app.format_mode = FormatMode::Pretty;
        select_path(&mut app, "$.a");

        app.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.line_selection_anchor, Some(1));
        assert_eq!(app.line_selection_cursor, Some(2));
        assert_eq!(app.line_selection_range(), Some(1..=2));

        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.line_selection_anchor, Some(1));
        assert_eq!(app.line_selection_cursor, Some(0));
        assert_eq!(app.line_selection_range(), Some(0..=1));
    }

    #[test]
    fn line_select_y_yanks_joined_pretty_lines() {
        let mut app = App::new();
        app.set_json(json!({"a": 1, "b": 2}), None);
        app.mode = Mode::Navigate;
        app.format_mode = FormatMode::Pretty;
        select_path(&mut app, "$.a");

        app.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        assert_eq!(
            app.take_clipboard(),
            Some("  \"a\": 1,\n  \"b\": 2".to_string())
        );
        assert_eq!(app.mode, Mode::Navigate);
        assert_eq!(app.line_selection_anchor, None);
        assert_eq!(app.line_selection_cursor, None);
    }

    #[test]
    fn line_select_escape_cancels_and_clears_selection() {
        let mut app = App::new();
        app.set_json(json!({"a": 1}), None);
        app.mode = Mode::Navigate;
        app.format_mode = FormatMode::Pretty;
        select_path(&mut app, "$.a");

        app.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.mode, Mode::Navigate);
        assert_eq!(app.line_selection_anchor, None);
        assert_eq!(app.line_selection_cursor, None);
    }

    #[test]
    fn line_select_requires_pretty_view() {
        let mut app = App::new();
        app.set_json(json!({"a": 1}), None);
        app.mode = Mode::Navigate;
        app.format_mode = FormatMode::Compact;
        select_path(&mut app, "$.a");

        app.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT));

        assert_eq!(app.mode, Mode::Navigate);
        assert!(app.error.is_some());
        assert_eq!(app.line_selection_anchor, None);
        assert_eq!(app.line_selection_cursor, None);
    }
}
