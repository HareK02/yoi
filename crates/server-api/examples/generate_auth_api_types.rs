fn main() {
    print!(
        "{}",
        server_api::normalized_typescript(server_api::auth_api_typescript())
    );
}
