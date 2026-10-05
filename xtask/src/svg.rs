//! Export Ratatui cells, without interpreting ANSI or knowing anything about UI layout.
use ratatui::{
    buffer::{Buffer, CellWidth},
    style::{Color, Modifier},
};
use std::fmt::Write;

pub const CELL_WIDTH: u32 = 10;
pub const CELL_HEIGHT: u32 = 20;
const DEFAULT_FG: (u8, u8, u8) = (224, 232, 236);
const DEFAULT_BG: (u8, u8, u8) = (16, 21, 27);
const ANSI: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (205, 49, 49),
    (13, 188, 121),
    (229, 229, 16),
    (36, 114, 200),
    (188, 63, 188),
    (17, 168, 205),
    (229, 229, 229),
    (102, 102, 102),
    (241, 76, 76),
    (35, 209, 139),
    (245, 245, 67),
    (59, 142, 234),
    (214, 112, 214),
    (41, 184, 219),
    (255, 255, 255),
];

fn rgb(color: Color, default: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        Color::Reset => default,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index @ 0..=15) => ANSI[index as usize],
        Color::Indexed(index @ 16..=231) => {
            let value = index - 16;
            let component = |level: u8| if level == 0 { 0 } else { 55 + 40 * level };
            (
                component(value / 36),
                component(value / 6 % 6),
                component(value % 6),
            )
        }
        Color::Indexed(index) => {
            let gray = 8 + 10 * (index - 232);
            (gray, gray, gray)
        }
        other => {
            ANSI[match other {
                Color::Black => 0,
                Color::Red => 1,
                Color::Green => 2,
                Color::Yellow => 3,
                Color::Blue => 4,
                Color::Magenta => 5,
                Color::Cyan => 6,
                Color::Gray => 7,
                Color::DarkGray => 8,
                Color::LightRed => 9,
                Color::LightGreen => 10,
                Color::LightYellow => 11,
                Color::LightBlue => 12,
                Color::LightMagenta => 13,
                Color::LightCyan => 14,
                Color::White => 15,
                _ => unreachable!(),
            }]
        }
    }
}

fn hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '\'' => "&apos;".into(),
            // XML 1.0 disallows control characters even as numeric references.
            ch if !matches!(ch, '\t' | '\n' | '\r')
                && (ch < ' ' || matches!(ch, '\u{fffe}' | '\u{ffff}')) =>
            {
                "�".into()
            }
            ch => ch.to_string(),
        })
        .collect()
}

