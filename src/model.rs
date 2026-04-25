use std::path::PathBuf;

pub(crate) const COLUMN_COUNT: usize = SyncState::ALL.len();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncState {
    Behind,
    Uncommitted,
    Ahead,
    InSync,
}

impl SyncState {
    pub(crate) const ALL: [SyncState; 4] = [
        SyncState::Behind,
        SyncState::Uncommitted,
        SyncState::InSync,
        SyncState::Ahead,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            SyncState::Behind => "Behind",
            SyncState::Uncommitted => "Uncommitted",
            SyncState::InSync => "In Sync",
            SyncState::Ahead => "Ahead",
        }
    }

    pub(crate) fn json_key(self) -> &'static str {
        match self {
            SyncState::Behind => "behind",
            SyncState::Uncommitted => "uncommitted",
            SyncState::InSync => "in_sync",
            SyncState::Ahead => "ahead",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FetchStatus {
    Queued,
    Fetching,
    Done,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForgeKind {
    GitHub,
    GitLab,
    Gitea,
}

impl ForgeKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Gitea => "Gitea",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scheme {
    Http,
    Https,
}

impl Scheme {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteInfo {
    pub kind: ForgeKind,
    pub host: String,
    pub scheme: Scheme,
    pub owner: String,
    pub repo_name: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ForgeStats {
    pub open_prs: u32,
    pub open_issues: u32,
    pub is_fork: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForgeStatus {
    NotApplicable,
    Queued,
    Fetching,
    Done,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortMode {
    Name,
    PullRequests,
    Issues,
}

impl SortMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SortMode::Name => "Name",
            SortMode::PullRequests => "PRs",
            SortMode::Issues => "Issues",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            SortMode::Name => SortMode::PullRequests,
            SortMode::PullRequests => SortMode::Issues,
            SortMode::Issues => SortMode::Name,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WorktreeInfo {
    pub path: PathBuf,
    pub name: String,
    pub branch: String,
    pub dirty_files: u32,
}

pub(crate) struct Repo {
    pub path: PathBuf,
    pub name: String,
    pub default_branch: Option<String>,
    pub current_branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub dirty_files: u32,
    pub has_remote: bool,
    pub fetch_status: FetchStatus,
    pub remote_url: Option<String>,
    pub remote_info: Option<RemoteInfo>,
    pub forge_stats: Option<ForgeStats>,
    pub forge_status: ForgeStatus,
    pub worktrees: Vec<WorktreeInfo>,
    pub is_worktree: bool,
    pub worktree_main: Option<PathBuf>,
}

impl Repo {
    #[cfg(test)]
    pub fn test_default(name: &str) -> Self {
        Self {
            path: std::path::PathBuf::from(format!("/tmp/{name}")),
            name: name.to_string(),
            default_branch: None,
            current_branch: "main".to_string(),
            ahead: 0,
            behind: 0,
            dirty_files: 0,
            has_remote: false,
            fetch_status: FetchStatus::Done,
            remote_url: None,
            remote_info: None,
            forge_stats: None,
            forge_status: ForgeStatus::NotApplicable,
            worktrees: Vec::new(),
            is_worktree: false,
            worktree_main: None,
        }
    }

    pub(crate) fn card_height(&self) -> u16 {
        5u16.saturating_add(self.worktrees.len() as u16)
    }

    /// Determine the primary sync state using priority:
    /// Behind > Uncommitted > Ahead > InSync
    pub(crate) fn sync_state(&self) -> SyncState {
        if self.behind > 0 {
            SyncState::Behind
        } else if self.dirty_files > 0 {
            SyncState::Uncommitted
        } else if self.ahead > 0 {
            SyncState::Ahead
        } else {
            SyncState::InSync
        }
    }

    /// Secondary states shown as tags on the card.
    /// Only states that differ from the primary are returned.
    pub(crate) fn secondary_states(&self) -> Vec<SyncState> {
        let primary = self.sync_state();
        let mut secondary = Vec::new();
        // Behind can never be secondary (it's always the highest priority primary)
        if primary != SyncState::Uncommitted && self.dirty_files > 0 {
            secondary.push(SyncState::Uncommitted);
        }
        if primary != SyncState::Ahead && self.ahead > 0 {
            secondary.push(SyncState::Ahead);
        }
        secondary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- sync_state priority tests --

    #[test]
    fn sync_state_behind_takes_priority() {
        let mut r = Repo::test_default("r");
        r.behind = 3;
        r.dirty_files = 5;
        r.ahead = 2;
        assert_eq!(r.sync_state(), SyncState::Behind);
    }

    #[test]
    fn sync_state_uncommitted_over_ahead() {
        let mut r = Repo::test_default("r");
        r.dirty_files = 1;
        r.ahead = 10;
        assert_eq!(r.sync_state(), SyncState::Uncommitted);
    }

    #[test]
    fn sync_state_ahead_only() {
        let mut r = Repo::test_default("r");
        r.ahead = 1;
        assert_eq!(r.sync_state(), SyncState::Ahead);
    }

    #[test]
    fn sync_state_in_sync() {
        let r = Repo::test_default("r");
        assert_eq!(r.sync_state(), SyncState::InSync);
    }

    #[test]
    fn sync_state_no_remote_all_zero_is_in_sync() {
        let r = Repo::test_default("r");
        assert!(!r.has_remote);
        assert_eq!(r.sync_state(), SyncState::InSync);
    }

    // -- secondary_states tests --

    #[test]
    fn secondary_states_behind_with_dirty_and_ahead() {
        let mut r = Repo::test_default("r");
        r.behind = 1;
        r.dirty_files = 2;
        r.ahead = 3;
        assert_eq!(r.secondary_states(), vec![SyncState::Uncommitted, SyncState::Ahead]);
    }

    #[test]
    fn secondary_states_uncommitted_with_ahead() {
        let mut r = Repo::test_default("r");
        r.dirty_files = 1;
        r.ahead = 5;
        assert_eq!(r.secondary_states(), vec![SyncState::Ahead]);
    }

    #[test]
    fn secondary_states_ahead_only_returns_empty() {
        let mut r = Repo::test_default("r");
        r.ahead = 1;
        assert!(r.secondary_states().is_empty());
    }

    #[test]
    fn secondary_states_in_sync_returns_empty() {
        let r = Repo::test_default("r");
        assert!(r.secondary_states().is_empty());
    }

    #[test]
    fn secondary_states_behind_never_appears_as_secondary() {
        let mut r = Repo::test_default("r");
        r.behind = 5;
        assert!(r.secondary_states().is_empty());
    }

    // -- label and constant tests --

    #[test]
    fn sync_state_labels() {
        assert_eq!(SyncState::Behind.label(), "Behind");
        assert_eq!(SyncState::Uncommitted.label(), "Uncommitted");
        assert_eq!(SyncState::InSync.label(), "In Sync");
        assert_eq!(SyncState::Ahead.label(), "Ahead");
    }

    #[test]
    fn column_count_matches_all_variants() {
        assert_eq!(COLUMN_COUNT, 4);
        assert_eq!(COLUMN_COUNT, SyncState::ALL.len());
    }

    #[test]
    fn sort_mode_labels() {
        assert_eq!(SortMode::Name.label(), "Name");
        assert_eq!(SortMode::PullRequests.label(), "PRs");
        assert_eq!(SortMode::Issues.label(), "Issues");
    }

    #[test]
    fn sort_mode_cycles() {
        assert_eq!(SortMode::Name.next(), SortMode::PullRequests);
        assert_eq!(SortMode::PullRequests.next(), SortMode::Issues);
        assert_eq!(SortMode::Issues.next(), SortMode::Name);
    }

    #[test]
    fn forge_kind_labels() {
        assert_eq!(ForgeKind::GitHub.label(), "GitHub");
        assert_eq!(ForgeKind::GitLab.label(), "GitLab");
        assert_eq!(ForgeKind::Gitea.label(), "Gitea");
    }

    #[test]
    fn forge_stats_default_is_zero() {
        let stats = ForgeStats::default();
        assert_eq!(stats.open_prs, 0);
        assert_eq!(stats.open_issues, 0);
        assert!(!stats.is_fork);
    }

    #[test]
    fn test_default_initializes_forge_fields() {
        let r = Repo::test_default("test");
        assert!(r.remote_url.is_none());
        assert!(r.remote_info.is_none());
        assert!(r.forge_stats.is_none());
        assert_eq!(r.forge_status, ForgeStatus::NotApplicable);
    }

    #[test]
    fn test_default_initializes_worktree_fields() {
        let r = Repo::test_default("test");
        assert!(r.worktrees.is_empty());
        assert!(!r.is_worktree);
        assert!(r.worktree_main.is_none());
    }

    #[test]
    fn card_height_no_worktrees() {
        let r = Repo::test_default("r");
        assert_eq!(r.card_height(), 5);
    }

    #[test]
    fn card_height_with_worktrees() {
        let mut r = Repo::test_default("r");
        r.worktrees.push(WorktreeInfo {
            path: PathBuf::from("/tmp/wt1"),
            name: "wt1".to_string(),
            branch: "feat".to_string(),
            dirty_files: 0,
        });
        r.worktrees.push(WorktreeInfo {
            path: PathBuf::from("/tmp/wt2"),
            name: "wt2".to_string(),
            branch: "fix".to_string(),
            dirty_files: 3,
        });
        assert_eq!(r.card_height(), 7);
    }
}
