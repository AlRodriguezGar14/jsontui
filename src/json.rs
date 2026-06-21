use ratatui::style::{Color, Style};
use serde_json::Value;

/// One hop in a JSON path: object key or array index.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum PathSegment {
    Key(String),
    Index(usize),
}

/// Kind tag for a tree row. Container variants carry child count for preview/label.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum RowKind {
    Object(usize),
    Array(usize),
    String,
    Number,
    Bool,
    Null,
}

impl RowKind {
    /// Short type name shown in the tree row (`"object"`, `"string"`, ...).
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Object(_) => "object",
            Self::Array(_) => "array",
            Self::String => "string",
            Self::Number => "number",
            Self::Bool => "bool",
            Self::Null => "null",
        }
    }

    /// Ratatui color style for the kind label. Picks a foreground color per JSON type.
    pub(crate) fn style(&self) -> Style {
        match self {
            Self::Object(_) => Style::default().fg(Color::Blue),
            Self::Array(_) => Style::default().fg(Color::Magenta),
            Self::String => Style::default().fg(Color::Green),
            Self::Number => Style::default().fg(Color::Yellow),
            Self::Bool => Style::default().fg(Color::Cyan),
            Self::Null => Style::default().fg(Color::DarkGray),
        }
    }
}

/// Flattened tree row produced from a JSON value.
///
/// One row per scalar or container; containers also emit rows for each child via [`flatten_json`].
/// `key_editable` is true only when the row sits under an object (parent segment is a `Key`).
#[derive(Debug, Clone)]
pub(crate) struct JsonRow {
    pub(crate) path: Vec<PathSegment>,
    pub(crate) depth: usize,
    pub(crate) name: String,
    pub(crate) kind: RowKind,
    pub(crate) preview: String,
    pub(crate) key_editable: bool,
}

impl JsonRow {
    /// Render this row's path as a JSONPath string (e.g. `$.items[0].name`).
    pub(crate) fn path_label(&self) -> String {
        format_path(&self.path)
    }
}

/// One rendered line of pretty-printed JSON paired with the path it represents.
/// Used to map cursor position in the pretty view back to a tree selection.
#[derive(Debug)]
pub(crate) struct PrettyJsonLine {
    pub(crate) text: String,
    pub(crate) path: Vec<PathSegment>,
}

/// Depth-first flatten of `value` into `rows`. Preserves object insertion order.
///
/// # Example
/// Input:
/// ```text
/// {"name": "Ada", "scores": [1, 2]}
/// ```
/// Produces rows with path labels (in order):
/// ```text
/// $
/// $.name
/// $.scores
/// $.scores[0]
/// $.scores[1]
/// ```
pub(crate) fn flatten_json(value: &Value, rows: &mut Vec<JsonRow>) {
    let mut path = Vec::new();
    flatten_value(value, &mut path, 0, "$".to_string(), false, rows);
}

