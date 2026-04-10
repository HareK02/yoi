use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, MessageKind};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .split(frame.area());

    draw_status_bar(frame, app, chunks[0]);
    draw_output(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let conn_style = if app.connected {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Red)
    };
    let conn_text = if app.connected {
        "connected"
    } else {
        "disconnected"
    };

    let line = Line::from(vec![
        Span::styled(&app.pod_name, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" | "),
        Span::styled(conn_text, conn_style),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn draw_output(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let display = app.display_lines();

    let lines: Vec<Line> = display
        .iter()
        .flat_map(|(kind, content)| {
            let style = kind_style(kind);
            content.lines().map(move |l| Line::from(Span::styled(l.to_owned(), style)))
        })
        .collect();

    let total = lines.len() as u16;
    let visible = area.height.saturating_sub(2); // block borders
    let max_scroll = total.saturating_sub(visible);
    if app.scroll > max_scroll {
        app.scroll = max_scroll;
    }

    let block = Block::default().borders(Borders::ALL);
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));

    frame.render_widget(paragraph, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let display = format!("> {}", app.input);
    let block = Block::default().borders(Borders::ALL).title("Input");
    let paragraph = Paragraph::new(display).block(block);
    frame.render_widget(paragraph, area);

    // Cursor position: "> " is 2 chars, plus cursor offset in the input
    let cursor_x = area.x + 1 + 2 + app.input[..app.cursor].chars().count() as u16;
    let cursor_y = area.y + 1;
    frame.set_cursor_position(Position::new(cursor_x, cursor_y));
}

fn kind_style(kind: &MessageKind) -> Style {
    match kind {
        MessageKind::User => Style::default().fg(Color::Green),
        MessageKind::Assistant => Style::default().fg(Color::White),
        MessageKind::Tool => Style::default().fg(Color::Cyan),
        MessageKind::Error => Style::default().fg(Color::Red),
        MessageKind::Status => Style::default().fg(Color::DarkGray),
    }
}
