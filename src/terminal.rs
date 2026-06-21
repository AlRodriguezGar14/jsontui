use std::{
    error::Error,
    io::{self, Stdout, Write},
    process::{Command, Stdio},
    time::Duration,
};

use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::app::App;

/// Switch the terminal into raw + alt-screen mode with bracketed paste enabled.
/// Pair with [`restore_terminal`] on exit.
pub(crate) fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>, Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Undo [`setup_terminal`]: leave alt-screen, disable raw mode, show the cursor.
pub(crate) fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<(), Box<dyn Error>> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

/// Main event loop: draw, poll for input (200ms), feed events into `app`, drain clipboard.
/// Returns when `app.should_quit()`.
pub(crate) fn run_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        terminal.draw(|frame| app.draw(frame))?;

        if app.should_quit() {
            break;
        }

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Paste(text) => app.handle_paste(&text),
                _ => {}
            }
        }

        if let Some(text) = app.take_clipboard() {
            match copy_to_clipboard(terminal.backend_mut(), &text) {
                Ok(method) => app.report_clipboard_success(method),
                Err(error) => app.report_clipboard_error(error),
            }
        }
    }

    Ok(())
}

/// Try the OS clipboard tool; on failure fall back to OSC 52 (works over SSH).
/// Returns the name of the backend used on success.
fn copy_to_clipboard(writer: &mut impl Write, text: &str) -> Result<&'static str, String> {
    if let Ok(method) = write_system_clipboard(text) {
        return Ok(method);
    }

    write_clipboard_osc52(writer, text)
        .map(|()| "OSC 52")
        .map_err(|error| format!("system clipboard unavailable and OSC 52 failed: {error}"))
}

/// macOS: pipe `text` into `pbcopy`.
#[cfg(target_os = "macos")]
fn write_system_clipboard(text: &str) -> io::Result<&'static str> {
    write_command_clipboard("pbcopy", &[], text)?;
    Ok("pbcopy")
}

/// Windows: pipe `text` into `clip`.
#[cfg(target_os = "windows")]
fn write_system_clipboard(text: &str) -> io::Result<&'static str> {
    write_command_clipboard("clip", &[], text)?;
    Ok("clip")
}

/// Linux/BSD: try Wayland (`wl-copy`), then X11 (`xclip`, `xsel`).
#[cfg(all(unix, not(target_os = "macos")))]
fn write_system_clipboard(text: &str) -> io::Result<&'static str> {
    if write_command_clipboard("wl-copy", &[], text).is_ok() {
        return Ok("wl-copy");
    }
    if write_command_clipboard("xclip", &["-selection", "clipboard"], text).is_ok() {
        return Ok("xclip");
    }
    if write_command_clipboard("xsel", &["--clipboard", "--input"], text).is_ok() {
        return Ok("xsel");
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no system clipboard command found",
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", unix)))]
fn write_system_clipboard(_text: &str) -> io::Result<&'static str> {
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no system clipboard command found",
    ))
}

/// Spawn `command args` with `text` piped to stdin. Returns an error if the process fails.
fn write_command_clipboard(command: &str, args: &[&str], text: &str) -> io::Result<()> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "clipboard stdin unavailable"))?;
    stdin.write_all(text.as_bytes())?;
    drop(stdin);

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{command} exited with status {status}"
        )))
    }
}

/// Emit an OSC 52 escape sequence asking the terminal emulator to put `text` on the clipboard.
/// Last-resort fallback when no native clipboard helper is available.
fn write_clipboard_osc52(writer: &mut impl Write, text: &str) -> io::Result<()> {
    write!(writer, "\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))?;
    writer.flush()
}

/// Standard base64 encoder (`A-Za-z0-9+/`, `=` padded). Used by [`write_clipboard_osc52`].
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);

        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);

        if chunk.len() > 1 {
            output.push(TABLE[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }

        if chunk.len() > 2 {
            output.push(TABLE[(third & 0b0011_1111) as usize] as char);
        } else {
            output.push('=');
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_clipboard_payload_as_base64() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"{\"a\":1}"), "eyJhIjoxfQ==");
    }

    #[test]
    fn writes_osc52_clipboard_sequence() {
        let mut output = Vec::new();

        write_clipboard_osc52(&mut output, "foo").unwrap();

        assert_eq!(output, b"\x1b]52;c;Zm9v\x07");
    }
}
