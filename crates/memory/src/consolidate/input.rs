//! Phase 2 sub-Worker への最初のユーザー入力を組み立てる。
//!
//! Phase 1 (`extract::build_extract_input`) と同じ方針で、固定 schema の
//! markdown セクション列にしてサブWorker に渡す。`docs/plan/memory.md`
//! §Phase 2 入力 / §整理材料 の項目に従い:
//!
//! 1. consumed staging エントリ全文（`source` 込み）
//! 2. 既存 `memory/*` 全文（summary / decisions / requests）
//! 3. Knowledge 化候補レポート（メトリクス未完なら空）
//! 4. 整理材料（Linter Warn ベース、メトリクス未完なら明示 invoke 頻度なし）
//!
//! 既存 `knowledge/*` 本文は埋めず、agent に `KnowledgeQuery` 経由で引かせる
//! 設計（`docs/plan/memory.md` §retrieval 経路 / §Phase 2 の Knowledge アクセス）。

use std::fmt::Write;

use crate::consolidate::staging::StagingEntry;
use crate::consolidate::tidy::TidyHints;
use crate::workspace::{RecordKind, WorkspaceLayout};

/// Knowledge 化候補レポート。`tickets/memory-usage-metrics.md` の成果物が
/// 出るまでは空で渡す前提（`docs/plan/memory.md` §Knowledge 化候補レポート）。
/// 空入力時、統合 phase は新規 Knowledge を作らず decisions / requests /
/// summary / 既存 Knowledge update に留まる。
#[derive(Debug, Default, Clone)]
pub struct KnowledgeCandidateReport {
    /// 候補に上がった `(kind, slug, frequency_per_mtoken)` の三つ組。
    /// 空配列を渡すと「候補なし」を意味する。
    pub entries: Vec<KnowledgeCandidateEntry>,
}

#[derive(Debug, Clone)]
pub struct KnowledgeCandidateEntry {
    pub source_kind: &'static str,
    pub source_slug: String,
    pub frequency_per_mtoken: f64,
}

