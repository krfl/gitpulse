use ratatui::prelude::*;
use ratatui::widgets::{
    Block, BorderType, Borders, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use crate::app::AppState;
use crate::model::{FetchStatus, ForgeStatus, Repo, SyncState};

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Find the smallest scroll offset such that the card at `selected` fits in `viewport`.
pub(crate) fn compute_scroll_offset(heights: &[u16], selected: usize, viewport: u16) -> usize {
    if heights.is_empty() {
        return 0;
    }
    let selected = selected.min(heights.len() - 1);
    let mut offset = 0;
    loop {
        let visible_height: u16 = heights[offset..=selected].iter().sum();
        if visible_height <= viewport {
            break;
        }
        offset += 1;
        if offset > selected {
            return selected; // card itself taller than viewport; show it at top
        }
    }
    offset
}

pub(crate) fn render_board(frame: &mut Frame, state: &AppState, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    for (i, &col_state) in SyncState::ALL.iter().enumerate() {
        let repos = state.repos_in_column(i);
        let is_selected = state.selected_column == i;
        let title = format!(" {} ({}) ", col_state.label(), repos.len());

        render_column(
            frame,
            &ColumnCtx {
                repos: &repos,
                title: &title,
                is_selected,
                selected_index: state.selected_index[i],
                spinner_index: state.spinner_index(),
            },
            columns[i],
        );
    }
}

struct ColumnCtx<'a> {
    repos: &'a [&'a Repo],
    title: &'a str,
    is_selected: bool,
    selected_index: usize,
    spinner_index: usize,
}

fn render_column(frame: &mut Frame, ctx: &ColumnCtx<'_>, area: Rect) {
    let ColumnCtx {
        repos,
        title,
        is_selected,
        selected_index,
        spinner_index,
    } = *ctx;
    let border_style = if is_selected {
        Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || repos.is_empty() {
        return;
    }

    let heights: Vec<u16> = repos.iter().map(|r| r.card_height()).collect();
    let scroll_offset = compute_scroll_offset(&heights, selected_index, inner.height);

    let mut y = inner.y;
    for (j, repo) in repos.iter().enumerate().skip(scroll_offset) {
        let h = repo.card_height();
        if y + h > inner.y + inner.height {
            break;
        }

        let card_area = Rect::new(inner.x, y, inner.width, h);
        let is_card_selected = is_selected && selected_index == j;
        render_card(
            frame,
            repo,
            card_area,
            is_card_selected,
            is_selected,
            spinner_index,
        );

        y += h;
    }

    let total_height: u16 = heights.iter().sum();
    if total_height > inner.height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some(" "))
            .track_style(border_style)
            .thumb_symbol("▐")
            .thumb_style(border_style)
            .begin_symbol(None)
            .end_symbol(None);
        let mut scrollbar_state = ScrollbarState::new(repos.len()).position(selected_index);
        let scrollbar_area = area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        });
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

fn render_card(
    frame: &mut Frame,
    repo: &Repo,
    area: Rect,
    selected: bool,
    in_focused_column: bool,
    spinner_index: usize,
) {
    let border_type = if selected {
        BorderType::Thick
    } else {
        BorderType::Rounded
    };
    let border_style = if selected {
        Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD)
    } else if in_focused_column {
        Style::default().fg(Color::Reset)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let name_color = status_color(repo);
    let title = build_card_title(repo, name_color);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    render_branch_line(frame, repo, inner, spinner_index);

    if inner.height >= 2 {
        render_stats_line(frame, repo, inner);
    }

    if inner.height >= 3 {
        render_forge_line(frame, repo, inner);
    }

    render_worktree_lines(frame, repo, inner);
}

