use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use tokio::net::TcpListener;
use yoi_workspace_server::{ServerConfig, SqliteWorkspaceStore, serve};

#[derive(Debug)]
struct ServeOptions {
    workspace: PathBuf,
    db: Option<PathBuf>,
    frontend: Option<PathBuf>,
    listen: SocketAddr,
}

#[derive(Debug)]
struct CliError(String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("yoi-workspace-server: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let Some((command, rest)) = args.split_first() else {
        print_help();
        return Ok(());
    };

    match command.as_str() {
        "serve" => {
            if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
                print_serve_help();
                return Ok(());
            }
            let options = parse_serve_options(rest)?;
            run_serve(options).await?;
            Ok(())
        }
        "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(Box::new(CliError(format!(
            "unknown command `{other}`; expected `serve`"
        )))),
    }
}

async fn run_serve(options: ServeOptions) -> Result<(), Box<dyn std::error::Error>> {
    let db = options
        .db
        .unwrap_or_else(|| options.workspace.join(".yoi/workspace.db"));
    if let Some(parent) = db.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let store = Arc::new(SqliteWorkspaceStore::open(&db)?);
    let mut config = ServerConfig::local_dev(&options.workspace);
    config.static_assets_dir = options.frontend;
    let listener = TcpListener::bind(options.listen).await?;
    eprintln!(
        "yoi-workspace-server: serving workspace `{}` on http://{}",
        options.workspace.display(),
        listener.local_addr()?
    );
    serve(config, store, listener).await?;
    Ok(())
}

fn parse_serve_options(args: &[String]) -> Result<ServeOptions, CliError> {
    let mut workspace = std::env::current_dir()
        .map_err(|error| CliError(format!("failed to resolve current directory: {error}")))?;
    let mut db = None;
    let mut frontend = None;
    let mut listen = "127.0.0.1:8787".parse::<SocketAddr>().unwrap();

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--workspace" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError("--workspace requires a value".to_string()))?;
                workspace = PathBuf::from(value);
            }
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError("--db requires a value".to_string()))?;
                db = Some(PathBuf::from(value));
            }
            "--frontend" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError("--frontend requires a value".to_string()))?;
                frontend = Some(PathBuf::from(value));
            }
            "--listen" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError("--listen requires a value".to_string()))?;
                listen = parse_listen(value)?;
            }
            _ if arg.starts_with("--workspace=") => {
                workspace = PathBuf::from(value_after_equals(arg, "--workspace")?);
            }
            _ if arg.starts_with("--db=") => {
                db = Some(PathBuf::from(value_after_equals(arg, "--db")?));
            }
            _ if arg.starts_with("--frontend=") => {
                frontend = Some(PathBuf::from(value_after_equals(arg, "--frontend")?));
            }
            _ if arg.starts_with("--listen=") => {
                listen = parse_listen(value_after_equals(arg, "--listen")?)?;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError(format!("unknown serve option `{arg}`")));
            }
            _ => {
                return Err(CliError(format!(
                    "unexpected positional argument `{arg}`; use --workspace <PATH>"
                )));
            }
        }
        index += 1;
    }

    let workspace = workspace.canonicalize().map_err(|error| {
        CliError(format!(
            "failed to canonicalize workspace `{}`: {error}",
            workspace.display()
        ))
    })?;

    Ok(ServeOptions {
        workspace,
        db,
        frontend,
        listen,
    })
}

fn value_after_equals<'a>(arg: &'a str, flag: &str) -> Result<&'a str, CliError> {
    let value = arg
        .strip_prefix(flag)
        .and_then(|rest| rest.strip_prefix('='))
        .unwrap_or_default();
    if value.is_empty() {
        return Err(CliError(format!("{flag} requires a value")));
    }
    Ok(value)
}

fn parse_listen(value: &str) -> Result<SocketAddr, CliError> {
    value
        .parse()
        .map_err(|_| CliError(format!("invalid --listen address `{value}`")))
}

fn print_help() {
    println!(
        "yoi-workspace-server\n\nUsage:\n  yoi-workspace-server serve [OPTIONS]\n\nOptions:\n  -h, --help    Print help"
    );
}

fn print_serve_help() {
    println!(
        "yoi-workspace-server serve\n\nUsage:\n  yoi-workspace-server serve [OPTIONS]\n\nOptions:\n      --workspace <PATH>  Workspace root containing .yoi project records (defaults to cwd)\n      --db <PATH>         SQLite database path (defaults to <workspace>/.yoi/workspace.db)\n      --frontend <PATH>   Static SPA build directory to serve\n      --listen <ADDR>     Listen address (defaults to 127.0.0.1:8787)\n  -h, --help              Print help"
    );
}
