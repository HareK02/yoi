fn main() {
    print!(
        "{}",
        server_api::normalized_typescript(server_api::legacy_catalog_typescript())
    );
}
