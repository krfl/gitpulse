# Gitocular

A TUI dashboard for monitoring git repository status. Inspired by
[kando-tui](https://github.com/krfl/kando-tui), Gitocular borrows the visual
language of a kanban board — repos are sorted into columns by their sync state
so you can see at a glance which ones need attention and which are clean.

## Why a kanban board?

Managing dozens of repos means constantly asking "did I push that?", "is this
one behind?", "do I have uncommitted work somewhere?". Gitocular answers all of
those questions in a single view. Repos that need work land in the leftmost
columns; repos that are clean sit on the right. The further left a card is, the
more urgently it needs your attention.

```
┌─ Behind ─────┐┌─ Uncommitted ─┐┌─ In Sync ─────┐┌─ Ahead ───────┐
│ api-server   ││ dotfiles      ││ cli-tools     ││ blog          │
│ branch:main  ││ branch:main   ││ branch:main   ││ branch:main   │
│ ↓3           ││ *5            ││ ✓             ││ ↑2            │
│ PR 1 Issue 3 ││               ││ PR 0  Issue 0 ││               │
└──────────────┘└───────────────┘└───────────────┘└───────────────┘
```

## Features

### Repository scanning

Point Gitocular at a directory and it finds all git repos one level deep,
classifies each into one of four columns:

| Column | Meaning |
|---|---|
| **Behind** | Remote has commits you haven't pulled |
| **Uncommitted** | You have local changes (dirty working tree) |
| **In Sync** | Clean and up to date with remote |
| **Ahead** | You have commits that haven't been pushed |

Column priority is Behind > Uncommitted > Ahead > In Sync. When a repo matches
multiple states, it lands in the highest-priority column and shows the secondary
states as tags on the card.

### Background git fetch

On startup Gitocular fetches all repos in the background with bounded concurrency
(8 threads, 15-second timeout). Interactive prompts are suppressed
(`GIT_TERMINAL_PROMPT=0`, `ssh -o BatchMode=yes`) so a stuck credential helper
won't block the dashboard.

### Pull, push, and shell

From the dashboard you can pull, push, or drop into a shell for any selected
repo — all without leaving the TUI. Pull and push run in background threads so
the interface stays responsive.

### Forge integration

Gitocular fetches stats from your forge and shows them directly on each card:

- **Open PRs** (green) and **open issues** (red)
- **Fork** status

Supported forges:

| Forge | Auth |
|---|---|
| GitHub | `GITHUB_TOKEN` or `gh auth token` |
| GitLab | `GITLAB_TOKEN` |
| Codeberg | `CODEBERG_TOKEN` or `GITEA_TOKEN` |
| Gitea | `GITEA_TOKEN` |

#### Self-hosted instances

Gitocular recognizes GitHub, GitLab, and Codeberg by their hostnames automatically.
For self-hosted Gitea or GitLab instances, register the host via environment
variables:

| Variable | Purpose |
|---|---|
| `GITEA_HOSTS` | Comma-separated list of Gitea/Forgejo hostnames (e.g. `localhost:3030,gitea.lan`) |
| `GITLAB_HOSTS` | Comma-separated list of self-hosted GitLab hostnames (e.g. `gitlab.corp.com`) |

The URL scheme (`http` or `https`) is inferred from the git remote URL, so
`http://` remotes will use plain HTTP for API calls.

GitHub Enterprise is not currently supported — only `github.com` is recognized.

Each forge type uses a single token (`GITEA_TOKEN`, `GITLAB_TOKEN`), so if you
have multiple self-hosted instances of the same type they will share the same
credentials.

API calls run on a separate thread pool (4 workers) and won't block navigation.

### Worktree support

Gitocular understands git worktrees:

- Standard worktrees are grouped under their main repo
- Bare container repos (`.git` → `.bare`) are scanned one level deeper
- Worktree cards show `parentrepo [branch]` with the branch in cyan
- Press `s` on a repo with worktrees to get a shell picker

### Sort modes

Press `v` to cycle the sort order within each column:

1. **Name** — alphabetical (default)
2. **PRs** — most open pull requests first
3. **Issues** — most open issues first

## Keybindings

### Navigation

| Key | Action |
|---|---|
| `h` / `←` | Move to previous column |
| `l` / `→` | Move to next column |
| `j` / `↓` | Move down within column |
| `k` / `↑` | Move up within column |
| `Tab` | Cycle through all repos (wraps) |
| `Shift+Tab` | Reverse cycle through all repos |

### Actions

| Key | Action |
|---|---|
| `Enter` | Open detail overlay |
| `p` | Pull selected repo |
| `P` | Push selected repo |
| `s` | Open shell in repo (or shell picker for worktrees) |
| `r` | Refresh — re-scan and fetch |
| `v` | Cycle sort mode |
| `?` | Toggle help screen |
| `Esc` / `q` | Close overlay or quit |
| `Ctrl+C` | Force quit |

## Installation

### From source

```sh
# Requires Rust 1.86+
cargo install --path .
```

## Usage

```sh
# Scan the current directory
gitocular

# Scan a specific directory
gitocular ~/projects
```

### Authentication

Set environment variables to enable forge integration:

```sh
export GITHUB_TOKEN="ghp_..."
export GITLAB_TOKEN="glpat-..."
export GITEA_TOKEN="..."
```

For GitHub, if no token is set Gitocular falls back to `gh auth token`.

For self-hosted instances, also set the host variables:

```sh
export GITEA_HOSTS="localhost:3030"
export GITLAB_HOSTS="gitlab.corp.com"
```

## Built with

- [ratatui](https://ratatui.rs/) + [crossterm](https://github.com/crossterm-rs/crossterm) — terminal UI
- [clap](https://github.com/clap-rs/clap) — CLI argument parsing
- [ureq](https://github.com/algesten/ureq) — HTTP client for forge APIs
- [color-eyre](https://github.com/eyre-rs/color-eyre) — error reporting

## License

Apache-2.0
