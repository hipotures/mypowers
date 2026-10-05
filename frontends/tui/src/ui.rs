use crate::{
    app::{App, View},
    model::safe,
    network::ClipboardTarget,
};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Sparkline, SparklineBar, Widget,
    },
};
use std::time::Duration;

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 18;
pub const DASHBOARD_WIDTH: u16 = 94;
pub const DASHBOARD_HEIGHT: u16 = 28;
const BACKGROUND: Color = Color::Rgb(16, 21, 27);
const TEXT: Color = Color::Rgb(224, 232, 236);
const MUTED: Color = Color::Rgb(119, 144, 153);
const BORDER: Color = Color::Rgb(68, 94, 105);
const DIM: Color = Color::Rgb(79, 93, 102);
const TRACK: Color = Color::Rgb(34, 46, 54);
const GREEN: Color = Color::Rgb(118, 203, 137);
const YELLOW: Color = Color::Rgb(220, 199, 111);
const ORANGE: Color = Color::Rgb(227, 151, 91);
const RED: Color = Color::Rgb(229, 101, 111);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let screen = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKGROUND).fg(TEXT)),
        screen,
    );
    app.controls = [Rect::default(); 3];
    app.title = Rect::default();
    app.quit_buttons = [Rect::default(); 2];
    app.logs.clear_hitboxes();
    if screen.width < MIN_WIDTH || screen.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small\nNeed at least 60x18")
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED)),
            centered(screen, screen.width, 2),
        );
        if app.no_color {
            for cell in &mut frame.buffer_mut().content {
                cell.set_fg(Color::Reset).set_bg(Color::Reset);
            }
        }
        return;
    }
    let area = if app.view != View::Help {
        centered(
            screen,
            screen.width.min(DASHBOARD_WIDTH),
            screen.height.min(DASHBOARD_HEIGHT),
        )
    } else {
        centered(screen, screen.width.min(120), screen.height.min(40))
    };
    app.title = Rect::new(area.x + (area.width - 10) / 2 + 1, area.y, 8, 1);
    let mut outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title_top(
            Line::from(" MYPOWERS ")
                .style(Style::default().fg(TEXT).add_modifier(Modifier::BOLD))
                .centered(),
        )
        .title_bottom(
            Line::from(if app.view == View::Dashboard {
                " a AC  d DC  l lamps  F3 logs  ? help  q quit "
            } else {
                " F2 dashboard  F3 logs  ? help  q quit "
            })
            .style(Style::default().fg(MUTED))
            .centered(),
        );
    if let Some((success, time, ClipboardTarget::Snapshot)) = app.clipboard_notice
        && time.elapsed() < Duration::from_secs(3)
    {
        outer = outer.title_top(
            Line::from(if success {
                " JSON copied "
            } else {
                " Copy failed "
            })
            .style(Style::default().fg(if success { GREEN } else { RED }))
            .right_aligned(),
        );
    }
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let content = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    match app.view {
        View::Dashboard => dashboard(frame, content, app),
        View::Logs => {
            dashboard(frame, content, app);
            dim_background(frame);
            app.controls = [Rect::default(); 3];
            app.title = Rect::default();
            let modal = centered(
                area,
                area.width.saturating_sub(4),
                area.height.saturating_sub(4),
            );
            frame.render_widget(Clear, modal);
            app.logs.title = Rect::new(modal.x + (modal.width - 6) / 2 + 1, modal.y, 4, 1);
            let mut block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(BORDER))
                .style(Style::default().bg(BACKGROUND).fg(TEXT))
                .title_top(
                    Line::from(" LOGS ")
                        .centered()
                        .style(Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
                )
                .title_bottom(
                    Line::from(" Esc close  End today/live ")
                        .centered()
                        .style(Style::default().fg(MUTED)),
                );
            if let Some((success, time, ClipboardTarget::Logs)) = app.clipboard_notice
                && time.elapsed() < Duration::from_secs(3)
            {
                block = block.title_top(
                    Line::from(if success {
                        " Logs copied "
                    } else {
                        " Copy failed "
                    })
                    .style(Style::default().fg(if success { GREEN } else { RED }))
                    .right_aligned(),
                );
            }
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            logs(frame, inner, app);
        }
        View::Help => help(frame, content),
        View::Quit => {
            dashboard(frame, content, app);
            dim_background(frame);
            app.controls = [Rect::default(); 3];
            app.title = Rect::default();
            quit_modal(frame, screen, app);
        }
    }
    if app.no_color {
        for cell in &mut frame.buffer_mut().content {
            cell.set_fg(Color::Reset).set_bg(Color::Reset);
        }
    }
}