pub fn export(buffer: &Buffer) -> Result<String, String> {
    if buffer.area.is_empty() || buffer.content.len() != buffer.area.area() as usize {
        return Err("Cannot export an empty or incomplete Ratatui buffer".into());
    }
    let width = u32::from(buffer.area.width) * CELL_WIDTH;
    let height = u32::from(buffer.area.height) * CELL_HEIGHT;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" role=\"img\">\n<title>MyPowers terminal UI</title>\n<g shape-rendering=\"crispEdges\">\n"
    );
    // Paint every background first, so a wide glyph can extend into the next cell.
    for (index, cell) in buffer.content.iter().enumerate() {
        let x = index as u32 % u32::from(buffer.area.width) * CELL_WIDTH;
        let y = index as u32 / u32::from(buffer.area.width) * CELL_HEIGHT;
        let background = if cell.modifier.contains(Modifier::REVERSED) {
            rgb(cell.fg, DEFAULT_FG)
        } else {
            rgb(cell.bg, DEFAULT_BG)
        };
        writeln!(svg, "<rect x=\"{x}\" y=\"{y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" fill=\"{}\"/>", hex(background)).unwrap();
    }
    svg.push_str("</g>\n<g font-family=\"DejaVu Sans Mono, Liberation Mono, monospace\" font-size=\"18\" xml:space=\"preserve\">\n");
    let mut covered_until = 0usize;
    for (index, cell) in buffer.content.iter().enumerate() {
        if index % usize::from(buffer.area.width) == 0 {
            covered_until = index;
        }
        if index < covered_until {
            continue;
        }
        let span = usize::from(cell.cell_width().max(1))
            .min(usize::from(buffer.area.width) - index % usize::from(buffer.area.width));
        covered_until = index + span;
        if cell.symbol() == " "
            || cell.symbol().is_empty()
            || cell.modifier.contains(Modifier::HIDDEN)
        {
            continue;
        }
        let x = index as u32 % u32::from(buffer.area.width) * CELL_WIDTH;
        let y = index as u32 / u32::from(buffer.area.width) * CELL_HEIGHT;
        let foreground = if cell.modifier.contains(Modifier::REVERSED) {
            rgb(cell.bg, DEFAULT_BG)
        } else {
            rgb(cell.fg, DEFAULT_FG)
        };
        let mut style = String::new();
        // Explicit transforms also work in viewers without textLength support.
        let shape = cell
            .symbol()
            .chars()
            .all(|character| ('\u{2500}'..='\u{259f}').contains(&character));
        let horizontal_scale = if shape { "0.833333" } else { "0.9" };
        if shape {
            style.push_str(" font-size=\"20\"");
        }
        let cell_width = span as u32 * CELL_WIDTH;
        if cell.modifier.contains(Modifier::BOLD) {
            style.push_str(" font-weight=\"bold\"");
        }
        if cell.modifier.contains(Modifier::DIM) {
            style.push_str(" opacity=\"0.5\"");
        }
        if cell.modifier.contains(Modifier::ITALIC) {
            style.push_str(" font-style=\"italic\"");
        }
        if cell
            .modifier
            .contains(Modifier::UNDERLINED | Modifier::CROSSED_OUT)
        {
            style.push_str(" text-decoration=\"underline line-through\"");
        } else if cell.modifier.contains(Modifier::UNDERLINED) {
            style.push_str(" text-decoration=\"underline\"");
        } else if cell.modifier.contains(Modifier::CROSSED_OUT) {
            style.push_str(" text-decoration=\"line-through\"");
        }
        // A cell viewport clips descenders, block glyphs, and bold/italic overhang.
        // This prevents a partial battery cell from painting outside its own row.
        writeln!(svg, "<svg x=\"{x}\" y=\"{y}\" width=\"{cell_width}\" height=\"{CELL_HEIGHT}\" overflow=\"hidden\"><text x=\"0\" y=\"16\" fill=\"{}\" transform=\"scale({horizontal_scale} 1)\"{style}>{}</text></svg>", hex(foreground), escape(cell.symbol())).unwrap();
    }
    svg.push_str("</g>\n</svg>\n");
    Ok(svg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{layout::Rect, style::Style};

    #[test]
    fn dimensions_and_cell_positions_are_fixed_even_for_offset_buffers() {
        let mut buffer = Buffer::empty(Rect::new(5, 9, 2, 3));
        buffer[(6, 11)].set_symbol("A");
        let svg = export(&buffer).unwrap();
        assert!(svg.contains("width=\"20\" height=\"60\" viewBox=\"0 0 20 60\""));
        assert!(svg.contains("<svg x=\"10\" y=\"40\" width=\"10\" height=\"20\" overflow=\"hidden\"><text x=\"0\" y=\"16\""));
        assert_eq!(svg.matches("<rect ").count(), 6);
        assert!(export(&Buffer::empty(Rect::default())).is_err());
    }

    #[test]
    fn escapes_xml_and_preserves_unicode_symbols() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer[(0, 0)].set_symbol("<&>\"'\u{1b}");
        assert!(
            export(&buffer)
                .unwrap()
                .contains("&lt;&amp;&gt;&quot;&apos;�")
        );
        let symbols = "╭╮╰╯▁▂▃▄▅▆▇█●⠁⠤⣀⣿";
        let mut buffer = Buffer::empty(Rect::new(0, 0, symbols.chars().count() as u16, 1));
        for (index, symbol) in symbols.chars().enumerate() {
            buffer[(index as u16, 0)].set_symbol(&symbol.to_string());
        }
        let svg = export(&buffer).unwrap();
        for symbol in symbols.chars() {
            assert!(svg.contains(&format!(">{symbol}</text>")));
        }
    }

    #[test]
    fn preserves_rgb_backgrounds_bold_and_dim_without_dimming_background() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
        buffer[(0, 0)].set_symbol("█").set_style(
            Style::default()
                .fg(Color::Rgb(1, 2, 3))
                .bg(Color::Rgb(4, 5, 6))
                .add_modifier(Modifier::BOLD | Modifier::DIM),
        );
        let svg = export(&buffer).unwrap();
        assert!(
            svg.contains("<rect x=\"0\" y=\"0\" width=\"10\" height=\"20\" fill=\"#040506\"/>")
        );
        assert!(svg.contains("fill=\"#010203\" transform=\"scale(0.833333 1)\" font-size=\"20\" font-weight=\"bold\" opacity=\"0.5\">█</text>"));
        assert!(
            !svg.lines()
                .find(|line| line.starts_with("<rect"))
                .unwrap()
                .contains("opacity")
        );
    }

    #[test]
    fn maps_ansi_indexed_reverse_and_hidden_colors() {
        assert_eq!(rgb(Color::Red, DEFAULT_FG), ANSI[1]);
        assert_eq!(rgb(Color::Indexed(196), DEFAULT_FG), (255, 0, 0));
        assert_eq!(rgb(Color::Indexed(255), DEFAULT_FG), (238, 238, 238));
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer[(0, 0)].set_symbol("X").set_style(
            Style::default()
                .fg(Color::Red)
                .bg(Color::Blue)
                .add_modifier(Modifier::REVERSED),
        );
        let svg = export(&buffer).unwrap();
        assert!(svg.contains("fill=\"#cd3131\"/>") && svg.contains("fill=\"#2472c8\" transform"));
        buffer[(0, 0)].set_style(Style::default().add_modifier(Modifier::HIDDEN));
        assert!(!export(&buffer).unwrap().contains("<text "));
    }

    #[test]
    fn wide_glyphs_and_combining_clusters_keep_their_cell_span() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        buffer.set_string(0, 0, "界e\u{301}●", Style::default());
        let svg = export(&buffer).unwrap();
        assert!(
            svg.contains("<svg x=\"0\" y=\"0\" width=\"20\" height=\"20\" overflow=\"hidden\">")
        );
        assert!(svg.contains("transform=\"scale(0.9 1)\">界</text>"));
        assert!(svg.contains("<svg x=\"20\" y=\"0\"") && svg.contains(">e\u{301}</text>"));
        assert!(svg.contains("<svg x=\"30\" y=\"0\""));
    }
}
