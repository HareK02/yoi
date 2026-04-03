#[test]
fn compile_fail_state_constraints() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/cache_locked_register_tool.rs");
    t.compile_fail("tests/ui/tool_server_handle_register_tool.rs");
}
