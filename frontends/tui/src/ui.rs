use crate::{
    app::{App, View},
    feedback::{Feedback, Severity},
    model::safe,
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

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 19;
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
            Paragraph::new("Terminal too small\nNeed at least 60x19")
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
    let surface = if app.view != View::Help {
        centered(
            screen,
            screen.width.min(DASHBOARD_WIDTH),
            screen.height.min(DASHBOARD_HEIGHT + 1),
        )
    } else {
        centered(screen, screen.width.min(120), screen.height.min(41))
    };
    let area = Rect {
        height: surface.height - 1,
        ..surface
    };
    let status_area = Rect::new(surface.x + 1, area.bottom(), surface.width - 2, 1);
    if app.view == View::Dashboard {
        app.title = Rect::new(area.x + (area.width - 10) / 2 + 1, area.y, 8, 1);
    }
    let footer = outer_footer(app.view, area.width);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title_top(
            Line::from(" MYPOWERS ")
                .style(Style::default().fg(TEXT).add_modifier(Modifier::BOLD))
                .centered(),
        )
        .title_bottom(
            Line::from(footer)
                .style(Style::default().fg(MUTED))
                .centered(),
        );
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
            let block = Block::default()
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
                    Line::from(logs_footer(modal.width))
                        .centered()
                        .style(Style::default().fg(MUTED)),
                );
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            logs(frame, inner, app);
        }
        View::Help => help(frame, content, app.help_context),
        View::Quit => {
            dashboard(frame, content, app);
            dim_background(frame);
            app.controls = [Rect::default(); 3];
            app.title = Rect::default();
            quit_modal(frame, screen, app);
        }
    }
    if matches!(app.view, View::Logs | View::Quit) {
        // The footer belongs to the active context, so it stays readable over a dimmed dashboard.
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(MUTED).bg(BACKGROUND)),
            centered(
                Rect::new(area.x, area.bottom() - 1, area.width, 1),
                Span::raw(footer).width() as u16,
                1,
            ),
        );
    }
    status_line(
        frame,
        status_area,
        app.feedback.as_ref(),
        context_status(app),
    );
    if app.no_color {
        for cell in &mut frame.buffer_mut().content {
            cell.set_fg(Color::Reset).set_bg(Color::Reset);
        }
    }
}

fn outer_footer(view: View, width: u16) -> &'static str {
    match view {
        View::Dashboard if width >= 80 => {
            " a AC  d DC  l lamps  F3 logs  r retry  p pause  ? help  q quit "
        }
        View::Dashboard => " a/d/l outputs  F3 logs  r retry  p pause  ? help  q quit ",
        View::Logs => " Esc close  ? help  q quit  Ctrl-Q quit now ",
        View::Help => " Esc close  q quit  Ctrl-Q quit now ",
        View::Quit => " Ctrl-Q quit now ",
    }
}

fn logs_footer(width: u16) -> &'static str {
    if width >= 80 {
        " ←/→ day  ↑/↓ scroll  f filter  +/- page  b DEBUG  Home start  End today/live "
    } else if width >= 64 {
        " ←/→ day  f filter  +/- page  b DEBUG  Home start  End live "
    } else {
        " ←/→ day  f filter  +/- page  b DEBUG  End live "
    }
}

fn context_status(app: &App) -> Line<'static> {
    if app.view != View::Logs {
        return Line::default();
    }
    let logging = app.status.as_ref().map(|status| &status.logging);
    let level = logging
        .and_then(|logging| logging["effective_level"].as_str())
        .filter(|level| crate::logs::LEVELS.contains(level))
        .unwrap_or("--");
    let mut text = format!("Log: {level}");
    if let Some(expiry) = logging
        .and_then(|logging| logging["override_expires_at"].as_str())
        .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
        .filter(|expiry| *expiry > chrono::Utc::now())
    {
        let expiry = match app.timezone {
            Some(zone) => expiry
                .with_timezone(&zone)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            None => expiry
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
        };
        text.push_str(&format!(" | until {expiry}"));
    }
    Line::from(text).style(Style::default().fg(MUTED))
}

