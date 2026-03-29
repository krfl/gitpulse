use std::path::PathBuf;

use clap::Parser;
use color_eyre::Result;

mod app;
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
    #[arg(default_value = ".")]
    path: PathBuf,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    let path = cli.path.canonicalize()?;

    let mut terminal = ratatui::init();
    let result = app::AppState::new(&path).run(&mut terminal);
    ratatui::restore();

    result
}
