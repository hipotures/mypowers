use crate::{
    app::{App, View},
    feedback::{Feedback, Severity},
    history::{Visualization, power_scale_from},
    model::safe,
    settings::{Field, SettingsTab},
};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Axis, Block, BorderType, Chart, Clear, Dataset, GraphType, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Sparkline, SparklineBar, Widget,
    },
};
use std::borrow::Cow;

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 19;
pub const DASHBOARD_WIDTH: u16 = 94;
pub const DASHBOARD_HEIGHT: u16 = 28;
// Six numeric columns cover the largest power scale (102400), plus the Y axis.
const CHART_AXIS_WIDTH: u16 = 7;
// Semantic telemetry colors stay recognizable across themes; all surfaces and
// interactive components use the same resolved client palette.
const GREEN: Color = Color::Rgb(118, 203, 137);
const CYAN: Color = Color::Rgb(92, 181, 204);
const YELLOW: Color = Color::Rgb(220, 199, 111);
const ORANGE: Color = Color::Rgb(227, 151, 91);
#[cfg(test)]
const TRACK: Color = Color::Rgb(34, 46, 54);
#[cfg(test)]
const RED: Color = Color::Rgb(229, 101, 111);

#[derive(Clone)]
struct Palette {
    theme: ratcn::Theme,
    dim: Color,
}
impl Palette {
    fn new(preferences: &crate::client_ui::ClientPreferences) -> Self {
        let theme = preferences.theme();
        let dim = if preferences.theme == "MyPowers" && preferences.colors.is_empty() {
            Color::Rgb(79, 93, 102)
        } else {
            theme.muted_foreground
        };
        Self { theme, dim }
    }
}

fn action_button<'a>(
    label: &'a str,
    theme: &ratcn::Theme,
    focused: bool,
    disabled: bool,
) -> ratcn::ButtonWidget<'a> {
    ratcn::ButtonWidget::new(label)
        .themed(theme)
        .ghost()
        .focused(focused)
        .disabled(disabled)
}

fn panel<'a>(theme: &ratcn::Theme, title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .title_top(
            title.into().centered().style(
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ),
        )
}

fn themed_list<'a>(
    items: &'a [ratatui::text::Text<'static>],
    theme: &ratcn::Theme,
) -> ratcn::ListWidget<'a> {
    let mut style = ratcn::ListStyle::from_theme(theme);
    style.background = theme.background;
    style.focused_background = theme.background;
    style.foreground = theme.foreground;
    ratcn::ListWidget::new(items).style(style).focus_symbol("")
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let palette = Palette::new(&app.client_preferences);
    let screen = frame.area();
    frame.render_widget(
        Block::default().style(
            Style::default()
                .bg(palette.theme.background)
                .fg(palette.theme.foreground),
        ),
        screen,
    );
    app.controls = [Rect::default(); 3];
    app.title = Rect::default();
    app.quit_buttons = [Rect::default(); 2];
    app.settings_tabs = [Rect::default(); 5];
    app.settings_fields.clear();
    app.settings_segments.clear();
    app.settings_choices.clear();
    app.settings_actions = [Rect::default(); 3];
    app.logs.clear_hitboxes();
    if screen.width < MIN_WIDTH || screen.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small\nNeed at least 60x19")
                .alignment(Alignment::Center)
                .style(Style::default().fg(palette.theme.muted_foreground)),
            centered(screen, screen.width, 2),
        );
        if app.no_color {
            for cell in &mut frame.buffer_mut().content {
                cell.set_fg(Color::Reset).set_bg(Color::Reset);
            }
        }
        return;
    }
    let surface = centered(
        screen,
        screen.width.min(DASHBOARD_WIDTH),
        screen.height.min(DASHBOARD_HEIGHT + 1),
    );
    let area = Rect {
        height: surface.height - 1,
        ..surface
    };
    let status_area = Rect::new(surface.x + 1, area.bottom(), surface.width - 2, 1);
    if app.view == View::Dashboard {
        app.title = Rect::new(area.x + (area.width - 10) / 2 + 1, area.y, 8, 1);
    }
    let footer = if app.snapshot {
        String::new()
    } else {
        outer_footer(
            app.view,
            area.width,
            app.graph.resolution.label(),
            app.graph.visualization,
            app.settings_tab,
        )
    };
    let outer = panel(&palette.theme, " MYPOWERS ").title_bottom(
        Line::from(footer.as_str())
            .style(Style::default().fg(palette.theme.muted_foreground))
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
            dim_background(frame, &palette);
            app.controls = [Rect::default(); 3];
            app.title = Rect::default();
            let modal = centered(
                area,
                area.width.saturating_sub(4),
                area.height.saturating_sub(4),
            );
            frame.render_widget(Clear, modal);
            app.logs.title = Rect::new(modal.x + (modal.width - 6) / 2 + 1, modal.y, 4, 1);
            let block = panel(&palette.theme, " LOGS ").title_bottom(
                Line::from(logs_footer(modal.width))
                    .centered()
                    .style(Style::default().fg(palette.theme.muted_foreground)),
            );
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            logs(frame, inner, app);
        }
        View::Help | View::Settings => {
            dashboard(frame, content, app);
            dim_background(frame, &palette);
            app.controls = [Rect::default(); 3];
            let modal = centered(area, area.width - 4, area.height - 4);
            frame.render_widget(Clear, modal);
            let theme = app.client_preferences.theme();
            let block = panel(
                &theme,
                if app.view == View::Help {
                    " HELP "
                } else {
                    " SETTINGS "
                },
            );
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            let inner = Rect {
                x: inner.x + 1,
                width: inner.width - 2,
                ..inner
            };
            if app.view == View::Help {
                help(frame, inner, app.help_context, app.settings_tab, &theme);
            } else {
                settings(frame, inner, app);
            }
        }
        View::Quit => {
            dashboard(frame, content, app);
            dim_background(frame, &palette);
            app.controls = [Rect::default(); 3];
            app.title = Rect::default();
            quit_modal(frame, screen, app);
        }
    }
    if app.view != View::Dashboard {
        // The footer belongs to the active context, so it stays readable over a dimmed dashboard.
        frame.render_widget(
            Paragraph::new(footer.as_str()).style(
                Style::default()
                    .fg(palette.theme.muted_foreground)
                    .bg(palette.theme.background),
            ),
            centered(
                Rect::new(area.x, area.bottom() - 1, area.width, 1),
                Span::raw(footer.as_str()).width() as u16,
                1,
            ),
        );
    }
    themed_status_line(
        frame,
        status_area,
        app.feedback.as_ref(),
        context_status(app, &palette),
        app.feedback
            .as_ref()
            .map(|feedback| app.clock.feedback_elapsed(feedback.started))
            .unwrap_or_default(),
        &palette,
    );
    if app.no_color {
        for cell in &mut frame.buffer_mut().content {
            cell.set_fg(Color::Reset).set_bg(Color::Reset);
        }
    }
}

