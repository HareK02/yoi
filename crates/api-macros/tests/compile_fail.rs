#[test]
fn invalid_api_declarations_have_stable_diagnostics() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/*.rs");
}
