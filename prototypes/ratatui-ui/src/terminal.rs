use std::io;

use crossterm::{
    cursor::{Hide, Show},
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    style::ResetColor,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

pub struct Session;

impl Session {
    pub fn enter() -> io::Result<Self> {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous_hook(info);
        }));
        enable_raw_mode()?;
        let session = Self;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture, Hide)?;
        Ok(session)
    }
}

fn restore() {
    // Try each step independently so one failed write cannot prevent raw-mode cleanup.
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), DisableMouseCapture);
    let _ = execute!(io::stdout(), ResetColor, LeaveAlternateScreen, Show);
}

impl Drop for Session {
    fn drop(&mut self) {
        restore();
    }
}
