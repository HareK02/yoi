fn main() {
    print!(
        "{}",
        server_api::normalized_typescript(server_api::repository_access_api_typescript())
    );
}
