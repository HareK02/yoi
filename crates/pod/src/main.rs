use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    pod::entrypoint::run_cli().await
}
