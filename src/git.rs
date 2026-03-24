use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::forge;
use crate::model::{FetchStatus, ForgeStatus, Repo, WorktreeInfo};

const GIT_TIMEOUT: Duration = Duration::from_secs(15);

/// Scan a directory for git repos (one level deep).
/// Worktrees are grouped under their main repo when both are present.
/// Bare repo containers (`.git` file pointing to `.bare`) are scanned one level deeper.
pub(crate) fn scan_repos(dir: &Path) -> Result<Vec<Repo>, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("Cannot read {}: {e}", dir.display()))?;

    let mut main_repos: HashMap<PathBuf, Repo> = HashMap::new();
    let mut worktree_entries: Vec<(PathBuf, WorktreeInfo)> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        classify_entry(&path, &mut main_repos, &mut worktree_entries);
    }

    // Group worktrees under their main repos
    for (main_path, wt) in worktree_entries {
        let canon = main_path.canonicalize().unwrap_or(main_path.clone());
        if let Some(repo) = main_repos.get_mut(&canon) {
            repo.worktrees.push(wt);
        } else {
            // Orphan worktree — main repo outside scan dir
            if let Some(mut repo) = build_repo_status(&wt.path) {
                repo.is_worktree = true;
                repo.worktree_main = Some(main_path);
                let repo_canon = wt.path.canonicalize().unwrap_or(wt.path.clone());
                main_repos.insert(repo_canon, repo);
            }
        }
    }

    let mut repos: Vec<Repo> = main_repos.into_values().collect();
    for repo in &mut repos {
        repo.worktrees.sort_by(|a, b| a.name.cmp(&b.name));
    }
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(repos)
}

/// Classify a directory entry as a normal repo, worktree, or bare container.
fn classify_entry(
    path: &Path,
    main_repos: &mut HashMap<PathBuf, Repo>,
    worktree_entries: &mut Vec<(PathBuf, WorktreeInfo)>,
) {
    let git_path = path.join(".git");
    if !git_path.exists() {
        return;
    }

    if git_path.is_dir() {
        // Normal repo (.git is a directory)
        if let Some(repo) = build_repo_status(path) {
            let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            main_repos.insert(canon, repo);
        }
        return;
    }

    // .git is a file — either a standard worktree or a bare container
    if let Some(main_path) = resolve_worktree_main(path) {
        // Standard worktree (gitdir points to .../worktrees/<name>)
        if let Some(wt) = build_worktree_info(path) {
            worktree_entries.push((main_path, wt));
        }
        return;
    }

    // Not a standard worktree — check if it's a bare container
    // (e.g., .git file contains "gitdir: ./.bare")
    if is_bare_container(path) {
        scan_bare_container(path, main_repos, worktree_entries);
    }
}

/// Check if a directory is a bare repo container.
/// A bare container has a `.git` file pointing to a local bare repo (e.g., `.bare`).
fn is_bare_container(path: &Path) -> bool {
    let git_path = path.join(".git");
    let Ok(content) = std::fs::read_to_string(&git_path) else {
        return false;
    };
    let Some(gitdir) = content.trim().strip_prefix("gitdir: ") else {
        return false;
    };
    // Resolve relative paths against the container directory
    let resolved = if Path::new(gitdir).is_relative() {
        path.join(gitdir)
    } else {
        PathBuf::from(gitdir)
    };
    // The target should be a directory (the bare repo)
    resolved.is_dir()
}

/// Scan inside a bare repo container for worktrees.
fn scan_bare_container(
    container: &Path,
    main_repos: &mut HashMap<PathBuf, Repo>,
    worktree_entries: &mut Vec<(PathBuf, WorktreeInfo)>,
) {
    let Ok(entries) = std::fs::read_dir(container) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join(".git").exists() {
            continue;
        }
        // Each child with a .git file should be a worktree of the bare repo
        if let Some(main_path) = resolve_worktree_main(&path) {
            if let Some(wt) = build_worktree_info(&path) {
                worktree_entries.push((main_path, wt));
            }
        } else {
            // Shouldn't normally happen, but handle gracefully
            if let Some(repo) = build_repo_status(&path) {
                let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                main_repos.insert(canon, repo);
            }
        }
    }
}

