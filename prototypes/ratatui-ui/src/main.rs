mod app;
mod terminal;
mod ui;

use std::{
    io::{self, Write},
    process::{Command, Stdio},
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
                    "MyPowers visual prototype — fake data only\n\ncargo run -- [--offline] [--uniform-sparklines]\n\na / d / l: toggle mock outputs; q / Esc / Ctrl-C: quit\nClick an output to toggle its mock state.\nDouble-click MYPOWERS to copy mock JSON (Wayland + wl-copy)."
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
                Event::Mouse(mouse) => {
                    if app.mouse(mouse) {
                        let success = copy_json_to_clipboard(&app.mock_json()).is_ok();
                        app.clipboard_notice = Some((success, Instant::now()));
                        next_frame = Instant::now();
                    }
                }
                Event::Resize(_, _) => {
                    app.resize();
                    next_frame = Instant::now();
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn copy_json_to_clipboard(json: &str) -> io::Result<()> {
    let mut child = Command::new("wl-copy")
        .args(["--type", "text/plain;charset=utf-8"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let write_result = child.stdin.take().unwrap().write_all(json.as_bytes());
    let status = child.wait()?;
    write_result?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("Clipboard copy failed."))
    }
}