impl KnowledgeCandidateReport {
    pub fn empty() -> Self {
        Self::default()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Phase 2 sub-Worker の最初の user 入力。
pub fn build_consolidate_input(
    layout: &WorkspaceLayout,
    staging: &[StagingEntry],
    tidy: &TidyHints,
    candidates: &KnowledgeCandidateReport,
) -> String {
    let mut out = String::new();
    out.push_str(
        "Phase 2 consolidation input. Run the consolidation phase first \
         (fold the staging activity logs into memory and knowledge), then the \
         tidy phase (clean up existing records). Use the memory tools for \
         every write — direct file writes are denied by the pod scope.\n\n",
    );

    out.push_str("## Staging entries (consumed by this run)\n\n");
    out.push_str(&render_staging_records(staging));
    out.push('\n');

    out.push_str("## Existing memory records (full content)\n\n");
    out.push_str(&render_existing_memory_records(layout));
    out.push('\n');

    out.push_str("## Knowledge candidate report\n\n");
    out.push_str(&render_candidate_report(candidates));
    out.push('\n');

    out.push_str("## Tidy hints\n\n");
    out.push_str(&render_tidy_hints(tidy));
    out.push('\n');

    out.push_str(
        "When done, end the turn with a short final assistant message describing \
         what changed.",
    );
    out
}

/// Staging エントリ群を「`### <id>` ヘッダ + 整形 JSON ブロック」で並べる。
/// 空配列なら「(none)」と書く。
pub fn render_staging_records(entries: &[StagingEntry]) -> String {
    if entries.is_empty() {
        return "(none)\n".to_string();
    }
    let mut out = String::new();
    for entry in entries {
        let _ = writeln!(&mut out, "### {}", entry.id);
        let json = serde_json::to_string_pretty(&entry.record).unwrap_or_else(|_| "{}".into());
        out.push_str("```json\n");
        out.push_str(&json);
        out.push_str("\n```\n\n");
    }
    out
}

/// `<workspace>/.insomnia/memory/{summary.md,decisions/*,requests/*}` を
/// 「`### <kind>:<slug>` ヘッダ + raw markdown ブロック」で全文渡す。
pub fn render_existing_memory_records(layout: &WorkspaceLayout) -> String {
    let mut out = String::new();

    let summary = layout.summary_path();
    if let Ok(content) = std::fs::read_to_string(&summary) {
        out.push_str("### summary\n");
        out.push_str("```markdown\n");
        out.push_str(content.trim_end_matches('\n'));
        out.push_str("\n```\n\n");
    }

    push_kind_records(&mut out, layout, RecordKind::Decision);
    push_kind_records(&mut out, layout, RecordKind::Request);

    if out.is_empty() {
        return "(none)\n".to_string();
    }
    out
}

fn push_kind_records(out: &mut String, layout: &WorkspaceLayout, kind: RecordKind) {
    let dir = match kind {
        RecordKind::Decision => layout.decisions_dir(),
        RecordKind::Request => layout.requests_dir(),
        RecordKind::Knowledge | RecordKind::Summary | RecordKind::Workflow => return,
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(_) => return,
    };
    let mut paths: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        paths.push((stem.to_string(), path));
    }
    paths.sort();
    for (slug, path) in paths {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let _ = writeln!(out, "### {}:{}", kind.as_str(), slug);
        out.push_str("```markdown\n");
        out.push_str(content.trim_end_matches('\n'));
        out.push_str("\n```\n\n");
    }
}

fn render_candidate_report(report: &KnowledgeCandidateReport) -> String {
    if report.is_empty() {
        return "(empty — usage metrics pipeline not populated. \
                Do not create new Knowledge records this run.)\n"
            .to_string();
    }
    let mut out = String::new();
    for c in &report.entries {
        let _ = writeln!(
            &mut out,
            "- {} `{}` — frequency {:.3} invokes/Mtoken",
            c.source_kind, c.source_slug, c.frequency_per_mtoken
        );
    }
    out
}

/// Tidy hints の Markdown 描画。空ヒントなら "(none)" 1 行。
pub fn render_tidy_hints(tidy: &TidyHints) -> String {
    if tidy.is_empty() {
        return "(none)\n".to_string();
    }
    let mut out = String::new();

    if !tidy.replaced_decisions.is_empty() {
        out.push_str("**Replaced decisions still on disk** — collapse if the chain has settled:\n");
        for (slug, replaced_by) in &tidy.replaced_decisions {
            match replaced_by {
                Some(target) => {
                    let _ = writeln!(&mut out, "- `{slug}` → `{target}`");
                }
                None => {
                    let _ = writeln!(&mut out, "- `{slug}` (no `replaced_by` set)");
                }
            }
        }
        out.push('\n');
    }

    if !tidy.sources_overflow.is_empty() {
        out.push_str(
            "**Sources overflow** — consider trimming to the most recent entries (git log keeps the rest):\n",
        );
        for s in &tidy.sources_overflow {
            let _ = writeln!(
                &mut out,
                "- {} `{}` ({} sources)",
                s.kind.as_str(),
                s.slug,
                s.count
            );
        }
        out.push('\n');
    }

    if !tidy.similar_slug_clusters.is_empty() {
        out.push_str("**Similar slug clusters** — evaluate for merge / rename:\n");
        for c in &tidy.similar_slug_clusters {
            let joined = c
                .slugs
                .iter()
                .map(|s| format!("`{s}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(&mut out, "- {}: {}", c.kind.as_str(), joined);
        }
        out.push('\n');
    }

    out.push_str(
        "Explicit-invoke metrics (protection threshold) are not yet wired up; \
         skip drop on long-standing records when uncertain.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consolidate::tidy::{SimilarSlugCluster, SourcesOverflow};
    use crate::extract::{ExtractedPayload, write_staging};
    use crate::schema::SourceRef;
    use chrono::Utc;
    use std::path::Path;

    fn now() -> String {
        Utc::now().to_rfc3339()
    }

    fn write(p: &Path, content: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn build_includes_all_sections_when_populated() {
        let dir = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());

        write(
            &dir.path().join(".insomnia/memory/summary.md"),
            &format!("---\nupdated_at: {n}\n---\nstate of the world\n", n = now()),
        );
        write(
            &dir.path().join(".insomnia/memory/decisions/dec.md"),
            &format!(
                "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\nbody\n",
                n = now()
            ),
        );
        let (_id, _) = write_staging(
            &layout,
            SourceRef {
                session_id: "s".into(),
                range: [0, 1],
            },
            ExtractedPayload::default(),
        )
        .unwrap();
        let staging = crate::consolidate::staging::list_staging_entries(&layout);
        let tidy = TidyHints {
            replaced_decisions: [("old".to_string(), Some("new".to_string()))]
                .into_iter()
                .collect(),
            sources_overflow: vec![SourcesOverflow {
                kind: RecordKind::Decision,
                slug: "dec".into(),
                count: 12,
            }],
            similar_slug_clusters: vec![SimilarSlugCluster {
                kind: RecordKind::Decision,
                slugs: vec!["a".into(), "ab".into()],
            }],
        };
        let report = KnowledgeCandidateReport::empty();

        let out = build_consolidate_input(&layout, &staging, &tidy, &report);
        assert!(out.contains("Staging entries"));
        assert!(out.contains("Existing memory records"));
        assert!(out.contains("Knowledge candidate report"));
        assert!(out.contains("Tidy hints"));
        assert!(out.contains("state of the world"));
        assert!(out.contains("decision:dec"));
        assert!(out.contains("Replaced decisions"));
        assert!(out.contains("Sources overflow"));
        assert!(out.contains("Similar slug clusters"));
        assert!(out.contains("usage metrics pipeline not populated"));
    }

    #[test]
    fn empty_inputs_render_placeholders() {
        let dir = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        let out = build_consolidate_input(
            &layout,
            &[],
            &TidyHints::default(),
            &KnowledgeCandidateReport::empty(),
        );
        // Both staging and tidy show "(none)"; existing memory records too.
        assert!(out.contains("Staging entries"));
        assert!(out.contains("(none)"));
    }
}
