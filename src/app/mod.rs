mod input;
mod render;

use std::ops::RangeInclusive;

use serde_json::Value;

use crate::json::{
    JsonRow, PathSegment, flatten_json, get_value_at_path, get_value_at_path_mut,
    line_col_for_cursor, pretty_json_lines,
};

/// Top-level UI mode. Decides which keymap is active and which pane has focus.
///
/// - `Source`: raw text editor before parse
/// - `LineSelect`: Vim-like line selection (shift + v)
/// - `Navigate`: formatted JSON plus outline, vim-style movement
/// - `Search`: filter rows by query
/// - `AddEntry`: insert a new object key/value pair
/// - `EditKey` / `EditValue`: in-place edit of the selected row
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum Mode {
    Source,
    LineSelect,
    Navigate,
    Search,
    AddEntry,
    EditKey,
    EditValue,
    Help,
}

impl Mode {
    /// Human-readable name for the header/status line.
    fn label(self) -> &'static str {
        match self {
            Self::Source => "Source",
            Self::LineSelect => "Line Select",
            Self::Navigate => "Navigate",
            Self::Search => "Search",
            Self::AddEntry => "Add Entry",
            Self::EditKey => "Edit Key",
            Self::EditValue => "Edit Value",
            Self::Help => "Help",
        }
    }
}

/// Modal keymap inside the source editor.
///
/// `Insert` accepts typed text. `Normal` interprets keys as Vim-style movement
/// and editing commands.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum SourceEditMode {
    Insert,
    Normal,
}

impl SourceEditMode {
    fn label(self) -> &'static str {
        match self {
            Self::Insert => "Insert",
            Self::Normal => "Normal",
        }
    }
}

/// Active input in the add-entry popup.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum AddEntryField {
    Key,
    Value,
}

/// Display format for the parsed JSON in the main pane.
///
/// `Raw` shows the original input text untouched; others re-render from the parsed value.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum FormatMode {
    Pretty,
    Compact,
    Raw,
}

impl FormatMode {
    /// Display order used by the tab bar and Tab/Shift-Tab cycling.
    const ALL: [Self; 3] = [Self::Pretty, Self::Compact, Self::Raw];

    /// Tab title for this mode.
    fn label(self) -> &'static str {
        match self {
            Self::Pretty => "Beautify",
            Self::Compact => "Compact",
            Self::Raw => "Raw",
        }
    }

    /// Position within [`Self::ALL`]. Drives the highlighted tab and cycling math.
    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|mode| *mode == self)
            .unwrap_or_default()
    }
}

/// Whole-app state. Single owner; the event loop in `terminal` drives it.
///
/// Invariant: when `mode != Source`, `json` is `Some` and `rows` reflects it.
/// `raw_source` is the unmodified text used by `FormatMode::Raw`; `source` may be reformatted.
#[derive(Debug)]
pub(crate) struct App {
    source: String,
    source_cursor: usize,
    source_scroll: usize,
    line_selection_anchor: Option<usize>,
    line_selection_cursor: Option<usize>,
    raw_source: String,
    json: Option<Value>,
    rows: Vec<JsonRow>,
    selected: usize,
    format_mode: FormatMode,
    mode: Mode,
    source_edit_mode: SourceEditMode,
    source_pending_g: bool,
    source_pending_d: bool,
    outline_panel: bool,
    display_scroll: u16,
    edit_buffer: String,
    edit_cursor: usize,
    editing_path: Vec<PathSegment>,
    entry_insert_index: Option<usize>,
    add_key_buffer: String,
    add_key_cursor: usize,
    add_value_buffer: String,
    add_value_cursor: usize,
    add_entry_field: AddEntryField,
    search_query: String,
    search_matches: Vec<usize>,
    search_match_index: Option<usize>,
    /// `y` pressed once; next key picks what to copy (`y`, `v`, `k`, ...).
    pending_yank: bool,
    /// Text staged for the event loop to push to the OS clipboard next tick.
    pending_clipboard: Option<String>,
    status: String,
    error: Option<String>,
    should_quit: bool,
}

