use std::path::PathBuf;

use clap::{Parser, Subcommand};
use color_eyre::Result;

mod app;
mod cli;
mod forge;
mod git;
mod model;
mod ui;

#[derive(Parser)]
#[command(
    name = "gitocular",
    about = "A TUI dashboard for monitoring git repository status"
)]
struct Cli {
    /// Directory to scan for git repos (defaults to current directory)
    #[arg(short, long, default_value = ".", global = true)]
    path: PathBuf,

    /// Emit machine-readable JSON output
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List repos grouped by sync state
    List,
    /// Fetch all repos that have remotes
    Fetch,
    /// Pull all repos that are behind their upstream
    Pull,
    /// Print a summary of repo sync states
    Status,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let path = cli.path.canonicalize()?;

    match cli.command {
        None => {
            let mut terminal = ratatui::init();
            let result = app::AppState::new(&path).run(&mut terminal);
            ratatui::restore();
            result
        }
        Some(Command::Status) => cli::cmd_status(&path, cli.json),
        Some(Command::List) => cli::cmd_list(&path, cli.json),
        Some(Command::Fetch) => cli::cmd_fetch(&path, cli.json),
        Some(Command::Pull) => cli::cmd_pull(&path, cli.json),
    }
}
