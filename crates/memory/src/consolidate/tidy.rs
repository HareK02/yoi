//! 整理 phase が prompt 入力に乗せる「整理材料」スキャナ。
//!
//! `docs/plan/memory.md` §整理（GC 相当）の扱い と
//! `tickets/memory-phase2-consolidation.md` の整理材料リストに従い、
//! メトリクス未完の現状で機械的に拾えるヒントだけを集める:
//!
//! - `replaced` chain: `status: replaced` の Decision とその `replaced_by`
//! - sources 過多: `sources` / `last_sources` 配列が閾値超過の record
//! - 類似 slug 乱立: 同 kind の slug が Levenshtein 2 以内のクラスター
//!
//! 使用頻度メトリクスベースの保護閾値情報は `tickets/memory-usage-metrics.md`
//! の成果物が出るまで空で渡る。

use std::collections::{BTreeMap, BTreeSet};

use crate::schema::{
    DecisionFrontmatter, KnowledgeFrontmatter, RequestFrontmatter, split_frontmatter,
};
use crate::slug::Slug;
use crate::workspace::{RecordKind, WorkspaceLayout};

/// `sources` overflow を flag する閾値。`linter::warnings::SOURCES_OVERFLOW_THRESHOLD`
/// と同値（10）を踏襲する。Linter Warn で sources 過多が検出されるラインと
/// 整理 phase で勧告するラインを揃える狙い。
pub const SOURCES_OVERFLOW_THRESHOLD: usize = 10;
/// 類似 slug クラスタリングの距離。`linter::warnings::SIMILAR_SLUG_DISTANCE`
/// と同値。
pub const SIMILAR_SLUG_DISTANCE: usize = 2;

