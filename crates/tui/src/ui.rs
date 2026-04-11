use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, MessageKind};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Min(1),    // messages (scroll area)
        Constraint::Length(1), // separator
        Constraint::Length(1), // status line
        Constraint::Length(1), // input
    ])
    .split(frame.area());

    draw_messages(frame, app, chunks[0]);
    draw_separator(frame, chunks[1]);
    draw_status(frame, app, chunks[2]);
    draw_input(frame, app, chunks[3]);
}

fn draw_messages(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let display = app.display_lines();

    let lines: Vec<Line> = display
        .iter()
        .flat_map(|(kind, content)| {
            let style = kind_style(kind);
            content
                .lines()
                .map(move |l| Line::from(Span::styled(l.to_owned(), style)))
        })
        .collect();

    let total = lines.len() as u16;
    let max_scroll = total.saturating_sub(area.height);
    if app.scroll > max_scroll {
        app.scroll = max_scroll;
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));

    frame.render_widget(paragraph, area);
}

fn draw_separator(frame: &mut Frame, area: ratatui::layout::Rect) {
    let line = "─".repeat(area.width as usize);
    let paragraph = Paragraph::new(Line::from(Span::styled(
        line,
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(paragraph, area);
}

fn draw_status(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let conn = if app.connected {
        Span::styled("●", Style::default().fg(Color::Green))
    } else {
        Span::styled("○", Style::default().fg(Color::Red))
    };

    let mut spans = vec![
        conn,
        Span::raw(" "),
        Span::styled(
            &app.pod_name,
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];

    if !app.status_line.is_empty() {
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(
            &app.status_line,
            Style::default().fg(Color::Yellow),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_input(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let line = Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::DarkGray)),
        Span::raw(&app.input),
    ]);
    frame.render_widget(Paragraph::new(line), area);

    let cursor_x = area.x + 2 + app.input[..app.cursor].chars().count() as u16;
    let cursor_y = area.y;
    frame.set_cursor_position(Position::new(cursor_x, cursor_y));
}

fn kind_style(kind: &MessageKind) -> Style {
    match kind {
        MessageKind::User => Style::default().fg(Color::Green),
        MessageKind::Assistant => Style::default().fg(Color::White),
        MessageKind::Tool => Style::default().fg(Color::Cyan),
        MessageKind::Error => Style::default().fg(Color::Red),
    }
}
