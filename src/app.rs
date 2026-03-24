use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::forge;
use crate::git;
use crate::model::{
    FetchStatus, ForgeStats, ForgeStatus, Repo, RemoteInfo, SortMode, SyncState, COLUMN_COUNT,
};

const MAX_CONCURRENT_FETCHES: usize = 8;
const MAX_CONCURRENT_FORGE_FETCHES: usize = 4;

#[derive(Debug)]
pub(crate) enum Overlay {
    Board,
    Detail,
    Help,
    ShellPicker {
        paths: Vec<(String, PathBuf)>,
        index: usize,
    },
}

enum ActionMessage {
    FetchStarting(PathBuf),
    FetchCompleted(PathBuf, std::result::Result<(), String>),
    PullCompleted(PathBuf, String, std::result::Result<String, String>),
    PushCompleted(PathBuf, String, std::result::Result<String, String>),
    ForgeStarting(PathBuf),
    ForgeCompleted(PathBuf, std::result::Result<ForgeStats, String>),
}

pub(crate) struct AppState {
    pub repos: Vec<Repo>,
    pub selected_column: usize,
    pub selected_index: [usize; COLUMN_COUNT],
    pub overlay: Overlay,
    pub status_message: Option<String>,
    pub should_quit: bool,
    pub scan_path: PathBuf,
    pub sort_mode: SortMode,
    frame: usize,
    pending_shell: Option<PathBuf>,
    msg_rx: mpsc::Receiver<ActionMessage>,
    msg_tx: mpsc::Sender<ActionMessage>,
}

impl AppState {
    pub(crate) fn new(scan_path: &Path) -> Self {
        let (repos, error) = match git::scan_repos(scan_path) {
            Ok(repos) => (repos, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let (msg_tx, msg_rx) = mpsc::channel();

        let mut state = Self {
            repos,
            selected_column: 0,
            selected_index: [0; COLUMN_COUNT],
            overlay: Overlay::Board,
            status_message: error,
            should_quit: false,
            frame: 0,
            scan_path: scan_path.to_path_buf(),
            sort_mode: SortMode::Name,
            pending_shell: None,
            msg_rx,
            msg_tx,
        };

        state.start_fetches();
        state.start_forge_fetches();
        state
    }

    pub(crate) fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            self.process_messages();
            self.frame = self.frame.wrapping_add(1);

            terminal.draw(|frame| crate::ui::render(frame, self))?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key(key);
                }
            }

