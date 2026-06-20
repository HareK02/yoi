CLI resume UX を explicit `yoi resume` subcommand model に変更し、top-level bare Pod name inference と legacy `-r` / `--resume` resume path を廃止した。

主な成果:
- `yoi resume` を explicit subcommand として追加。
- `yoi resume --workspace <PATH>` で指定 workspace scoped picker を開くようにした。
- `yoi resume --all` で host/data-dir-wide Pod listing を明示 opt-in にした。
- Default `yoi resume` は workspace-scoped picker にし、stored Pod metadata `workspace_root` semantics を使う。
- Top-level bare word (`yoi agent` など) は Pod name inference ではなく unknown command になる。
- `yoi -r` / `yoi --resume` は legacy resume alias ではなく unknown argument になる。
- Explicit `yoi --pod <NAME>` path は維持。
- Help から `[POD_NAME]` と `-r, --resume` guidance を削除し、`yoi resume [--workspace <PATH>] [--all]` を案内。
- Dashboard / picker no-pods guidance を新 CLI に合わせて更新し、workspace-scoped empty result から `yoi resume --all` を案内。
- Parser / picker / workspace tests を更新。

統合・検証:
- Merge commit: `bdc11c77 merge: explicit resume command`
- Implementation commits: `861c351a`, `d25ca6ff`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p yoi parse_`, `cargo test -p tui picker_`, `cargo test -p tui workspace`, `cargo check -p yoi -p tui`, `cargo build -p yoi`, `target/debug/yoi ticket doctor`, and CLI smoke for help/resume help/unknown bare word/-r/--resume/mutually-exclusive resume args。

範囲外:
- Legacy `-r` / `--resume` compatibility alias は意図的に追加していない。
- LLM-facing Pod scope/tool visibility には変更を加えていない。