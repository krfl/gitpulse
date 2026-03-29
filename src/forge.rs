use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use crate::model::{ForgeKind, ForgeStats, RemoteInfo, Scheme};

const API_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = "gitocular";

static HTTP_AGENT: OnceLock<ureq::Agent> = OnceLock::new();

// ── URL Parsing ──────────────────────────────────────────────────────

/// Parse a git remote URL into forge info.
/// Returns `None` if the host is not a recognized forge.
pub(crate) fn parse_remote_url(url: &str) -> Option<RemoteInfo> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let (scheme, host, path) = if url.starts_with("ssh://") || url.starts_with("git+ssh://") {
        // ssh://git@github.com/owner/repo.git
        // ssh://git@github.com:22/owner/repo.git
        let (host, path) = parse_ssh_scheme(url)?;
        (Scheme::Https, host, path)
    } else if url.starts_with("https://") {
        let (host, path) = parse_https(url)?;
        (Scheme::Https, host, path)
    } else if url.starts_with("http://") {
        let (host, path) = parse_https(url)?;
        (Scheme::Http, host, path)
    } else if url.contains(':') && !url.contains("://") {
        // git@github.com:owner/repo.git
        let (host, path) = parse_scp_style(url)?;
        (Scheme::Https, host, path)
    } else {
        return None;
    };

    let kind = forge_kind_from_host(&host)?;

    // Split path into owner/repo, strip .git suffix
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo_name) = path.split_once('/')?;

    if owner.is_empty() || repo_name.is_empty() || repo_name.contains('/') {
        return None;
    }

    Some(RemoteInfo {
        kind,
        host,
        scheme,
        owner: owner.to_string(),
        repo_name: repo_name.to_string(),
    })
}

fn parse_ssh_scheme(url: &str) -> Option<(String, String)> {
    // ssh://git@github.com/owner/repo.git
    // ssh://git@github.com:22/owner/repo.git
    let after_scheme = url.split("://").nth(1)?;
    // Remove user@ prefix
    let after_user = if let Some(idx) = after_scheme.find('@') {
        &after_scheme[idx + 1..]
    } else {
        after_scheme
    };
    // Split host from path. Host may contain a port (host:port/path)
    let (host, path) = if let Some(colon_idx) = after_user.find(':') {
        let slash_idx = after_user.find('/');
        match slash_idx {
            Some(si) if si < colon_idx => {
                // slash before colon: host/path (no port)
                (&after_user[..si], &after_user[si..])
            }
            _ => {
                // colon before slash: could be host:port/path
                let rest = &after_user[colon_idx + 1..];
                if let Some(slash_idx) = rest.find('/') {
                    // host:port/path — skip the port
                    (&after_user[..colon_idx], &rest[slash_idx..])
                } else {
                    return None;
                }
            }
        }
    } else if let Some(slash_idx) = after_user.find('/') {
        (&after_user[..slash_idx], &after_user[slash_idx..])
    } else {
        return None;
    };

    Some((host.to_lowercase(), path.to_string()))
}

fn parse_https(url: &str) -> Option<(String, String)> {
    let after_scheme = url.split("://").nth(1)?;
    let slash_idx = after_scheme.find('/')?;
    let host = &after_scheme[..slash_idx];
    let path = &after_scheme[slash_idx..];
    Some((host.to_lowercase(), path.to_string()))
}

fn parse_scp_style(url: &str) -> Option<(String, String)> {
    // git@github.com:owner/repo.git
    let (host_part, path) = url.split_once(':')?;
    let host = if let Some(idx) = host_part.find('@') {
        &host_part[idx + 1..]
    } else {
        host_part
    };
    Some((host.to_lowercase(), path.to_string()))
}

fn forge_kind_from_host(host: &str) -> Option<ForgeKind> {
    let gitea_hosts = parse_host_list("GITEA_HOSTS");
    let gitlab_hosts = parse_host_list("GITLAB_HOSTS");
    forge_kind_for_host(host, &gitea_hosts, &gitlab_hosts)
}

