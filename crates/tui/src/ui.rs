use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use protocol::Greeting;

use crate::app::{App, MessageKind, OutputItem, fmt_tokens};

/// Draw the fixed viewport (3 lines: separator, status, input).
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(1), // separator
        Constraint::Length(1), // status
        Constraint::Length(1), // input
    ])
    .split(area);

    draw_separator(frame, chunks[0]);
    draw_status(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);
}

/// Flush queued output items above the inline viewport via insert_before.
pub fn flush_output(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> std::io::Result<()> {
    let items: Vec<OutputItem> = app.output_queue.drain(..).collect();
    if items.is_empty() {
        return Ok(());
    }

    let width = terminal.size()?.width;

    for item in items {
        match item {
            OutputItem::Blank => {
                terminal.insert_before(1, |buf| {
                    // empty line
                    let _ = buf;
                })?;
            }
            OutputItem::TurnHeader(text) => {
                terminal.insert_before(1, |buf| {
                    let style = kind_style(&MessageKind::TurnHeader);
                    Paragraph::new(Line::from(Span::styled(text, style))).render(buf.area, buf);
                })?;
            }
            OutputItem::Padded(kind, text) => {
                let style = kind_style(&kind);
                let lines: Vec<Line> = text
                    .lines()
                    .map(|l| Line::from(Span::styled(l.to_owned(), style)))
                    .collect();
                let height = wrapped_height(&lines, width.saturating_sub(1));
                terminal.insert_before(height, |buf| {
                    Paragraph::new(lines)
                        .block(Block::default().padding(Padding::left(1)))
                        .wrap(Wrap { trim: false })
                        .render(buf.area, buf);
                })?;
            }
            OutputItem::GreetingCard(g) => {
                let lines = greeting_lines(&g);
                let inner_width = width.saturating_sub(4);
                let body_height: u16 = lines
                    .iter()
                    .map(|l| {
                        let w = l.width() as u16;
                        if inner_width == 0 || w == 0 {
                            1
                        } else {
                            w.div_ceil(inner_width)
                        }
                    })
                    .sum();
                let height = body_height + 2; // top + bottom border
                terminal.insert_before(height, |buf| {
                    Paragraph::new(lines)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_type(BorderType::Rounded)
                                .border_style(Style::default().fg(Color::DarkGray))
                                .padding(Padding::horizontal(1)),
                        )
                        .wrap(Wrap { trim: false })
                        .render(buf.area, buf);
                })?;
            }
            OutputItem::PaddedRight(kind, text) => {
                let style = kind_style(&kind);
                let lines: Vec<Line> = text
                    .lines()
                    .map(|l| {
                        Line::from(Span::styled(l.to_owned(), style)).alignment(Alignment::Right)
                    })
                    .collect();
                let height = wrapped_height(&lines, width.saturating_sub(1));
                terminal.insert_before(height, |buf| {
                    Paragraph::new(lines)
                        .block(Block::default().padding(Padding::left(1)))
                        .wrap(Wrap { trim: false })
                        .render(buf.area, buf);
                })?;
            }
        }
    }

    Ok(())
}

fn wrapped_height(lines: &[Line], avail_width: u16) -> u16 {
    if avail_width == 0 {
        return lines.len().max(1) as u16;
    }
    lines
        .iter()
        .map(|line| {
            let w = line.width() as u16;
            if w == 0 { 1 } else { w.div_ceil(avail_width) }
        })
        .sum::<u16>()
        .max(1)
}

fn draw_separator(frame: &mut Frame, area: Rect) {
    let line = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            line,
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let conn = if app.connected {
        Span::styled("●", Style::default().fg(Color::Green))
    } else {
        Span::styled("○", Style::default().fg(Color::Red))
    };

    let mut spans = vec![
        conn,
        Span::raw(" "),
        Span::styled(&app.pod_name, Style::default().add_modifier(Modifier::BOLD)),
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

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let line = Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::DarkGray)),
        Span::raw(&app.input),
    ]);
    frame.render_widget(Paragraph::new(line), area);

    let cursor_x = area.x + 2 + UnicodeWidthStr::width(&app.input[..app.cursor]) as u16;
    let cursor_y = area.y;
    frame.set_cursor_position(Position::new(cursor_x, cursor_y));
}

fn greeting_lines(g: &Greeting) -> Vec<Line<'static>> {
    let label = Style::default().fg(Color::DarkGray);
    let value = Style::default().fg(Color::White);
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(Line::from(Span::styled(
        g.pod_name.clone(),
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("{} ({})", g.model, g.provider),
        Style::default().fg(Color::Cyan),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("cwd:   ", label),
        Span::styled(g.cwd.clone(), value),
    ]));
    lines.push(Line::from(vec![
        Span::styled("tools: ", label),
        Span::styled(g.tools.join(", "), value),
    ]));

    if !g.scope_summary.is_empty() {
        lines.push(Line::from(""));
        for line in g.scope_summary.lines() {
            lines.push(Line::from(Span::styled(line.to_owned(), value)));
        }
    }

    lines
}

pub fn kind_style(kind: &MessageKind) -> Style {
    match kind {
        MessageKind::TurnHeader => Style::default().fg(Color::DarkGray),
        MessageKind::User => Style::default().fg(Color::Green),
        MessageKind::Assistant => Style::default().fg(Color::White),
        MessageKind::Tool => Style::default().fg(Color::Cyan),
        MessageKind::Error => Style::default().fg(Color::Red),
        MessageKind::TurnStats => Style::default().fg(Color::DarkGray),
    }
}

use ratatui::widgets::Widget;