impl App {
    /// Construct an empty app in `Source` mode, waiting for JSON input.
    pub(crate) fn new() -> Self {
        Self {
            source: String::new(),
            source_cursor: 0,
            source_scroll: 0,
            line_selection_anchor: None,
            line_selection_cursor: None,
            raw_source: String::new(),
            json: None,
            rows: Vec::new(),
            selected: 0,
            format_mode: FormatMode::Pretty,
            mode: Mode::Source,
            source_edit_mode: SourceEditMode::Insert,
            source_pending_g: false,
            source_pending_d: false,
            outline_panel: true,
            display_scroll: 0,
            edit_buffer: String::new(),
            edit_cursor: 0,
            editing_path: Vec::new(),
            entry_insert_index: None,
            add_key_buffer: String::new(),
            add_key_cursor: 0,
            add_value_buffer: String::new(),
            add_value_cursor: 0,
            add_entry_field: AddEntryField::Key,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_match_index: None,
            pending_yank: false,
            pending_clipboard: None,
            status: "Paste JSON. Ctrl+P parses, Ctrl+B beautifies, Ctrl+M compacts.".to_string(),
            error: None,
            should_quit: false,
        }
    }

    /// True when the user pressed `q` / `Ctrl+C`. Event loop should exit.
    pub(crate) fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Drain text staged for the OS clipboard. Called once per tick by the event loop.
    pub(crate) fn take_clipboard(&mut self) -> Option<String> {
        self.pending_clipboard.take()
    }

    /// Set status after a successful clipboard write. `method` is the backend name (`pbcopy`, `OSC 52`, ...).
    pub(crate) fn report_clipboard_success(&mut self, method: &'static str) {
        self.status = format!("Copied to clipboard via {method}.");
        self.error = None;
    }

    /// Surface a clipboard failure to the user via the error line.
    pub(crate) fn report_clipboard_error(&mut self, error: String) {
        self.error = Some(format!("Copy failed: {error}"));
    }

    /// Reset all state to a fresh `Source` buffer (triggered by `Ctrl+N`).
    fn begin_new_json(&mut self) {
        self.source.clear();
        self.source_cursor = 0;
        self.source_scroll = 0;
        self.line_selection_anchor = None;
        self.line_selection_cursor = None;
        self.raw_source.clear();
        self.json = None;
        self.rows.clear();
        self.selected = 0;
        self.format_mode = FormatMode::Pretty;
        self.mode = Mode::Source;
        self.source_edit_mode = SourceEditMode::Insert;
        self.source_pending_g = false;
        self.source_pending_d = false;
        self.outline_panel = true;
        self.display_scroll = 0;
        self.edit_buffer.clear();
        self.edit_cursor = 0;
        self.editing_path.clear();
        self.clear_add_entry_state();
        self.search_query.clear();
        self.search_matches.clear();
        self.search_match_index = None;
        self.pending_yank = false;
        self.pending_clipboard = None;
        self.status = "New JSON buffer. Paste JSON or type source, then Ctrl+P parses.".to_string();
        self.error = None;
    }

    /// Parse `source` into `json`. On success enters `Navigate`; on failure sets `error`.
    /// Returns `Ok(())` so callers can chain (e.g. parse-then-beautify).
    fn parse_source(&mut self) -> Result<(), ()> {
        match serde_json::from_str::<Value>(&self.source) {
            Ok(value) => {
                self.raw_source = self.source.clone();
                self.set_json(value, None);
                self.mode = Mode::Navigate;
                self.status = "JSON parsed. Use j/k to move and b/m/r to switch beautified, compact, and raw views."
                    .to_string();
                self.error = None;
                Ok(())
            }
            Err(error) => {
                self.error = Some(format!("Invalid JSON: {error}"));
                Err(())
            }
        }
    }

    /// `Ctrl+B`: parse `source`, then rewrite it as pretty JSON and switch to `Pretty` view.
    fn parse_and_beautify_source(&mut self) {
        if self.parse_source().is_ok() {
            self.beautify_source();
            self.format_mode = FormatMode::Pretty;
            self.mode = Mode::Navigate;
        }
    }

