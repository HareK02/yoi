use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Wrap};
use ratatui::Frame;
use tui_scrollview::{ScrollView, ScrollbarVisibility};

use crate::app::{fmt_tokens, App, MessageKind};

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

fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width;
    let padded_inner = width.saturating_sub(1); // content width inside Block::padding(left=1)

    // Build segments: (is_padded, lines, wrapped_height)
    struct Seg<'a> {
        lines: Vec<Line<'a>>,
        padded: bool,
        height: u16,
    }

    let mut segs: Vec<Seg> = Vec::new();
    let mut content: Vec<Line> = Vec::new();

    macro_rules! flush_content {
        () => {
            if !content.is_empty() {
                let h = wrapped_height(&content, padded_inner);
                segs.push(Seg { lines: std::mem::take(&mut content), padded: true, height: h });
            }
        };
    }

    for msg in &app.messages {
        let style = kind_style(&msg.kind);
        match msg.kind {
            MessageKind::TurnHeader => {
                flush_content!();
                if !segs.is_empty() {
                    segs.push(Seg { lines: vec![Line::raw("")], padded: false, height: 1 });
                }
                let lines = vec![Line::from(Span::styled(msg.content.clone(), style))];
                segs.push(Seg { lines, padded: false, height: 1 });
            }
            MessageKind::TurnStats => {
                flush_content!();
                let lines: Vec<Line> = msg.content.lines()
                    .map(|l| Line::from(Span::styled(l.to_owned(), style)).alignment(Alignment::Right))
                    .collect();
                let h = wrapped_height(&lines, padded_inner);
                segs.push(Seg { lines, padded: true, height: h });
                segs.push(Seg { lines: vec![Line::raw("")], padded: false, height: 1 });
            }
            MessageKind::User => {
                for l in msg.content.lines() {
                    content.push(Line::from(Span::styled(l.to_owned(), style)));
                }
                content.push(Line::raw(""));
            }
            _ => {
                for l in msg.content.lines() {
                    content.push(Line::from(Span::styled(l.to_owned(), style)));
                }
            }
        }
    }

    // In-progress streaming text
    if !app.current_text.is_empty() {
        let style = kind_style(&MessageKind::Assistant);
        for l in app.current_text.lines() {
            content.push(Line::from(Span::styled(l.to_owned(), style)));
        }
    }

    flush_content!();

    // Total content height
    let total_height: u16 = segs.iter().map(|s| s.height).sum();

    // Build ScrollView
    let mut sv = ScrollView::new(Size::new(width, total_height.max(1)))
        .horizontal_scrollbar_visibility(ScrollbarVisibility::Never);

    let mut y: u16 = 0;
    for seg in segs {
        let rect = Rect::new(0, y, width, seg.height);
        if seg.padded {
            sv.render_widget(
                Paragraph::new(seg.lines)
                    .block(Block::default().padding(Padding::left(1)))
                    .wrap(Wrap { trim: false }),
                rect,
            );
        } else {
            sv.render_widget(Paragraph::new(seg.lines), rect);
        }
        y += seg.height;
    }

    frame.render_stateful_widget(sv, area, &mut app.scroll_state);
}

/// Estimate the number of visual rows after wrapping.
fn wrapped_height(lines: &[Line], avail_width: u16) -> u16 {
    if avail_width == 0 {
        return lines.len() as u16;
    }
    lines
        .iter()
        .map(|line| {
            let w = line.width() as u16;
            if w == 0 { 1 } else { w.div_ceil(avail_width) }
        })
        .sum()
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

    if app.running {
        let status = if let Some(tool) = &app.current_tool {
            format!(
                "request: {} | ↑{}/↓{} | tool: {tool}",
                app.run_requests,
                fmt_tokens(app.run_input_tokens),
                fmt_tokens(app.run_output_tokens),
            )
        } else {
            format!(
                "request: {} | ↑{}/↓{}",
                app.run_requests,
                fmt_tokens(app.run_input_tokens),
                fmt_tokens(app.run_output_tokens),
            )
        };
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(status, Style::default().fg(Color::Yellow)));
    } else {
        spans.push(Span::styled(" idle", Style::default().fg(Color::DarkGray)));
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
        MessageKind::TurnHeader => Style::default().fg(Color::DarkGray),
        MessageKind::User => Style::default().fg(Color::Green),
        MessageKind::Assistant => Style::default().fg(Color::White),
        MessageKind::Tool => Style::default().fg(Color::Cyan),
        MessageKind::Error => Style::default().fg(Color::Red),
        MessageKind::TurnStats => Style::default().fg(Color::DarkGray),
    }
}
