# jsontui

`jsontui` is a terminal UI for pasting, formatting, navigating, editing, searching,
and copying JSON.

It keeps object key order intact, provides synchronized tree context for large
documents, and supports quick clipboard workflows from inside the terminal.

## Run It

```bash
cargo run
```

Paste JSON into the source buffer. The app parses and beautifies valid JSON
automatically on paste, or you can use the source-mode shortcuts below.

## Key Commands

Global:

- `Ctrl+C`: quit
- `Ctrl+N`: start a new JSON buffer
- `?`: open help

Source mode:

- `Ctrl+P`: parse JSON
- `Ctrl+B`: parse and beautify
- `Ctrl+M`: parse and compact
- `Esc`: return to the navigator after JSON has been parsed

Navigate mode:

- `j` / `k`: move through JSON rows
- `h` / `l`: move to parent / first child
- `g` / `G`: jump to top / bottom
- `b` / `m` / `r`: switch to beautified, compact, or raw view
- `Tab` / `Shift+Tab`: cycle views
- `/`: search rows
- `n` / `N`: next / previous search match
- `e` or `Enter`: edit the selected value as JSON
- `K`: rename the selected object key
- `yy`: copy the current view
- `Y` or `yv`: copy the selected value
- `yk`: copy the selected `"key": value` pair

## Features

- Beautify, compact, and raw JSON views
- Synchronized outline for navigating nested objects and arrays
- Case-insensitive search across names, paths, types, and previews
- In-place value editing with JSON validation
- Object key renaming without changing key order
- Clipboard support through native tools with OSC 52 fallback

## Development Checks

```bash
cargo fmt --check
cargo test
cargo clippy -- -D warnings
```
