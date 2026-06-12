# テスト妥当性レビュー: workflow
- 判定: 概ね良い

## 確認範囲
- Crate: `crates/workflow`
- 確認した責務:
  - Workflow フロントマターの分割と schema defaults
  - 人間編集向け Workflow linter
  - builtin resources と `.yoi/workflow` からの Workflow discovery/loading
  - `SKILL.md` parsing と Skill-to-Workflow projection
  - Workflow registry の collision/shadowing behavior
  - Workflow directory write-deny scope helper
- 主に読んだファイル:
  - `crates/workflow/README.md`
  - `crates/workflow/Cargo.toml`
  - `crates/workflow/src/{lib.rs,schema.rs,linter.rs,workflow.rs,skill.rs,scope.rs,error.rs}`
  - 狭い integration 参照: Workflow invocation resolution 周辺の `crates/pod/src/workflow/mod.rs` tests

## 現在のテストがよくカバーしていること
- この crate には、純粋な parsing/loading 責務に対する堅実な unit-test set がある: `cargo test -p workflow` は 34 unit tests を実行し、すべて成功している。
- Workflow loading は妥当にカバーされている:
  - valid workspace Workflow loading と default flags
  - `model_invokation` / `user_invocable` behavior
  - workspace Workflow が slug で builtin Workflow を override すること
  - 少なくとも 1 つの builtin Workflow の builtin provenance
  - invalid filename が hard error になること
  - required `description` の欠落が hard error になること
  - legacy `.yoi/memory/workflow` が無視されること
  - resident description cap enforcement
- Registry/Skill collision behavior がカバーされている:
  - collision がない場合の insertion
  - workspace Workflow が Skill を shadow すること
  - first-fed Skill が later-fed Skill に勝つこと
  - human-readable shadow message の smoke check
- `SKILL.md` parsing は主要な invariants をよくカバーしている:
  - minimal valid skill
  - directory/name mismatch
  - invalid slug names
  - empty description
  - description が cap ちょうど、および cap 超過の場合
  - missing frontmatter
  - `allowed-tools` を含む optional spec fields が受け入れられること
  - scan behavior: missing root、deterministic ordering、broken sibling を skip しつつ good sibling を保持すること
  - Skill-to-Workflow defaults
- Linter tests は現在の重要な linter checks をカバーしている:
  - required Knowledge が存在する valid file
  - missing required Knowledge
  - resident description cap
  - Workflow body size limit
- テストは temporary directories を使い、private implementation details よりも observable API behavior を主に assert しており、この crate には適切である。

## ギャップ / 疑問のあるテスト
- Builtin Workflow coverage はやや弱い。`missing_directory_loads_builtin_registry` は unrelated slug が存在しないことだけを確認しており、`builtin_workflow_records_have_visible_provenance` は `multi-agent-workflow` だけを確認している。別のテストがたまたま触れない限り、1 つの builtin slug が削除または misconfigure される regression を見逃す可能性がある。
- Loader と linter の invariants がテスト上で完全には揃っていない。linter は `WORKFLOW_BODY_LIMIT` を enforce するが、`load_workflows` は現在 enforce していない。body size が runtime/load invariant の意図なら loader test がない。意図的に lint-only なら、その境界を明示したままにすべきである。
- `requires` validation は linter と後段の `pod` invocation resolution でのみテストされており、`load_workflows` ではテストされていない。これは意図的かもしれないが、crate tests だけでは責務分担が明確ではない。
- linter の Knowledge existence check は filename-stem ベースである。テストは malformed Knowledge files、invalid Knowledge filenames、または「一致する `.md` file が存在する」と「valid Knowledge record が存在する」の違いをカバーしていない。意図する invariant が「valid Knowledge record」なら、現在のテストは許容しすぎている。
- 複数の hard-error paths が十分にテストされていない:
  - Workflow files の malformed YAML frontmatter
  - Workflow files の missing frontmatter
  - `SKILL.md` の missing `name` / missing `description`
  - `.yoi/workflow` 配下の non-`.md` files と subdirectories
  - Workflow description cap と body cap の exact boundary
- `skill::tests::invalid_slug_name_is_error` は必要以上に緩い: fixture は uppercase の directory/name が一致しているため、期待される error は `InvalidName` であるべき。`NameDirMismatch` も許容すると、validation ordering や fixture の regression を隠してしまう。
- `skill::tests::extra_frontmatter_fields_are_kept` は名前がやや不自然である: parsed `SkillRecord` はそれらの optional fields を保持しない。このテストが実際に assert しているのは「optional/spec-compatible fields are accepted and ignored」であり、それ自体は妥当だが、より直接的に表現すべきである。
- crate-level tests は、「workspace Workflows を load し、configured directories から Skills を load し、registry に merge し、shadowed Skills を report する」という full pipeline を exercise していない。隣接する一部 behavior は `pod` でカバーされているが、`workflow` crate 自体では pieces を独立にテストしているだけである。

## 追加するとよいもの
- builtin assertions を強化する:
  - 期待されるすべての builtin slugs が存在することを assert する
  - それらの `source`, `path`, `model_invokation`, `user_invocable`, および選択した `requires` values を assert する
  - builtin resource changes が明確に fail するよう、小さな contract test として維持する
- Workflow loader の negative/edge tests を追加する:
  - missing frontmatter
  - malformed YAML
  - `model_invokation: true` のとき description cap ちょうどが accepted になること
  - non-`.md` files が ignored されること
  - subdirectories が ignored されること
  - workspace Workflow が `requires` values を preserve すること
  - body limit が lint-only なのか load-time でもあるのかをテストで明確にする
- linter edge tests を追加する:
  - body が `WORKFLOW_BODY_LIMIT` ちょうどなら accepted
  - resident description が cap ちょうどなら accepted
  - 複数の `requires` がすべての missing references を report すること
  - invalid `requires` slug が malformed frontmatter または slug parse error になること
  - valid Knowledge parsing が意図されている場合、malformed Knowledge record behavior
- Skill tests を引き締める:
  - matching invalid directory/name に対して invalid slug test が `SkillParseError::InvalidName` を assert するようにする
  - missing `name` と missing `description` を追加する
  - optional-field test を、それらの fields が accepted/ignored されることを表す名前または内容にする
- parsed Skills からの registry assembly に対する crate-level integration-style unit test を 1 つ追加する:
  - builtin/workspace registry を load する
  - 2 つの Skill directories を parse する
  - priority order で merge する
  - accepted Skill、shadowed Skill、および結果の user-invocable/resident entries を assert する

## 実行したコマンド
- `cargo test -p workflow`
  - 結果: passed
  - 34 unit tests passed, 0 failed; doc-tests: 0 tests
- `cargo test -p workflow -- --list`
  - 結果: passed
  - 34 unit tests, 0 benchmarks を確認; doc-tests: 0 tests
- `git status --short`
  - Read-only check; この review の変更外に既存の modified paths があることを示した:
    - `.yoi/tickets/00001KTVPS6K3/item.md`
    - `.yoi/tickets/00001KTVPS6K3/thread.md`
    - `crates/tui/src/multi_pod.rs`

