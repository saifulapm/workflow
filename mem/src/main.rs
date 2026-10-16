use std::process::ExitCode;

use clap::Parser;

use mem::cli::Cli;

fn main() -> ExitCode {
    mem::exit::end_where_the_reader_did();
    let cli = Cli::parse();
    // Parsed first, so a usage error shows here and not as the server's.
    if let Some(url) = mem::remote::url() {
        return ExitCode::from(mem::remote::run_with_local_hub(&url, &cli) as u8);
    }
    ExitCode::from(mem::run(cli) as u8)
}