/// Line 1: branch info + fetch indicator.
fn render_branch_line(frame: &mut Frame, repo: &Repo, inner: Rect, spinner_index: usize) {
    let fetch_indicator = match &repo.fetch_status {
        FetchStatus::Queued => " ⏳".to_string(),
        FetchStatus::Fetching => {
            let idx = spinner_index % SPINNER.len();
            format!(" {}", SPINNER[idx])
        }
        FetchStatus::Done => String::new(),
        FetchStatus::Failed(_) => " ✗".to_string(),
    };

    let mut parts: Vec<Span> = Vec::new();
    parts.push(Span::styled(
        format!("branch:{}", repo.current_branch),
        Style::default().fg(Color::White),
    ));
    if let Some(default) = &repo.default_branch {
        if *default != repo.current_branch {
            parts.push(Span::raw("  "));
            parts.push(Span::styled(
                format!("default:{default}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    if !fetch_indicator.is_empty() {
        parts.push(Span::styled(
            fetch_indicator,
            Style::default().fg(Color::Yellow),
        ));
    }

    let line = Line::from(parts);
    frame.render_widget(line, Rect::new(inner.x, inner.y, inner.width, 1));
}

/// Line 2: git stats + secondary state tags.
fn render_stats_line(frame: &mut Frame, repo: &Repo, inner: Rect) {
    let mut parts: Vec<Span> = Vec::new();
    if repo.behind > 0 {
        parts.push(Span::styled(
            format!("↓{}", repo.behind),
            Style::default().fg(Color::Red),
        ));
        parts.push(Span::raw(" "));
    }
    if repo.ahead > 0 {
        parts.push(Span::styled(
            format!("↑{}", repo.ahead),
            Style::default().fg(Color::Green),
        ));
        parts.push(Span::raw(" "));
    }
    if repo.dirty_files > 0 {
        parts.push(Span::styled(
            format!("*{}", repo.dirty_files),
            Style::default().fg(Color::Yellow),
        ));
        parts.push(Span::raw(" "));
    }

    for secondary in repo.secondary_states() {
        parts.push(Span::styled(
            format!("[{}]", secondary.label().to_lowercase()),
            Style::default().fg(Color::DarkGray),
        ));
        parts.push(Span::raw(" "));
    }

    if parts.is_empty() {
        parts.push(Span::styled("✓", Style::default().fg(Color::Green)));
    }

    let line = Line::from(parts);
    frame.render_widget(line, Rect::new(inner.x, inner.y + 1, inner.width, 1));
}

/// Line 3: forge stats (PRs, Issues, Fork).
fn render_forge_line(frame: &mut Frame, repo: &Repo, inner: Rect) {
    let line = match &repo.forge_status {
        ForgeStatus::Done => {
            if let Some(stats) = &repo.forge_stats {
                let mut spans = vec![
                    Span::styled(
                        format!("PR {}", stats.open_prs),
                        Style::default().fg(Color::Green),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("Issues {}", stats.open_issues),
                        Style::default().fg(Color::Red),
                    ),
                ];
                if stats.is_fork {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled("Fork", Style::default().fg(Color::DarkGray)));
                }
                Line::from(spans)
            } else {
                Line::from("")
            }
        }
        ForgeStatus::Queued | ForgeStatus::Fetching => Line::from(Span::styled(
            "PR ...  Issues ...",
            Style::default().fg(Color::DarkGray),
        )),
        ForgeStatus::Failed(_) => Line::from(Span::styled(
            "err",
            Style::default().fg(Color::Red),
        )),
        ForgeStatus::NotApplicable => Line::from(""),
    };
    frame.render_widget(line, Rect::new(inner.x, inner.y + 2, inner.width, 1));
}

/// Lines 4+: worktree sub-items.
fn render_worktree_lines(frame: &mut Frame, repo: &Repo, inner: Rect) {
    for (i, wt) in repo.worktrees.iter().enumerate() {
        let row = 3 + i as u16;
        if inner.height <= row {
            break;
        }
        let dirty = if wt.dirty_files > 0 {
            Span::styled(
                format!("  *{}", wt.dirty_files),
                Style::default().fg(Color::Yellow),
            )
        } else {
            Span::styled("  ✓", Style::default().fg(Color::Green))
        };
        let wt_line = Line::from(vec![
            Span::styled("↳ ", Style::default().fg(Color::DarkGray)),
            Span::styled(&wt.branch, Style::default().fg(Color::Cyan)),
            dirty,
        ]);
        frame.render_widget(
            wt_line,
            Rect::new(inner.x, inner.y + row, inner.width, 1),
        );
    }
}

/// Color for repo name based on sync status.
fn status_color(repo: &Repo) -> Color {
    if !repo.has_remote {
        return Color::Yellow;
    }
    match repo.sync_state() {
        SyncState::Behind => Color::Red,
        SyncState::Uncommitted => Color::Yellow,
        SyncState::Ahead => Color::Green,
        SyncState::InSync => Color::Reset,
    }
}

/// Build the card title with colored repo name.
/// Normal repo: " reponame (+N wt) ⚠ "
/// Worktree:    " reponame [branch] "
fn build_card_title(repo: &Repo, name_color: Color) -> Line<'_> {
    let name_style = Style::default().fg(name_color).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];

    if repo.is_worktree {
        // Worktree: show "parentrepo [branch]"
        let parent_name = repo
            .worktree_main
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| repo.name.clone());
        spans.push(Span::styled(parent_name, name_style));
        spans.push(Span::styled(
            format!(" [{}]", repo.current_branch),
            Style::default().fg(Color::Cyan),
        ));
    } else {
        spans.push(Span::styled(&repo.name, name_style));
    }

    if !repo.is_worktree && !repo.worktrees.is_empty() {
        spans.push(Span::styled(
            format!(" (+{} wt)", repo.worktrees.len()),
            Style::default().fg(Color::DarkGray),
        ));
    }

    if !repo.has_remote {
        spans.push(Span::styled(" ⚠", Style::default().fg(Color::Yellow)));
    }

    spans.push(Span::raw(" "));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_offset_selected_at_top() {
        let heights = vec![5, 5, 5, 5];
        assert_eq!(compute_scroll_offset(&heights, 0, 20), 0);
    }

    #[test]
    fn scroll_offset_selected_fits() {
        let heights = vec![5, 5, 5, 5];
        // viewport=15, cards 0-2 fit (15 total), selected=2
        assert_eq!(compute_scroll_offset(&heights, 2, 15), 0);
    }

    #[test]
    fn scroll_offset_selected_needs_scroll() {
        let heights = vec![5, 5, 5, 5];
        // viewport=10, only 2 cards fit, selected=3
        assert_eq!(compute_scroll_offset(&heights, 3, 10), 2);
    }

    #[test]
    fn scroll_offset_mixed_heights() {
        let heights = vec![5, 7, 5, 6]; // total 23
        // viewport=12, selected=2: heights[1..=2] = 7+5 = 12, fits from offset 1
        assert_eq!(compute_scroll_offset(&heights, 2, 12), 1);
    }

    #[test]
    fn scroll_offset_card_taller_than_viewport() {
        let heights = vec![5, 20, 5];
        // Card 1 is 20, viewport is 10 — offset becomes selected itself
        assert_eq!(compute_scroll_offset(&heights, 1, 10), 1);
    }

    #[test]
    fn scroll_offset_empty() {
        assert_eq!(compute_scroll_offset(&[], 0, 20), 0);
    }

    #[test]
    fn scroll_offset_single_card() {
        assert_eq!(compute_scroll_offset(&[5], 0, 20), 0);
        assert_eq!(compute_scroll_offset(&[5], 0, 3), 0);
    }
}