/// 整理 phase 用の機械集計ヒント。空フィールドは「対象なし」を意味する。
#[derive(Debug, Default, Clone)]
pub struct TidyHints {
    /// `status: replaced` で残っている Decision の slug → `replaced_by` map。
    /// `replaced_by` が None でも置き換え滞留として列挙する。
    pub replaced_decisions: BTreeMap<String, Option<String>>,
    /// kind / slug / sources count の三つ組で sources 累積ラインを表す。
    pub sources_overflow: Vec<SourcesOverflow>,
    /// 同 kind 内で Levenshtein 距離 `<= SIMILAR_SLUG_DISTANCE` のクラスター。
    /// クラスター内の slug は sorted。
    pub similar_slug_clusters: Vec<SimilarSlugCluster>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcesOverflow {
    pub kind: RecordKind,
    pub slug: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimilarSlugCluster {
    pub kind: RecordKind,
    pub slugs: Vec<String>,
}

impl TidyHints {
    pub fn is_empty(&self) -> bool {
        self.replaced_decisions.is_empty()
            && self.sources_overflow.is_empty()
            && self.similar_slug_clusters.is_empty()
    }
}

/// workspace を一通りスキャンして [`TidyHints`] を組み立てる。読めない /
/// parse できない record は黙ってスキップ（Linter は write 経路で守って
/// いるので、ここで顕在化してもどうしようもない）。
pub fn collect_tidy_hints(layout: &WorkspaceLayout) -> TidyHints {
    let mut hints = TidyHints::default();

    let decisions = read_kind_records(layout, RecordKind::Decision);
    let requests = read_kind_records(layout, RecordKind::Request);
    let knowledge = read_kind_records(layout, RecordKind::Knowledge);

    for (slug, content) in &decisions {
        let fm = parse_yaml::<DecisionFrontmatter>(content);
        if let Some(fm) = fm.as_ref() {
            if matches!(fm.status, crate::schema::DecisionStatus::Replaced) {
                hints
                    .replaced_decisions
                    .insert(slug.clone(), fm.replaced_by.as_ref().map(|s| s.to_string()));
            }
            if fm.sources.len() > SOURCES_OVERFLOW_THRESHOLD {
                hints.sources_overflow.push(SourcesOverflow {
                    kind: RecordKind::Decision,
                    slug: slug.clone(),
                    count: fm.sources.len(),
                });
            }
        }
    }
    for (slug, content) in &requests {
        if let Some(fm) = parse_yaml::<RequestFrontmatter>(content) {
            if fm.sources.len() > SOURCES_OVERFLOW_THRESHOLD {
                hints.sources_overflow.push(SourcesOverflow {
                    kind: RecordKind::Request,
                    slug: slug.clone(),
                    count: fm.sources.len(),
                });
            }
        }
    }
    for (slug, content) in &knowledge {
        if let Some(fm) = parse_yaml::<KnowledgeFrontmatter>(content) {
            if fm.last_sources.len() > SOURCES_OVERFLOW_THRESHOLD {
                hints.sources_overflow.push(SourcesOverflow {
                    kind: RecordKind::Knowledge,
                    slug: slug.clone(),
                    count: fm.last_sources.len(),
                });
            }
        }
    }
    hints.sources_overflow.sort_by(|a, b| {
        (a.kind.as_str(), a.slug.as_str()).cmp(&(b.kind.as_str(), b.slug.as_str()))
    });

    let decision_slugs: Vec<&str> = decisions.keys().map(|s| s.as_str()).collect();
    let request_slugs: Vec<&str> = requests.keys().map(|s| s.as_str()).collect();
    let knowledge_slugs: Vec<&str> = knowledge.keys().map(|s| s.as_str()).collect();
    if let Some(c) = cluster_similar(&decision_slugs, RecordKind::Decision) {
        hints.similar_slug_clusters.extend(c);
    }
    if let Some(c) = cluster_similar(&request_slugs, RecordKind::Request) {
        hints.similar_slug_clusters.extend(c);
    }
    if let Some(c) = cluster_similar(&knowledge_slugs, RecordKind::Knowledge) {
        hints.similar_slug_clusters.extend(c);
    }
    hints
        .similar_slug_clusters
        .sort_by(|a, b| (a.kind.as_str(), &a.slugs).cmp(&(b.kind.as_str(), &b.slugs)));

    hints
}

/// `<root>/.insomnia/memory/<kind>/*.md` (Knowledge は
/// `<root>/.insomnia/knowledge/*.md`) を slug ごとに `(slug, full content)`
/// 化して返す。
fn read_kind_records(layout: &WorkspaceLayout, kind: RecordKind) -> BTreeMap<String, String> {
    let dir = match kind {
        RecordKind::Decision => layout.decisions_dir(),
        RecordKind::Request => layout.requests_dir(),
        RecordKind::Knowledge => layout.knowledge_dir(),
        RecordKind::Summary | RecordKind::Workflow => return BTreeMap::new(),
    };
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
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
        if Slug::parse(stem).is_err() {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        out.insert(stem.to_string(), content);
    }
    out
}

fn parse_yaml<F: serde::de::DeserializeOwned>(content: &str) -> Option<F> {
    let (yaml, _body) = split_frontmatter(content).ok()?;
    serde_yaml::from_str::<F>(yaml).ok()
}

/// Connected-component clustering over the `levenshtein <= SIMILAR_SLUG_DISTANCE`
/// graph among same-kind slugs. Returns each cluster of size >= 2 (singleton
/// clusters are not interesting for the integration phase). Returns `None`
/// when there are no clusters at all.
fn cluster_similar(slugs: &[&str], kind: RecordKind) -> Option<Vec<SimilarSlugCluster>> {
    if slugs.len() < 2 {
        return None;
    }
    let n = slugs.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] == i {
            i
        } else {
            let root = find(parent, parent[i]);
            parent[i] = root;
            root
        }
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }
    for i in 0..n {
        for j in (i + 1)..n {
            if levenshtein(slugs[i], slugs[j]) <= SIMILAR_SLUG_DISTANCE {
                union(&mut parent, i, j);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(slugs[i].to_string());
    }
    let mut out: Vec<SimilarSlugCluster> = Vec::new();
    let mut seen_canonical: BTreeSet<Vec<String>> = BTreeSet::new();
    for (_, mut group) in groups {
        if group.len() < 2 {
            continue;
        }
        group.sort();
        if seen_canonical.insert(group.clone()) {
            out.push(SimilarSlugCluster { kind, slugs: group });
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Iterative two-row Levenshtein distance over chars (matches the Linter's
/// implementation; kept private to avoid widening that crate-internal API).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn workspace() -> (tempfile::TempDir, WorkspaceLayout) {
        let dir = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        (dir, layout)
    }

    #[test]
    fn collects_replaced_chain() {
        let (dir, layout) = workspace();
        write(
            &dir.path().join(".insomnia/memory/decisions/replaced.md"),
            &format!(
                "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: replaced\nreplaced_by: winner\n---\n",
                n = now()
            ),
        );
        write(
            &dir.path().join(".insomnia/memory/decisions/winner.md"),
            &format!(
                "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\n",
                n = now()
            ),
        );
        let hints = collect_tidy_hints(&layout);
        assert_eq!(
            hints.replaced_decisions.get("replaced").cloned(),
            Some(Some("winner".into()))
        );
        assert!(!hints.replaced_decisions.contains_key("winner"));
    }

    #[test]
    fn flags_sources_overflow() {
        let (dir, layout) = workspace();
        let many_sources: String = (0..15)
            .map(|i| format!("  - session_id: s{i}\n    range: [{i}, {i}]\n"))
            .collect();
        write(
            &dir.path().join(".insomnia/memory/decisions/big.md"),
            &format!(
                "---\ncreated_at: {n}\nupdated_at: {n}\nstatus: open\nsources:\n{m}---\n",
                n = now(),
                m = many_sources
            ),
        );
        let hints = collect_tidy_hints(&layout);
        assert_eq!(hints.sources_overflow.len(), 1);
        assert_eq!(hints.sources_overflow[0].slug, "big");
        assert_eq!(hints.sources_overflow[0].kind, RecordKind::Decision);
        assert_eq!(hints.sources_overflow[0].count, 15);
    }

    #[test]
    fn clusters_similar_slugs() {
        let (dir, layout) = workspace();
        for slug in ["db-pool", "db-pol", "db-pools", "alpha"] {
            write(
                &dir.path()
                    .join(format!(".insomnia/memory/decisions/{slug}.md")),
                &format!(
                    "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\n",
                    n = now()
                ),
            );
        }
        let hints = collect_tidy_hints(&layout);
        assert_eq!(hints.similar_slug_clusters.len(), 1);
        assert_eq!(
            hints.similar_slug_clusters[0].slugs,
            vec![
                "db-pol".to_string(),
                "db-pool".to_string(),
                "db-pools".to_string(),
            ]
        );
    }

    #[test]
    fn empty_workspace_yields_empty_hints() {
        let (_dir, layout) = workspace();
        let hints = collect_tidy_hints(&layout);
        assert!(hints.is_empty());
    }
}
