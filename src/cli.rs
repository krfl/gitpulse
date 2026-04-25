use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use color_eyre::Result;
use serde::Serialize;

use crate::git;
use crate::model::{Repo, SyncState};

const MAX_CONCURRENT_FETCHES: usize = 8;

// -- Serialization structs --

#[derive(Serialize)]
struct RepoEntry {
    name: String,
    path: String,
    branch: String,
    sync_state: &'static str,
    ahead: u32,
    behind: u32,
    dirty_files: u32,
    has_remote: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_url: Option<String>,
}

impl RepoEntry {
    fn from_repo(repo: &Repo) -> Self {
        Self {
            name: repo.name.clone(),
            path: repo.path.display().to_string(),
            branch: repo.current_branch.clone(),
            sync_state: repo.sync_state().json_key(),
            ahead: repo.ahead,
            behind: repo.behind,
            dirty_files: repo.dirty_files,
            has_remote: repo.has_remote,
            remote_url: repo.remote_url.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct FetchResult {
    name: String,
    path: String,
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct PullResult {
    name: String,
    path: String,
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct StatusSummary {
    behind: usize,
    uncommitted: usize,
    in_sync: usize,
    ahead: usize,
    total: usize,
}

// -- Helpers --

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    match serde_json::to_writer_pretty(&mut out, value) {
        Ok(()) => {
            let _ = writeln!(out);
            Ok(())
        }
        Err(e) if e.io_error_kind() == Some(io::ErrorKind::BrokenPipe) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn scan(path: &Path) -> Result<Vec<Repo>> {
    git::scan_repos(path).map_err(|e| color_eyre::eyre::eyre!(e))
}

fn count_by_state(repos: &[Repo]) -> StatusSummary {
    let mut s = StatusSummary {
        behind: 0,
        uncommitted: 0,
        in_sync: 0,
        ahead: 0,
        total: repos.len(),
    };
    for repo in repos {
        match repo.sync_state() {
            SyncState::Behind => s.behind += 1,
            SyncState::Uncommitted => s.uncommitted += 1,
            SyncState::InSync => s.in_sync += 1,
            SyncState::Ahead => s.ahead += 1,
        }
    }
    s
}

fn fetch_all(repos: &mut [Repo]) -> Vec<FetchResult> {
    let work: Vec<(String, std::path::PathBuf)> = repos
        .iter()
        .filter(|r| r.has_remote)
        .map(|r| (r.name.clone(), r.path.clone()))
        .collect();

    if work.is_empty() {
        return Vec::new();
    }

    let queue = Arc::new(Mutex::new(work));
    let results: Arc<Mutex<Vec<FetchResult>>> = Arc::new(Mutex::new(Vec::new()));
    let num_workers = MAX_CONCURRENT_FETCHES.min(queue.lock().unwrap().len());

    thread::scope(|s| {
        for _ in 0..num_workers {
            let queue = Arc::clone(&queue);
            let results = Arc::clone(&results);
            s.spawn(move || loop {
                let item = queue.lock().unwrap().pop();
                let Some((name, path)) = item else { break };
                let result = git::fetch_repo(&path);
                results.lock().unwrap().push(FetchResult {
                    name,
                    path: path.display().to_string(),
                    success: result.is_ok(),
                    error: result.err(),
                });
            });
        }
    });

    for repo in repos.iter_mut().filter(|r| r.has_remote) {
        git::refresh_repo_status(repo);
    }

    Arc::try_unwrap(results).unwrap().into_inner().unwrap()
}

// -- Commands --

pub(crate) fn cmd_status(path: &Path, json: bool) -> Result<()> {
    let repos = scan(path)?;
    let summary = count_by_state(&repos);

    if json {
        print_json(&summary)?;
    } else {
        println!(
            "{} behind, {} uncommitted, {} in sync, {} ahead",
            summary.behind, summary.uncommitted, summary.in_sync, summary.ahead,
        );
    }

    let all_clean = summary.behind == 0 && summary.uncommitted == 0 && summary.ahead == 0;
    if !all_clean {
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn cmd_list(path: &Path, json: bool) -> Result<()> {
    let repos = scan(path)?;

    if json {
        let entries: Vec<RepoEntry> = repos.iter().map(RepoEntry::from_repo).collect();
        print_json(&entries)?;
        return Ok(());
    }

    for state in SyncState::ALL {
        let in_col: Vec<&Repo> = repos.iter().filter(|r| r.sync_state() == state).collect();
        if in_col.is_empty() {
            continue;
        }
        println!("{} ({})", state.label(), in_col.len());
        println!("{}", "─".repeat(state.label().len() + 4));
        for repo in &in_col {
            let mut parts = vec![format!("  {:<20} {}", repo.name, repo.current_branch)];
            if repo.behind > 0 {
                parts.push(format!("↓{}", repo.behind));
            }
            if repo.ahead > 0 {
                parts.push(format!("↑{}", repo.ahead));
            }
            if repo.dirty_files > 0 {
                parts.push(format!("*{}", repo.dirty_files));
            }
            println!("{}", parts.join("  "));
        }
        println!();
    }

    Ok(())
}

pub(crate) fn cmd_fetch(path: &Path, json: bool) -> Result<()> {
    let mut repos = scan(path)?;
    let results = fetch_all(&mut repos);

    if json {
        print_json(&results)?;
    } else {
        for r in &results {
            if r.success {
                println!("  {} ok", r.name);
            } else {
                println!(
                    "  {} failed: {}",
                    r.name,
                    r.error.as_deref().unwrap_or("unknown")
                );
            }
        }
        let failed = results.iter().filter(|r| !r.success).count();
        println!(
            "Fetched {} repos ({} ok, {} failed)",
            results.len(),
            results.len() - failed,
            failed,
        );
    }

    if results.iter().any(|r| !r.success) {
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn cmd_pull(path: &Path, json: bool) -> Result<()> {
    let mut repos = scan(path)?;
    let fetch_results = fetch_all(&mut repos);

    let fetch_failures: Vec<&FetchResult> = fetch_results.iter().filter(|r| !r.success).collect();
    if !json {
        for f in &fetch_failures {
            println!(
                "  fetch {} failed: {}",
                f.name,
                f.error.as_deref().unwrap_or("unknown"),
            );
        }
    }

    let behind: Vec<&mut Repo> = repos
        .iter_mut()
        .filter(|r| r.sync_state() == SyncState::Behind)
        .collect();

    if behind.is_empty() && !json {
        println!("No repos are behind — nothing to pull.");
        if !fetch_failures.is_empty() {
            std::process::exit(1);
        }
        return Ok(());
    }

    let mut results: Vec<PullResult> = Vec::new();
    for repo in behind {
        let result = git::pull_repo(&repo.path);
        results.push(PullResult {
            name: repo.name.clone(),
            path: repo.path.display().to_string(),
            success: result.is_ok(),
            output: result.as_deref().ok().map(|s| s.to_string()),
            error: result.err(),
        });
        if !json {
            let last = results.last().unwrap();
            if last.success {
                println!("  {} pulled", last.name);
            } else {
                println!(
                    "  {} failed: {}",
                    last.name,
                    last.error.as_deref().unwrap_or("unknown"),
                );
            }
        }
    }

    if json {
        print_json(&results)?;
    } else {
        let failed = results.iter().filter(|r| !r.success).count();
        println!(
            "Pulled {} repos ({} ok, {} failed)",
            results.len(),
            results.len() - failed,
            failed,
        );
    }

    if results.iter().any(|r| !r.success) || !fetch_failures.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Repo;

    #[test]
    fn count_by_state_empty() {
        let s = count_by_state(&[]);
        assert_eq!(s.total, 0);
        assert_eq!(s.behind, 0);
        assert_eq!(s.in_sync, 0);
    }

    #[test]
    fn count_by_state_mixed() {
        let mut behind = Repo::test_default("a");
        behind.behind = 1;
        let mut dirty = Repo::test_default("b");
        dirty.dirty_files = 2;
        let clean = Repo::test_default("c");
        let mut ahead = Repo::test_default("d");
        ahead.ahead = 3;

        let s = count_by_state(&[behind, dirty, clean, ahead]);
        assert_eq!(s.behind, 1);
        assert_eq!(s.uncommitted, 1);
        assert_eq!(s.in_sync, 1);
        assert_eq!(s.ahead, 1);
        assert_eq!(s.total, 4);
    }

    #[test]
    fn repo_entry_from_repo() {
        let mut r = Repo::test_default("myrepo");
        r.ahead = 2;
        r.dirty_files = 1;
        let entry = RepoEntry::from_repo(&r);
        assert_eq!(entry.name, "myrepo");
        assert_eq!(entry.sync_state, "uncommitted");
        assert_eq!(entry.ahead, 2);
        assert_eq!(entry.dirty_files, 1);
        assert!(!entry.has_remote);
        assert!(entry.remote_url.is_none());
    }

    #[test]
    fn status_summary_serializes() {
        let s = StatusSummary {
            behind: 1,
            uncommitted: 2,
            in_sync: 3,
            ahead: 4,
            total: 10,
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["behind"], 1);
        assert_eq!(json["in_sync"], 3);
        assert_eq!(json["total"], 10);
    }

    #[test]
    fn json_key_values() {
        assert_eq!(SyncState::Behind.json_key(), "behind");
        assert_eq!(SyncState::Uncommitted.json_key(), "uncommitted");
        assert_eq!(SyncState::InSync.json_key(), "in_sync");
        assert_eq!(SyncState::Ahead.json_key(), "ahead");
    }
}
