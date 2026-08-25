use super::*;

pub(super) fn draw(frame: &mut Frame<'_>, app: &mut DashboardApp) {
    let area = frame.area();
    let input_content_width = area.width.saturating_sub(2).max(1);
    let mut input_render = app.input.render(input_content_width);
    let input_height = input_area_height(&input_render, area.height);
    app.input
        .apply_cursor_viewport(&mut input_render, input_height);
    let layout = dashboard_layout(area, input_height);

    draw_title(frame, app, layout.title);
    draw_list(frame, app, layout.list);
    draw_separator(frame, layout.boundary);
    draw_target_status(frame, app, layout.target_status);
    draw_input(frame, &input_render, layout.input);
    draw_actionbar(frame, app, layout.actionbar);
    if app.panel_diagnostic_open {
        render_panel_diagnostic(frame, app, area);
    }
}

pub(super) fn panel_diagnostic_area(area: Rect) -> Rect {
    let width = if area.width <= 20 {
        area.width
    } else {
        area.width.saturating_sub(4).min(100).max(20)
    };
    let height = if area.height <= 8 {
        area.height
    } else {
        area.height.saturating_sub(4).min(24).max(8)
    };
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

pub(super) fn render_panel_diagnostic(frame: &mut Frame<'_>, app: &DashboardApp, area: Rect) {
    let Some(diagnostic) = app.panel_diagnostic.as_ref() else {
        return;
    };
    let popup_area = panel_diagnostic_area(area);
    let title = format!(" {} ", diagnostic.title);
    let text = format!("{}\n\nF2/Esc: close", diagnostic.details);
    let paragraph = Paragraph::new(text)
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(Clear, popup_area);
    frame.render_widget(paragraph, popup_area);
}

pub(super) fn input_area_height(render: &crate::input::InputRender, terminal_height: u16) -> u16 {
    let needed = render.lines.len().max(1) as u16;
    let cap = (terminal_height / 3).max(1).min(10);
    needed.clamp(1, cap)
}

pub(super) fn draw_title(frame: &mut Frame<'_>, app: &DashboardApp, area: Rect) {
    frame.render_widget(Paragraph::new(title_line(app)), area);
}

pub(super) fn title_line(app: &DashboardApp) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "workspace dashboard",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    if let Some(companion) = &app.panel.header.companion {
        spans.push(Span::styled(
            " · companion ",
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            companion.status.label(),
            companion_status_style(companion.status),
        ));
        if let Some(detail) = companion.detail.as_deref() {
            spans.push(Span::styled(
                format!(" ({detail})"),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    if let Some(orchestrator) = &app.panel.header.orchestrator {
        spans.push(Span::styled(
            " · orchestrator ",
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            orchestrator.status.label(),
            orchestrator_status_style(orchestrator.status),
        ));
    }
    Line::from(spans)
}

pub(super) fn companion_status_style(status: CompanionPanelStatus) -> Style {
    match status {
        CompanionPanelStatus::Live
        | CompanionPanelStatus::Restored
        | CompanionPanelStatus::Spawned => Style::default().fg(Color::Green),
        CompanionPanelStatus::Stopped | CompanionPanelStatus::Missing => {
            Style::default().fg(Color::Yellow)
        }
        CompanionPanelStatus::Unavailable => Style::default().fg(Color::Red),
    }
}

pub(super) fn orchestrator_status_style(status: OrchestratorPanelStatus) -> Style {
    match status {
        OrchestratorPanelStatus::Live
        | OrchestratorPanelStatus::Restored
        | OrchestratorPanelStatus::Spawned => Style::default().fg(Color::Green),
        OrchestratorPanelStatus::Stopped | OrchestratorPanelStatus::Missing => {
            Style::default().fg(Color::Yellow)
        }
        OrchestratorPanelStatus::Unavailable => Style::default().fg(Color::Red),
    }
}

pub(super) fn draw_list(frame: &mut Frame<'_>, app: &mut DashboardApp, area: Rect) {
    if area.width == 0 || area.height == 0 {
        app.row_hit_boxes.clear();
        return;
    }
    let rows = list_rows(app, area.width, area.height);
    app.set_row_hit_boxes(&rows, area);
    let lines = rows.into_iter().map(|row| row.line).collect::<Vec<_>>();
    Paragraph::new(lines).render(area, frame.buffer_mut());
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PanelListRow {
    pub(super) line: Line<'static>,
    pub(super) key: Option<PanelRowKey>,
}

impl PanelListRow {
    fn inert(line: Line<'static>) -> Self {
        Self { line, key: None }
    }

    fn selectable(line: Line<'static>, key: PanelRowKey) -> Self {
        Self {
            line,
            key: Some(key),
        }
    }
}

#[cfg(test)]
pub(super) fn list_lines(app: &DashboardApp, width: u16, height: u16) -> Vec<Line<'static>> {
    list_rows(app, width, height)
        .into_iter()
        .map(|row| row.line)
        .collect()
}

pub(super) fn list_rows(app: &DashboardApp, width: u16, height: u16) -> Vec<PanelListRow> {
    let sections = sectioned_entries(&app.list);
    let selected = app.selected_row.as_ref();
    let diagnostic_rows = panel_diagnostic_lines(&app.panel, width)
        .into_iter()
        .map(PanelListRow::inert)
        .collect::<Vec<_>>();
    let action_rows = panel_action_rows(&app.panel, selected, width);
    let live_rows = sections
        .iter()
        .filter(|section| section.kind != DashboardSectionKind::Closed)
        .flat_map(|section| section_rows(&app.list, section, selected, width))
        .collect::<Vec<_>>();
    let closed_rows = sections
        .iter()
        .find(|section| section.kind == DashboardSectionKind::Closed)
        .map(|section| section_rows(&app.list, section, selected, width))
        .unwrap_or_default();

    let available = height as usize;
    let diagnostic_len = diagnostic_rows.len().min(available);
    let remaining_after_diagnostics = available.saturating_sub(diagnostic_len);
    let action_len = action_rows.len().min(remaining_after_diagnostics);
    let remaining_after_actions = remaining_after_diagnostics.saturating_sub(action_len);
    let closed_len = closed_rows.len().min(remaining_after_actions);
    let live_len = live_rows
        .len()
        .min(remaining_after_actions.saturating_sub(closed_len));
    let spacer_len = available.saturating_sub(diagnostic_len + action_len + live_len + closed_len);

    let mut rows = Vec::with_capacity(available);
    rows.extend(diagnostic_rows.into_iter().take(diagnostic_len));
    rows.extend(action_rows.into_iter().take(action_len));
    rows.extend(live_rows.into_iter().take(live_len));
    rows.extend(
        std::iter::repeat_with(|| PanelListRow::inert(Line::from(Span::raw("")))).take(spacer_len),
    );
    rows.extend(closed_rows.into_iter().take(closed_len));
    rows
}

pub(super) fn row_hit_boxes(rows: &[PanelListRow], area: Rect) -> Vec<PanelRowHitBox> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }

    let mut hit_boxes: Vec<PanelRowHitBox> = Vec::new();
    for (offset, row) in rows.iter().enumerate() {
        let Some(key) = row.key.clone() else {
            continue;
        };
        let Some(y) = area.y.checked_add(offset as u16) else {
            continue;
        };
        if y >= area.y.saturating_add(area.height) {
            continue;
        }
        if let Some(last) = hit_boxes.last_mut() {
            if last.key == key
                && last.rect.x == area.x
                && last.rect.width == area.width
                && last.rect.y.saturating_add(last.rect.height) == y
            {
                last.rect.height = last.rect.height.saturating_add(1);
                continue;
            }
        }
        hit_boxes.push(PanelRowHitBox {
            rect: Rect::new(area.x, y, area.width, 1),
            key,
        });
    }
    hit_boxes
}

pub(super) fn panel_diagnostic_lines(
    panel: &WorkspacePanelViewModel,
    width: u16,
) -> Vec<Line<'static>> {
    panel
        .header
        .diagnostics
        .iter()
        .map(|diagnostic| {
            Line::from(vec![
                Span::styled("⚠ ", Style::default().fg(Color::Yellow)),
                Span::styled(
                    truncate_with_ellipsis(diagnostic, width.saturating_sub(2) as usize),
                    Style::default().fg(Color::Yellow),
                ),
            ])
        })
        .collect()
}

pub(super) fn panel_action_rows(
    panel: &WorkspacePanelViewModel,
    selected: Option<&PanelRowKey>,
    width: u16,
) -> Vec<PanelListRow> {
    let rows = panel
        .rows
        .iter()
        .filter(|row| row.is_ticket_section_row())
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity((rows.len() * 2) + 1);
    lines.push(PanelListRow::inert(panel_action_header_line(
        rows.len(),
        width,
    )));
    for row in rows {
        for line in panel_row_lines(row, selected == Some(&row.key), width) {
            lines.push(PanelListRow::selectable(line, row.key.clone()));
        }
    }
    lines
}

pub(super) fn panel_action_header_line(total: usize, width: u16) -> Line<'static> {
    let detail = if total == 1 {
        " 1 row".to_string()
    } else {
        format!(" {total} rows")
    };
    let text = truncate_with_ellipsis(&format!("--tickets{detail}---"), width as usize);
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}