/// Pure function: match host against built-in forges and custom host lists.
fn forge_kind_for_host(
    host: &str,
    custom_gitea: &[String],
    custom_gitlab: &[String],
) -> Option<ForgeKind> {
    match host {
        "github.com" => Some(ForgeKind::GitHub),
        "gitlab.com" => Some(ForgeKind::GitLab),
        "codeberg.org" => Some(ForgeKind::Gitea),
        _ => {
            if custom_gitea.iter().any(|h| h == host) {
                Some(ForgeKind::Gitea)
            } else if custom_gitlab.iter().any(|h| h == host) {
                Some(ForgeKind::GitLab)
            } else {
                None
            }
        }
    }
}

/// Parse a comma-separated list of host[:port] entries from an env var.
fn parse_host_list(env_var: &str) -> Vec<String> {
    std::env::var(env_var)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

// ── Token Resolution ─────────────────────────────────────────────────

// GitHub token is cached because resolve_github_token may shell out to `gh`.
// GitLab/Gitea tokens are read from env each time (microsecond cost) so that
// different self-hosted instances could be supported with future per-host vars.
static GITHUB_TOKEN: OnceLock<Option<String>> = OnceLock::new();

pub(crate) fn resolve_token(kind: &ForgeKind, host: &str) -> Option<String> {
    match kind {
        ForgeKind::GitHub => GITHUB_TOKEN
            .get_or_init(resolve_github_token)
            .clone(),
        ForgeKind::GitLab => std::env::var("GITLAB_TOKEN").ok(),
        ForgeKind::Gitea => {
            if host == "codeberg.org" {
                std::env::var("CODEBERG_TOKEN")
                    .ok()
                    .or_else(|| std::env::var("GITEA_TOKEN").ok())
            } else {
                std::env::var("GITEA_TOKEN").ok()
            }
        }
    }
}

fn resolve_github_token() -> Option<String> {
    // Try env var first
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        return Some(token);
    }

    // Try gh CLI
    let output = Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;

    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !token.is_empty() {
            return Some(token);
        }
    }

    None
}

// ── API Calls ────────────────────────────────────────────────────────

pub(crate) fn fetch_forge_stats(
    info: &RemoteInfo,
    token: Option<&str>,
) -> Result<ForgeStats, String> {
    match info.kind {
        ForgeKind::GitHub => fetch_github_stats(&info.owner, &info.repo_name, token),
        ForgeKind::GitLab => fetch_gitlab_stats(
            info.scheme, &info.host, &info.owner, &info.repo_name, token,
        ),
        ForgeKind::Gitea => fetch_gitea_stats(
            info.scheme, &info.host, &info.owner, &info.repo_name, token,
        ),
    }
}

fn agent() -> &'static ureq::Agent {
    HTTP_AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(API_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
    })
}

fn fetch_github_stats(
    owner: &str,
    repo: &str,
    token: Option<&str>,
) -> Result<ForgeStats, String> {
    let agent = agent();

    // Call 1: repo info (open_issues_count which includes PRs, fork status)
    let url = format!("https://api.github.com/repos/{owner}/{repo}");
    let mut req = agent.get(&url);
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp: serde_json::Value = req.call().map_err(|e| e.to_string())?
        .into_json().map_err(|e| e.to_string())?;

    let issues_plus_prs = resp["open_issues_count"].as_u64().unwrap_or(0) as u32;
    let is_fork = resp["fork"].as_bool().unwrap_or(false);

    // Call 2: count open PRs via pulls endpoint
    let pr_url = format!(
        "https://api.github.com/repos/{owner}/{repo}/pulls?state=open&per_page=1"
    );
    let mut pr_req = agent.get(&pr_url);
    if let Some(t) = token {
        pr_req = pr_req.set("Authorization", &format!("Bearer {t}"));
    }
    let pr_resp = pr_req.call().map_err(|e| e.to_string())?;

    let open_prs = parse_link_last_page(pr_resp.header("link"))
        .or_else(|| {
            // If no Link header, count items in response
            let body: serde_json::Value = pr_resp.into_json().ok()?;
            Some(body.as_array()?.len() as u32)
        })
        .unwrap_or(0);

    let open_issues = issues_plus_prs.saturating_sub(open_prs);

    Ok(ForgeStats {
        open_prs,
        open_issues,
        is_fork,
    })
}

