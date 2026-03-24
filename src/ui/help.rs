use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

pub(crate) fn render_help(frame: &mut Frame) {
    let area = super::centered_rect(50, 60, frame.area());

    let help_text = vec![
        Line::from(Span::styled(
            "Keybindings",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  h/l  ←/→     Move between columns"),
        Line::from("  j/k  ↑/↓     Move between repos"),
        Line::from("  Tab/S-Tab    Cycle repos across columns"),
        Line::from("  Enter        Open repo detail"),
        Line::from("  r            Refresh (re-scan + fetch)"),
        Line::from("  p            Pull selected repo"),
        Line::from("  P            Push selected repo"),
        Line::from("  s            Open shell in repo dir"),
        Line::from("  v            Cycle sort: Name/PRs/Issues"),
        Line::from("  ?            Toggle this help"),
        Line::from("  q / Esc      Quit / close overlay"),
        Line::from("  Ctrl+c       Force quit"),
    ];

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD));

    frame.render_widget(Clear, area);
    let paragraph = Paragraph::new(help_text).block(block);
    frame.render_widget(paragraph, area);
}
