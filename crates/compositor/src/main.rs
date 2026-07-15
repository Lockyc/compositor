use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "compositor", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render the site to the output directory.
    Build {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Serve the site with live-reload, rebuilding on change.
    Serve {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to bind. Omit to let the OS pick a free one (printed on start).
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        open: bool,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Build { dir } => compositor::build::run_build(&dir),
        Command::Serve {
            dir,
            host,
            port,
            open,
        } => compositor::serve::run_serve(&dir, &host, port, open),
    }
}