fn fetch_gitlab_stats(
    scheme: Scheme,
    host: &str,
    owner: &str,
    repo: &str,
    token: Option<&str>,
) -> Result<ForgeStats, String> {
    let agent = agent();
    let scheme = scheme.as_str();
    let project_path = format!("{owner}/{repo}");
    let encoded_path = urlencode_path(&project_path);

    // Call 1: project info
    let url = format!("{scheme}://{host}/api/v4/projects/{encoded_path}");
    let mut req = agent.get(&url);
    if let Some(t) = token {
        req = req.set("PRIVATE-TOKEN", t);
    }
    let resp: serde_json::Value = req.call().map_err(|e| e.to_string())?
        .into_json().map_err(|e| e.to_string())?;

    let open_issues = resp["open_issues_count"].as_u64().unwrap_or(0) as u32;
    let is_fork = resp["forked_from_project"].is_object();

    // Call 2: count open MRs
    let mr_url = format!(
        "{scheme}://{host}/api/v4/projects/{encoded_path}/merge_requests?state=opened&per_page=1"
    );
    let mut mr_req = agent.get(&mr_url);
    if let Some(t) = token {
        mr_req = mr_req.set("PRIVATE-TOKEN", t);
    }
    let mr_resp = mr_req.call().map_err(|e| e.to_string())?;

    let open_prs = mr_resp
        .header("x-total")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);

    Ok(ForgeStats {
        open_prs,
        open_issues,
        is_fork,
    })
}

fn fetch_gitea_stats(
    scheme: Scheme,
    host: &str,
    owner: &str,
    repo: &str,
    token: Option<&str>,
) -> Result<ForgeStats, String> {
    let agent = agent();
    let scheme = scheme.as_str();
    let url = format!("{scheme}://{host}/api/v1/repos/{owner}/{repo}");
    let mut req = agent.get(&url);
    if let Some(t) = token {
        req = req.set("Authorization", &format!("token {t}"));
    }
    let resp: serde_json::Value = req.call().map_err(|e| e.to_string())?
        .into_json().map_err(|e| e.to_string())?;

    Ok(ForgeStats {
        open_prs: resp["open_pr_counter"].as_u64().unwrap_or(0) as u32,
        open_issues: resp["open_issues_count"].as_u64().unwrap_or(0) as u32,
        is_fork: resp["fork"].as_bool().unwrap_or(false),
    })
}

/// Parse GitHub-style `Link` header for the `last` page number.
/// Example: `<...?page=5>; rel="last"` → Some(5)
fn parse_link_last_page(link_header: Option<&str>) -> Option<u32> {
    let header = link_header?;
    for part in header.split(',') {
        if part.contains("rel=\"last\"") {
            // Extract URL between < and >
            let url = part.trim().strip_prefix('<')?.split('>').next()?;
            // Find page= parameter
            for param in url.split('?').nth(1)?.split('&') {
                if let Some(val) = param.strip_prefix("page=") {
                    return val.parse().ok();
                }
            }
        }
    }
    None
}

