use std::env;
use std::io::{self, BufRead, Write};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

fn main() {
    let mode = env::var("YOI_MCP_MOCK_MODE").unwrap_or_else(|_| "success".to_string());
    match mode.as_str() {
        "success" => success(),
        "fail-init" => fail_init(),
        "sampling" => sampling_request(),
        "shutdown-hang" => shutdown_hang(),
        other => panic!("unknown mock mode: {other}"),
    }
}

fn success() {
    let init = read_json();
    assert_eq!(init["method"], "initialize");
    assert!(init["params"]["capabilities"].get("sampling").is_none());
    assert!(init["params"]["capabilities"].get("elicitation").is_none());
    write_json(json!({
        "jsonrpc": "2.0",
        "id": init["id"],
        "result": initialize_result(),
    }));
    let initialized = read_json();
    assert_eq!(initialized["method"], "notifications/initialized");
    drain_stdin();
}

fn fail_init() {
    let secret = env::var("MCP_TEST_SECRET").unwrap_or_default();
    for idx in 0..5 {
        eprintln!("diagnostic {idx}: secret={secret}");
    }
    let init = read_json();
    write_json(json!({
        "jsonrpc": "2.0",
        "id": init["id"],
        "error": {
            "code": -32000,
            "message": format!("init rejected with {secret}"),
        }
    }));
}

fn sampling_request() {
    let init = read_json();
    write_json(json!({
        "jsonrpc": "2.0",
        "id": init["id"],
        "result": initialize_result(),
    }));
    let initialized = read_json();
    assert_eq!(initialized["method"], "notifications/initialized");
    write_json(json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "sampling/createMessage",
        "params": {},
    }));
    let response = read_json();
    assert_eq!(response["id"], 99);
    assert_eq!(response["error"]["code"], -32601);
}

fn shutdown_hang() {
    let init = read_json();
    write_json(json!({
        "jsonrpc": "2.0",
        "id": init["id"],
        "result": initialize_result(),
    }));
    let initialized = read_json();
    assert_eq!(initialized["method"], "notifications/initialized");
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2025-11-25",
        "capabilities": {
            "tools": { "listChanged": true }
        },
        "serverInfo": {
            "name": "mock-mcp",
            "version": "0.1.0"
        }
    })
}

fn read_json() -> Value {
    let mut line = String::new();
    let read = io::stdin().lock().read_line(&mut line).expect("read stdin");
    assert_ne!(read, 0, "stdin closed before JSON-RPC message");
    serde_json::from_str(&line).expect("valid JSON-RPC line")
}

fn write_json(value: Value) {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &value).expect("write JSON");
    stdout.write_all(b"\n").expect("write newline");
    stdout.flush().expect("flush stdout");
}

fn drain_stdin() {
    let mut line = String::new();
    while io::stdin().lock().read_line(&mut line).unwrap_or(0) != 0 {
        line.clear();
    }
}