    /// `Ctrl+M`: parse `source`, then rewrite it as compact JSON and switch to `Compact` view.
    fn parse_and_compact_source(&mut self) {
        if self.parse_source().is_ok() {
            self.compact_source();
            self.format_mode = FormatMode::Compact;
            self.mode = Mode::Navigate;
        }
    }

    /// `Esc` in `Source`: jump back to `Navigate` if we already have a parsed value.
    fn return_to_navigate_if_parsed(&mut self) {
        if self.json.is_some() {
            self.mode = Mode::Navigate;
            self.status = "Returned to navigator.".to_string();
        }
    }

    /// Replace `json` with `value` and rebuild rows. Optional `select_path` focuses a row.
    fn set_json(&mut self, value: Value, select_path: Option<Vec<PathSegment>>) {
        self.json = Some(value);
        self.rebuild_rows(select_path);
    }

    /// Re-flatten `json` into `rows`. If `select_path` matches an emitted row, focus it;
    /// otherwise clamp the prior `selected` index to the new row count.
    fn rebuild_rows(&mut self, select_path: Option<Vec<PathSegment>>) {
        self.rows.clear();
        if let Some(json) = &self.json {
            flatten_json(json, &mut self.rows);
        }

        if self.rows.is_empty() {
            self.selected = 0;
            self.refresh_search_matches();
            return;
        }

        if let Some(path) = select_path
            && let Some(index) = self.rows.iter().position(|row| row.path == path)
        {
            self.selected = index;
            self.refresh_search_matches();
            return;
        }

        self.selected = self.selected.min(self.rows.len() - 1);
        self.refresh_search_matches();
    }

    /// Rewrite `source` (and `raw_source`) from `json` as pretty-printed JSON. No-op if unparsed.
    fn beautify_source(&mut self) {
        if let Some(json) = &self.json {
            self.source = serde_json::to_string_pretty(json).unwrap_or_default();
            self.source_cursor = self.source.len();
            self.raw_source = self.source.clone();
            self.status = "Source beautified.".to_string();
        }
    }

    /// Rewrite `source` (and `raw_source`) from `json` as one-line compact JSON.
    fn compact_source(&mut self) {
        if let Some(json) = &self.json {
            self.source = serde_json::to_string(json).unwrap_or_default();
            self.source_cursor = self.source.len();
            self.raw_source = self.source.clone();
            self.status = "Source compacted without extra spacing.".to_string();
        }
    }

    /// Render `json` for the main pane according to the current [`FormatMode`].
    /// Falls back to the raw `source` text when nothing is parsed yet.
    fn formatted_json(&self) -> String {
        match (&self.json, self.format_mode) {
            (Some(json), FormatMode::Pretty) => {
                serde_json::to_string_pretty(json).unwrap_or_default()
            }
            (Some(json), FormatMode::Compact) => serde_json::to_string(json).unwrap_or_default(),
            (Some(_), FormatMode::Raw) => self.raw_source.clone(),
            (None, _) => self.source.clone(),
        }
    }

    /// Text of whatever pane the user is looking at. Used by `yy` (yank current view).
    fn current_view_text(&self) -> Option<String> {
        if self.mode == Mode::Source || self.json.is_none() {
            return (!self.source.is_empty()).then(|| self.source.clone());
        }

        Some(self.formatted_json())
    }

    /// Pretty JSON for the value under the current selection. `None` if nothing selected.
    fn selected_value_text(&self) -> Option<String> {
        let row = self.selected_row()?;
        let json = self.json.as_ref()?;
        let value = get_value_at_path(json, &row.path)?;
        serde_json::to_string_pretty(value).ok()
    }