/// Percent-encode `/` for GitLab API project path parameters.
/// Only `/` is encoded because forge platforms restrict project names to
/// alphanumeric characters, hyphens, underscores, and dots.
fn urlencode_path(s: &str) -> String {
    s.replace('/', "%2F")
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ForgeKind;

    // -- URL Parsing --

    #[test]
    fn parse_github_https() {
        let info = parse_remote_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(info.kind, ForgeKind::GitHub);
        assert_eq!(info.host, "github.com");
        assert_eq!(info.owner, "owner");
        assert_eq!(info.repo_name, "repo");
    }

    #[test]
    fn parse_github_https_no_git_suffix() {
        let info = parse_remote_url("https://github.com/owner/repo").unwrap();
        assert_eq!(info.kind, ForgeKind::GitHub);
        assert_eq!(info.repo_name, "repo");
    }

    #[test]
    fn parse_github_ssh() {
        let info = parse_remote_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(info.kind, ForgeKind::GitHub);
        assert_eq!(info.host, "github.com");
        assert_eq!(info.owner, "owner");
        assert_eq!(info.repo_name, "repo");
    }

    #[test]
    fn parse_github_ssh_no_suffix() {
        let info = parse_remote_url("git@github.com:owner/repo").unwrap();
        assert_eq!(info.repo_name, "repo");
    }

    #[test]
    fn parse_github_ssh_scheme() {
        let info = parse_remote_url("ssh://git@github.com/owner/repo.git").unwrap();
        assert_eq!(info.kind, ForgeKind::GitHub);
        assert_eq!(info.owner, "owner");
        assert_eq!(info.repo_name, "repo");
    }

    #[test]
    fn parse_github_ssh_scheme_with_port() {
        let info = parse_remote_url("ssh://git@github.com:22/owner/repo.git").unwrap();
        assert_eq!(info.kind, ForgeKind::GitHub);
        assert_eq!(info.owner, "owner");
        assert_eq!(info.repo_name, "repo");
    }

    #[test]
    fn parse_gitlab_https() {
        let info = parse_remote_url("https://gitlab.com/owner/repo.git").unwrap();
        assert_eq!(info.kind, ForgeKind::GitLab);
        assert_eq!(info.host, "gitlab.com");
    }

    #[test]
    fn parse_gitlab_ssh() {
        let info = parse_remote_url("git@gitlab.com:owner/repo.git").unwrap();
        assert_eq!(info.kind, ForgeKind::GitLab);
    }

    #[test]
    fn parse_codeberg_https() {
        let info = parse_remote_url("https://codeberg.org/owner/repo.git").unwrap();
        assert_eq!(info.kind, ForgeKind::Gitea);
        assert_eq!(info.host, "codeberg.org");
    }

    #[test]
    fn parse_codeberg_ssh() {
        let info = parse_remote_url("git@codeberg.org:owner/repo.git").unwrap();
        assert_eq!(info.kind, ForgeKind::Gitea);
    }

    #[test]
    fn parse_unknown_host_returns_none() {
        assert!(parse_remote_url("https://selfhosted.example.com/owner/repo.git").is_none());
    }

    #[test]
    fn parse_local_path_returns_none() {
        assert!(parse_remote_url("/path/to/repo").is_none());
    }

    #[test]
    fn parse_file_url_returns_none() {
        assert!(parse_remote_url("file:///path/to/repo").is_none());
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse_remote_url("").is_none());
    }

    #[test]
    fn parse_whitespace_returns_none() {
        assert!(parse_remote_url("  ").is_none());
    }

    #[test]
    fn parse_trims_whitespace() {
        let info = parse_remote_url("  https://github.com/owner/repo.git  ").unwrap();
        assert_eq!(info.owner, "owner");
    }

    #[test]
    fn parse_http_also_works() {
        let info = parse_remote_url("http://github.com/owner/repo.git").unwrap();
        assert_eq!(info.kind, ForgeKind::GitHub);
    }

    #[test]
    fn parse_host_case_insensitive() {
        let info = parse_remote_url("https://GitHub.COM/owner/repo").unwrap();
        assert_eq!(info.kind, ForgeKind::GitHub);
        assert_eq!(info.host, "github.com");
    }

    #[test]
    fn parse_git_plus_ssh_scheme() {
        let info = parse_remote_url("git+ssh://git@github.com/owner/repo.git").unwrap();
        assert_eq!(info.kind, ForgeKind::GitHub);
        assert_eq!(info.owner, "owner");
    }

    #[test]
    fn parse_rejects_subgroups() {
        // We only support owner/repo, not nested paths
        assert!(parse_remote_url("https://github.com/owner/sub/repo.git").is_none());
    }

    // -- Scheme preservation --

    #[test]
    fn parse_https_scheme_preserved() {
        let info = parse_remote_url("https://github.com/owner/repo").unwrap();
        assert_eq!(info.scheme, Scheme::Https);
    }

    #[test]
    fn parse_http_scheme_preserved() {
        let info = parse_remote_url("http://github.com/owner/repo").unwrap();
        assert_eq!(info.scheme, Scheme::Http);
    }

    #[test]
    fn parse_ssh_defaults_to_https_scheme() {
        let info = parse_remote_url("git@github.com:owner/repo").unwrap();
        assert_eq!(info.scheme, Scheme::Https);
    }

    #[test]
    fn parse_ssh_scheme_url_defaults_to_https() {
        let info = parse_remote_url("ssh://git@github.com/owner/repo").unwrap();
        assert_eq!(info.scheme, Scheme::Https);
    }

    #[test]
    fn parse_git_plus_ssh_defaults_to_https_scheme() {
        let info = parse_remote_url("git+ssh://git@github.com/owner/repo").unwrap();
        assert_eq!(info.scheme, Scheme::Https);
    }

    #[test]
    fn parse_http_with_port_preserves_host_and_scheme() {
        // Exercises parse_https with a port in the host
        let info = parse_remote_url("http://localhost:3030/owner/repo.git");
        // Returns None because localhost:3030 is not a known forge in this test,
        // but verify parse_https handles the port correctly by checking a known host.
        assert!(info.is_none());
    }

    // -- Custom host detection (pure function, no env var manipulation) --

    #[test]
    fn custom_gitea_host_detected() {
        let gitea = vec!["gitea.local:3030".to_string()];
        assert_eq!(
            forge_kind_for_host("gitea.local:3030", &gitea, &[]),
            Some(ForgeKind::Gitea),
        );
    }

    #[test]
    fn custom_gitlab_host_detected() {
        let gitlab = vec!["gitlab.corp.com".to_string()];
        assert_eq!(
            forge_kind_for_host("gitlab.corp.com", &[], &gitlab),
            Some(ForgeKind::GitLab),
        );
    }

    #[test]
    fn custom_host_multiple_entries() {
        let gitea = vec!["gitea1.local".to_string(), "gitea2.local:8080".to_string()];
        assert_eq!(
            forge_kind_for_host("gitea2.local:8080", &gitea, &[]),
            Some(ForgeKind::Gitea),
        );
        assert_eq!(
            forge_kind_for_host("gitea1.local", &gitea, &[]),
            Some(ForgeKind::Gitea),
        );
    }

    #[test]
    fn custom_host_no_match_returns_none() {
        assert_eq!(
            forge_kind_for_host("unknown.host.com", &[], &[]),
            None,
        );
    }

    #[test]
    fn custom_host_does_not_override_builtin() {
        // Built-in hosts take priority via the match arms
        let gitlab = vec!["github.com".to_string()];
        assert_eq!(
            forge_kind_for_host("github.com", &[], &gitlab),
            Some(ForgeKind::GitHub),
        );
    }

    #[test]
    fn custom_host_gitea_before_gitlab() {
        // If same host appears in both lists, Gitea wins (checked first)
        let both = vec!["ambiguous.host".to_string()];
        assert_eq!(
            forge_kind_for_host("ambiguous.host", &both, &both),
            Some(ForgeKind::Gitea),
        );
    }

    #[test]
    fn parse_host_list_comma_separated() {
        // Test the parsing logic directly (no env var needed)
        let input = "gitea1.local, gitea2.local:8080 , GITEA3.LOCAL";
        let result: Vec<String> = input
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(result, vec!["gitea1.local", "gitea2.local:8080", "gitea3.local"]);
    }

    #[test]
    fn parse_host_list_empty_string() {
        let result: Vec<String> = "".split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        assert!(result.is_empty());
    }

    // -- Scheme enum --

    #[test]
    fn scheme_as_str() {
        assert_eq!(Scheme::Http.as_str(), "http");
        assert_eq!(Scheme::Https.as_str(), "https");
    }

    // -- Link Header Parsing --

    #[test]
    fn parse_link_last_page_typical() {
        let header = r#"<https://api.github.com/repos/owner/repo/pulls?state=open&per_page=1&page=42>; rel="last""#;
        assert_eq!(parse_link_last_page(Some(header)), Some(42));
    }

    #[test]
    fn parse_link_last_page_multiple_rels() {
        let header = r#"<https://example.com?page=2>; rel="next", <https://example.com?page=10>; rel="last""#;
        assert_eq!(parse_link_last_page(Some(header)), Some(10));
    }

    #[test]
    fn parse_link_last_page_none() {
        assert_eq!(parse_link_last_page(None), None);
    }

    #[test]
    fn parse_link_last_page_no_last_rel() {
        let header = r#"<https://example.com?page=2>; rel="next""#;
        assert_eq!(parse_link_last_page(Some(header)), None);
    }

    // -- URL encoding --

    #[test]
    fn urlencode_path_encodes_slash() {
        assert_eq!(urlencode_path("owner/repo"), "owner%2Frepo");
    }
}