/// Check if a path is a git worktree (`.git` is a file, not a directory).
/// Returns the main repo path if it is a worktree.
fn resolve_worktree_main(path: &Path) -> Option<PathBuf> {
    let git_path = path.join(".git");
    if git_path.is_dir() {
        return None; // Normal repo
    }
    let content = std::fs::read_to_string(&git_path).ok()?;
    let gitdir_raw = content.trim().strip_prefix("gitdir: ")?;

    // Resolve relative paths against the worktree's parent directory
    let gitdir = if Path::new(gitdir_raw).is_relative() {
        path.join(gitdir_raw)
    } else {
        PathBuf::from(gitdir_raw)
    };

    parse_gitdir_path(&gitdir)
}

/// Extract the main repo path from a resolved gitdir path.
/// Expected: the path ends in `.../<something>/worktrees/<name>`
/// where `<something>` is either `.git` or a bare repo dir like `.bare`.
/// Returns the parent of `<something>` as the main repo path.
fn parse_gitdir_path(gitdir: &Path) -> Option<PathBuf> {
    let parent = gitdir.parent()?; // strip <name>
    if parent.file_name()?.to_str()? != "worktrees" {
        return None;
    }
    let bare_or_git_dir = parent.parent()?; // strip "worktrees" → get .git or .bare
    Some(bare_or_git_dir.parent()?.to_path_buf()) // strip ".git"/".bare" → repo root
}

/// Parse a `.git` file's content to extract the main repo path (for absolute paths only).
#[cfg(test)]
fn parse_gitdir_file(content: &str) -> Option<PathBuf> {
    let gitdir_raw = content.trim().strip_prefix("gitdir: ")?;
    parse_gitdir_path(Path::new(gitdir_raw))
}

