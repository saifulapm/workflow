use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(workflow::main(std::env::args().collect()) as u8)
}