/// Recursive worker for [`flatten_json`].
///
/// # Args
/// - `value`: current node.
/// - `path`: scratch buffer pushed/popped as we descend; cloned into each emitted row.
/// - `depth`: indentation level.
/// - `name`: label shown for the row (key string, `"[i]"`, or `"$"` at root).
/// - `key_editable`: true if `value` sits under an object (rename is legal).
/// - `rows`: output sink.
///
/// # Example
/// Called with `value = {"a": [10, 20]}`, `path = []`, `depth = 0`, `name = "$"`,
/// `key_editable = false`. It pushes the root row, then recurses into `a` (an array
/// row at depth 1 with `key_editable = true`), then into each element (index rows at
/// depth 2 with `key_editable = false`). Final `rows` contains four entries with
/// paths `[]`, `[Key("a")]`, `[Key("a"), Index(0)]`, `[Key("a"), Index(1)]`.
fn flatten_value(
    value: &Value,
    path: &mut Vec<PathSegment>,
    depth: usize,
    name: String,
    key_editable: bool,
    rows: &mut Vec<JsonRow>,
) {
    let kind = row_kind(value);
    let preview = preview_value(value);
    rows.push(JsonRow {
        path: path.clone(),
        depth,
        name,
        kind,
        preview,
        key_editable,
    });

    match value {
        Value::Object(map) => {
            for (key, child) in map {
                path.push(PathSegment::Key(key.clone()));
                flatten_value(child, path, depth + 1, key.clone(), true, rows);
                path.pop();
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                path.push(PathSegment::Index(index));
                flatten_value(child, path, depth + 1, format!("[{index}]"), false, rows);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Render `value` as pretty JSON lines, each tagged with its source path.
///
/// # Example
/// Input: `{"a": 1, "b": [true]}`. Output (text → path):
/// ```text
/// {                  []
///   "a": 1,          [Key("a")]
///   "b": [           [Key("b")]
///     true           [Key("b"), Index(0)]
///   ]                [Key("b")]
/// }                  []
/// ```
pub(crate) fn pretty_json_lines(value: &Value) -> Vec<PrettyJsonLine> {
    let mut lines = Vec::new();
    let mut path = Vec::new();
    push_pretty_json_lines(
        value,
        &mut path,
        0,
        String::new(),
        String::new(),
        &mut lines,
    );
    lines
}

/// Recursive worker for [`pretty_json_lines`].
///
/// Emits one `PrettyJsonLine` per opening brace, scalar, and closing brace.
/// `prefix` is what goes before the value on its first line (typically `"  \"key\": "` or
/// indent spaces); `suffix` is appended to the last line (`","` between siblings, else empty).
///
/// # Example - object child
/// Rendering the `a` field of `{"a": 1, "b": 2}` recurses with
/// `prefix = "  \"a\": "`, `suffix = ","`, `indent = 2`. Since `1` is a scalar it falls
/// into the `_` arm and emits one line: `  "a": 1,`.
///
/// # Example - array child
/// Rendering element `0` of `[true, false]` recurses with `prefix = "  "`,
/// `suffix = ","`, producing the line `  true,`.
///
/// # Example - nested container
/// Rendering `{"b": [1]}` for the value of `b`: prefix `"  \"b\": "`, suffix `""`,
/// indent 2. The object arm emits:
/// ```text
///   "b": [        // prefix + '['
///     1           // recurse: prefix = "    ", suffix = ""
///   ]             // " ".repeat(2) + "]" + suffix
/// ```
fn push_pretty_json_lines(
    value: &Value,
    path: &mut Vec<PathSegment>,
    indent: usize,
    prefix: String,
    suffix: String,
    lines: &mut Vec<PrettyJsonLine>,
) {
    match value {
        Value::Object(map) if map.is_empty() => {
            lines.push(PrettyJsonLine {
                text: format!("{prefix}{{}}{suffix}"),
                path: path.clone(),
            });
        }
        Value::Object(map) => {
            lines.push(PrettyJsonLine {
                text: format!("{prefix}{{"),
                path: path.clone(),
            });
            let last_index = map.len().saturating_sub(1);
            for (index, (key, child)) in map.iter().enumerate() {
                path.push(PathSegment::Key(key.clone()));
                let child_prefix = format!("{}{}: ", " ".repeat(indent + 2), json_key(key));
                let child_suffix = if index == last_index { "" } else { "," };
                push_pretty_json_lines(
                    child,
                    path,
                    indent + 2,
                    child_prefix,
                    child_suffix.to_string(),
                    lines,
                );
                path.pop();
            }
            lines.push(PrettyJsonLine {
                text: format!("{}{}{}", " ".repeat(indent), "}", suffix),
                path: path.clone(),
            });
        }
        Value::Array(values) if values.is_empty() => {
            lines.push(PrettyJsonLine {
                text: format!("{prefix}[]{suffix}"),
                path: path.clone(),
            });
        }
        Value::Array(values) => {
            lines.push(PrettyJsonLine {
                text: format!("{prefix}["),
                path: path.clone(),
            });
            let last_index = values.len().saturating_sub(1);
            for (index, child) in values.iter().enumerate() {
                path.push(PathSegment::Index(index));
                let child_prefix = " ".repeat(indent + 2);
                let child_suffix = if index == last_index { "" } else { "," };
                push_pretty_json_lines(
                    child,
                    path,
                    indent + 2,
                    child_prefix,
                    child_suffix.to_string(),
                    lines,
                );
                path.pop();
            }
            lines.push(PrettyJsonLine {
                text: format!("{}]{}", " ".repeat(indent), suffix),
                path: path.clone(),
            });
        }
        _ => {
            lines.push(PrettyJsonLine {
                text: format!("{prefix}{}{suffix}", scalar_json(value)),
                path: path.clone(),
            });
        }
    }
}

/// Quote `key` as a JSON string literal (`foo` → `"foo"`).
fn json_key(key: &str) -> String {
    serde_json::to_string(key).unwrap_or_default()
}

/// Serialize a scalar JSON value to its canonical text form.
fn scalar_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// Classify `value` into a [`RowKind`]. Containers carry their child count.
///
/// # Examples
/// - `{"a":1}`   → `RowKind::Object(1)`
/// - `[1,2,3]`   → `RowKind::Array(3)`
/// - `"x"`       → `RowKind::String`
/// - `null`      → `RowKind::Null`
fn row_kind(value: &Value) -> RowKind {
    match value {
        Value::Object(map) => RowKind::Object(map.len()),
        Value::Array(values) => RowKind::Array(values.len()),
        Value::String(_) => RowKind::String,
        Value::Number(_) => RowKind::Number,
        Value::Bool(_) => RowKind::Bool,
        Value::Null => RowKind::Null,
    }
}

/// Short single-line preview shown next to a row.
///
/// Containers show item counts; scalars show their JSON text.
///
/// # Examples
/// - `{"a":1,"b":2}` → `"{2 keys}"`
/// - `[10, 20, 30]` → `"[3 items]"`
/// - `"hello"`      → `"\"hello\""`
/// - `42`           → `"42"`
/// - `null`         → `"null"`
fn preview_value(value: &Value) -> String {
    match value {
        Value::Object(map) => format!("{{{} keys}}", map.len()),
        Value::Array(values) => format!("[{} items]", values.len()),
        _ => scalar_json(value),
    }
}

/// True when `query` matches any searchable text for a row.
///
/// Search is case-insensitive and covers the displayed name, path, type label,
/// and preview so `/ada`, `/$.items`, `/string`, and `/3 items` all work.
pub(crate) fn row_matches_query(row: &JsonRow, query: &str) -> bool {
    let query = query.to_lowercase();
    if query.is_empty() {
        return true;
    }

    row.name.to_lowercase().contains(&query)
        || row.path_label().to_lowercase().contains(&query)
        || row.kind.label().contains(&query)
        || row.preview.to_lowercase().contains(&query)
}

/// Walk `path` from `value` and return the node at the end.
///
/// Returns `None` if any segment doesn't exist or type-mismatches (e.g. `Index` into an object).
///
/// # Example
/// `value = {"items": [{"name": "Ada"}]}`, `path = [Key("items"), Index(0), Key("name")]`
/// → `Some("Ada")`. Same call with `path = [Key("missing")]` → `None`.
pub(crate) fn get_value_at_path<'a>(value: &'a Value, path: &[PathSegment]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = match segment {
            PathSegment::Key(key) => current.get(key)?,
            PathSegment::Index(index) => current.get(*index)?,
        };
    }
    Some(current)
}

/// Mutable variant of [`get_value_at_path`]. Same lookup, returns `&mut`.
pub(crate) fn get_value_at_path_mut<'a>(
    value: &'a mut Value,
    path: &[PathSegment],
) -> Option<&'a mut Value> {
    let mut current = value;
    for segment in path {
        current = match segment {
            PathSegment::Key(key) => current.get_mut(key)?,
            PathSegment::Index(index) => current.get_mut(*index)?,
        };
    }
    Some(current)
}

