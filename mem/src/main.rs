use std::process::ExitCode;

use clap::Parser;

use mem::cli::Cli;

fn main() -> ExitCode {
    mem::exit::end_where_the_reader_did();
    let cli = Cli::parse();
    ExitCode::from(mem::run(cli) as u8)
}