pub(super) const TICKET_STATE_COLUMN_WIDTH: usize = 10;
pub(super) const POD_STATUS_COLUMN_WIDTH: usize = 18;

pub(super) fn panel_row_lines(row: &PanelRow, selected: bool, width: u16) -> Vec<Line<'static>> {
    if row.kind == PanelRowKind::TicketIntakeWorker {
        vec![panel_intake_child_line(row, selected, width)]
    } else {
        vec![
            panel_row_title_line(row, selected, width),
            panel_row_detail_line(row, selected, width),
        ]
    }
}

pub(super) fn panel_row_title_line(row: &PanelRow, selected: bool, width: u16) -> Line<'static> {
    let title_style = if selected {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Magenta)
    };
    let mut spans = Vec::new();
    let mut remaining = width as usize;

    push_ticket_primary_marker_span(&mut spans, selected, &mut remaining);
    push_column_span(
        &mut spans,
        &row.status,
        TICKET_STATE_COLUMN_WIDTH,
        panel_priority_style(row.priority),
        &mut remaining,
    );
    push_bounded_span(&mut spans, row.title.as_str(), title_style, &mut remaining);

    Line::from(spans)
}

pub(super) fn panel_intake_child_line(row: &PanelRow, selected: bool, width: u16) -> Line<'static> {
    let title_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let mut spans = Vec::new();
    let mut remaining = width as usize;

    push_intake_child_marker_span(&mut spans, selected, &mut remaining);
    push_column_span(
        &mut spans,
        &row.status,
        TICKET_STATE_COLUMN_WIDTH,
        intake_status_style(&row.status),
        &mut remaining,
    );
    push_bounded_span(&mut spans, row.title.as_str(), title_style, &mut remaining);

    Line::from(spans)
}

