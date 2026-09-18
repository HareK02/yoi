# Ticket validation image lacks `rustfmt`

While implementing T-613, `cargo fmt --all` failed before reading project sources because the active Rust toolchain does not provide the `rustfmt` component. The standalone `rustfmt` binary is also absent.

Observed commands:

```text
$ cargo fmt --all
error: 'cargo-fmt' is not installed for the toolchain '1.90.0-x86_64-unknown-linux-gnu'.

$ rustfmt --version
bash: rustfmt: command not found
```

This prevents Workers from executing the repository-required formatting check or automatically normalizing Rust changes. The Coder can still run compile/tests and `git diff --check`, but formatting evidence remains weaker and review may surface avoidable formatting differences.

Suggested improvement: include the `rustfmt` component in the standard coding Workdir/toolchain image and verify `cargo fmt --all -- --check` during image qualification.
