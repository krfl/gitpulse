mod board_view;
mod help;
mod repo_detail;

use std::path::PathBuf;

use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::{AppState, Overlay};

pub(crate) fn render(frame: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(frame.area());

    board_view::render_board(frame, state, chunks[0]);
    render_status_bar(frame, state, chunks[1]);

    match &state.overlay {
        Overlay::Detail => repo_detail::render_detail(frame, state),
        Overlay::Help => help::render_help(frame),
        Overlay::ShellPicker { paths, index } => render_shell_picker(frame, paths, *index),
        Overlay::Board => {}
    }
}

fn render_status_bar(frame: &mut Frame, state: &AppState, area: Rect) {
    let sort_label = format!("[Sort: {}]", state.sort_mode.label());
    let default_msg = format!(
        "{sort_label}  v sort  ? help  r refresh  p pull  P push  s shell  q quit"
    );
    let msg = state
        .status_message
        .as_deref()
        .unwrap_or(&default_msg);
    let status = Paragraph::new(msg.to_string()).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, area);
}

fn render_shell_picker(frame: &mut Frame, items: &[(String, PathBuf)], selected: usize) {
    if items.is_empty() {
        return;
    }

    let area = frame.area();
    let max_label_len = items.iter().map(|(l, _): &(String, PathBuf)| l.len()).max().unwrap_or(0) as u16;
    let popup_width = max_label_len.saturating_add(6).max(20).min(area.width.saturating_sub(4));
    let popup_height = (items.len() as u16 + 2).max(3).min(area.height.saturating_sub(4));

    // Bottom-right positioning (kando style)
    let popup_area = Rect::new(
        area.x + area.width.saturating_sub(popup_width),
        area.y + area.height.saturating_sub(popup_height),
        popup_width,
        popup_height,
    );

    let block = Block::default()
        .title(Span::styled(
            " Open shell in... ",
            Style::default().add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Reset));

    frame.render_widget(Clear, popup_area);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    for (i, (label, _)) in items.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let style = if i == selected {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(Color::Reset)
        };
        let line = Paragraph::new(format!("  {label}")).style(style);
        frame.render_widget(
            line,
            Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
        );
    }
}

pub(crate) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
