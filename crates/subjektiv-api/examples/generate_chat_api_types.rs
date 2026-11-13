fn main() {
    let source = subjektiv_api::chat_api_typescript();
    let normalized = source
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    println!("{}", normalized.trim_end());
}
