use std::time::Duration;

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Sparkline, SparklineBar, Widget},
};

use crate::app::App;

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 18;
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
    if screen.width < MIN_WIDTH || screen.height < MIN_HEIGHT {
        let message = centered(screen, screen.width, 2);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("Terminal too small").style(Style::default().fg(TEXT)),
                Line::from("Need at least 60x18").style(Style::default().fg(MUTED)),
            ])
            .alignment(Alignment::Center),
            message,
        );
        return;
    }

    let area = centered(screen, screen.width.min(94), screen.height.min(22));
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
            Line::from(vec![
                Span::styled(" a ", Style::default().fg(TEXT)),
                Span::styled("AC  ", Style::default().fg(MUTED)),
                Span::styled("d ", Style::default().fg(TEXT)),
                Span::styled("DC  ", Style::default().fg(MUTED)),
                Span::styled("l ", Style::default().fg(TEXT)),
                Span::styled("lamps  ", Style::default().fg(MUTED)),
                Span::styled("q ", Style::default().fg(TEXT)),
                Span::styled("quit ", Style::default().fg(MUTED)),
            ])
            .centered(),
        );
    if let Some((success, time)) = app.clipboard_notice
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
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Fill(1),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(content);

    let header = Layout::horizontal([Constraint::Fill(1), Constraint::Length(16)]).split(rows[0]);
    frame.render_widget(
        Paragraph::new("AP S300 V2.0").style(Style::default().fg(MUTED)),
        header[0],
    );
    let status = if app.live {
        Line::from(vec![
            Span::styled("SIMULATED  ", Style::default().fg(DIM)),
            Span::styled("●", Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
        ])
    } else {
        Line::from(vec![
            Span::styled("OFFLINE  ", Style::default().fg(RED)),
            Span::styled("●", Style::default().fg(RED).add_modifier(Modifier::BOLD)),
        ])
    };
    frame.render_widget(
        Paragraph::new(status).alignment(Alignment::Right),
        header[1],
    );

    frame.render_widget(
        Paragraph::new("78%")
            .alignment(Alignment::Center)
            .style(Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
        rows[2],
    );
    let bar = centered(rows[3], (content.width * 3 / 4).min(54), 1);
    frame.render_widget(Battery { percent: 78 }, bar);
    frame.render_widget(
        Paragraph::new("48h 57m")
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
    for (rect, label, value, history, maximum) in [
        (power[0], "INPUT", app.input, &app.input_history, 100),
        (power[2], "OUTPUT", app.output, &app.output_history, 300),
    ] {
        let parts = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .split(rect);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(label).style(Style::default().fg(MUTED)),
                Line::from(vec![
                    Span::styled(
                        value.to_string(),
                        Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" W", Style::default().fg(MUTED)),
                ]),
            ])
            .alignment(Alignment::Center),
            parts[0],
        );
        graph(frame, parts[2], value, history, maximum, app.colored_bars);
    }

    let controls = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(rows[8]);
    for (index, label) in ["AC", "DC", "LAMPS"].iter().enumerate() {
        let rect = controls[index];
        app.controls[index] = rect;
        let active = app.hovered == Some(index) || app.selected == Some(index);
        let style = Style::default().bg(if active { TRACK } else { BACKGROUND });
        let label_style = Style::default().fg(if active { TEXT } else { MUTED });
        let state_style = Style::default().fg(if app.enabled[index] { GREEN } else { DIM });
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(*label).style(label_style),
                Line::from(if app.enabled[index] { "ON" } else { "OFF" }).style(
                    if app.enabled[index] {
                        state_style.add_modifier(Modifier::BOLD)
                    } else {
                        state_style
                    },
                ),
            ])
            .style(style)
            .alignment(Alignment::Center),
            rect,
        );
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let column = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(area)[0];
    Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(column)[0]
}

