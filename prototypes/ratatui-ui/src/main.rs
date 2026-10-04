mod app;
mod terminal;
mod ui;

use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{Terminal, backend::CrosstermBackend};

use app::App;

fn main() -> io::Result<()> {
    let mut app = App::default();
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--offline" => app.live = false,
            "--uniform-sparklines" => app.colored_bars = false,
            "--help" | "-h" => {
                println!(
                    "MyPowers visual prototype — fake data only\n\ncargo run -- [--offline] [--uniform-sparklines]\n\na / d / l: toggle mock outputs; q / Esc / Ctrl-C: quit\nClick an output to toggle its mock state."
                );
                return Ok(());
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Unknown argument: {argument}"),
                ));
            }
        }
    }
    let _session = terminal::Session::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut next_frame = Instant::now();
    let mut next_sample = next_frame + Duration::from_secs(1);
    loop {
        let now = Instant::now();
        if now >= next_sample {
            if app.live {
                app.sample();
            }
            next_sample = now + Duration::from_secs(1);
        }
        if now >= next_frame {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            next_frame = now + Duration::from_millis(125);
        }
        if event::poll(next_frame.saturating_duration_since(Instant::now()))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.key(key) {
                        break;
                    }
                }
                Event::Mouse(mouse) => app.mouse(mouse),
                Event::Resize(_, _) => {
                    app.hovered = None;
                    app.controls = [ratatui::layout::Rect::default(); 3];
                    next_frame = Instant::now();
                }
                _ => {}
            }
        }
    }
    Ok(())
}
