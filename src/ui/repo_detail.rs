use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::AppState;
use crate::model::{FetchStatus, ForgeStatus};

pub(crate) fn render_detail(frame: &mut Frame, state: &AppState) {
    let Some(repo) = state.selected_repo() else {
        return;
    };

    let area = super::centered_rect(70, 60, frame.area());

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Path: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(repo.path.display().to_string()),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Current branch: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(&repo.current_branch, Style::default().fg(Color::Cyan)),
        ]),
    ];

    if let Some(default) = &repo.default_branch {
        lines.push(Line::from(vec![
            Span::styled(
                "Default branch: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(default, Style::default().fg(Color::Cyan)),
        ]));
    }

    lines.push(Line::from(""));

    if let Some(url) = &repo.remote_url {
        lines.push(Line::from(vec![
            Span::styled("Remote: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(url.as_str()),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Remote: ", Style::default().add_modifier(Modifier::BOLD)),
            if repo.has_remote {
                Span::styled("yes", Style::default().fg(Color::Green))
            } else {
                Span::styled("none ⚠", Style::default().fg(Color::Yellow))
            },
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("Behind: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            repo.behind.to_string(),
            if repo.behind > 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default()
            },
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled("Ahead: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            repo.ahead.to_string(),
            if repo.ahead > 0 {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            },
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(
            "Dirty files: ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            repo.dirty_files.to_string(),
            if repo.dirty_files > 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            },
        ),
    ]));

    lines.push(Line::from(""));

    let fetch_line = match &repo.fetch_status {
        FetchStatus::Queued => Line::from(Span::styled(
            "Fetch: queued",
            Style::default().fg(Color::DarkGray),
        )),
        FetchStatus::Fetching => Line::from(Span::styled(
            "Fetch: in progress...",
            Style::default().fg(Color::Yellow),
        )),
        FetchStatus::Done => Line::from(Span::styled(
            "Fetch: complete",
            Style::default().fg(Color::Green),
        )),
        FetchStatus::Failed(e) => Line::from(Span::styled(
            format!("Fetch: failed — {e}"),
            Style::default().fg(Color::Red),
        )),
    };
    lines.push(fetch_line);

    // Forge stats
    if let Some(info) = &repo.remote_info {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Forge: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                "{} ({}/{})",
                info.kind.label(),
                info.owner,
                info.repo_name
            )),
        ]));

        match &repo.forge_status {
            ForgeStatus::Done => {
                if let Some(stats) = &repo.forge_stats {
                    if stats.is_fork {
                        lines.push(Line::from(vec![
                            Span::styled(
                                "Fork: ",
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::styled("yes", Style::default().fg(Color::DarkGray)),
                        ]));
                    }
                    lines.push(Line::from(vec![
                        Span::styled(
                            "Pull Requests: ",
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            stats.open_prs.to_string(),
                            Style::default().fg(Color::Green),
                        ),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled(
                            "Issues: ",
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            stats.open_issues.to_string(),
                            Style::default().fg(Color::Red),
                        ),
                    ]));
                }
            }
            ForgeStatus::Queued | ForgeStatus::Fetching => {
                lines.push(Line::from(Span::styled(
                    "Loading forge stats...",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            ForgeStatus::Failed(e) => {
                lines.push(Line::from(Span::styled(
                    format!("Forge error: {e}"),
                    Style::default().fg(Color::Red),
                )));
            }
            ForgeStatus::NotApplicable => {}
        }
    }

    // Worktree info
    if repo.is_worktree {
        if let Some(main_path) = &repo.worktree_main {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    "Main repo: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    main_path.display().to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    if !repo.worktrees.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Worktrees ({})", repo.worktrees.len()),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for wt in &repo.worktrees {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(&wt.branch, Style::default().fg(Color::Cyan)),
                Span::raw("  "),
                if wt.dirty_files > 0 {
                    Span::styled(
                        format!("*{}", wt.dirty_files),
                        Style::default().fg(Color::Yellow),
                    )
                } else {
                    Span::styled("✓", Style::default().fg(Color::Green))
                },
                Span::raw("  "),
                Span::styled(
                    wt.path.display().to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "p pull  P push  s shell  q/Esc close",
        Style::default().fg(Color::DarkGray),
    )));

    let title = format!(" {} ", repo.name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD));

    frame.render_widget(Clear, area);
    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}
