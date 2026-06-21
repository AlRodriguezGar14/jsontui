mod app;
mod json;
mod terminal;

use std::error::Error;

use app::App;
use terminal::{restore_terminal, run_app, setup_terminal};

/// Entry point. Sets up the terminal, runs the event loop, restores the terminal
/// even on error, then prints any error from the loop to stderr.
fn main() -> Result<(), Box<dyn Error>> {
    let mut terminal = setup_terminal()?;
    let mut app = App::new();
    let result = run_app(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;

    if let Err(error) = result {
        eprintln!("{error}");
    }

    Ok(())
}
