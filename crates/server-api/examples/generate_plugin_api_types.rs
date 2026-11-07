fn main() {
    print!(
        "{}",
        server_api::normalized_typescript(server_api::plugin_api_typescript())
    );
}