pub(crate) fn status_line(
    frame: &mut Frame,
    area: Rect,
    feedback: Option<&Feedback>,
    indicators: Line<'_>,
) {
    let right_width = indicators.width().min(usize::from(area.width)) as u16;
    let left_width = area
        .width
        .saturating_sub(right_width.saturating_add(u16::from(right_width > 0)));
    if let Some(feedback) = feedback
        && let Some(stage) = feedback.stage()
    {
        let color = match feedback.severity {
            Severity::Success => GREEN,
            Severity::Info => MUTED,
            Severity::Warning => YELLOW,
            Severity::Error => RED,
        };
        let Color::Rgb(r, g, b) = color else {
            unreachable!()
        };
        let intensity = [1.0, 0.8, 0.45, 0.2][usize::from(stage)];
        let fade = |channel: u8, background: u8| {
            (f64::from(background) + (f64::from(channel) - f64::from(background)) * intensity)
                .round() as u8
        };
        let style = Style::default()
            .fg(Color::Rgb(fade(r, 16), fade(g, 21), fade(b, 27)))
            .add_modifier(match stage {
                0 => Modifier::BOLD,
                2 | 3 => Modifier::DIM,
                _ => Modifier::empty(),
            });
        frame.render_widget(
            Paragraph::new(ellipsize(&feedback.message, left_width)).style(style),
            Rect {
                width: left_width,
                ..area
            },
        );
    }
    frame.render_widget(
        Paragraph::new(indicators).alignment(Alignment::Right),
        Rect::new(area.right() - right_width, area.y, right_width, 1),
    );
}

fn ellipsize(message: &str, width: u16) -> String {
    let width = usize::from(width);
    if width == 0 {
        return String::new();
    }
    if Span::raw(message).width() <= width {
        return message.to_owned();
    }
    let mut result = String::new();
    for character in message.chars() {
        let mut next = result.clone();
        next.push(character);
        if Span::raw(next.as_str()).width() >= width {
            break;
        }
        result = next;
    }
    result.push('…');
    result
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
        ("CONNECTED", GREEN)
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
        let data = app.graph_data(parts[2].width, output);
        if app.live() && value == Some(0) && !app.has_power_history(output) {
            idle_graph(frame, parts[2], app.animation_started.elapsed());
            continue;
        }
        let data: Vec<_> = data
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
}

fn idle_graph(frame: &mut Frame, area: Rect, elapsed: std::time::Duration) {
    // One cell every two seconds, reversing at either end of an eleven-cell track.
    let step = (elapsed.as_secs() / 2 % 20) as usize;
    let position = step.min(20 - step);
    let track = Line::from(
        (0..11)
            .map(|index| {
                Span::styled(
                    if index == position { "○" } else { "·" },
                    Style::default().fg(if index == position { DIM } else { TRACK }),
                )
            })
            .collect::<Vec<_>>(),
    );
    frame.render_widget(
        Paragraph::new(track).alignment(Alignment::Center),
        Rect::new(area.x, area.bottom() - 1, area.width, 1),
    );
}

fn logs(frame: &mut Frame, area: Rect, app: &mut App) {
    let parts = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(area);
    let header = Layout::horizontal([
        Constraint::Length(8),
        Constraint::Length(14),
        Constraint::Length(8),
        Constraint::Fill(1),
    ])
    .split(Rect {
        height: 1,
        ..parts[0]
    });
    app.logs.buttons = [header[0], header[2]];
    for (rect, text) in [(header[0], "[ prev ]"), (header[2], "[ next ]")] {
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
            "{mode} | {} | Filter ≥ {} | page {}",
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
    let viewport = parts[1].height as usize;
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
        Paragraph::new(safe(&format!(
            "{} | {} loaded{}",
            app.logs.message,
            app.logs.records.len(),
            if app.logs.unseen > 0 {
                format!(" | {} new", app.logs.unseen)
            } else {
                String::new()
            }
        )))
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

fn help(frame: &mut Frame, area: Rect, context: View) {
    let text = if context == View::Logs {
        "HELP — LOGS\n\nUp/Down, PageUp/PageDown   Scroll records\nMouse wheel / scrollbar   Scroll or drag\nLeft/Right or [ / ]       Previous/next day\nf                        Change minimum log level\n+ / -                    Change page size\nHome                     Beginning of selected day\nEnd                      Today: latest records and live follow\nb                        Toggle runtime DEBUG override\nDouble-click LOGS        Copy all loaded records\n\nEsc close   q confirm quit   Ctrl-Q quit immediately"
    } else {
        "HELP — DASHBOARD\n\na / d / l                Request AC / DC / lamps ON/OFF\nTab / Shift-Tab          Focus output control\nEnter / Space            Activate focused output\nF3                       Open Logs\nr                        Retry station connection\np                        Pause/resume station connection\nF1 / ?                   Help for the active window\nDouble-click MYPOWERS    Copy current API snapshot\n\nEsc close   q confirm quit   Ctrl-Q quit immediately\n\nClosing this client leaves the daemon and outputs running."
    };
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(MUTED)), area);
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
