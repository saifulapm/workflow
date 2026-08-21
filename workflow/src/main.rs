use std::process::ExitCode;

fn main() -> ExitCode {
    workflow::exit::end_where_the_reader_did();
    ExitCode::from(workflow::main(std::env::args().collect()) as u8)
}