/// Format `path` as a JSONPath-style string (e.g. `$.user["display name"][2]`).
///
/// # Examples
/// - `[]`                                          → `"$"`
/// - `[Key("name")]`                               → `"$.name"`
/// - `[Key("user"), Key("display name"), Index(2)]` → `"$.user[\"display name\"][2]"`
pub(crate) fn format_path(path: &[PathSegment]) -> String {
    let mut output = "$".to_string();

    for segment in path {
        match segment {
            PathSegment::Key(key) if is_simple_key(key) => {
                output.push('.');
                output.push_str(key);
            }
            PathSegment::Key(key) => {
                output.push('[');
                output.push_str(&json_key(key));
                output.push(']');
            }
            PathSegment::Index(index) => {
                output.push('[');
                output.push_str(&index.to_string());
                output.push(']');
            }
        }
    }

    output
}

/// True if `key` is `[A-Za-z_][A-Za-z0-9_]*` and can be emitted as `.key` rather than `["key"]`.
fn is_simple_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|char| char == '_' || char.is_ascii_alphanumeric())
}

/// Byte offset of the next UTF-8 char boundary after `cursor`, clamped to `text.len()`.
pub(crate) fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }

    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| cursor + offset)
        .unwrap_or(text.len())
}

/// Convert a byte cursor offset into `(line, col)` (both 0-based, column counted in chars).
///
/// # Example
/// `text = "ab\ncd"`, `cursor = 4` (the `d`) → `(1, 1)`.
pub(crate) fn line_col_for_cursor(text: &str, cursor: usize) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;

    for (index, char) in text.char_indices() {
        if index >= cursor {
            break;
        }
        if char == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }

    (line, col)
}

