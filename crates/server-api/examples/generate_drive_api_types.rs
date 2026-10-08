fn main() {
    print!(
        "{}",
        server_api::normalized_typescript(server_api::drive_api_typescript())
    );
}