fn dim_background(frame: &mut Frame) {
    for cell in &mut frame.buffer_mut().content {
        cell.set_fg(DIM).set_bg(BACKGROUND);
        cell.set_style(Style::default().remove_modifier(Modifier::BOLD));
    }
}

fn dashboard(frame: &mut Frame, content: Rect, app: &mut App) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(4),
        Constraint::Fill(1),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(3),
    ])
    .split(content);
    let header = Layout::horizontal([Constraint::Fill(1), Constraint::Length(21)]).split(rows[0]);
    let station = app
        .status
        .as_ref()
        .and_then(|s| s.device["name"].as_str())
        .map(safe)
        .unwrap_or_else(|| "Waiting for station".into());
    frame.render_widget(
        Paragraph::new(station).style(Style::default().fg(MUTED)),
        header[0],
    );
    let (label, color) = if !app.connected {
        ("DAEMON OFFLINE", RED)
    } else if app.live() {
        ("LIVE", GREEN)
    } else if app
        .status
        .as_ref()
        .is_some_and(|s| s.telemetry.sample.is_some())
    {
        ("LAST KNOWN", YELLOW)
    } else {
        ("NO TELEMETRY", RED)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{label}  "), Style::default().fg(color)),
            Span::styled("●", Style::default().fg(color).add_modifier(Modifier::BOLD)),
        ]))
        .alignment(Alignment::Right),
        header[1],
    );
    let sample = app
        .status
        .as_ref()
        .and_then(|s| s.telemetry.sample.as_ref());
    frame.render_widget(
        Paragraph::new(
            sample
                .map(|s| format!("{}%", s.battery_percent))
                .unwrap_or_else(|| "--%".into()),
        )
        .alignment(Alignment::Center)
        .style(Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
        rows[2],
    );
    frame.render_widget(
        Battery {
            percent: sample.map_or(0, |s| s.battery_percent),
            no_color: app.no_color,
        },
        centered(rows[3], (content.width * 3 / 4).min(54), 1),
    );
    frame.render_widget(
        Paragraph::new(
            sample
                .map(|s| {
                    format!(
                        "{}h {:02}m",
                        s.remaining_minutes / 60,
                        s.remaining_minutes % 60
                    )
                })
                .unwrap_or_else(|| "--h --m".into()),
        )
        .alignment(Alignment::Center)
        .style(Style::default().fg(MUTED)),
        rows[4],
    );
    let power = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(4),
        Constraint::Fill(1),
    ])
    .split(rows[6]);
    for (rect, label, value, output, maximum) in [
        (
            power[0],
            "INPUT",
            sample.map(|s| s.input_power_w),
            false,
            100,
        ),
        (
            power[2],
            "OUTPUT",
            sample.map(|s| s.output_power_w),
            true,
            300,
        ),
    ] {
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .split(rect);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{label} "), Style::default().fg(MUTED)),
                Span::styled(
                    value.map(|v| v.to_string()).unwrap_or_else(|| "--".into()),
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" W", Style::default().fg(MUTED)),
            ]))
            .alignment(Alignment::Center),
            parts[0],
        );
        let data: Vec<_> = app
            .graph_data(parts[2].width, output)
            .into_iter()
            .map(|value| {
                SparklineBar::from(value).style(Style::default().fg(if app.live() {
                    load_color(value, maximum)
                } else {
                    DIM
                }))
            })
            .collect();
        frame.render_widget(Sparkline::default().data(data).max(maximum), parts[2]);
    }
    let controls = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(rows[8]);
    for (index, label) in ["AC", "DC", "LAMPS"].iter().enumerate() {
        let rect = controls[index];
        app.controls[index] = rect;
        let active = app.hovered == Some(index) || app.selected == Some(index);
        let enabled = sample.map(|s| [s.ac_enabled, s.dc_enabled, s.light_enabled][index]);
        let pending_label = app
            .pending
            .as_ref()
            .filter(|pending| pending.starts_with(["AC", "DC", "LIGHT"][index]));
        let title = pending_label.map(String::as_str).unwrap_or(label);
        let state = enabled
            .map(|on| if on { "ON" } else { "OFF" })
            .unwrap_or("--");
        let color = if enabled == Some(true) && app.live() {
            GREEN
        } else {
            DIM
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(title).style(Style::default().fg(if active { TEXT } else { MUTED })),
                Line::from(state).style(Style::default().fg(color).add_modifier(
                    if enabled == Some(true) {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    },
                )),
            ])
            .style(Style::default().bg(if active { TRACK } else { BACKGROUND }))
            .alignment(Alignment::Center),
            rect,
        );
    }
    let history = app
        .status
        .as_ref()
        .and_then(|s| s.history["state"].as_str())
        .unwrap_or("unknown");
    let adapter = app
        .status
        .as_ref()
        .and_then(|s| s.connection.adapter_id.as_deref())
        .unwrap_or("--");
    let debug = app
        .status
        .as_ref()
        .and_then(|s| s.logging["effective_level"].as_str())
        .unwrap_or("--");
    let age = app
        .age()
        .map(|age| format!("{age:.1}s"))
        .unwrap_or_else(|| "--".into());
    let details = format!(
        "Age {age}  History {}  {}  Log {}",
        safe(history),
        safe(adapter),
        safe(debug)
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(details).style(Style::default().fg(DIM)),
            Line::from(safe(app.display_notice())).style(Style::default().fg(if app.live() {
                MUTED
            } else {
                YELLOW
            })),
        ])
        .wrap(ratatui::widgets::Wrap { trim: false }),
        rows[10],
    );
}