    /// `"key": value` text for the selection. `None` when the row sits under an array.
    fn selected_key_value_text(&self) -> Option<String> {
        let row = self.selected_row()?;
        let key = match row.path.last()? {
            PathSegment::Key(key) => key,
            PathSegment::Index(_) => return None,
        };
        let json = self.json.as_ref()?;
        let value = get_value_at_path(json, &row.path)?;
        let key = serde_json::to_string(key).ok()?;
        let value = serde_json::to_string_pretty(value).ok()?;
        Some(format!("{key}: {value}"))
    }

    /// Pretty-view line index for the currently selected JSON row.
    fn selected_pretty_line_index(&self) -> Option<usize> {
        let json = self.json.as_ref()?;
        let selected_path = self.selected_row()?.path.as_slice();

        pretty_json_lines(json)
            .iter()
            .position(|line| line.path.as_slice() == selected_path)
    }

    /// Inclusive line range currently covered by visual-line selection.
    fn line_selection_range(&self) -> Option<RangeInclusive<usize>> {
        let anchor = self.line_selection_anchor?;
        let cursor = self.line_selection_cursor?;

        Some(anchor.min(cursor)..=anchor.max(cursor))
    }

    /// Pretty JSON text currently covered by visual-line selection.
    fn selected_lines_text(&self) -> Option<String> {
        let json = self.json.as_ref()?;
        let lines = pretty_json_lines(json);
        if lines.is_empty() {
            return None;
        }

        let range = self.line_selection_range()?;
        let start = (*range.start()).min(lines.len() - 1);
        let end = (*range.end()).min(lines.len() - 1);

        Some(
            lines[start..=end]
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Clear visual-line selection state without changing the active mode.
    fn clear_line_selection(&mut self) {
        self.line_selection_anchor = None;
        self.line_selection_cursor = None;
    }

    /// Stage `text` for the event loop to push to the clipboard. `description` shows in the status.
    fn queue_clipboard(&mut self, text: String, description: &str) {
        let bytes = text.len();
        self.pending_clipboard = Some(text);
        self.pending_yank = false;
        self.status = format!("Yanking {description} ({bytes} bytes)...");
        self.error = None;
    }

    /// `yy`: copy the rendered text of the current pane to the clipboard.
    fn yank_current_view(&mut self) {
        let Some(text) = self.current_view_text() else {
            self.error = Some("Nothing to yank yet.".to_string());
            self.pending_yank = false;
            return;
        };

        self.queue_clipboard(text, self.format_mode.label());
    }

    /// `Y` / `yv`: copy just the selected value as pretty JSON.
    fn yank_selected_value(&mut self) {
        let Some(text) = self.selected_value_text() else {
            self.error = Some("No selected JSON value to yank.".to_string());
            self.pending_yank = false;
            return;
        };

        self.queue_clipboard(text, "selected value");
    }

    /// `yk`: copy the selected entry as a `"key": value` snippet.
    fn yank_selected_key_value(&mut self) {
        let Some(text) = self.selected_key_value_text() else {
            self.error = Some("Selected row is not an object key/value pair.".to_string());
            self.pending_yank = false;
            return;
        };

        self.queue_clipboard(text, "selected key/value pair");
    }

    /// `y` in line-select mode: copy the selected pretty JSON lines.
    fn yank_selected_lines(&mut self) {
        let Some(text) = self.selected_lines_text() else {
            self.error = Some("No selected JSON lines to yank.".to_string());
            self.clear_line_selection();
            self.mode = Mode::Navigate;
            return;
        };

        self.clear_line_selection();
        self.mode = Mode::Navigate;
        self.queue_clipboard(text, "selected lines");
    }

    /// `e` / `Enter`: load the selected value into `edit_buffer` and enter `EditValue`.
    fn begin_value_edit(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let path = row.path.clone();
        let Some(json) = &self.json else {
            return;
        };
        let Some(value) = get_value_at_path(json, &path) else {
            return;
        };

        self.edit_buffer = serde_json::to_string(value).unwrap_or_default();
        self.edit_cursor = self.edit_buffer.len();
        self.editing_path = path;
        self.clear_add_entry_state();
        self.mode = Mode::EditValue;
        self.status = "Editing value. Enter a valid JSON value.".to_string();
    }

    /// `K`: load the selected object key into `edit_buffer` and enter `EditKey`.
    /// Refuses rows that aren't object entries.
    fn begin_key_edit(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };

        if !row.key_editable {
            self.error = Some("Only object entry keys can be renamed.".to_string());
            return;
        }

        let path = row.path.clone();
        let Some(PathSegment::Key(key)) = path.last() else {
            self.error = Some("Selected row does not have a JSON object key.".to_string());
            return;
        };
        let key = key.clone();

        self.edit_buffer = key;
        self.edit_cursor = self.edit_buffer.len();
        self.editing_path = path;
        self.clear_add_entry_state();
        self.mode = Mode::EditKey;
        self.status = "Editing key. Enter saves and keeps the JSON valid.".to_string();
    }

    /// `o`: start inserting a new key/value entry near the current cursor.
    ///
    /// Selecting an object appends inside that object. Selecting an object entry inserts
    /// a sibling immediately after that entry. Arrays and scalar roots cannot accept keys.
    fn begin_entry_add(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let selected_path = row.path.clone();

        let (parent_path, insert_index) = match self.entry_add_target(&selected_path) {
            Ok(target) => target,
            Err(message) => {
                self.error = Some(message);
                return;
            }
        };

        let Some(json) = &self.json else {
            return;
        };
        let Some(Value::Object(_)) = get_value_at_path(json, &parent_path) else {
            self.error = Some("Target is not a JSON object.".to_string());
            return;
        };

        self.add_key_buffer.clear();
        self.add_key_cursor = 0;
        self.add_value_buffer.clear();
        self.add_value_cursor = 0;
        self.add_entry_field = AddEntryField::Key;
        self.editing_path = parent_path;
        self.entry_insert_index = Some(insert_index);
        self.pending_delete = false;
        self.mode = Mode::AddEntry;
        self.status = "Adding key/value. Type a key, Tab to value, Enter saves.".to_string();
    }

    /// Resolve where `o` should insert a new entry.
    fn entry_add_target(
        &self,
        selected_path: &[PathSegment],
    ) -> Result<(Vec<PathSegment>, usize), String> {
        let Some(json) = &self.json else {
            return Err("No parsed JSON to edit.".to_string());
        };
        let Some(selected_value) = get_value_at_path(json, selected_path) else {
            return Err("Selected path no longer exists.".to_string());
        };

        if let Value::Object(map) = selected_value {
            return Ok((selected_path.to_vec(), map.len()));
        }

        let Some((last_segment, parent_path)) = selected_path.split_last() else {
            return Err("Select a JSON object before adding a key/value pair.".to_string());
        };
        let PathSegment::Key(selected_key) = last_segment else {
            return Err(
                "Array entries cannot have object keys. Select an object first.".to_string(),
            );
        };
        let Some(Value::Object(parent_map)) = get_value_at_path(json, parent_path) else {
            return Err("Selected parent is not a JSON object.".to_string());
        };
        let Some(index) = parent_map.keys().position(|key| key == selected_key) else {
            return Err("Selected key no longer exists.".to_string());
        };

        Ok((parent_path.to_vec(), index + 1))
    }

    /// `Enter` in an edit popup. Dispatches to the value or key commit path.
    fn commit_edit(&mut self) {
        match self.mode {
            Mode::EditValue => self.commit_value_edit(),
            Mode::EditKey => self.commit_key_edit(),
            Mode::AddEntry => self.commit_entry_add(),
            _ => {}
        }
    }

    /// Parse `edit_buffer` as JSON and replace the value at `editing_path`.
    /// Sets `error` if the text is not valid JSON or the path vanished.
    fn commit_value_edit(&mut self) {
        let new_value = match serde_json::from_str::<Value>(&self.edit_buffer) {
            Ok(value) => value,
            Err(error) => {
                self.error = Some(format!("Value edit is not valid JSON: {error}"));
                return;
            }
        };

        let path = self.editing_path.clone();
        let Some(json) = &mut self.json else {
            return;
        };
        let Some(target) = get_value_at_path_mut(json, &path) else {
            self.error = Some("Selected path no longer exists.".to_string());
            return;
        };

        *target = new_value;
        self.after_structural_edit(path, "Value updated.");
    }

    /// Rename the selected object key to `edit_buffer`, preserving its position in the map.
    /// Errors out on duplicate keys or non-object parents.
    fn commit_key_edit(&mut self) {
        let new_key = self.edit_buffer.clone();
        let old_path = self.editing_path.clone();
        let Some((old_key, parent_path)) = old_path.split_last() else {
            self.error = Some("Root value has no editable key.".to_string());
            return;
        };
        let PathSegment::Key(old_key) = old_key else {
            self.error = Some("Array indexes cannot be renamed.".to_string());
            return;
        };

        let Some(json) = &mut self.json else {
            return;
        };
        let Some(parent) = get_value_at_path_mut(json, parent_path) else {
            self.error = Some("Selected parent no longer exists.".to_string());
            return;
        };
        let Value::Object(map) = parent else {
            self.error = Some("Selected parent is not an object.".to_string());
            return;
        };

        if new_key != *old_key && map.contains_key(&new_key) {
            self.error = Some(format!(
                "The key \"{new_key}\" already exists at this level."
            ));
            return;
        }

        if new_key != *old_key {
            let Some(index) = map.keys().position(|key| key == old_key) else {
                self.error = Some("Original key no longer exists.".to_string());
                return;
            };

            if map
                .shift_insert(index, new_key.clone(), Value::Null)
                .is_some()
            {
                self.error = Some("Could not rename key without overwriting an entry.".to_string());
                return;
            }

            let Some(value) = map.shift_remove(old_key) else {
                self.error = Some("Original key no longer exists.".to_string());
                return;
            };
            let Some(slot) = map.get_mut(&new_key) else {
                self.error = Some("Renamed key was not inserted.".to_string());
                return;
            };
            *slot = value;
        }

        let mut new_path = parent_path.to_vec();
        new_path.push(PathSegment::Key(new_key));
        self.after_structural_edit(new_path, "Key renamed.");
    }

    /// Parse and insert a new object entry at the pending target.
    fn commit_entry_add(&mut self) {
        let new_key = self.add_key_buffer.clone();
        if new_key.trim().is_empty() {
            self.error = Some("Key cannot be empty.".to_string());
            self.add_entry_field = AddEntryField::Key;
            return;
        }

        let new_value = match parse_inferred_json_value(&self.add_value_buffer) {
            Ok(value) => value,
            Err(message) => {
                self.error = Some(message);
                self.add_entry_field = AddEntryField::Value;
                return;
            }
        };

        let parent_path = self.editing_path.clone();
        let Some(insert_index) = self.entry_insert_index else {
            self.error = Some("No insertion target is active.".to_string());
            return;
        };
        let Some(parent) = self
            .json
            .as_ref()
            .and_then(|json| get_value_at_path(json, &parent_path))
        else {
            self.error = Some("Target object no longer exists.".to_string());
            return;
        };
        let Value::Object(map) = parent else {
            self.error = Some("Target is not a JSON object.".to_string());
            return;
        };

        if map.contains_key(&new_key) {
            self.error = Some(format!(
                "The key \"{new_key}\" already exists at this level."
            ));
            self.add_entry_field = AddEntryField::Key;
            return;
        }

        self.stash_undo("key/value add");

        let Some(json) = &mut self.json else {
            return;
        };
        let Some(parent) = get_value_at_path_mut(json, &parent_path) else {
            self.error = Some("Target object no longer exists.".to_string());
            return;
        };
        let Value::Object(map) = parent else {
            self.error = Some("Target is not a JSON object.".to_string());
            return;
        };
        let index = insert_index.min(map.len());
        if map
            .shift_insert(index, new_key.clone(), new_value)
            .is_some()
        {
            self.error = Some("Could not insert key without overwriting an entry.".to_string());
            return;
        }

        let mut new_path = parent_path;
        new_path.push(PathSegment::Key(new_key));
        self.clear_add_entry_state();
        self.after_structural_edit(new_path, "Key/value added.");
    }
    /// reset to `Navigate`/`Pretty`, rebuild rows, and re-focus `select_path`.
    fn after_structural_edit(&mut self, select_path: Vec<PathSegment>, message: &str) {
        if let Some(json) = &self.json {
            self.source = serde_json::to_string_pretty(json).unwrap_or_default();
            self.raw_source = self.source.clone();
            self.source_cursor = self.source.len();
        }
        self.mode = Mode::Navigate;
        self.format_mode = FormatMode::Pretty;
        self.rebuild_rows(Some(select_path));
        self.status = message.to_string();
        self.error = None;
    }

    /// Row currently selected in the JSON outline.
    fn selected_row(&self) -> Option<&JsonRow> {
        self.rows.get(self.selected)
    }

    /// `(line, col)` of `source_cursor` in `source`. Used by the source editor renderer.
    fn source_line_col(&self) -> (usize, usize) {
        line_col_for_cursor(&self.source, self.source_cursor)
    }

    fn clear_add_entry_state(&mut self) {
        self.entry_insert_index = None;
        self.add_key_buffer.clear();
        self.add_key_cursor = 0;
        self.add_value_buffer.clear();
        self.add_value_cursor = 0;
        self.add_entry_field = AddEntryField::Key;
    }
fn parse_inferred_json_value(text: &str) -> Result<Value, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }

