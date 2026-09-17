fn main() {
    print!(
        "{}",
        server_api::normalized_typescript(server_api::memory_api_typescript())
    );
}