fn build_worktree_info(path: &Path) -> Option<WorktreeInfo> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let branch = git_output(path, &["branch", "--show-current"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let dirty_files = git_output(path, &["status", "--porcelain"])
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .count() as u32;
    Some(WorktreeInfo {
        path: path.to_path_buf(),
        name,
        branch,
        dirty_files,
    })
}

/// Build a Repo from the current local git state (no fetch).
fn build_repo_status(path: &Path) -> Option<Repo> {
    let name = path.file_name()?.to_string_lossy().to_string();

    let current_branch = git_output(path, &["branch", "--show-current"])
        .unwrap_or_default()
        .trim()
        .to_string();

    let has_remote = !git_output(path, &["remote"])
        .unwrap_or_default()
        .trim()
        .is_empty();

    let default_branch = if has_remote {
        detect_default_branch(path).or_else(|| detect_local_default_branch(path))
    } else {
        detect_local_default_branch(path)
    };

    let dirty_files = git_output(path, &["status", "--porcelain"])
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .count() as u32;

    let (ahead, behind) = if has_remote {
        get_ahead_behind(path)
    } else {
        (0, 0)
    };

    let remote_url = if has_remote {
        git_output(path, &["remote", "get-url", "origin"])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    let remote_info = remote_url.as_deref().and_then(forge::parse_remote_url);
    let forge_status = if remote_info.is_some() {
        ForgeStatus::Queued
    } else {
        ForgeStatus::NotApplicable
    };

    Some(Repo {
        path: path.to_path_buf(),
        name,
        default_branch,
        current_branch,
        ahead,
        behind,
        dirty_files,
        has_remote,
        fetch_status: if has_remote {
            FetchStatus::Queued
        } else {
            FetchStatus::Done
        },
        remote_url,
        remote_info,
        forge_stats: None,
        forge_status,
        worktrees: Vec::new(),
        is_worktree: false,
        worktree_main: None,
    })
}

fn detect_default_branch(path: &Path) -> Option<String> {
    if let Some(output) = git_output(path, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        let trimmed = output.trim();
        if let Some(branch) = trimmed.strip_prefix("refs/remotes/origin/") {
            return Some(branch.to_string());
        }
    }
    for name in &["main", "master"] {
        let refspec = format!("refs/remotes/origin/{name}");
        if git_output(path, &["rev-parse", "--verify", &refspec]).is_some() {
            return Some(name.to_string());
        }
    }
    None
}

/// Detect default branch from local refs (no remote needed).
fn detect_local_default_branch(path: &Path) -> Option<String> {
    for name in &["main", "master"] {
        let refspec = format!("refs/heads/{name}");
        if git_output(path, &["rev-parse", "--verify", &refspec]).is_some() {
            return Some(name.to_string());
        }
    }
    None
}

fn get_ahead_behind(path: &Path) -> (u32, u32) {
    let output = git_output(
        path,
        &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    )
    .unwrap_or_default();

    let parts: Vec<&str> = output.trim().split('\t').collect();
    if parts.len() == 2 {
        let ahead = parts[0].parse().unwrap_or(0);
        let behind = parts[1].parse().unwrap_or(0);
        (ahead, behind)
    } else {
        (0, 0)
    }
}

/// Run `git fetch` with safety precautions.
pub(crate) fn fetch_repo(path: &Path) -> Result<(), String> {
    run_git_with_timeout(path, &["fetch", "--quiet"], GIT_TIMEOUT)?;
    Ok(())
}

/// Re-read repo status after a fetch or action.
pub(crate) fn refresh_repo_status(repo: &mut Repo) {
    if let Some(updated) = build_repo_status(&repo.path) {
        repo.current_branch = updated.current_branch;
        repo.default_branch = updated.default_branch;
        repo.ahead = updated.ahead;
        repo.behind = updated.behind;
        repo.dirty_files = updated.dirty_files;
        repo.has_remote = updated.has_remote;
    }
    // Refresh worktree info
    for wt in &mut repo.worktrees {
        wt.branch = git_output(&wt.path, &["branch", "--show-current"])
            .unwrap_or_default()
            .trim()
            .to_string();
        wt.dirty_files = git_output(&wt.path, &["status", "--porcelain"])
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.is_empty())
            .count() as u32;
    }
}

pub(crate) fn pull_repo(path: &Path) -> Result<String, String> {
    run_git_with_timeout(path, &["pull"], GIT_TIMEOUT)
}

pub(crate) fn push_repo(path: &Path) -> Result<String, String> {
    run_git_with_timeout(path, &["push"], GIT_TIMEOUT)
}

/// Run a git command with timeout and non-interactive safety env vars.
fn run_git_with_timeout(
    path: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let child = Command::new("git")
        .args(args)
        .current_dir(path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    let output = wait_with_timeout(child, timeout)?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|e| e.to_string()),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return Err("timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn git_output(path: &Path, args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gitdir_valid() {
        let content = "gitdir: /home/user/main/.git/worktrees/feat\n";
        let result = parse_gitdir_file(content).unwrap();
        assert_eq!(result, PathBuf::from("/home/user/main"));
    }

    #[test]
    fn parse_gitdir_no_trailing_newline() {
        let content = "gitdir: /repos/proj/.git/worktrees/fix-bug";
        let result = parse_gitdir_file(content).unwrap();
        assert_eq!(result, PathBuf::from("/repos/proj"));
    }

    #[test]
    fn parse_gitdir_not_a_worktree() {
        // gitdir pointing somewhere else (e.g. submodule)
        let content = "gitdir: /some/other/.git/modules/sub";
        assert!(parse_gitdir_file(content).is_none());
    }

    #[test]
    fn parse_gitdir_no_prefix() {
        assert!(parse_gitdir_file("not a gitdir line").is_none());
    }

    #[test]
    fn parse_gitdir_empty() {
        assert!(parse_gitdir_file("").is_none());
    }

    // -- Bare container worktree paths --

    #[test]
    fn parse_gitdir_path_bare_worktree() {
        // Bare container: gitdir resolves to .bare/worktrees/main
        let path = Path::new("/home/user/project/.bare/worktrees/main");
        let result = parse_gitdir_path(path).unwrap();
        assert_eq!(result, PathBuf::from("/home/user/project"));
    }

    #[test]
    fn parse_gitdir_path_standard_worktree() {
        let path = Path::new("/home/user/project/.git/worktrees/feat");
        let result = parse_gitdir_path(path).unwrap();
        assert_eq!(result, PathBuf::from("/home/user/project"));
    }

    #[test]
    fn parse_gitdir_path_not_worktree() {
        let path = Path::new("/some/other/.git/modules/sub");
        assert!(parse_gitdir_path(path).is_none());
    }

    #[test]
    fn parse_gitdir_file_bare_worktree() {
        // Absolute path variant as would appear in a .git file
        let content = "gitdir: /home/user/project/.bare/worktrees/main";
        let result = parse_gitdir_file(content).unwrap();
        assert_eq!(result, PathBuf::from("/home/user/project"));
    }
}
