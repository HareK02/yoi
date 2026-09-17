fn main() {
    print!(
        "{}",
        server_api::normalized_typescript(server_api::workdir_api_typescript())
    );
}
