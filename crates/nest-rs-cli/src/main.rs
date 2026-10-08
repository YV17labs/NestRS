#![expect(
    clippy::print_stderr,
    reason = "a command-line tool's output is its interface; tracing is for the apps it scaffolds"
)]

use clap::Parser;

use nest_rs_cli::cli;

fn main() {
    // `--version` / `-V` answered here rather than through clap's flag, so both
    // spellings print what `nestrs version` prints.
    if std::env::args()
        .nth(1)
        .is_some_and(|a| a == "--version" || a == "-V")
        && std::env::args().count() == 2
    {
        cli::print_version();
        return;
    }
    let cli = cli::Cli::parse();
    if let Err(err) = cli::run(cli) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