fn graph(frame: &mut Frame, area: Rect, value: u64, history: &[u64], maximum: u64, colored: bool) {
    let visible = history.len().min(area.width as usize);
    let data: Vec<SparklineBar> = history[history.len() - visible..]
        .iter()
        .map(|&sample| {
            SparklineBar::from(sample).style(Style::default().fg(if colored {
                load_color(sample, maximum)
            } else {
                load_color(value, maximum)
            }))
        })
        .collect();
    frame.render_widget(Sparkline::default().data(data).max(maximum), area);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn render(width: u16, height: u16, app: &mut App) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn lines(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn layouts_keep_controls_separate_and_values_visible() {
        for (width, height) in [(60, 18), (80, 24), (120, 40), (200, 60)] {
            let mut app = App::default();
            let buffer = render(width, height, &mut app);
            let text = lines(&buffer).join("\n");
            let title: String = (app.title.x..app.title.right())
                .map(|x| buffer[(x, app.title.y)].symbol())
                .collect();
            assert_eq!(title, "MYPOWERS");
            for expected in [
                "MYPOWERS",
                "AP S300 V2.0",
                "78%",
                "48h 57m",
                "63 W",
                "181 W",
                "LAMPS",
                "╭",
                "╮",
                "╰",
                "╯",
            ] {
                assert!(
                    text.contains(expected),
                    "Missing {expected} at {width}x{height}"
                );
            }
            for expected in ["INPUT", "OUTPUT", "63 W", "181 W"] {
                assert_eq!(
                    text.matches(expected).count(),
                    1,
                    "Duplicate {expected} at {width}x{height}"
                );
            }
            assert!(!text.contains("100 W") && !text.contains("300 W"));
            for (i, rect) in app.controls.iter().enumerate() {
                assert_eq!(rect.height, 2);
                assert!(rect.width >= 18);
                for other in &app.controls[i + 1..] {
                    assert!(rect.intersection(*other).is_empty());
                }
            }
            if let Ok(directory) = std::env::var("MYPOWERS_PREVIEW_DIR") {
                write_preview(&buffer, &directory, &format!("{width}x{height}"));
            }
        }
    }

    #[test]
    fn undersized_terminals_have_no_clickable_controls() {
        let mut app = App::default();
        render(80, 24, &mut app);
        for (width, height) in [(59, 18), (60, 17), (22, 4), (1, 1), (0, 0)] {
            let buffer = render(width, height, &mut app);
            assert!(app.controls.iter().all(|rect| rect.is_empty()));
            assert!(app.title.is_empty());
            if width >= 22 && height >= 2 {
                let text = lines(&buffer).join("\n");
                assert!(text.contains("Terminal too small"));
                assert!(text.contains("Need at least 60x18"));
            }
        }
    }

    #[test]
    fn clipboard_feedback_expires_without_moving_the_clickable_title() {
        for (width, height) in [(60, 18), (61, 18), (80, 24), (81, 24), (120, 40)] {
            let mut app = App::default();
            render(width, height, &mut app);
            let title = app.title;
            for (success, expected) in [(true, "JSON copied"), (false, "Copy failed")] {
                app.clipboard_notice = Some((success, std::time::Instant::now()));
                let buffer = render(width, height, &mut app);
                let text = lines(&buffer).join("\n");
                assert!(text.contains(expected));
                assert_eq!(app.title, title);
                let title_text: String = (title.x..title.right())
                    .map(|x| buffer[(x, title.y)].symbol())
                    .collect();
                assert_eq!(title_text, "MYPOWERS");
            }
            app.clipboard_notice = Some((true, std::time::Instant::now() - Duration::from_secs(4)));
            assert!(
                !lines(&render(width, height, &mut app))
                    .join("\n")
                    .contains("JSON copied")
            );
        }
    }

    #[test]
    fn sparkline_scales_are_fixed_and_each_bar_can_have_its_own_color() {
        let mut app = App::default();
        app.input_history.fill(50);
        app.output_history.fill(150);
        let buffer = render(80, 24, &mut app);
        let text = lines(&buffer);
        let row = text.iter().position(|line| line.contains("INPUT")).unwrap() + 3;
        for half in [0..40, 40..80] {
            let symbols: String = half
                .clone()
                .map(|x| buffer[(x, row as u16)].symbol())
                .collect();
            assert!(
                !symbols.contains('█'),
                "Half scale must leave the upper graph row empty"
            );
            let lower: String = half.map(|x| buffer[(x, row as u16 + 1)].symbol()).collect();
            assert!(
                lower.contains("████"),
                "Half scale should fill exactly one of the two rows"
            );
        }
        for (i, value) in app.input_history.iter_mut().enumerate() {
            *value = [31, 37][i % 2];
        }
        let finer = render(80, 24, &mut app);
        let lower: String = (0..40)
            .map(|x| finer[(x, row as u16 + 1)].symbol())
            .collect();
        assert!(
            lower.contains('▄') && lower.contains('▅'),
            "31% and 37% should occupy different two-row height levels"
        );
        for (i, value) in app.input_history.iter_mut().enumerate() {
            *value = [10, 50, 90][i % 3];
        }
        let colored = render(80, 24, &mut app);
        let row = row as u16;
        for color in [GREEN, YELLOW, ORANGE] {
            assert!((0..40).any(|x| colored[(x, row)].fg == color));
        }
        assert!(!(0..80).any(|x| colored[(x, row)].fg == RED));
        app.colored_bars = false;
        let uniform = render(80, 24, &mut app);
        assert!(!(0..80).any(|x| uniform[(x, row)].fg == GREEN || uniform[(x, row)].fg == ORANGE));
    }

    #[test]
    fn battery_gradient_and_fill_are_bounded() {
        let area = Rect::new(0, 0, 50, 1);
        let mut buffer = Buffer::empty(area);
        Battery { percent: 78 }.render(area, &mut buffer);
        assert!((0..39).all(|x| buffer[(x, 0)].fg != TRACK));
        assert!((39..50).all(|x| buffer[(x, 0)].fg == TRACK));
        assert_eq!(buffer[(0, 0)].fg, RED);
        assert_ne!(buffer[(10, 0)].fg, buffer[(11, 0)].fg);
        assert_eq!(battery_color(1.0), GREEN);
        Battery { percent: 0 }.render(area, &mut buffer);
        assert!(buffer.content.iter().all(|cell| cell.fg == TRACK));
        Battery { percent: 200 }.render(area, &mut buffer);
        assert!(buffer.content.iter().all(|cell| cell.fg != TRACK));
        Battery { percent: 78 }.render(Rect::default(), &mut Buffer::empty(Rect::default()));
        Battery { percent: 78 }.render(
            Rect::new(0, 0, 1, 1),
            &mut Buffer::empty(Rect::new(0, 0, 1, 1)),
        );
    }

    #[test]
    fn connection_indicators_are_static_green_and_red_circles() {
        let mut app = App::default();
        let connected = render(80, 24, &mut app);
        assert!(
            connected
                .content
                .iter()
                .any(|cell| cell.symbol() == "●" && cell.fg == GREEN)
        );
        assert_eq!(connected, render(80, 24, &mut app));
        app.live = false;
        let offline = render(80, 24, &mut app);
        assert!(lines(&offline).join("\n").contains("OFFLINE"));
        assert!(
            offline
                .content
                .iter()
                .any(|cell| cell.symbol() == "●" && cell.fg == RED)
        );
        assert_eq!(offline, render(80, 24, &mut app));
    }

    fn write_preview(buffer: &Buffer, directory: &str, name: &str) {
        let directory = std::path::Path::new(directory);
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(
            directory.join(format!("{name}.txt")),
            lines(buffer).join("\n"),
        )
        .unwrap();
        let mut html = String::from(
            "<!doctype html><meta charset=\"utf-8\"><style>body{margin:0;padding:24px;background:#10151b}pre{margin:0;font:16px/20px 'DejaVu Sans Mono',monospace;font-variant-ligatures:none}</style><pre>",
        );
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                let rgb = |color| {
                    if let Color::Rgb(r, g, b) = color {
                        format!("#{r:02x}{g:02x}{b:02x}")
                    } else {
                        "inherit".into()
                    }
                };
                let escaped = cell
                    .symbol()
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                html.push_str(&format!(
                    "<span style=\"color:{};background:{};font-weight:{}\">{escaped}</span>",
                    rgb(cell.fg),
                    rgb(cell.bg),
                    if cell.modifier.contains(Modifier::BOLD) {
                        "bold"
                    } else {
                        "normal"
                    }
                ));
            }
            html.push('\n');
        }
        html.push_str("</pre>");
        std::fs::write(directory.join(format!("{name}.html")), html).unwrap();
    }
}
