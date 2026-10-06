mod terminal;

use crossterm::event::{self, Event as TerminalEvent, KeyEventKind};
use mypowers_tui::{
    app::{App, Effect},
    clipboard, config, feedback, model, network, ui,
};
use network::{Api, ClipboardTarget, Event};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{self, IsTerminal},
    sync::Arc,
    time::Duration,
};
use tokio::sync::mpsc;

fn main() {
    if let Err(error) = run() {
        eprintln!("{}", model::safe(&error));
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let Some(config) = config::Config::load().map_err(|e| e.to_string())? else {
        return Ok(());
    };
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(
            "TUI requires a terminal. Use mypowers status for non-interactive output.".into(),
        );
    }
    let api = Arc::new(Api::new(&config)?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|_| "Cannot start network runtime.")?;
    // Register signals before raw mode; install cleanup before spawning workers.
    let (mut term, mut interrupt) = {
        use tokio::signal::unix::{SignalKind, signal};
        let _scope = runtime.enter();
        (
            signal(SignalKind::terminate()).map_err(|_| "Cannot register termination signal.")?,
            signal(SignalKind::interrupt()).map_err(|_| "Cannot register interrupt signal.")?,
        )
    };
    let (events, mut incoming) = mpsc::channel(256);
    let (requests, operations) = mpsc::channel(8);
    let (log_requests, log_operations) = tokio::sync::watch::channel(None);
    let (history_requests, history_operations) = tokio::sync::watch::channel(None);
    let mut app = App::new(config.no_color, config.timezone);
    app.client_preferences = config.client_preferences.clone();
    app.server_url = config.server.as_str().trim_end_matches('/').to_owned();
    let session =
        terminal::Session::enter(!config.no_mouse).map_err(|_| "Cannot initialize terminal.")?;
    runtime.spawn(api.clone().stream(false, events.clone()));
    runtime.spawn(api.clone().stream(true, events.clone()));
    runtime.spawn(api.clone().log_pages(log_operations, events.clone()));
    runtime.spawn(
        api.clone()
            .history_aggregates(history_operations, events.clone()),
    );
    runtime.spawn(api.clone().load_settings(events.clone()));
    runtime.spawn(api.operations(operations, events.clone()));
    let signal_events = events.clone();
    runtime.spawn(async move {
        tokio::select! { _ = term.recv() => {}, _ = interrupt.recv() => {} }
        let _ = signal_events.send(Event::Exit).await;
    });
    let result = (|| -> io::Result<()> {
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        let mut next_frame = std::time::Instant::now();
        'ui: loop {
            for _ in 0..256 {
                match incoming.try_recv() {
                    Ok(Event::Exit) => break 'ui,
                    Ok(Event::ClipboardTerminal(text, target)) => {
                        let success =
                            clipboard::write_terminal(&mut io::stdout().lock(), &text).is_ok();
                        app.update(Event::Copied(success, target));
                    }
                    Ok(event) => {
                        if matches!(event, Event::Disconnected(_)) {
                            let _ = history_requests.send(None);
                        }
                        app.update(event);
                    }
                    Err(_) => break,
                }
            }
            if app.quit_after_settings && !app.settings_dirty() && app.pending.is_none() {
                break;
            }
            if let Effect::Request(intent) = app.autosave()
                && requests.try_send(intent).is_err()
            {
                app.update(Event::Finished(feedback::Feedback::new(
                    "Could not save settings; retrying",
                    feedback::Severity::Error,
                )));
            }
            if let Some(request) = app.history_request() {
                let _ = history_requests.send(Some(request));
            }
            if let Some(request) = app.logs.maintenance() {
                let _ = log_requests.send(Some(request));
            }
            if std::time::Instant::now() >= next_frame {
                app.prune_graph();
                terminal.draw(|frame| ui::draw(frame, &mut app))?;
                next_frame = std::time::Instant::now() + Duration::from_millis(250);
            }
            if event::poll(next_frame.saturating_duration_since(std::time::Instant::now()))? {
                let effect = match event::read()? {
                    TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => app.key(key),
                    TerminalEvent::Mouse(mouse) if !config.no_mouse => app.mouse(mouse),
                    TerminalEvent::Resize(_, _) => {
                        app.resize();
                        Effect::None
                    }
                    // Bracketed paste is data, never a sequence of control actions.
                    _ => Effect::None,
                };
                match effect {
                    Effect::Quit => break,
                    Effect::None => {}
                    Effect::Logs(request) => {
                        let _ = log_requests.send(Some(request));
                    }
                    Effect::Request(intent) => {
                        if requests.try_send(intent).is_err() {
                            app.update(Event::Finished(feedback::Feedback::new(
                                "Request unavailable; no command sent",
                                feedback::Severity::Error,
                            )));
                        }
                    }
                    Effect::Copy => {
                        if let Some(status) = &app.status {
                            let json =
                                serde_json::to_string_pretty(status).map_err(io::Error::other)?;
                            let events = events.clone();
                            runtime.spawn(clipboard::copy(json, ClipboardTarget::Snapshot, events));
                        } else {
                            app.update(Event::Copied(false, ClipboardTarget::Snapshot));
                        }
                    }
                    Effect::CopySettings => {
                        let json = serde_json::to_string_pretty(&app.settings_clipboard_json())
                            .map_err(io::Error::other)?;
                        runtime.spawn(clipboard::copy(
                            json,
                            ClipboardTarget::Settings,
                            events.clone(),
                        ));
                    }
                    Effect::CopyLogs => {
                        let text = app.logs.clipboard_text();
                        let events = events.clone();
                        runtime.spawn(clipboard::copy(text, ClipboardTarget::Logs, events));
                    }
                }
            }
        }
        Ok(())
    })();
    drop(session);
    runtime.shutdown_timeout(Duration::from_millis(200));
    result.map_err(|_| "TUI stopped after an I/O error; terminal settings restored.".into())
}
