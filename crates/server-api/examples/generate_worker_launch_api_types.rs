fn main() {
    print!(
        "{}",
        server_api::normalized_typescript(server_api::worker_launch_api_typescript())
    );
}
