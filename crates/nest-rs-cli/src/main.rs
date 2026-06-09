mod cli;
mod commands;
mod context;
mod error;
mod naming;
mod port;
mod scaffold;
mod templates;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    if let Err(err) = cli::run(cli) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