            if let Some(path) = self.pending_shell.take() {
                ratatui::restore();

                let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
                let _ = std::process::Command::new(&shell)
                    .current_dir(&path)
                    .status();

                *terminal = ratatui::init();
            }
        }
        Ok(())
    }

    pub(crate) fn spinner_index(&self) -> usize {
        self.frame / 2
    }

    fn start_forge_fetches(&mut self) {
        let items: Vec<(PathBuf, RemoteInfo)> = self
            .repos
            .iter()
            .filter_map(|r| {
                r.remote_info
                    .as_ref()
                    .map(|info| (r.path.clone(), info.clone()))
            })
            .collect();

        if items.is_empty() {
            return;
        }

        let work_queue = Arc::new(Mutex::new(items));
        let num_workers = MAX_CONCURRENT_FORGE_FETCHES.min(work_queue.lock().unwrap().len());

        for _ in 0..num_workers {
            let queue = Arc::clone(&work_queue);
            let tx = self.msg_tx.clone();
            thread::spawn(move || loop {
                let item = {
                    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                    q.pop()
                };
                let Some((path, info)) = item else { break };
                let _ = tx.send(ActionMessage::ForgeStarting(path.clone()));
                let token = forge::resolve_token(&info.kind, &info.host);
                let result = forge::fetch_forge_stats(&info, token.as_deref());
                let _ = tx.send(ActionMessage::ForgeCompleted(path, result));
            });
        }
    }

    fn start_fetches(&mut self) {
        let paths: Vec<PathBuf> = self
            .repos
            .iter()
            .filter(|r| r.has_remote)
            .map(|r| r.path.clone())
            .collect();

        if paths.is_empty() {
            return;
        }

        let work_queue = Arc::new(Mutex::new(paths));
        let num_workers =
            MAX_CONCURRENT_FETCHES.min(self.repos.iter().filter(|r| r.has_remote).count());

        for _ in 0..num_workers {
            let queue = Arc::clone(&work_queue);
            let tx = self.msg_tx.clone();
            thread::spawn(move || loop {
                let path = {
                    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                    q.pop()
                };
                let Some(path) = path else { break };
                let _ = tx.send(ActionMessage::FetchStarting(path.clone()));
                let result = git::fetch_repo(&path);
                let _ = tx.send(ActionMessage::FetchCompleted(path, result));
            });
        }
    }

    fn process_messages(&mut self) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                ActionMessage::FetchStarting(path) => {
                    if let Some(repo) = self.repo_by_path_mut(&path) {
                        repo.fetch_status = FetchStatus::Fetching;
                    }
                }
                ActionMessage::FetchCompleted(path, result) => {
                    if let Some(repo) = self.repo_by_path_mut(&path) {
                        match result {
                            Ok(()) => {
                                git::refresh_repo_status(repo);
                                repo.fetch_status = FetchStatus::Done;
                            }
                            Err(e) => {
                                repo.fetch_status = FetchStatus::Failed(e);
                            }
                        }
                    }
                    self.clamp_all_indices();
                }
                ActionMessage::PullCompleted(path, name, result) => {
                    self.handle_action_result(&path, &name, result, "Pulled", "Pull");
                }
                ActionMessage::PushCompleted(path, name, result) => {
                    self.handle_action_result(&path, &name, result, "Pushed", "Push");
                }
                ActionMessage::ForgeStarting(path) => {
                    if let Some(repo) = self.repo_by_path_mut(&path) {
                        repo.forge_status = ForgeStatus::Fetching;
                    }
                }
                ActionMessage::ForgeCompleted(path, result) => {
                    if let Some(repo) = self.repo_by_path_mut(&path) {
                        match result {
                            Ok(stats) => {
                                repo.forge_stats = Some(stats);
                                repo.forge_status = ForgeStatus::Done;
                            }
                            Err(e) => {
                                repo.forge_status = ForgeStatus::Failed(e);
                            }
                        }
                    }
                    self.clamp_all_indices();
                }
            }
        }
    }

    fn repo_by_path_mut(&mut self, path: &Path) -> Option<&mut Repo> {
        self.repos.iter_mut().find(|r| r.path == path)
    }

    fn handle_action_result(
        &mut self,
        path: &Path,
        name: &str,
        result: std::result::Result<String, String>,
        success_verb: &str,
        fail_verb: &str,
    ) {
        match result {
            Ok(msg) => {
                self.status_message = Some(format!("{success_verb} {name}: {msg}"));
                if let Some(repo) = self.repo_by_path_mut(path) {
                    git::refresh_repo_status(repo);
                }
            }
            Err(e) => {
                self.status_message = Some(format!("{fail_verb} failed for {name}: {e}"));
            }
        }
        self.clamp_all_indices();
    }

    fn clamp_all_indices(&mut self) {
        for i in 0..COLUMN_COUNT {
            let count = self.column_count(i);
            if count == 0 {
                self.selected_index[i] = 0;
            } else {
                self.selected_index[i] = self.selected_index[i].min(count - 1);
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        match &self.overlay {
            Overlay::Detail => self.handle_detail_key(key),
            Overlay::Help => self.handle_help_key(key),
            Overlay::Board => self.handle_board_key(key),
            Overlay::ShellPicker { .. } => self.handle_shell_picker_key(key),
        }
    }

    fn handle_board_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('h') | KeyCode::Left => self.move_column(-1),
            KeyCode::Char('l') | KeyCode::Right => self.move_column(1),
            KeyCode::Char('j') | KeyCode::Down => self.move_card(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_card(-1),
            KeyCode::Tab => self.cycle_repo(1),
            KeyCode::BackTab => self.cycle_repo(-1),
            KeyCode::Enter => {
                if self.selected_repo().is_some() {
                    self.overlay = Overlay::Detail;
                }
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('p') => self.pull_selected(),
            KeyCode::Char('P') => self.push_selected(),
            KeyCode::Char('s') => self.open_shell(),
            KeyCode::Char('v') => self.sort_mode = self.sort_mode.next(),
            _ => {}
        }
    }

    fn handle_detail_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::Board,
            KeyCode::Tab => self.cycle_repo(1),
            KeyCode::BackTab => self.cycle_repo(-1),
            KeyCode::Char('p') => self.pull_selected(),
            KeyCode::Char('P') => self.push_selected(),
            KeyCode::Char('s') => self.open_shell(),
            _ => {}
        }
    }

    fn handle_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                self.overlay = Overlay::Board;
            }
            _ => {}
        }
    }

    fn handle_shell_picker_key(&mut self, key: KeyEvent) {
        let Overlay::ShellPicker { paths, index } = &mut self.overlay else {
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::Board,
            KeyCode::Char('j') | KeyCode::Down => {
                if !paths.is_empty() {
                    *index = (*index + 1).min(paths.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                *index = index.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some((_, path)) = paths.get(*index) {
                    self.pending_shell = Some(path.clone());
                    self.overlay = Overlay::Board;
                }
            }
            _ => {}
        }
    }

    fn move_column(&mut self, delta: i32) {
        let new = if delta < 0 {
            self.selected_column.checked_sub(1)
        } else {
            let next = self.selected_column + 1;
            (next < COLUMN_COUNT).then_some(next)
        };
        if let Some(col) = new {
            self.selected_column = col;
            let count = self.column_count(col);
            if count > 0 {
                self.selected_index[col] = self.selected_index[col].min(count - 1);
            }
        }
    }

    fn move_card(&mut self, delta: i32) {
        let col = self.selected_column;
        let count = self.column_count(col);
        if count == 0 {
            return;
        }
        let current = self.selected_index[col] as i32;
        let new = (current + delta).clamp(0, count as i32 - 1);
        self.selected_index[col] = new as usize;
    }

    /// Cycle through all repos across columns. Wraps around.
    fn cycle_repo(&mut self, direction: i32) {
        let mut positions: Vec<(usize, usize)> = Vec::new();
        for col in 0..COLUMN_COUNT {
            let count = self.column_count(col);
            for card in 0..count {
                positions.push((col, card));
            }
        }

        if positions.is_empty() {
            return;
        }

        let current = (self.selected_column, self.selected_index[self.selected_column]);
        let len = positions.len() as i32;

        let new_pos = match positions.iter().position(|&pos| pos == current) {
            Some(idx) => ((idx as i32 + direction).rem_euclid(len)) as usize,
            None => {
                if direction > 0 {
                    positions
                        .iter()
                        .position(|&(col, _)| col >= self.selected_column)
                        .unwrap_or(0)
                } else {
                    positions
                        .iter()
                        .rposition(|&(col, _)| col <= self.selected_column)
                        .unwrap_or(positions.len() - 1)
                }
            }
        };

        let (col, card) = positions[new_pos];
        self.selected_column = col;
        self.selected_index[col] = card;
    }

    pub(crate) fn column_count(&self, col: usize) -> usize {
        let state = SyncState::ALL[col];
        self.repos.iter().filter(|r| r.sync_state() == state).count()
    }

    pub(crate) fn repos_in_column(&self, col: usize) -> Vec<&Repo> {
        let state = SyncState::ALL[col];
        let mut repos: Vec<&Repo> = self
            .repos
            .iter()
            .filter(|r| r.sync_state() == state)
            .collect();
        match self.sort_mode {
            SortMode::Name => {} // already sorted by name from scan
            SortMode::PullRequests => repos.sort_by(|a, b| {
                forge_sort_val(b, SortMode::PullRequests)
                    .cmp(&forge_sort_val(a, SortMode::PullRequests))
            }),
            SortMode::Issues => repos.sort_by(|a, b| {
                forge_sort_val(b, SortMode::Issues)
                    .cmp(&forge_sort_val(a, SortMode::Issues))
            }),
        }
        repos
    }

    pub(crate) fn selected_repo(&self) -> Option<&Repo> {
        let idx = self.selected_index[self.selected_column];
        self.repos_in_column(self.selected_column).get(idx).copied()
    }

    fn refresh(&mut self) {
        match git::scan_repos(&self.scan_path) {
            Ok(repos) => {
                self.repos = repos;
                self.status_message = Some("Refreshing...".to_string());
            }
            Err(e) => {
                self.repos = Vec::new();
                self.status_message = Some(e);
            }
        }
        self.selected_index = [0; COLUMN_COUNT];
        let (tx, rx) = mpsc::channel();
        self.msg_tx = tx;
        self.msg_rx = rx;
        self.start_fetches();
        self.start_forge_fetches();
    }

    fn pull_selected(&mut self) {
        let Some(repo) = self.selected_repo() else {
            return;
        };
        let path = repo.path.clone();
        let name = repo.name.clone();
        let tx = self.msg_tx.clone();

        self.status_message = Some(format!("Pulling {name}..."));
        thread::spawn(move || {
            let result = git::pull_repo(&path);
            let _ = tx.send(ActionMessage::PullCompleted(path, name, result));
        });
    }

    fn push_selected(&mut self) {
        let Some(repo) = self.selected_repo() else {
            return;
        };
        let path = repo.path.clone();
        let name = repo.name.clone();
        let tx = self.msg_tx.clone();

        self.status_message = Some(format!("Pushing {name}..."));
        thread::spawn(move || {
            let result = git::push_repo(&path);
            let _ = tx.send(ActionMessage::PushCompleted(path, name, result));
        });
    }

    fn open_shell(&mut self) {
        let Some(repo) = self.selected_repo() else {
            return;
        };
        if repo.worktrees.is_empty() {
            self.pending_shell = Some(repo.path.clone());
        } else {
            let mut paths = vec![(format!("main: {}", repo.name), repo.path.clone())];
            for wt in &repo.worktrees {
                paths.push((format!("wt: {}", wt.branch), wt.path.clone()));
            }
            self.overlay = Overlay::ShellPicker { paths, index: 0 };
        }
    }

    #[cfg(test)]
    fn new_with_repos(repos: Vec<Repo>) -> Self {
        let (msg_tx, msg_rx) = mpsc::channel();
        Self {
            repos,
            selected_column: 0,
            selected_index: [0; COLUMN_COUNT],
            overlay: Overlay::Board,
            status_message: None,
            should_quit: false,
            frame: 0,
            scan_path: std::path::PathBuf::from("/tmp"),
            sort_mode: SortMode::Name,
            pending_shell: None,
            msg_rx,
            msg_tx,
        }
    }
}

fn forge_sort_val(repo: &Repo, mode: SortMode) -> u32 {
    repo.forge_stats
        .as_ref()
        .map(|s| match mode {
            SortMode::PullRequests => s.open_prs,
            SortMode::Issues => s.open_issues,
            SortMode::Name => unreachable!("forge_sort_val not used for Name sort"),
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use crate::model::Repo;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Helper: repos covering all 4 states.
    fn mixed_repos() -> Vec<Repo> {
        let mut behind = Repo::test_default("behind-repo");
        behind.behind = 3;

        let mut dirty1 = Repo::test_default("dirty-repo-1");
        dirty1.dirty_files = 2;

        let mut dirty2 = Repo::test_default("dirty-repo-2");
        dirty2.dirty_files = 1;

        let mut ahead = Repo::test_default("ahead-repo");
        ahead.ahead = 1;

        let synced = Repo::test_default("synced-repo");

        vec![behind, dirty1, dirty2, ahead, synced]
    }

    // -- Navigation: columns --

    #[test]
    fn move_column_right_from_zero() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.move_column(1);
        assert_eq!(app.selected_column, 1);
    }

    #[test]
    fn move_column_left_from_zero_stays() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.move_column(-1);
        assert_eq!(app.selected_column, 0);
    }

    #[test]
    fn move_column_right_at_last_stays() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.selected_column = COLUMN_COUNT - 1;
        app.move_column(1);
        assert_eq!(app.selected_column, COLUMN_COUNT - 1);
    }

    #[test]
    fn move_column_clamps_index_on_arrival() {
        let mut app = AppState::new_with_repos(mixed_repos());
        // Uncommitted column (index 1) has 2 repos. Set stale index.
        app.selected_index[1] = 10;
        app.selected_column = 0;
        app.move_column(1);
        assert_eq!(app.selected_column, 1);
        assert_eq!(app.selected_index[1], 1); // clamped to count - 1
    }

    // -- Navigation: cards --

    #[test]
    fn move_card_down_from_top() {
        let mut app = AppState::new_with_repos(mixed_repos());
        // Move to Uncommitted column (has 2 repos)
        app.selected_column = 1;
        app.move_card(1);
        assert_eq!(app.selected_index[1], 1);
    }

    #[test]
    fn move_card_up_from_top_stays() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.move_card(-1);
        assert_eq!(app.selected_index[0], 0);
    }

    #[test]
    fn move_card_down_at_bottom_stays() {
        let mut app = AppState::new_with_repos(mixed_repos());
        // Uncommitted has 2 repos
        app.selected_column = 1;
        app.selected_index[1] = 1;
        app.move_card(1);
        assert_eq!(app.selected_index[1], 1);
    }

    #[test]
    fn move_card_in_empty_column_is_noop() {
        // All repos are InSync, other columns empty
        let repos = vec![Repo::test_default("a"), Repo::test_default("b")];
        let mut app = AppState::new_with_repos(repos);
        app.selected_column = 0; // Behind column, empty
        app.move_card(1);
        assert_eq!(app.selected_index[0], 0);
        app.move_card(-1);
        assert_eq!(app.selected_index[0], 0);
    }

    // -- Index clamping --

    #[test]
    fn clamp_all_indices_after_column_shrinks() {
        let mut app = AppState::new_with_repos(mixed_repos());
        // Uncommitted has 2 repos, set index to 1
        app.selected_index[1] = 1;
        // Remove one dirty repo so column has 1
        app.repos.retain(|r| r.name != "dirty-repo-2");
        app.clamp_all_indices();
        assert_eq!(app.selected_index[1], 0);
    }

    #[test]
    fn clamp_all_indices_empty_column() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.selected_index[0] = 5;
        // Remove the behind repo
        app.repos.retain(|r| r.name != "behind-repo");
        app.clamp_all_indices();
        assert_eq!(app.selected_index[0], 0);
    }

    // -- Selection --

    #[test]
    fn selected_repo_returns_correct_repo() {
        let app = AppState::new_with_repos(mixed_repos());
        // Column 0 (Behind) has 1 repo
        let repo = app.selected_repo().unwrap();
        assert_eq!(repo.name, "behind-repo");
    }

    #[test]
    fn selected_repo_returns_none_for_empty_column() {
        let repos = vec![Repo::test_default("a")]; // only InSync
        let mut app = AppState::new_with_repos(repos);
        app.selected_column = 0; // Behind, empty
        assert!(app.selected_repo().is_none());
    }

    #[test]
    fn repos_in_column_groups_correctly() {
        let app = AppState::new_with_repos(mixed_repos());
        assert_eq!(app.repos_in_column(0).len(), 1); // Behind
        assert_eq!(app.repos_in_column(1).len(), 2); // Uncommitted
        assert_eq!(app.repos_in_column(2).len(), 1); // InSync
        assert_eq!(app.repos_in_column(3).len(), 1); // Ahead
    }

    #[test]
    fn column_count_sum_equals_total_repos() {
        let app = AppState::new_with_repos(mixed_repos());
        let total: usize = (0..COLUMN_COUNT).map(|i| app.column_count(i)).sum();
        assert_eq!(total, app.repos.len());
    }

    // -- Key handling --

    #[test]
    fn key_q_sets_quit() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.handle_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn key_ctrl_c_quits_from_any_overlay() {
        for overlay in [Overlay::Board, Overlay::Detail, Overlay::Help] {
            let mut app = AppState::new_with_repos(mixed_repos());
            app.overlay = overlay;
            app.handle_key(key_ctrl('c'));
            assert!(app.should_quit);
        }
    }

    #[test]
    fn key_enter_opens_detail_when_repo_selected() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.overlay, Overlay::Detail));
    }

    #[test]
    fn key_enter_noop_when_column_empty() {
        let repos = vec![Repo::test_default("a")]; // only InSync
        let mut app = AppState::new_with_repos(repos);
        app.selected_column = 0; // Behind, empty
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.overlay, Overlay::Board));
    }

    #[test]
    fn key_esc_from_detail_returns_to_board() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.overlay = Overlay::Detail;
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.overlay, Overlay::Board));
    }

    #[test]
    fn key_question_mark_toggles_help() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.handle_key(key(KeyCode::Char('?')));
        assert!(matches!(app.overlay, Overlay::Help));
        app.handle_key(key(KeyCode::Char('?')));
        assert!(matches!(app.overlay, Overlay::Board));
    }

    #[test]
    fn key_h_l_moves_columns() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.selected_column, 1);
        app.handle_key(key(KeyCode::Char('h')));
        assert_eq!(app.selected_column, 0);
    }

    #[test]
    fn key_j_k_moves_cards() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.selected_column = 1; // Uncommitted, 2 repos
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected_index[1], 1);
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.selected_index[1], 0);
    }

    // -- Tab cycling --

    #[test]
    fn tab_cycles_forward_across_columns() {
        let mut app = AppState::new_with_repos(mixed_repos());
        // Start at Behind column, index 0
        assert_eq!(app.selected_column, 0);
        assert_eq!(app.selected_index[0], 0);

        // Tab moves to Uncommitted column, first repo
        app.cycle_repo(1);
        assert_eq!(app.selected_column, 1);
        assert_eq!(app.selected_index[1], 0);

        // Tab again moves to Uncommitted column, second repo
        app.cycle_repo(1);
        assert_eq!(app.selected_column, 1);
        assert_eq!(app.selected_index[1], 1);

        // Tab again moves to InSync column
        app.cycle_repo(1);
        assert_eq!(app.selected_column, 2);
        assert_eq!(app.selected_index[2], 0);
    }

    #[test]
    fn tab_wraps_around_at_end() {
        let mut app = AppState::new_with_repos(mixed_repos());
        // Go to last repo (Ahead column)
        app.selected_column = 3;
        app.selected_index[3] = 0;

        // Tab wraps to Behind column
        app.cycle_repo(1);
        assert_eq!(app.selected_column, 0);
        assert_eq!(app.selected_index[0], 0);
    }

    #[test]
    fn shift_tab_cycles_backward() {
        let mut app = AppState::new_with_repos(mixed_repos());
        // Start at Behind column
        // Shift+Tab wraps to last repo (Ahead)
        app.cycle_repo(-1);
        assert_eq!(app.selected_column, 3);
        assert_eq!(app.selected_index[3], 0);
    }

    #[test]
    fn tab_from_empty_column_selects_first_in_next() {
        // Only Uncommitted repos, Behind column empty
        let mut dirty1 = Repo::test_default("dirty-1");
        dirty1.dirty_files = 1;
        let mut dirty2 = Repo::test_default("dirty-2");
        dirty2.dirty_files = 2;
        let mut app = AppState::new_with_repos(vec![dirty1, dirty2]);
        app.selected_column = 0; // Behind, empty

        app.cycle_repo(1);
        assert_eq!(app.selected_column, 1); // Uncommitted
        assert_eq!(app.selected_index[1], 0); // first repo, not second
    }

    #[test]
    fn shift_tab_from_empty_column_selects_last_before() {
        let mut dirty = Repo::test_default("dirty");
        dirty.dirty_files = 1;
        let synced = Repo::test_default("synced");
        let mut app = AppState::new_with_repos(vec![dirty, synced]);
        app.selected_column = 3; // Ahead, empty

        app.cycle_repo(-1);
        assert_eq!(app.selected_column, 2); // InSync
        assert_eq!(app.selected_index[2], 0);
    }

    #[test]
    fn tab_noop_with_no_repos() {
        let mut app = AppState::new_with_repos(vec![]);
        app.cycle_repo(1);
        assert_eq!(app.selected_column, 0);
        app.cycle_repo(-1);
        assert_eq!(app.selected_column, 0);
    }

    #[test]
    fn tab_key_binding_works() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.selected_column, 1);
        assert_eq!(app.selected_index[1], 0);
    }

    #[test]
    fn backtab_key_binding_works() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.handle_key(key(KeyCode::BackTab));
        assert_eq!(app.selected_column, 3); // wraps to Ahead
    }

    #[test]
    fn tab_works_in_detail_overlay() {
        let mut app = AppState::new_with_repos(mixed_repos());
        app.overlay = Overlay::Detail;
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.selected_column, 1);
        assert!(matches!(app.overlay, Overlay::Detail)); // stays in detail
    }

    #[test]
    fn spinner_index_does_not_panic_at_max() {
        let mut app = AppState::new_with_repos(vec![]);
        app.frame = usize::MAX;
        let _ = app.spinner_index(); // should not panic
    }

    // -- Sort mode --

    fn forge_repo(name: &str, prs: u32, issues: u32) -> Repo {
        use crate::model::{ForgeKind, ForgeStats, RemoteInfo};
        let mut r = Repo::test_default(name);
        r.remote_info = Some(RemoteInfo {
            kind: ForgeKind::GitHub,
            host: "github.com".to_string(),
            owner: "owner".to_string(),
            repo_name: name.to_string(),
        });
        r.forge_stats = Some(ForgeStats {
            open_prs: prs,
            open_issues: issues,
            is_fork: false,
        });
        r.forge_status = ForgeStatus::Done;
        r
    }

    #[test]
    fn key_v_cycles_sort_mode() {
        let mut app = AppState::new_with_repos(mixed_repos());
        assert_eq!(app.sort_mode, SortMode::Name);
        app.handle_key(key(KeyCode::Char('v')));
        assert_eq!(app.sort_mode, SortMode::PullRequests);
        app.handle_key(key(KeyCode::Char('v')));
        assert_eq!(app.sort_mode, SortMode::Issues);
        app.handle_key(key(KeyCode::Char('v')));
        assert_eq!(app.sort_mode, SortMode::Name);
    }

    #[test]
    fn sort_by_prs_within_column() {
        // All repos InSync (same column), different PR counts
        let mut repos = vec![
            forge_repo("low", 1, 3),
            forge_repo("high", 10, 1),
            forge_repo("mid", 5, 7),
        ];
        // Make all InSync
        for r in &mut repos {
            r.behind = 0;
            r.dirty_files = 0;
            r.ahead = 0;
        }
        let mut app = AppState::new_with_repos(repos);
        app.sort_mode = SortMode::PullRequests;
        let col = app.repos_in_column(2); // InSync column
        assert_eq!(col[0].name, "high"); // 10 PRs
        assert_eq!(col[1].name, "mid");  // 5 PRs
        assert_eq!(col[2].name, "low");  // 1 PR
    }

    #[test]
    fn sort_by_issues_within_column() {
        let mut repos = vec![
            forge_repo("a", 1, 3),
            forge_repo("b", 10, 100),
        ];
        for r in &mut repos {
            r.behind = 0;
            r.dirty_files = 0;
            r.ahead = 0;
        }
        let mut app = AppState::new_with_repos(repos);
        app.sort_mode = SortMode::Issues;
        let col = app.repos_in_column(2); // InSync
        assert_eq!(col[0].name, "b"); // 100 issues
        assert_eq!(col[1].name, "a"); // 3 issues
    }

    #[test]
    fn sort_by_name_is_default() {
        let mut repos = vec![
            forge_repo("alpha", 1, 1),
            forge_repo("zebra", 10, 10),
        ];
        for r in &mut repos {
            r.behind = 0;
            r.dirty_files = 0;
            r.ahead = 0;
        }
        let app = AppState::new_with_repos(repos);
        let col = app.repos_in_column(2); // InSync
        // Name sort preserves insertion order (scan sorts by name)
        assert_eq!(col[0].name, "alpha");
        assert_eq!(col[1].name, "zebra");
    }

    // -- Shell picker --

    #[test]
    fn shell_picker_opens_for_repo_with_worktrees() {
        use crate::model::WorktreeInfo;
        let mut r = Repo::test_default("main-repo");
        r.worktrees.push(WorktreeInfo {
            path: std::path::PathBuf::from("/tmp/wt1"),
            name: "wt1".to_string(),
            branch: "feat".to_string(),
            dirty_files: 0,
        });
        let mut app = AppState::new_with_repos(vec![r]);
        app.selected_column = 2; // InSync
        app.open_shell();
        let Overlay::ShellPicker { paths, .. } = &app.overlay else {
            panic!("expected ShellPicker overlay");
        };
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn shell_picker_skipped_for_repo_without_worktrees() {
        let r = Repo::test_default("simple-repo");
        let mut app = AppState::new_with_repos(vec![r]);
        app.selected_column = 2; // InSync
        app.open_shell();
        assert!(matches!(app.overlay, Overlay::Board));
        assert!(app.pending_shell.is_some());
    }

    #[test]
    fn shell_picker_navigation_and_select() {
        let mut app = AppState::new_with_repos(vec![]);
        app.overlay = Overlay::ShellPicker {
            paths: vec![
                ("main".to_string(), PathBuf::from("/a")),
                ("wt: feat".to_string(), PathBuf::from("/b")),
            ],
            index: 0,
        };

        // Move down
        app.handle_key(key(KeyCode::Char('j')));
        let Overlay::ShellPicker { index, .. } = &app.overlay else {
            panic!("expected ShellPicker");
        };
        assert_eq!(*index, 1);

        // Select
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.pending_shell, Some(PathBuf::from("/b")));
        assert!(matches!(app.overlay, Overlay::Board));
    }

    #[test]
    fn shell_picker_esc_cancels() {
        let mut app = AppState::new_with_repos(vec![]);
        app.overlay = Overlay::ShellPicker {
            paths: vec![("test".to_string(), PathBuf::from("/a"))],
            index: 0,
        };
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.overlay, Overlay::Board));
        assert!(app.pending_shell.is_none());
    }
}
