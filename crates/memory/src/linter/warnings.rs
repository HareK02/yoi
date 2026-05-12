//! Soft warnings: low-importance large records, sources accumulation.
//!
//! Similar-slug warnings need the existing record set and are
//! integrated into the main linter pass when implemented; this file
//! covers per-write checks that only need the proposed content.

use crate::Slug;
use crate::error::LintWarning;
use crate::linter::LintReport;
use crate::linter::existing::ExistingRecords;
use crate::workspace::{ClassifiedPath, RecordKind};

const LARGE_BODY_THRESHOLD: usize = 1500;
const SOURCES_OVERFLOW_THRESHOLD: usize = 10;
const SIMILAR_SLUG_DISTANCE: usize = 2;
/// Cluster size (including the new slug) at which the similar-slug
/// warning fires. 3 follows `docs/plan/memory.md` §Linter (`類似 slug
/// 乱立`) — two existing close neighbours plus the new write.
const SIMILAR_SLUG_CLUSTER_MIN: usize = 3;

/// For kinds that don't carry a `sources` array (Summary), emit only
/// the body-size warning.
pub fn check_warnings_kindless(_cp: &ClassifiedPath, body: &str, _report: &mut LintReport) {
    let _ = body;
    // Summary intentionally has no warning band — the per-record
    // size:importance heuristic doesn't apply to a single rolling file.
}

/// For kinds with `sources` (Decisions / Requests / Knowledge), consult
/// both the body length and the sources count.
pub fn check_warnings_with_sources(body: &str, source_count: usize, report: &mut LintReport) {
    let chars = body.chars().count();
    if source_count <= 1 && chars >= LARGE_BODY_THRESHOLD {
        report.push_warning(LintWarning::LowImportanceLargeRecord { chars });
    }
    if source_count > SOURCES_OVERFLOW_THRESHOLD {
        report.push_warning(LintWarning::SourcesOverflow {
            count: source_count,
        });
    }
}

/// Emit a `SimilarSlugs` warning when the proposed slug joins a cluster
/// of `SIMILAR_SLUG_CLUSTER_MIN` or more slugs in the same kind that
/// are pairwise within `SIMILAR_SLUG_DISTANCE` Levenshtein steps of the
/// new one. The reported list includes the new slug, sorted to keep
/// the warning text deterministic.
pub fn check_similar_slugs(
    new_slug: &Slug,
    kind: RecordKind,
    existing: &ExistingRecords,
    report: &mut LintReport,
) {
    let mut neighbours: Vec<String> = existing
        .slugs(kind)
        .into_iter()
        .filter(|s| *s != new_slug)
        .filter(|s| levenshtein(new_slug.as_str(), s.as_str()) <= SIMILAR_SLUG_DISTANCE)
        .map(|s| s.to_string())
        .collect();
    if neighbours.len() + 1 < SIMILAR_SLUG_CLUSTER_MIN {
        return;
    }
    neighbours.push(new_slug.to_string());
    neighbours.sort();
    report.push_warning(LintWarning::SimilarSlugs(neighbours));
}

/// Iterative two-row Levenshtein distance over chars.
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

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("a", ""), 1);
        assert_eq!(levenshtein("", "ab"), 2);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("foo", "foo"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        assert_eq!(levenshtein("abcd", "acbd"), 2);
    }
}