fn logs(frame: &mut Frame, area: Rect, app: &mut App) {
    let parts = Layout::vertical([
        Constraint::Length(4),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(area);
    let header = Layout::horizontal([
        Constraint::Length(8),
        Constraint::Length(14),
        Constraint::Length(8),
        Constraint::Fill(1),
        Constraint::Length(11),
    ])
    .split(Rect {
        height: 1,
        ..parts[0]
    });
    app.logs.buttons = [header[0], header[2], header[4]];
    for (rect, text) in [
        (header[0], "[ prev ]"),
        (header[2], "[ next ]"),
        (header[4], "[ refresh ]"),
    ] {
        frame.render_widget(Paragraph::new(text).style(Style::default().fg(MUTED)), rect);
    }
    frame.render_widget(
        Paragraph::new(app.logs.day.to_string())
            .alignment(Alignment::Center)
            .style(Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
        header[1],
    );
    let mode = if app.logs.follow { "LIVE" } else { "ARCHIVE" };
    frame.render_widget(
        Paragraph::new(format!(
            "{mode} | {} | >= {} | page {}",
            app.logs.zone_name(),
            crate::logs::LEVELS[app.logs.level],
            app.logs.page_size
        ))
        .style(Style::default().fg(if app.logs.follow { GREEN } else { MUTED })),
        Rect {
            y: parts[0].y + 1,
            height: 1,
            ..parts[0]
        },
    );
    frame.render_widget(
        Paragraph::new("←/→ day  r refresh  f filter  +/- page  b DEBUG")
            .style(Style::default().fg(DIM)),
        Rect {
            y: parts[0].y + 2,
            height: 1,
            ..parts[0]
        },
    );
    let viewport = parts[1].height as usize;
    let logging = app.status.as_ref().map(|status| &status.logging);
    let effective = logging
        .and_then(|value| value["effective_level"].as_str())
        .unwrap_or("--");
    let expiry = logging
        .and_then(|value| value["override_expires_at"].as_str())
        .unwrap_or("none");
    frame.render_widget(
        Paragraph::new(format!(
            "Log {} | override until {}",
            safe(effective),
            safe(expiry)
        ))
        .style(Style::default().fg(DIM)),
        Rect {
            y: parts[0].y + 3,
            height: 1,
            ..parts[0]
        },
    );
    app.logs.layout(viewport);
    let max_scroll = app.logs.max_offset();
    let start = app.logs.offset;
    let lines: Vec<_> = app
        .logs
        .records
        .iter()
        .skip(start)
        .take(viewport)
        .map(|log| {
            let timestamp = log["timestamp"]
                .as_str()
                .or(log["time"].as_str())
                .unwrap_or("");
            let time = chrono::DateTime::parse_from_rfc3339(timestamp)
                .map(|time| {
                    if let Some(zone) = app.timezone {
                        time.with_timezone(&zone).format("%H:%M:%S").to_string()
                    } else {
                        time.with_timezone(&chrono::Local)
                            .format("%H:%M:%S")
                            .to_string()
                    }
                })
                .unwrap_or_else(|_| "--:--:--".into());
            let level = log["level"].as_str().unwrap_or("--");
            let color = match level {
                "ERROR" => RED,
                "WARNING" => YELLOW,
                _ => MUTED,
            };
            Line::from(vec![
                Span::styled(format!("{time} "), Style::default().fg(DIM)),
                Span::styled(format!("{:<7} ", safe(level)), Style::default().fg(color)),
                Span::raw(safe(log["message"].as_str().unwrap_or(""))),
            ])
        })
        .collect();
    let mut text_area = parts[1];
    if max_scroll > 0 {
        text_area.width = text_area.width.saturating_sub(2);
    }
    frame.render_widget(
        Paragraph::new(if lines.is_empty() {
            vec![
                Line::from(if app.logs.loading {
                    "Loading logs..."
                } else {
                    "No records for this day and filter."
                })
                .style(Style::default().fg(DIM)),
            ]
        } else {
            lines
        }),
        text_area,
    );
    if max_scroll > 0 {
        let mut state = ScrollbarState::new(max_scroll + 1)
            .position(start)
            .viewport_content_length(viewport);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(Style::default().fg(TRACK))
                .thumb_symbol("█")
                .thumb_style(Style::default().fg(MUTED)),
            parts[1],
            &mut state,
        );
        let x = parts[1].right() - 1;
        app.logs.scrollbar = Rect::new(x, parts[1].y, 1, parts[1].height);
        let thumb_rows: Vec<_> = (parts[1].y..parts[1].bottom())
            .filter(|&y| frame.buffer_mut()[(x, y)].symbol() == "█")
            .collect();
        if let (Some(&first), Some(&last)) = (thumb_rows.first(), thumb_rows.last()) {
            app.logs.thumb = Rect::new(x, first, 1, last - first + 1);
        }
    }
    frame.render_widget(
        Paragraph::new(safe(&if app.pending.is_some() {
            app.notice.clone()
        } else if let Some((message, time)) = &app.logs.action
            && time.elapsed() < Duration::from_secs(3)
        {
            message.clone()
        } else {
            format!(
                "{} | {} loaded{}",
                app.logs.message,
                app.logs.records.len(),
                if app.logs.unseen > 0 {
                    format!(" | {} new", app.logs.unseen)
                } else {
                    String::new()
                }
            )
        }))
        .style(Style::default().fg(MUTED)),
        parts[2],
    );
}

fn quit_modal(frame: &mut Frame, screen: Rect, app: &mut App) {
    let area = centered(screen, 50, 8);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(BACKGROUND))
        .title_top(
            Line::from(" QUIT ")
                .centered()
                .style(Style::default().fg(TEXT)),
        )
        .title_bottom(
            Line::from(" Enter select  Esc cancel ")
                .centered()
                .style(Style::default().fg(MUTED)),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new("Quit MyPowers?\nThe daemon will keep running.")
            .alignment(Alignment::Center)
            .style(Style::default().fg(TEXT)),
        rows[1],
    );
    let buttons = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(9),
        Constraint::Length(4),
        Constraint::Length(12),
        Constraint::Fill(1),
    ])
    .split(rows[3]);
    app.quit_buttons = [buttons[1], buttons[3]];
    for (index, label) in ["[ Yes ]", "[ Cancel ]"].into_iter().enumerate() {
        frame.render_widget(
            Paragraph::new(label).alignment(Alignment::Center).style(
                Style::default()
                    .fg(if app.quit_yes == (index == 0) {
                        TEXT
                    } else {
                        DIM
                    })
                    .add_modifier(if app.quit_yes == (index == 0) {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            app.quit_buttons[index],
        );
    }
}

fn help(frame: &mut Frame, area: Rect) {
    frame.render_widget(Paragraph::new("HELP\n\na / d / l     AC / DC / lamps: request desired ON/OFF\nTab / Shift-Tab  Focus control; Enter / Space activate\nF2 dashboard  F3 logs modal  F1 / ? help\nLOGS: Up/Down, PageUp/PageDown, wheel or drag scrollbar\nLeft/Right or [/] previous/next day\nr / F5 refresh archive; f filter; +/- page size\nHome day start; End today/live bottom\nb runtime DEBUG; p pause/resume; r retry on dashboard\nEsc close modal; q confirm quit; Ctrl-Q quit immediately\nDouble-click MYPOWERS to copy the current API snapshot.\nDouble-click LOGS to copy all loaded log records.\n\nObserved states change only when confirmed by telemetry.\nLast-known readings remain visible during outages.\nPending and stale data disable output controls.\nUncertain commands are never automatically replayed.\nClosing this client leaves daemon and outputs running.").style(Style::default().fg(MUTED)), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let column = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(area)[0];
    Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(column)[0]
}

fn load_color(value: u64, maximum: u64) -> Color {
    match value * 100 / maximum {
        0..=44 => GREEN,
        45..=74 => YELLOW,
        _ => ORANGE,
    }
}
struct Battery {
    percent: u16,
    no_color: bool,
}

impl Widget for Battery {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        // The final filled cell has half-cell precision; all output goes through Ratatui.
        let half_cells = (u32::from(area.width) * 2 * u32::from(self.percent.min(100)) + 50) / 100;
        for offset in 0..area.width {
            let remaining = half_cells.saturating_sub(u32::from(offset) * 2);
            let color =
                battery_color(f64::from(offset) / f64::from(area.width.saturating_sub(1).max(1)));
            let cell = &mut buffer[(area.x + offset, area.y)];
            if self.no_color {
                cell.set_symbol(match remaining {
                    0 => "░",
                    1 => "▌",
                    _ => "█",
                });
                continue;
            }
            match remaining {
                0 => {
                    cell.set_symbol(" ").set_fg(TRACK).set_bg(TRACK);
                }
                1 => {
                    cell.set_symbol("▌").set_fg(color).set_bg(TRACK);
                }
                _ => {
                    cell.set_symbol(" ").set_fg(color).set_bg(color);
                }
            }
        }
    }
}

fn battery_color(position: f64) -> Color {
    let stops = [
        (229.0, 101.0, 111.0),
        (227.0, 151.0, 91.0),
        (220.0, 199.0, 111.0),
        (169.0, 209.0, 106.0),
        (118.0, 203.0, 137.0),
    ];
    let scaled = position.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
    let index = (scaled.floor() as usize).min(stops.len() - 2);
    let blend = scaled - index as f64;
    let (a, b) = (stops[index], stops[index + 1]);
    let lerp = |x: f64, y: f64| (x + (y - x) * blend).round() as u8;
    Color::Rgb(lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}
