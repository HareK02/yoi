# lint-common

Shared frontmatter splitting and slug validation for Markdown records. Only syntax handling is shared; owning crates retain validation of record-specific required fields and semantics.

## Main entry points

- [`split_frontmatter`] and [`Frontmatter`] — frontmatter/body boundary
- [`Slug`] and [`is_valid_slug`] — identifier syntax validation
- [`RecordLintError`] — shared syntax errors
