use clap::Parser;

use nest_rs_cli::cli;

fn main() {
    // `nestrs version` is the documented spelling, but `--version` / `-V` is
    // the near-universal convention and used to be rejected with `unexpected
    // argument`, which reads as a broken install. Answered here rather than
    // through clap's own flag so both spellings print the identical line.
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