pub(super) fn panel_row_detail_line(row: &PanelRow, selected: bool, width: u16) -> Line<'static> {
    let mut spans = Vec::new();
    let mut remaining = width as usize;

    push_ticket_detail_marker_span(&mut spans, selected, &mut remaining);
    push_bounded_span(
        &mut spans,
        "meta ",
        Style::default().fg(Color::DarkGray),
        &mut remaining,
    );
    push_bounded_span(
        &mut spans,
        &panel_ticket_detail(row),
        ticket_detail_style(row),
        &mut remaining,
    );

    Line::from(spans)
}

pub(super) fn push_ticket_primary_marker_span(
    spans: &mut Vec<Span<'static>>,
    selected: bool,
    remaining: &mut usize,
) {
    let (marker, style) = if selected {
        (
            "▶ ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ("  ", Style::default().fg(Color::DarkGray))
    };
    push_bounded_span(spans, marker, style, remaining);
}

pub(super) fn push_ticket_detail_marker_span(
    spans: &mut Vec<Span<'static>>,
    selected: bool,
    remaining: &mut usize,
) {
    let (marker, style) = if selected {
        (
            "│ ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ("  ", Style::default().fg(Color::DarkGray))
    };
    push_bounded_span(spans, marker, style, remaining);
}

pub(super) fn push_intake_child_marker_span(
    spans: &mut Vec<Span<'static>>,
    selected: bool,
    remaining: &mut usize,
) {
    let (marker, style) = if selected {
        (
            "  ▶ ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ("  └ ", Style::default().fg(Color::DarkGray))
    };
    push_bounded_span(spans, marker, style, remaining);
}

pub(super) fn panel_ticket_detail(row: &PanelRow) -> String {
    if row.kind == PanelRowKind::InvalidTicket {
        let mut parts = vec![panel_ticket_reference(row), "Gate: unavailable".to_string()];
        if let Some(reason) = panel_ticket_reason(row) {
            parts.push(format!("Reason: {reason}"));
        }
        return parts.join(" · ");
    }

    if row.kind == PanelRowKind::TicketIntakeWorker {
        let mut parts = row
            .subtitle
            .as_ref()
            .map(|subtitle| vec![subtitle.clone()])
            .unwrap_or_else(|| vec![panel_ticket_reference(row)]);
        if let Some(action) = row.next_action {
            parts.push(format!("Action: {}", action.label()));
        }
        if let Some(reason) = panel_ticket_reason(row) {
            parts.push(format!("Reason: {reason}"));
        }
        return parts.join(" · ");
    }

    let mut parts = vec![panel_ticket_reference(row)];
    if let Some(overlay_detail) = panel_ticket_overlay_detail(row) {
        parts.push(overlay_detail);
    }
    if let Some(blocked_reason) = row
        .ticket
        .as_ref()
        .and_then(|ticket| ticket.blocked_reason.as_deref())
    {
        parts.push(format!("Dependencies: {blocked_reason}"));
    } else {
        parts.push("Gate: clear".to_string());
    }
    if let Some(action) = row.next_action {
        parts.push(format!(
            "Action: {}",
            panel_ticket_action_label(row, action)
        ));
    }
    if let Some(reason) = panel_ticket_reason(row) {
        parts.push(format!("Reason: {reason}"));
    }
    parts.join(" · ")
}

pub(super) fn panel_ticket_action_label(row: &PanelRow, action: NextUserAction) -> &'static str {
    if action == NextUserAction::Wait
        && row
            .ticket
            .as_ref()
            .and_then(|ticket| ticket.blocked_reason.as_ref())
            .is_some()
    {
        "queue disabled"
    } else {
        action.label()
    }
}

pub(super) fn panel_ticket_overlay_detail(row: &PanelRow) -> Option<String> {
    let ticket = row.ticket.as_ref()?;
    let overlay = ticket.orchestration_overlay.as_ref()?;
    let mut detail = format!(
        "Overlay: local {} · {} {}",
        ticket.workflow_state.as_str(),
        overlay.source,
        overlay.workflow_state.as_str()
    );
    if matches!(
        overlay.workflow_state,
        TicketWorkflowState::Done | TicketWorkflowState::Closed
    ) {
        detail.push_str(" · merge pending");
    }
    Some(detail)
}

pub(super) fn panel_ticket_reason(row: &PanelRow) -> Option<&str> {
    row.disabled_reason
        .as_deref()
        .or_else(|| row.key_hint.as_deref())
}

pub(super) fn ticket_detail_style(row: &PanelRow) -> Style {
    if row.kind == PanelRowKind::InvalidTicket {
        return Style::default().fg(Color::Yellow);
    }
    if row
        .ticket
        .as_ref()
        .and_then(|ticket| ticket.blocked_reason.as_ref())
        .is_some()
    {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

pub(super) fn panel_ticket_reference(row: &PanelRow) -> String {
    row.ticket
        .as_ref()
        .map(|ticket| {
            ticket
                .resource_key
                .clone()
                .unwrap_or_else(|| "resource key unavailable".to_string())
        })
        .unwrap_or_else(|| match &row.key {
            PanelRowKey::Ticket(id) | PanelRowKey::InvalidTicket(id) => id.clone(),
            PanelRowKey::TicketIntakeWorker { ticket_id, .. } => ticket_id.clone(),
            PanelRowKey::Worker(name) => name.clone(),
        })
}

pub(super) fn push_column_span(
    spans: &mut Vec<Span<'static>>,
    value: &str,
    column_width: usize,
    style: Style,
    remaining: &mut usize,
) {
    if *remaining == 0 {
        return;
    }
    let mut content = padded_cell(value, column_width);
    content.push(' ');
    push_bounded_span(spans, &content, style, remaining);
}

pub(super) fn push_bounded_span(
    spans: &mut Vec<Span<'static>>,
    value: &str,
    style: Style,
    remaining: &mut usize,
) {
    if *remaining == 0 || value.is_empty() {
        return;
    }
    let content = truncate_with_ellipsis(value, *remaining);
    *remaining = remaining.saturating_sub(content.width());
    spans.push(Span::styled(content, style));
}

pub(super) fn padded_cell(value: &str, width: usize) -> String {
    let mut cell = truncate_with_ellipsis(value, width);
    let padding = width.saturating_sub(cell.width());
    cell.extend(std::iter::repeat_n(' ', padding));
    cell
}

pub(super) fn panel_priority_style(priority: ActionPriority) -> Style {
    match priority {
        ActionPriority::ReadyForQueue => Style::default().fg(Color::Green),
        ActionPriority::ActiveWork => Style::default().fg(Color::Cyan),
        ActionPriority::Background => Style::default().fg(Color::DarkGray),
    }
}

pub(super) fn intake_status_style(status: &str) -> Style {
    match status {
        "live" => Style::default().fg(Color::Green),
        "restorable" => Style::default().fg(Color::Yellow),
        "stale" => Style::default().fg(Color::DarkGray),
        _ => Style::default().fg(Color::Cyan),
    }
}

pub(super) fn section_rows(
    list: &WorkerList,
    section: &DashboardSection,
    selected: Option<&PanelRowKey>,
    width: u16,
) -> Vec<PanelListRow> {
    let visible = visible_section_indices(section);
    if visible.is_empty() {
        return Vec::new();
    }

    let mut rows = Vec::with_capacity(visible.len() + 1);
    rows.push(PanelListRow::inert(section_header_line(
        section.kind,
        section.entries.len(),
        section.hidden_count(),
        width,
    )));
    for index in visible {
        if let Some(entry) = list.entries.get(index) {
            let key = PanelRowKey::Worker(entry.name.clone());
            let selected = selected == Some(&key);
            rows.push(PanelListRow::selectable(
                row_line(entry, selected, width),
                key,
            ));
        }
    }
    rows
}

pub(super) fn row_line(entry: &WorkerListEntry, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { "▶ " } else { "  " };
    let name_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let (status, status_style) = row_status_label(entry);
    let mut spans = Vec::new();
    let mut remaining = width as usize;

    push_bounded_span(
        &mut spans,
        marker,
        if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        },
        &mut remaining,
    );
    push_column_span(
        &mut spans,
        status,
        POD_STATUS_COLUMN_WIDTH,
        status_style,
        &mut remaining,
    );
    push_bounded_span(&mut spans, entry.name.as_str(), name_style, &mut remaining);

    Line::from(spans)
}

pub(super) fn draw_separator(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

pub(super) fn draw_target_status(frame: &mut Frame<'_>, app: &DashboardApp, area: Rect) {
    frame.render_widget(Paragraph::new(target_status_line(app)), area);
}

pub(super) fn target_status_line(_app: &DashboardApp) -> Line<'static> {
    Line::from(Span::raw(""))
}

pub(super) fn draw_input(frame: &mut Frame<'_>, render: &crate::input::InputRender, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(render.lines.len());
    for (i, src) in render.lines.iter().enumerate() {
        let absolute_row = render.viewport_start_row as usize + i;
        let prefix = if absolute_row == 0 { "> " } else { "  " };
        let mut spans = vec![Span::styled(prefix, Style::default().fg(Color::DarkGray))];
        spans.extend(src.spans.iter().cloned());
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);

    let cursor_x = area.x + 2 + render.cursor_col;
    let cursor_y = area.y + render.cursor_row;
    if cursor_y < area.y + area.height {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

pub(super) fn actionbar_left_text(app: &DashboardApp) -> String {
    if app.sending && app.composer_target() == ComposerTarget::TicketIntake {
        "launching Ticket Intake…".to_string()
    } else if app.sending {
        "working…".to_string()
    } else if app.refreshing {
        match app.notice.as_deref() {
            Some(notice) if notice.contains("Refreshing") || notice.contains("refreshing") => {
                notice.to_string()
            }
            Some(notice) => format!("{notice} Refreshing workspace…"),
            None => "Refreshing workspace…".to_string(),
        }
    } else if let Some(notice) = app.notice.as_deref() {
        notice.to_string()
    } else {
        String::new()
    }
}

pub(super) fn actionbar_right_text(app: &DashboardApp) -> &'static str {
    if app.panel_diagnostic_open {
        "F2/Esc close details"
    } else if app.panel_diagnostic.is_some() {
        "F2 details"
    } else {
        ""
    }
}

pub(super) fn draw_actionbar(frame: &mut Frame<'_>, app: &DashboardApp, area: Rect) {
    let left = actionbar_left_text(app);
    let right = actionbar_right_text(app);
    let left_width = area
        .width
        .saturating_sub(right.width() as u16)
        .saturating_sub(2) as usize;
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate_with_ellipsis(&left, left_width),
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            right,
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(ratatui::layout::Alignment::Right),
        area,
    );
}

pub(super) fn truncate_with_ellipsis(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if width + cw > max_width - 1 {
            break;
        }
        out.push(c);
        width += cw;
    }
    out.push('…');
    out
}