fn outer_footer(
    view: View,
    width: u16,
    interval: &str,
    visualization: Visualization,
    _tab: SettingsTab,
) -> String {
    let alternate = if visualization == Visualization::Sparkline {
        "chart"
    } else {
        "spark"
    };
    match view {
        View::Dashboard if width >= 80 => {
            format!(" l logs s settings t {interval} g {alternate} ? help q quit ")
        }
        View::Dashboard => {
            format!(" l logs s settings t {interval} g view ? q quit ")
        }
        View::Logs => " Esc close  ? help  q quit ".into(),
        View::Settings if width < 80 => {
            " Tab tabs  ↑↓ select  Enter choose  Esc close  ?  q ".into()
        }
        View::Settings => " Tab tabs  ↑↓ select  Enter choose  Esc close  ? help  q quit ".into(),
        View::Help => " Esc close  q quit ".into(),
        View::Quit => String::new(),
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

fn context_status(app: &App, palette: &Palette) -> Line<'static> {
    let mut spans = Vec::new();
    if app.view == View::Logs {
        spans.push(Span::styled(
            logging_status(app),
            Style::default().fg(palette.theme.muted_foreground),
        ));
    }
    for (count, name, color) in [
        (app.warning_count, "warn", palette.theme.warning),
        (app.error_count, "err", palette.theme.destructive),
    ] {
        if count > 0 {
            if !spans.is_empty() {
                spans.push(Span::raw(" • "));
            }
            spans.push(Span::styled(
                format!("{name}:{count}"),
                Style::default().fg(color),
            ));
        }
    }
    Line::from(spans)
}

fn logging_status(app: &App) -> String {
    let logging = app.status.as_ref().map(|status| &status.logging);
    let level = logging
        .and_then(|logging| logging["effective_level"].as_str())
        .filter(|level| crate::logs::LEVELS.contains(level))
        .unwrap_or("--");
    let mut text = format!("Log: {level}");
    if let Some(expiry) = logging
        .and_then(|logging| logging["override_expires_at"].as_str())
        .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
        .filter(|expiry| *expiry > app.clock.now())
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
    text
}

#[cfg(test)]
pub(crate) fn status_line(
    frame: &mut Frame,
    area: Rect,
    feedback: Option<&Feedback>,
    indicators: Line<'_>,
    elapsed: std::time::Duration,
) {
    themed_status_line(
        frame,
        area,
        feedback,
        indicators,
        elapsed,
        &Palette::new(&crate::client_ui::ClientPreferences::default()),
    );
}

fn themed_status_line(
    frame: &mut Frame,
    area: Rect,
    feedback: Option<&Feedback>,
    indicators: Line<'_>,
    feedback_elapsed: std::time::Duration,
    palette: &Palette,
) {
    let right_width = indicators.width().min(usize::from(area.width)) as u16;
    let left_width = area
        .width
        .saturating_sub(right_width.saturating_add(u16::from(right_width > 0)));
    if let Some(feedback) = feedback
        && let Some(stage) = feedback.stage(feedback_elapsed)
    {
        let color = match feedback.severity {
            Severity::Success => GREEN,
            Severity::Info => palette.theme.muted_foreground,
            Severity::Warning => palette.theme.warning,
            Severity::Error => palette.theme.destructive,
        };
        let intensity = [1.0, 0.8, 0.45, 0.2][usize::from(stage)];
        let faded = match (color, palette.theme.background) {
            (Color::Rgb(r, g, b), Color::Rgb(br, bg, bb)) => {
                let fade = |channel: u8, background: u8| {
                    (f64::from(background)
                        + (f64::from(channel) - f64::from(background)) * intensity)
                        .round() as u8
                };
                Color::Rgb(fade(r, br), fade(g, bg), fade(b, bb))
            }
            _ => color,
        };
        let style = Style::default().fg(faded).add_modifier(match stage {
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

fn ellipsize(message: &str, width: u16) -> Cow<'_, str> {
    let width = usize::from(width);
    if width == 0 {
        return Cow::Borrowed("");
    }
    let span = Span::raw(message);
    if span.width() <= width {
        return Cow::Borrowed(message);
    }
    let mut result = String::new();
    let mut used = 0;
    for grapheme in span.styled_graphemes(Style::default()) {
        let grapheme_width = Span::raw(grapheme.symbol).width();
        if used + grapheme_width >= width {
            break;
        }
        result.push_str(grapheme.symbol);
        used += grapheme_width;
    }
    result.push('…');
    Cow::Owned(result)
}

fn dim_background(frame: &mut Frame, palette: &Palette) {
    for cell in &mut frame.buffer_mut().content {
        cell.set_fg(palette.dim).set_bg(palette.theme.background);
        cell.set_style(Style::default().remove_modifier(Modifier::BOLD));
    }
}

fn dashboard(frame: &mut Frame, content: Rect, app: &mut App) {
    let palette = Palette::new(&app.client_preferences);
    let live = app.live();
    let chart = app.graph.visualization == Visualization::Chart;
    let graph_height = app
        .graph
        .visualization
        .height()
        .min(content.height.saturating_sub(8));
    // Keep the taller chart compact above the battery and on either side of the plot.
    // Remaining space stays below the controls; small terminals can use zero gaps.
    let gap = if chart {
        Constraint::Length(content.height.saturating_sub(8 + graph_height) / 4)
    } else {
        Constraint::Fill(1)
    };
    let rows = Layout::vertical([
        Constraint::Length(1),
        gap,
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        gap,
        Constraint::Length(2 + graph_height),
        gap,
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
        Paragraph::new(station).style(Style::default().fg(palette.theme.muted_foreground)),
        header[0],
    );
    let (label, color) = if !app.connected {
        ("DAEMON OFFLINE", palette.theme.destructive)
    } else if live {
        ("CONNECTED", GREEN)
    } else if app
        .status
        .as_ref()
        .is_some_and(|s| s.telemetry.sample.is_some())
    {
        ("LAST KNOWN", palette.theme.warning)
    } else {
        ("NO TELEMETRY", palette.theme.destructive)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{label}  "), Style::default().fg(color)),
            Span::styled("●", Style::default().fg(color).add_modifier(Modifier::BOLD)),
        ]))
        .alignment(Alignment::Right),
        header[1],
    );
    let power = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(4),
        Constraint::Fill(1),
    ])
    .split(rows[6]);
    app.set_graph_width(if chart {
        rows[6].width.saturating_sub(CHART_AXIS_WIDTH)
    } else {
        power[0].width.max(power[2].width)
    });
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
        .style(
            Style::default()
                .fg(palette.theme.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        rows[2],
    );
    frame.render_widget(
        Battery {
            percent: sample.map_or(0, |s| s.battery_percent),
            no_color: app.no_color,
            track: palette.theme.field,
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
        .style(Style::default().fg(palette.theme.muted_foreground)),
        rows[4],
    );
    for (rect, label, value, output) in [
        (power[0], "INPUT", sample.map(|s| s.input_power_w), false),
        (power[2], "OUTPUT", sample.map(|s| s.output_power_w), true),
    ] {
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(graph_height),
        ])
        .split(rect);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{label} "),
                    Style::default().fg(if chart && live {
                        if output { CYAN } else { GREEN }
                    } else {
                        palette.theme.muted_foreground
                    }),
                ),
                Span::styled(
                    value.map(|v| v.to_string()).unwrap_or_else(|| "--".into()),
                    Style::default()
                        .fg(palette.theme.foreground)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" W", Style::default().fg(palette.theme.muted_foreground)),
            ]))
            .alignment(Alignment::Center),
            parts[0],
        );
        if chart {
            continue;
        }
        if live && value == Some(0) && !app.has_power_history(output) {
            idle_graph(
                frame,
                parts[2],
                app.clock.animation_elapsed(app.animation_started),
                &palette,
            );
            continue;
        }
        let columns = app
            .graph
            .columns(app.timeline_now_ms(), parts[2].width, output);
        let maximum = power_scale_from(
            app.graph_base_scale_w,
            columns
                .iter()
                .flatten()
                .copied()
                .chain(value.map(|v| v as f64)),
        );
        frame.render_widget(
            Paragraph::new(format!("0–{maximum} W"))
                .alignment(Alignment::Right)
                .style(Style::default().fg(palette.dim)),
            parts[1],
        );
        // Sparkline floors to eighth-cell ticks; keep positive readings visible at the selected scale.
        let scale = maximum * 1000;
        let minimum = scale.div_ceil(8 * u64::from(parts[2].height.max(1)));
        let data = columns
            .into_iter()
            .map(|value| value.unwrap_or(0.0))
            .map(|value| {
                let visible = if value == 0.0 {
                    0
                } else {
                    ((value * 1000.0).round() as u64).max(minimum)
                };
                SparklineBar::from(visible).style(Style::default().fg(if live {
                    load_color(value, maximum)
                } else {
                    palette.dim
                }))
            });
        frame.render_widget(Sparkline::default().data(data).max(scale), parts[2]);
    }
    if chart {
        let area = Rect::new(rows[6].x, rows[6].y + 2, rows[6].width, graph_height);
        let idle = live
            && sample.is_some_and(|s| s.input_power_w == 0 && s.output_power_w == 0)
            && !app.has_power_history(false)
            && !app.has_power_history(true);
        let plot_width = area.width.saturating_sub(CHART_AXIS_WIDTH);
        let input = app.graph.columns(app.timeline_now_ms(), plot_width, false);
        let output = app.graph.columns(app.timeline_now_ms(), plot_width, true);
        let maximum = power_scale_from(
            app.graph_base_scale_w,
            input.iter().chain(&output).flatten().copied().chain(
                sample
                    .into_iter()
                    .flat_map(|s| [s.input_power_w as f64, s.output_power_w as f64]),
            ),
        );
        let time_ticks = chart_time_ticks(
            app.timeline_now_ms(),
            app.graph.resolution,
            plot_width,
            app.timezone,
        );
        power_chart(
            frame,
            area,
            (
                if idle { &[] } else { &input },
                if idle { &[] } else { &output },
            ),
            maximum,
            live,
            time_ticks,
            &palette,
        );
        if idle {
            idle_graph(
                frame,
                Rect::new(
                    area.x + CHART_AXIS_WIDTH,
                    area.y,
                    plot_width,
                    area.height - 2,
                ),
                app.clock.animation_elapsed(app.animation_started),
                &palette,
            );
        }
    }
    let controls = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(rows[8]);
    for (index, label) in ["AC", "DC", "LIGHT"].iter().enumerate() {
        let rect = controls[index];
        let active = !app.snapshot && (app.hovered == Some(index) || app.selected == Some(index));
        let enabled = sample.map(|s| [s.ac_enabled, s.dc_enabled, s.light_enabled][index]);
        let pending_label = app
            .pending
            .as_ref()
            .filter(|pending| pending.starts_with(["AC", "DC", "LIGHT"][index]));
        let title = pending_label.map(String::as_str).unwrap_or(label);
        let button = centered(
            rect,
            ratcn::ButtonWidget::new(title).width().min(rect.width),
            rect.height,
        );
        app.controls[index] = button;
        let state = enabled
            .map(|on| if on { "ON" } else { "OFF" })
            .unwrap_or("--");
        let color = if enabled == Some(true) && live {
            GREEN
        } else {
            palette.dim
        };
        frame.render_widget(
            action_button(title, &palette.theme, active, false),
            Rect {
                height: 1,
                ..button
            },
        );
        frame.render_widget(
            Paragraph::new(state).alignment(Alignment::Center).style(
                Style::default()
                    .fg(color)
                    .bg(palette.theme.background)
                    .add_modifier(if enabled == Some(true) {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Rect::new(rect.x, rect.y + 1, rect.width, 1),
        );
    }
}

/// Separate datasets prevent the built-in line renderer from bridging recording gaps.
pub(crate) fn chart_runs(columns: &[Option<f64>]) -> Vec<Vec<(f64, f64)>> {
    let mut runs = Vec::new();
    let mut run = Vec::new();
    for (column, value) in columns.iter().enumerate() {
        match value {
            Some(value) => run.push((column as f64, *value)),
            None if !run.is_empty() => runs.push(std::mem::take(&mut run)),
            None => {}
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

fn power_chart(
    frame: &mut Frame,
    area: Rect,
    series: (&[Option<f64>], &[Option<f64>]),
    maximum: u64,
    live: bool,
    time_ticks: Vec<(u16, String)>,
    palette: &Palette,
) {
    let (input, output) = series;
    let input_runs = chart_runs(input);
    let output_runs = chart_runs(output);
    let input_datasets = input_runs
        .iter()
        .map(|run| {
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(if live { GREEN } else { palette.dim }))
                .data(run)
        })
        .collect();
    let output_datasets = output_runs
        .iter()
        .map(|run| {
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(if live { CYAN } else { palette.dim }))
                .data(run)
        })
        .collect();
    let chart = |datasets| {
        Chart::new(datasets)
            .legend_position(None)
            .x_axis(
                Axis::default()
                    .bounds([
                        0.0,
                        f64::from(area.width.saturating_sub(CHART_AXIS_WIDTH + 1).max(1)),
                    ])
                    // Reserve a label row; time ticks have actual timestamp positions.
                    .labels([Line::from("")])
                    .style(Style::default().fg(palette.theme.border)),
            )
            .y_axis(
                Axis::default()
                    .bounds([0.0, maximum as f64])
                    .labels(
                        [
                            // Chart puts the lowest label above the horizontal axis.
                            // Keep its reserved width, and label the origin below instead.
                            Line::from("      "),
                            Line::from(format!("{:>6}", maximum / 2)),
                            Line::from(format!("{maximum:>6}")),
                        ]
                        .map(|label| {
                            label.style(Style::default().fg(palette.theme.muted_foreground))
                        }),
                    )
                    .labels_alignment(Alignment::Right)
                    .style(Style::default().fg(palette.theme.border)),
            )
            .style(Style::default().bg(palette.theme.background))
    };
    frame.render_widget(chart(input_datasets), area);
    // Chart layers replace whole Braille cells. Merge patterns rather than erasing a series.
    let mut overlay = Buffer::empty(area);
    chart(output_datasets).render(area, &mut overlay);
    for (position, cell) in overlay.content.iter().enumerate() {
        let Some(output) = braille_pattern(cell.symbol()) else {
            continue;
        };
        let x = area.x + (position % usize::from(area.width)) as u16;
        let y = area.y + (position / usize::from(area.width)) as u16;
        let target = &mut frame.buffer_mut()[(x, y)];
        if let Some(input) = braille_pattern(target.symbol()) {
            target.set_char(char::from_u32(0x2800 + (input | output)).unwrap());
            target.set_fg(if live {
                palette.theme.foreground
            } else {
                palette.dim
            });
        } else {
            *target = cell.clone();
        }
    }
    frame.render_widget(
        Paragraph::new("0")
            .alignment(Alignment::Right)
            .style(Style::default().fg(palette.theme.muted_foreground)),
        Rect::new(area.x, area.bottom() - 2, CHART_AXIS_WIDTH - 1, 1),
    );
    let plot_x = area.x + CHART_AXIS_WIDTH;
    let plot_width = area.width.saturating_sub(CHART_AXIS_WIDTH);
    for (column, label) in time_ticks {
        frame.buffer_mut()[(plot_x + column, area.bottom() - 2)]
            .set_symbol("┬")
            .set_fg(palette.theme.border);
        let label_width = label.len() as u16;
        let label_x = column
            .saturating_sub(label_width / 2)
            .min(plot_width.saturating_sub(label_width));
        frame.render_widget(
            Paragraph::new(label).style(Style::default().fg(palette.theme.muted_foreground)),
            Rect::new(plot_x + label_x, area.bottom() - 1, label_width, 1),
        );
    }
}

fn braille_pattern(symbol: &str) -> Option<u32> {
    symbol
        .chars()
        .next()
        .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
        .map(|c| c as u32 - 0x2800)
}

pub(crate) fn chart_time_ticks(
    now_ms: i64,
    resolution: crate::history::Resolution,
    width: u16,
    timezone: Option<chrono_tz::Tz>,
) -> Vec<(u16, String)> {
    let span = resolution.seconds() * 1000;
    let latest = now_ms.div_euclid(span) * span;
    let duration = i64::from(width.saturating_sub(1)) * span;
    let first = latest - duration;
    let unit = if resolution == crate::history::Resolution::Hour {
        3_600_000
    } else {
        60_000
    };
    // Anchor equal intervals to UTC, so ticks move with the data instead of relabeling endpoints.
    let step = (duration / 3 / unit).max(1) * unit;
    let format = if resolution == crate::history::Resolution::Hour {
        "%m-%d %Hh"
    } else {
        "%H:%M"
    };
    let mut ticks = Vec::new();
    let mut time = first.div_euclid(step) * step;
    if time < first {
        time += step;
    }
    while time <= latest {
        if let Some(timestamp) = chrono::DateTime::from_timestamp_millis(time) {
            let label = if let Some(zone) = timezone {
                timestamp.with_timezone(&zone).format(format).to_string()
            } else {
                timestamp
                    .with_timezone(&chrono::Local)
                    .format(format)
                    .to_string()
            };
            ticks.push((((time - first) / span) as u16, label));
        }
        time += step;
    }
    if ticks.len() > 4 {
        ticks.drain(..ticks.len() - 4);
    }
    ticks
}

fn idle_graph(frame: &mut Frame, area: Rect, elapsed: std::time::Duration, palette: &Palette) {
    // One cell every two seconds, reversing at either end of an eleven-cell track.
    let step = (elapsed.as_secs() / 2 % 20) as usize;
    let position = step.min(20 - step);
    let track = Line::from(
        (0..11)
            .map(|index| {
                Span::styled(
                    if index == position { "○" } else { "·" },
                    Style::default().fg(if index == position {
                        palette.dim
                    } else {
                        palette.theme.field
                    }),
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
    let palette = Palette::new(&app.client_preferences);
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
    for (index, (rect, text)) in [(header[0], "prev"), (header[2], "next")]
        .into_iter()
        .enumerate()
    {
        frame.render_widget(
            action_button(
                text,
                &palette.theme,
                false,
                app.logs.loading || !app.logs.can_navigate(index == 1),
            ),
            rect,
        );
    }
    frame.render_widget(
        Paragraph::new(app.logs.day.to_string())
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(palette.theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ),
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
        .style(Style::default().fg(if app.logs.follow {
            GREEN
        } else {
            palette.theme.muted_foreground
        })),
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
                "ERROR" => palette.theme.destructive,
                "WARNING" => palette.theme.warning,
                _ => palette.theme.muted_foreground,
            };
            Line::from(vec![
                Span::styled(format!("{time} "), Style::default().fg(palette.dim)),
                Span::styled(format!("{:<7} ", safe(level)), Style::default().fg(color)),
                Span::raw(safe(log["message"].as_str().unwrap_or("")).into_owned()),
            ])
        })
        .collect();
    let mut text_area = parts[1];
    if max_scroll > 0 {
        text_area.width = text_area.width.saturating_sub(2);
    }
    let items: Vec<ratatui::text::Text<'static>> = if lines.is_empty() {
        vec![
            Line::from(if app.logs.loading {
                "Loading logs..."
            } else {
                "No records for this day and filter."
            })
            .style(Style::default().fg(palette.dim))
            .into(),
        ]
    } else {
        lines.into_iter().map(Into::into).collect()
    };
    frame.render_widget(themed_list(&items, &palette.theme), text_area);
    if max_scroll > 0 {
        let mut state = ScrollbarState::new(max_scroll + 1)
            .position(start)
            .viewport_content_length(viewport);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(Style::default().fg(palette.theme.field))
                .thumb_symbol("█")
                .thumb_style(Style::default().fg(palette.theme.muted_foreground)),
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
        .style(Style::default().fg(palette.theme.muted_foreground)),
        parts[2],
    );
}

fn quit_modal(frame: &mut Frame, screen: Rect, app: &mut App) {
    let palette = Palette::new(&app.client_preferences);
    let area = centered(screen, 50, 8);
    frame.render_widget(Clear, area);
    let block = panel(&palette.theme, " QUIT ").title_bottom(
        Line::from(" Enter select  Esc cancel ")
            .centered()
            .style(Style::default().fg(palette.theme.muted_foreground)),
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
            .style(Style::default().fg(palette.theme.foreground)),
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
            action_button(label, &palette.theme, app.quit_yes == (index == 0), false),
            app.quit_buttons[index],
        );
    }
}

fn settings(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = app.client_preferences.theme();
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .split(area);
    let mut x = rows[0].x;
    for (index, tab) in SettingsTab::ALL.iter().enumerate() {
        let width = tab.title().len() as u16 + 2;
        let rect = Rect::new(
            x,
            rows[0].y,
            width.min(rows[0].right().saturating_sub(x)),
            1,
        );
        app.settings_tabs[index] = rect;
        frame.render_widget(
            ratcn::ButtonWidget::new(tab.title())
                .themed(&theme)
                .variant(if app.settings_tab as usize == index {
                    ratcn::ButtonVariant::Default
                } else {
                    ratcn::ButtonVariant::Ghost
                }),
            rect,
        );
        x += width + 1;
    }
    let status = app.status.as_ref();
    let row = |label: &str, value: String| {
        Line::from(vec![
            Span::styled(
                format!("{label:<20}"),
                Style::default().fg(theme.muted_foreground),
            ),
            Span::styled(
                safe(&value).into_owned(),
                Style::default().fg(theme.foreground),
            ),
        ])
    };
    let heading = |title: &'static str| {
        Line::from(title).style(
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )
    };
    let note =
        |text: &'static str| Line::from(text).style(Style::default().fg(theme.muted_foreground));
    let lines = match app.settings_tab {
        SettingsTab::Preferences | SettingsTab::Charts | SettingsTab::Alerts => {
            let fields: &[Field] = if app.settings_tab == SettingsTab::Charts {
                &Field::CHARTS
            } else if app.settings_tab == SettingsTab::Alerts {
                &Field::ALERTS
            } else {
                &Field::PREFERENCES
            };
            let mut lines = vec![heading(if app.settings_tab == SettingsTab::Charts {
                "Power graphs"
            } else if app.settings_tab == SettingsTab::Alerts {
                "Battery alerts"
            } else {
                "Preferences"
            })];
            for (index, field) in fields.iter().enumerate() {
                let available = *field == Field::Theme || app.settings.is_some();
                let value = if field.is_segmented() {
                    String::new()
                } else if available {
                    format!(
                        "{} ▾",
                        if *field == Field::Theme {
                            app.client_preferences.theme.clone()
                        } else {
                            app.settings_draft.value(*field)
                        }
                    )
                } else if app.settings_error {
                    "Unavailable; retrying".into()
                } else {
                    "Loading...".into()
                };
                let line = row(field.label(), value).style(Style::default().bg(
                    if app.settings_selected == index && !field.is_segmented() {
                        theme.field
                    } else {
                        theme.background
                    },
                ));
                app.settings_fields.push((
                    Rect::new(rows[2].x, rows[2].y + index as u16 + 1, rows[2].width, 1),
                    *field,
                ));
                lines.push(line);
            }
            lines.push(Line::default());
            lines.push(note("←/→ changes segments; Enter opens lists."));
            lines.push(note(if app.settings_tab == SettingsTab::Preferences {
                "Theme saves locally; server autosave after 5s."
            } else {
                "Autosave after 5s; closing settings saves now."
            }));
            lines
        }
        SettingsTab::Notify => vec![
            heading("Telegram notifications"),
            row(
                "Server connector",
                match app.settings.as_ref() {
                    Some(settings) if settings.telegram_configured => "Configured",
                    Some(_) => "Not configured",
                    None => "Unavailable",
                }
                .into(),
            ),
            note("Credentials are configured on the server."),
            note("Alerts run even when this TUI is closed."),
        ],
        SettingsTab::Debug => {
            let mut lines = vec![
                heading("Debug / Diagnostics"),
                row("Server", app.server_url.clone()),
                row(
                    "API connection",
                    if app.connected {
                        "Connected"
                    } else {
                        "Offline"
                    }
                    .into(),
                ),
                row(
                    "Station phase",
                    status
                        .map(|s| s.connection.phase.clone())
                        .unwrap_or_else(|| "Unknown".into()),
                ),
                row(
                    "BLE adapter",
                    status
                        .map(
                            |s| match (&s.connection.adapter_id, &s.connection.adapter_address) {
                                (Some(id), Some(address)) => format!("{id} · {address}"),
                                (Some(id), None) => id.clone(),
                                (_, Some(address)) => address.clone(),
                                _ => "--".into(),
                            },
                        )
                        .unwrap_or_else(|| "--".into()),
                ),
                row(
                    "Telemetry",
                    status
                        .map(|s| s.telemetry.state.clone())
                        .unwrap_or_else(|| "unknown".into()),
                ),
            ];
            let capacity = rows[2].height.saturating_sub(2);
            if capacity >= 8 {
                lines.push(row(
                    "History",
                    status
                        .and_then(|s| s.history["state"].as_str())
                        .unwrap_or("unknown")
                        .into(),
                ));
            }
            if capacity >= 9 {
                lines.push(row(
                    "Sample age",
                    app.age()
                        .map(|age| format!("{age:.1} s"))
                        .unwrap_or_else(|| "--".into()),
                ));
            }
            if capacity >= 10 {
                lines.push(row(
                    "Station mode",
                    status
                        .map(|s| s.connection.desired.clone())
                        .unwrap_or_else(|| "Unknown".into()),
                ));
            }
            lines.push(row("Runtime logging", logging_status(app)));
            lines
        }
    };
    let body = if app.settings_tab == SettingsTab::Debug {
        Rect {
            height: rows[2].height.saturating_sub(2),
            ..rows[2]
        }
    } else {
        rows[2]
    };
    frame.render_widget(Paragraph::new(lines.clone()), body);
    for (index, (rect, field)) in app.settings_fields.iter().enumerate() {
        if field.is_segmented() {
            frame.render_widget(
                Paragraph::new(field.label()).style(Style::default().fg(theme.muted_foreground)),
                Rect { width: 20, ..*rect },
            );
            let choices = field.choices();
            let selected = app
                .settings
                .is_some()
                .then(|| {
                    choices
                        .iter()
                        .position(|value| *value == app.settings_draft.value(*field))
                })
                .flatten();
            let mut x = rect.x + 20;
            frame.render_widget(Paragraph::new("["), Rect::new(x, rect.y, 1, 1));
            x += 1;
            for choice in 0..choices.len() {
                if choice > 0 {
                    frame.render_widget(
                        Paragraph::new("|").style(Style::default().fg(theme.border)),
                        Rect::new(x, rect.y, 1, 1),
                    );
                    x += 1;
                }
                let label = field.segment_label(choice);
                let width = label.len() as u16 + 2;
                let segment = Rect::new(x, rect.y, width.min(rect.right().saturating_sub(x)), 1);
                frame.render_widget(
                    ratcn::ButtonWidget::new(&label)
                        .themed(&theme)
                        .variant(if selected == Some(choice) {
                            ratcn::ButtonVariant::Default
                        } else {
                            ratcn::ButtonVariant::Ghost
                        })
                        .focused(app.settings_selected == index && selected == Some(choice))
                        .disabled(app.settings.is_none() || app.pending.is_some()),
                    segment,
                );
                if selected == Some(choice) {
                    for x in segment.x..segment.right() {
                        let emphasis = Modifier::BOLD
                            | if app.no_color {
                                Modifier::REVERSED
                            } else {
                                Modifier::empty()
                            };
                        frame.buffer_mut()[(x, segment.y)]
                            .set_style(Style::default().add_modifier(emphasis));
                    }
                }
                app.settings_segments.push((segment, *field, choice));
                x += width;
            }
            if x < rect.right() {
                frame.render_widget(Paragraph::new("]"), Rect::new(x, rect.y, 1, 1));
            }
        } else {
            let mut line = lines[index + 1].clone();
            line.style.bg = None;
            let items = [ratatui::text::Text::from(line)];
            frame.render_widget(
                themed_list(&items, &theme)
                    .focused(app.settings_selected == index)
                    .focused_item(Some(0)),
                *rect,
            );
        }
    }
    if app.settings_tab == SettingsTab::Notify {
        app.settings_actions[0] = Rect::new(rows[2].x, rows[2].y + 5, rows[2].width, 1);
        let label = if app.pending.as_deref() == Some("telegram test") {
            "[ Sending test... ]"
        } else {
            "[ Send test message ]"
        };
        app.settings_actions[0].width = app.settings_actions[0]
            .width
            .min(ratcn::ButtonWidget::new(label).width());
        frame.render_widget(
            action_button(label, &theme, true, !app.connected || app.pending.is_some()),
            app.settings_actions[0],
        );
    } else if app.settings_tab == SettingsTab::Debug {
        let logging = status
            .and_then(|s| s.logging["effective_level"].as_str())
            .unwrap_or("Unknown");
        let labels = [
            "[ Retry ]".to_owned(),
            format!(
                "[ {} ]",
                if status.is_some_and(|s| s.connection.desired == "paused") {
                    "Resume"
                } else {
                    "Pause"
                }
            ),
            format!(
                "[ Debug {} ]",
                if logging == "DEBUG" { "ON" } else { "OFF" }
            ),
        ];
        frame.render_widget(
            Paragraph::new("─".repeat(rows[2].width as usize))
                .style(Style::default().fg(theme.border)),
            Rect::new(
                rows[2].x,
                rows[2].bottom().saturating_sub(2),
                rows[2].width,
                1,
            ),
        );
        let buttons = Layout::horizontal([Constraint::Fill(1); 3]).split(Rect::new(
            rows[2].x,
            rows[2].bottom().saturating_sub(1),
            rows[2].width,
            1,
        ));
        for (index, label) in labels.into_iter().enumerate() {
            let button = Rect {
                width: buttons[index]
                    .width
                    .min(ratcn::ButtonWidget::new(&label).width()),
                ..buttons[index]
            };
            app.settings_actions[index] = button;
            frame.render_widget(
                action_button(
                    &label,
                    &theme,
                    app.settings_selected == index,
                    !app.connected || app.pending.is_some(),
                ),
                button,
            );
        }
    }
    if let Some(picker) = &app.settings_picker {
        let options = picker.options();
        let search_rows = u16::from(picker.field == Field::Timezone);
        let height = (options.len() as u16 + 2 + search_rows)
            .min(area.height)
            .max(3 + search_rows);
        let popup = centered(area, area.width.min(48), height);
        frame.render_widget(Clear, popup);
        let block = panel(&theme, format!(" {} ", picker.field.label())).title_bottom(
            Line::from(" Enter select  Esc cancel ")
                .style(Style::default().fg(theme.muted_foreground)),
        );
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        if search_rows > 0 {
            frame.render_widget(
                Paragraph::new(format!("Search: {}", picker.query))
                    .style(Style::default().fg(theme.muted_foreground)),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
        }
        if options.is_empty() {
            frame.render_widget(
                Paragraph::new("No matching values")
                    .style(Style::default().fg(theme.muted_foreground)),
                Rect::new(inner.x, inner.y + search_rows, inner.width, 1),
            );
        }
        let viewport = inner.height.saturating_sub(search_rows) as usize;
        let start = picker.selected.saturating_sub(viewport.saturating_sub(1));
        let visible: Vec<_> = options
            .iter()
            .skip(start)
            .take(viewport)
            .map(|(_, value)| ratatui::text::Text::from(value.clone()))
            .collect();
        for (offset, (index, _)) in options.iter().skip(start).take(viewport).enumerate() {
            let rect = Rect::new(
                inner.x,
                inner.y + offset as u16 + search_rows,
                inner.width,
                1,
            );
            app.settings_choices.push((rect, *index));
        }
        frame.render_widget(
            ratcn::ListWidget::new(&visible)
                .themed(&theme)
                .focused(true)
                .focused_item(Some(picker.selected.saturating_sub(start)))
                .focus_symbol("› "),
            Rect::new(
                inner.x,
                inner.y + search_rows,
                inner.width,
                inner.height.saturating_sub(search_rows),
            ),
        );
    }
}

fn help(frame: &mut Frame, area: Rect, context: View, tab: SettingsTab, theme: &ratcn::Theme) {
    let text = if context == View::Settings {
        let actions = match tab {
            SettingsTab::Preferences | SettingsTab::Charts => {
                "Up/Down   Select a field
Left/Right Change a segmented choice\nEnter     Cycle segment / open / confirm
Click     Choose a field or value
Timezone list supports typing to search."
            }
            SettingsTab::Debug => {
                "Up/Down   Select Retry / Pause / Debug
Enter     Activate selected button
Click     Activate a button"
            }
            SettingsTab::Alerts => {
                "←/→ changes segments; Enter opens other values.\nAutosave after 5s; closing settings saves now.\nALERT at battery <= threshold.\nRECOVERED at battery >= threshold + hysteresis.\nCooldown keeps only the latest state; no repeats.\nOnly fresh LIVE telemetry is evaluated."
            }
            SettingsTab::Notify => {
                "Enter / click sends a Telegram test message.\nThe server stores credentials and sends notifications.\nThe test does not change battery alert state or cooldown."
            }
        };
        format!(
            "HELP — SETTINGS / {}\n\nTab / Shift-Tab   Switch tab\n\n{}\n\nEsc close   q confirm quit",
            tab.title(),
            actions
        )
    } else if context == View::Logs {
        "HELP — LOGS\n\nUp/Down, PageUp/PageDown   Scroll records\nMouse wheel / scrollbar   Scroll or drag\nLeft/Right or [ / ]       Previous/next day\nf                        Change minimum log level\n+ / -                    Change page size\nHome                     Beginning of selected day\nEnd                      Today: latest records and live follow\nb                        Toggle runtime DEBUG override\nDouble-click LOGS        Copy all loaded records\n\nEsc close   q confirm quit".into()
    } else {
        "HELP — DASHBOARD\n\nTab / Shift-Tab          Focus output control\nEnter / Space            Activate focused output\nl                        Open Logs\ns                        Open Settings / Diagnostics\nt                        Cycle average per bar: 10s / 60s / 1h\ng                        Switch Sparkline / Chart (session only)\nF1 / ?                   Help for the active window\nDouble-click MYPOWERS    Copy current API snapshot\n\nEsc close   q confirm quit".into()
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(theme.muted_foreground)),
        area,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let column = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(area)[0];
    Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(column)[0]
}

fn load_color(value: f64, maximum: u64) -> Color {
    let ratio = value / maximum as f64;
    if ratio < 0.45 {
        GREEN
    } else if ratio < 0.75 {
        YELLOW
    } else {
        ORANGE
    }
}

struct Battery {
    percent: u16,
    no_color: bool,
    track: Color,
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
                    cell.set_symbol(" ").set_fg(self.track).set_bg(self.track);
                }
                1 => {
                    cell.set_symbol("▌").set_fg(color).set_bg(self.track);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Cell, layout::Position};

    #[test]
    fn battery_fill_is_monotonic_accurate_and_confined_to_its_row() {
        let mut marker = Cell::new(".");
        marker.set_fg(Color::Magenta).set_bg(Color::Blue);
        for width in [1, 2, 3, 5, 10, 27, 54] {
            let area = Rect::new(9, 12, width, 1);
            let original = Buffer::filled(Rect::new(7, 11, width + 4, 3), marker.clone());
            let mut previous_fill = 0;
            for percent in 0..=100 {
                let mut buffer = original.clone();
                Battery {
                    percent,
                    no_color: false,
                    track: TRACK,
                }
                .render(area, &mut buffer);
                let mut fill = 0;
                let mut unfilled = false;
                for x in area.x..area.right() {
                    let cell = &buffer[(x, area.y)];
                    let coverage = if cell.symbol() == "▌" {
                        assert_eq!(cell.bg, TRACK);
                        assert_ne!(cell.fg, TRACK);
                        1
                    } else if cell.bg == TRACK {
                        assert_eq!(cell.symbol(), " ");
                        assert_eq!(cell.fg, TRACK);
                        0
                    } else {
                        assert_eq!(cell.symbol(), " ");
                        assert_eq!(cell.fg, cell.bg);
                        2
                    };
                    assert!(!unfilled || coverage == 0, "Gap before a filled cell");
                    unfilled |= coverage < 2;
                    fill += coverage;
                }
                assert!(fill >= previous_fill);
                previous_fill = fill;
                let actual = f64::from(fill) / 2.0;
                let desired = f64::from(width) * f64::from(percent) / 100.0;
                assert!((actual - desired).abs() <= 0.25 + f64::EPSILON * 100.0);
                for y in original.area.y..original.area.bottom() {
                    for x in original.area.x..original.area.right() {
                        if !area.contains(Position::new(x, y)) {
                            assert_eq!(buffer[(x, y)], original[(x, y)]);
                        }
                    }
                }
            }
            assert_eq!(previous_fill, width * 2);
        }
        let original = Buffer::filled(Rect::new(7, 11, 10, 3), marker);
        for area in [Rect::new(9, 12, 0, 1), Rect::new(9, 12, 3, 0)] {
            let mut buffer = original.clone();
            Battery {
                percent: 50,
                no_color: false,
                track: TRACK,
            }
            .render(area, &mut buffer);
            assert_eq!(buffer, original);
        }
    }

    #[test]
    fn battery_keeps_gradient_stops_and_monochrome_half_cells() {
        let area = Rect::new(0, 0, 5, 1);
        let mut buffer = Buffer::empty(area);
        Battery {
            percent: 100,
            no_color: false,
            track: TRACK,
        }
        .render(area, &mut buffer);
        assert_eq!(
            buffer
                .content
                .iter()
                .map(|cell| cell.bg)
                .collect::<Vec<_>>(),
            vec![RED, ORANGE, YELLOW, Color::Rgb(169, 209, 106), GREEN,]
        );
        let full = buffer.clone();
        Battery {
            percent: u16::MAX,
            no_color: false,
            track: TRACK,
        }
        .render(area, &mut buffer);
        assert_eq!(buffer, full);
        let area = Rect::new(0, 0, 10, 1);
        for (percent, expected) in [
            (0, "░░░░░░░░░░"),
            (5, "▌░░░░░░░░░"),
            (10, "█░░░░░░░░░"),
            (55, "█████▌░░░░"),
            (100, "██████████"),
        ] {
            let mut buffer = Buffer::empty(area);
            Battery {
                percent,
                no_color: true,
                track: TRACK,
            }
            .render(area, &mut buffer);
            assert_eq!(
                buffer.content.iter().map(Cell::symbol).collect::<String>(),
                expected
            );
        }
    }
}