/// Inverse of [`line_col_for_cursor`]. Clamps to line end / text end if out of range.
///
/// # Example
/// `text = "ab\ncd"`, `target_line = 1`, `target_col = 1` → `4` (byte offset of `d`).
pub(crate) fn cursor_for_line_col(text: &str, target_line: usize, target_col: usize) -> usize {
    let mut line = 0;
    let mut col = 0;

    for (index, char) in text.char_indices() {
        if line == target_line && col == target_col {
            return index;
        }
        if char == '\n' {
            if line == target_line {
                return index;
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }

    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn formats_paths_for_simple_and_escaped_keys() {
        let path = vec![
            PathSegment::Key("user".to_string()),
            PathSegment::Key("display name".to_string()),
            PathSegment::Index(2),
        ];

        assert_eq!(format_path(&path), "$.user[\"display name\"][2]");
    }

    #[test]
    fn flattens_objects_and_arrays_into_navigable_rows() {
        let value = json!({"name": "Ada", "scores": [1, 2]});
        let mut rows = Vec::new();
        flatten_json(&value, &mut rows);

        let labels = rows.iter().map(|row| row.path_label()).collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec!["$", "$.name", "$.scores", "$.scores[0]", "$.scores[1]"]
        );
    }

    #[test]
    fn mutates_values_by_path() {
        let mut value = json!({"items": [{"name": "old"}]});
        let path = vec![
            PathSegment::Key("items".to_string()),
            PathSegment::Index(0),
            PathSegment::Key("name".to_string()),
        ];

        *get_value_at_path_mut(&mut value, &path).unwrap() = json!("new");

        assert_eq!(value, json!({"items": [{"name": "new"}]}));
    }

    #[test]
    fn matches_rows_by_path_type_name_and_preview() {
        let value = json!({"items": [{"name": "Ada"}]});
        let mut rows = Vec::new();
        flatten_json(&value, &mut rows);

        let name = rows
            .iter()
            .find(|row| row.path_label() == "$.items[0].name")
            .unwrap();
        let items = rows
            .iter()
            .find(|row| row.path_label() == "$.items")
            .unwrap();

        assert!(row_matches_query(name, "ada"));
        assert!(row_matches_query(name, "$.ITEMS[0].NAME"));
        assert!(row_matches_query(name, "string"));
        assert!(row_matches_query(items, "1 items"));
        assert!(!row_matches_query(name, "missing"));
    }

    #[test]
    fn renders_object_keys_in_original_order() {
        let value = serde_json::from_str::<Value>(r#"{"z":1,"a":{"b":2,"a":3}}"#).unwrap();
        let mut rows = Vec::new();
        flatten_json(&value, &mut rows);

        let labels = rows.iter().map(|row| row.path_label()).collect::<Vec<_>>();
        assert_eq!(labels, vec!["$", "$.z", "$.a", "$.a.b", "$.a.a"]);

        let pretty = pretty_json_lines(&value)
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            pretty,
            "{\n  \"z\": 1,\n  \"a\": {\n    \"b\": 2,\n    \"a\": 3\n  }\n}"
        );
        assert!(serde_json::from_str::<Value>(&pretty).is_ok());
    }
}