    match trimmed.chars().next() {
        Some('{') | Some('[') => serde_json::from_str::<Value>(trimmed)
            .map_err(|error| format!("Value is not valid JSON: {error}")),
        Some('"') => match serde_json::from_str::<Value>(trimmed) {
            Ok(Value::String(value)) => Ok(Value::String(value)),
            Ok(_) => Err("Quoted value must be a JSON string.".to_string()),
            Err(error) => Err(format!("String value is not valid JSON: {error}")),
        },
        Some('-' | '0'..='9') => match serde_json::from_str::<Value>(trimmed) {
            Ok(Value::Number(number)) => Ok(Value::Number(number)),
            Ok(_) => Err("Number-like value did not parse as a JSON number.".to_string()),
            Err(error) => Err(format!(
                "Number-like value is not valid JSON. Quote it for text: {error}"
            )),
        },
        _ if matches!(trimmed, "true" | "false" | "null") => serde_json::from_str::<Value>(trimmed)
            .map_err(|error| format!("Value is not valid JSON: {error}")),
        _ => Ok(Value::String(trimmed.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renames_object_key_without_moving_its_position() {
        let mut app = App::new();
        let value = serde_json::from_str::<Value>(r#"{"z":1,"a":2,"m":3}"#).unwrap();
        app.set_json(value, None);

        app.editing_path = vec![PathSegment::Key("a".to_string())];
        app.edit_buffer = "x".to_string();
        app.mode = Mode::EditKey;
        app.commit_edit();

        let json = app.json.as_ref().unwrap();
        assert_eq!(
            serde_json::to_string(json).unwrap(),
            r#"{"z":1,"x":2,"m":3}"#
        );
        assert_eq!(
            app.rows.iter().map(JsonRow::path_label).collect::<Vec<_>>(),
            vec!["$", "$.z", "$.x", "$.m"]
        );
    }

    #[test]
    fn infers_add_entry_values_from_plain_text() {
        assert_eq!(parse_inferred_json_value("Ada").unwrap(), json!("Ada"));
        assert_eq!(parse_inferred_json_value("37").unwrap(), json!(37));
        assert_eq!(parse_inferred_json_value("true").unwrap(), json!(true));
        assert_eq!(parse_inferred_json_value("").unwrap(), Value::Null);
        assert_eq!(parse_inferred_json_value(r#""37""#).unwrap(), json!("37"));
        assert_eq!(
            parse_inferred_json_value(r#"{"active": true}"#).unwrap(),
            json!({"active": true})
        );
    }
}
