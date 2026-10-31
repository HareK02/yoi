fn main() {
    print!(
        "{}",
        server_api::normalized_typescript(server_api::chat_api_typescript())
    );
}
