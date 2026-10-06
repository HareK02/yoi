//! Pure text-operation policy shared by filesystem providers and operation frontends.
//!
//! This module owns no paths, descriptors, version fences, or I/O. Limits count
//! UTF-8 bytes, not characters, and default to unlimited.

use schemars::JsonSchema;
use serde::{Deserialize, de::DeserializeOwned};
use thiserror::Error;

/// The shared text Operation definitions, independent of paths and frontends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOperation {
    Read,
    ReadLines,
    Edit,
    Write,
}

impl TextOperation {
    /// Resolve a lower-case Operation name. A frontend chooses whether its
    /// `read` Operation accepts whole-text or line-range arguments.
    pub fn from_name(name: &str, line_read: bool) -> Option<Self> {
        match name {
            "read" if line_read => Some(Self::ReadLines),
            "read" => Some(Self::Read),
            "edit" => Some(Self::Edit),
            "write" => Some(Self::Write),
            _ => None,
        }
    }

    pub fn argument_schema(self) -> schemars::Schema {
        match self {
            Self::Read => schemars::schema_for!(ReadArgs),
            Self::ReadLines => schemars::schema_for!(LineReadArgs),
            Self::Edit => schemars::schema_for!(EditArgs),
            Self::Write => schemars::schema_for!(WriteArgs),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct EditArgs {
    /// String to replace. Must be nonempty and unique unless replace_all is true.
    pub old_string: String,
    /// Replacement text. Must differ from old_string; may be empty to delete it.
    pub new_string: String,
    /// Replace every non-overlapping occurrence. Defaults to false.
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct WriteArgs {
    /// Full text to write.
    pub content: String,
}

/// Whole-text reads accept no operation-specific arguments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct LineReadArgs {
    /// Zero-based line offset. Omitted or null means zero.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum lines to return. Omitted or null uses the frontend's default.
    #[serde(default)]
    pub limit: Option<usize>,
}

impl LineReadArgs {
    pub fn range(&self, default_limit: usize) -> (usize, usize) {
        (
            self.offset.unwrap_or(0),
            self.limit.unwrap_or(default_limit).max(1),
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextLimits {
    pub max_input_bytes: Option<usize>,
    pub max_output_bytes: Option<usize>,
    pub max_replacements: Option<usize>,
}

#[derive(Debug, Error)]
pub enum TextError {
    #[error("invalid arguments: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("old_string must not be empty")]
    EmptyOldString,
    #[error("old_string and new_string are identical")]
    IdenticalStrings,
    #[error("old_string was not found")]
    NotFound,
    #[error(
        "old_string matched {occurrences} times; set replace_all=true or provide a unique string"
    )]
    MultipleMatches { occurrences: usize },
    #[error("input size {bytes} exceeds limit {limit}")]
    InputTooLarge { bytes: usize, limit: usize },
    #[error("output size {bytes} exceeds limit {limit}")]
    OutputTooLarge { bytes: usize, limit: usize },
    #[error("replacement count {replacements} exceeds limit {limit}")]
    ReplacementLimitExceeded { replacements: usize, limit: usize },
    #[error("edited content size overflowed")]
    Overflow,
}

pub fn decode<T: DeserializeOwned>(value: serde_json::Value) -> Result<T, TextError> {
    serde_json::from_value(value).map_err(TextError::Decode)
}

/// Decode raw Tool JSON directly, retaining duplicate-field rejection rather
/// than losing that information through an intermediate JSON object.
pub fn decode_json<T: DeserializeOwned>(input: &str) -> Result<T, TextError> {
    serde_json::from_str(input).map_err(TextError::Decode)
}

impl EditArgs {
    /// Validate argument-only policy before a caller acquires a preimage.
    pub fn validate(&self, limits: TextLimits) -> Result<(), TextError> {
        if self.old_string.is_empty() {
            return Err(TextError::EmptyOldString);
        }
        if self.old_string == self.new_string {
            return Err(TextError::IdenticalStrings);
        }
        check_input_bytes(self.old_string.len(), limits)?;
        check_input_bytes(self.new_string.len(), limits)?;
        Ok(())
    }
}

/// A numbered, bounded text response with provider line metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub body: String,
    pub line_count: usize,
    pub total_lines: usize,
    pub truncated: bool,
}

const RENDERED_TRUNCATION_MARKER: &str = "\n[truncated at rendered output byte limit]\n";

/// Render selected text with one-based line numbers and a hard UTF-8 byte bound.
///
/// As in the Tool renderer, room for the truncation marker is always reserved,
/// and a partially emitted numbered line counts as a line. Tiny bounds emit
/// only the marker prefix that fits. Line numbers saturate rather than wrap.
pub fn render_numbered(
    text: &str,
    start_line: usize,
    total_lines: usize,
    truncated: bool,
    max_bytes: usize,
) -> Rendered {
    let budget = max_bytes.saturating_sub(RENDERED_TRUNCATION_MARKER.len());
    let mut body = String::with_capacity(text.len().min(max_bytes));
    let mut line_count = 0usize;
    let mut output_truncated = false;
    for (index, line) in text.lines().enumerate() {
        // Build only the small numeric prefix, not an unbounded copy of a line.
        let number = start_line.saturating_add(index).saturating_add(1);
        let prefix = format!("{number:>6}\t");
        let numbered_len = prefix.len().saturating_add(line.len()).saturating_add(1);
        let mut remaining = budget.saturating_sub(body.len());
        if remaining > 0 {
            line_count = line_count.saturating_add(1);
        }
        let line_truncated = numbered_len > remaining;
        for fragment in [prefix.as_str(), line, "\n"] {
            let mut end = remaining.min(fragment.len());
            while !fragment.is_char_boundary(end) {
                end -= 1;
            }
            body.push_str(&fragment[..end]);
            remaining -= end;
            // Do not skip a character that failed to fit and then emit later
            // fragments; the response must be a prefix of the numbered line.
            if end < fragment.len() {
                break;
            }
        }
        if line_truncated {
            let marker_len = max_bytes
                .saturating_sub(body.len())
                .min(RENDERED_TRUNCATION_MARKER.len());
            body.push_str(&RENDERED_TRUNCATION_MARKER[..marker_len]);
            output_truncated = true;
            break;
        }
    }
    Rendered {
        body,
        line_count,
        total_lines,
        truncated: start_line > 0 || truncated || output_truncated,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOutcome {
    pub content: String,
    pub replacements: usize,
    pub bytes_written: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextOutcome {
    pub content: String,
    pub bytes: usize,
}

pub fn edit(original: &str, args: &EditArgs, limits: TextLimits) -> Result<EditOutcome, TextError> {
    args.validate(limits)?;
    check_input_bytes(original.len(), limits)?;
    let occurrences = original.matches(&args.old_string).count();
    if occurrences == 0 {
        return Err(TextError::NotFound);
    }
    if !args.replace_all && occurrences != 1 {
        return Err(TextError::MultipleMatches { occurrences });
    }
    let replacements = occurrences;
    if let Some(limit) = limits.max_replacements
        && replacements > limit
    {
        return Err(TextError::ReplacementLimitExceeded {
            replacements,
            limit,
        });
    }
    // All arithmetic and output policy are checked before allocating the result.
    let bytes_written = edited_len(
        original.len(),
        args.old_string.len(),
        args.new_string.len(),
        replacements,
    )?;
    check_output_bytes(bytes_written, limits)?;
    let mut content = String::with_capacity(bytes_written);
    let mut cursor = 0;
    for (start, matched) in original.match_indices(&args.old_string) {
        content.push_str(&original[cursor..start]);
        content.push_str(&args.new_string);
        cursor = start + matched.len();
    }
    content.push_str(&original[cursor..]);
    debug_assert_eq!(content.len(), bytes_written);
    Ok(EditOutcome {
        content,
        replacements,
        bytes_written,
    })
}

fn edited_len(
    original: usize,
    old: usize,
    new: usize,
    replacements: usize,
) -> Result<usize, TextError> {
    let removed = old.checked_mul(replacements).ok_or(TextError::Overflow)?;
    let added = new.checked_mul(replacements).ok_or(TextError::Overflow)?;
    let bytes = original
        .checked_sub(removed)
        .and_then(|bytes| bytes.checked_add(added))
        .ok_or(TextError::Overflow)?;
    // String/Vec capacity is bounded by isize::MAX even on platforms where usize
    // arithmetic still fits. Reject that case before String::with_capacity.
    if bytes > isize::MAX as usize {
        return Err(TextError::Overflow);
    }
    Ok(bytes)
}

pub fn write(content: String, limits: TextLimits) -> Result<TextOutcome, TextError> {
    text_outcome(content, limits)
}

pub fn read(content: String, limits: TextLimits) -> Result<TextOutcome, TextError> {
    text_outcome(content, limits)
}

fn text_outcome(content: String, limits: TextLimits) -> Result<TextOutcome, TextError> {
    let bytes = content.len();
    check_input_bytes(bytes, limits)?;
    check_output_bytes(bytes, limits)?;
    Ok(TextOutcome { content, bytes })
}

pub(crate) fn check_input_bytes(bytes: usize, limits: TextLimits) -> Result<(), TextError> {
    if let Some(limit) = limits.max_input_bytes
        && bytes > limit
    {
        return Err(TextError::InputTooLarge { bytes, limit });
    }
    Ok(())
}

pub(crate) fn check_output_bytes(bytes: usize, limits: TextLimits) -> Result<(), TextError> {
    if let Some(limit) = limits.max_output_bytes
        && bytes > limit
    {
        return Err(TextError::OutputTooLarge { bytes, limit });
    }
    Ok(())
}

/// Keep valid UTF-8 prefixes intact without changing byte-read behavior on
/// genuinely invalid UTF-8. Providers must remain able to return binary data.
pub(crate) fn bounded_utf8_len(bytes: &[u8], max_bytes: usize) -> usize {
    let mut end = max_bytes.min(bytes.len());
    while end > 0 {
        match std::str::from_utf8(&bytes[..end]) {
            Ok(_) => break,
            Err(error) if error.error_len().is_none() => end -= 1,
            Err(_) => break,
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(old: &str, new: &str, replace_all: bool) -> EditArgs {
        EditArgs {
            old_string: old.into(),
            new_string: new.into(),
            replace_all,
        }
    }

    #[test]
    fn decode_uses_shared_argument_defaults_and_types() {
        let edit = decode::<EditArgs>(json!({"old_string": "a", "new_string": "b"})).unwrap();
        assert!(!edit.replace_all);
        assert!(
            decode::<EditArgs>(json!({"old_string": "a", "new_string": "b", "replace_all": true}))
                .unwrap()
                .replace_all
        );
        assert_eq!(
            decode::<WriteArgs>(json!({"content": "😀"}))
                .unwrap()
                .content,
            "😀"
        );
        assert_eq!(decode::<ReadArgs>(json!({})).unwrap(), ReadArgs {});
        for value in [
            json!({"old_string": "a"}),
            json!({"old_string": 2, "new_string": "b"}),
            json!({"old_string": "a", "new_string": "b", "replace_all": null}),
        ] {
            assert!(matches!(
                decode::<EditArgs>(value),
                Err(TextError::Decode(_))
            ));
        }
        assert!(matches!(
            decode::<WriteArgs>(json!({})),
            Err(TextError::Decode(_))
        ));
        assert!(matches!(
            decode::<ReadArgs>(json!({"offset": 0})),
            Err(TextError::Decode(_))
        ));
        assert!(matches!(
            decode::<ReadArgs>(json!(null)),
            Err(TextError::Decode(_))
        ));
    }

    #[test]
    fn line_range_preserves_null_defaults_and_minimum_one() {
        for value in [json!({}), json!({"offset": null, "limit": null})] {
            assert_eq!(
                decode::<LineReadArgs>(value).unwrap().range(2000),
                (0, 2000)
            );
        }
        assert_eq!(
            decode::<LineReadArgs>(json!({"offset": 7, "limit": 0}))
                .unwrap()
                .range(99),
            (7, 1)
        );
        assert_eq!(LineReadArgs::default().range(0), (0, 1));
        assert_eq!(
            LineReadArgs {
                offset: Some(usize::MAX),
                limit: Some(usize::MAX)
            }
            .range(1),
            (usize::MAX, usize::MAX)
        );
        for value in [
            json!({"offset": -1}),
            json!({"limit": "2"}),
            json!({"limit": 1.5}),
        ] {
            assert!(matches!(
                decode::<LineReadArgs>(value),
                Err(TextError::Decode(_))
            ));
        }
    }

    #[test]
    fn schema_is_the_argument_contract() {
        let edit = serde_json::to_value(schemars::schema_for!(EditArgs)).unwrap();
        assert_eq!(edit["required"], json!(["old_string", "new_string"]));
        assert_eq!(edit["properties"]["replace_all"]["default"], json!(false));
        let read = serde_json::to_value(schemars::schema_for!(ReadArgs)).unwrap();
        assert_eq!(read["additionalProperties"], json!(false));
        assert!(
            read.get("properties")
                .is_none_or(|properties| properties == &json!({}))
        );
        let lines = serde_json::to_value(schemars::schema_for!(LineReadArgs)).unwrap();
        assert_eq!(
            lines["properties"]["offset"]["type"],
            json!(["integer", "null"])
        );
        assert_eq!(lines["properties"]["limit"]["default"], json!(null));
    }

    #[test]
    fn edit_rejects_empty_identical_missing_and_ambiguous_matches() {
        assert!(matches!(
            args("", "x", true).validate(TextLimits::default()),
            Err(TextError::EmptyOldString)
        ));
        assert!(matches!(
            edit("a", &args("a", "a", false), TextLimits::default()),
            Err(TextError::IdenticalStrings)
        ));
        assert!(matches!(
            edit("a", &args("b", "c", false), TextLimits::default()),
            Err(TextError::NotFound)
        ));
        assert!(matches!(
            edit("aa", &args("a", "b", false), TextLimits::default()),
            Err(TextError::MultipleMatches { occurrences: 2 })
        ));
        assert!(matches!(
            edit("", &args("a", "", false), TextLimits::default()),
            Err(TextError::NotFound)
        ));
    }

    #[test]
    fn edit_handles_unique_all_deletion_literal_and_nonoverlapping_matches() {
        for (original, old, new, all, expected, replacements) in [
            (
                "before old after",
                "old",
                "new",
                false,
                "before new after",
                1,
            ),
            ("aaa", "aa", "x", true, "xa", 1),
            ("a-a-a", "a", "", true, "--", 3),
            ("[a].* [a].*", "[a].*", "$1", true, "$1 $1", 2),
            ("x", "x", "", false, "", 1),
        ] {
            let result = edit(original, &args(old, new, all), TextLimits::default()).unwrap();
            assert_eq!(result.content, expected);
            assert_eq!(result.replacements, replacements);
            assert_eq!(result.bytes_written, expected.len());
        }
    }

    #[test]
    fn edit_preserves_unicode_and_counts_bytes() {
        let result = edit("é😀\né😀", &args("é😀", "界", true), TextLimits::default()).unwrap();
        assert_eq!(result.content, "界\n界");
        assert_eq!(result.replacements, 2);
        assert_eq!(result.bytes_written, 7);
        // Matching is literal, without Unicode normalization.
        assert!(matches!(
            edit("e\u{301}", &args("é", "x", false), TextLimits::default()),
            Err(TextError::NotFound)
        ));
    }

    #[test]
    fn edit_input_limits_cover_arguments_and_preimage_with_inclusive_boundaries() {
        let limits = TextLimits {
            max_input_bytes: Some(4),
            ..Default::default()
        };
        args("😀", "a", false).validate(limits).unwrap();
        assert!(matches!(
            args("😀x", "a", false).validate(limits),
            Err(TextError::InputTooLarge { bytes: 5, limit: 4 })
        ));
        assert!(matches!(
            args("a", "😀x", false).validate(limits),
            Err(TextError::InputTooLarge { bytes: 5, limit: 4 })
        ));
        assert!(matches!(
            edit("aaaaa", &args("a", "b", true), limits),
            Err(TextError::InputTooLarge { bytes: 5, limit: 4 })
        ));
        assert_eq!(
            edit("aaaa", &args("a", "b", true), limits).unwrap().content,
            "bbbb"
        );
    }

    #[test]
    fn edit_output_and_replacement_limits_apply_before_constructing_result() {
        let args = args("a", "😀", true);
        let limits = TextLimits {
            max_output_bytes: Some(7),
            ..Default::default()
        };
        assert!(matches!(
            edit("aa", &args, limits),
            Err(TextError::OutputTooLarge { bytes: 8, limit: 7 })
        ));
        assert_eq!(
            edit(
                "aa",
                &args,
                TextLimits {
                    max_output_bytes: Some(8),
                    ..limits
                }
            )
            .unwrap()
            .bytes_written,
            8
        );
        assert!(matches!(
            edit(
                "aa",
                &args,
                TextLimits {
                    max_replacements: Some(1),
                    ..Default::default()
                }
            ),
            Err(TextError::ReplacementLimitExceeded {
                replacements: 2,
                limit: 1
            })
        ));
        assert!(matches!(
            edit(
                "a",
                &args,
                TextLimits {
                    max_replacements: Some(0),
                    ..Default::default()
                }
            ),
            Err(TextError::ReplacementLimitExceeded {
                replacements: 1,
                limit: 0
            })
        ));
        assert_eq!(
            edit(
                "aa",
                &args,
                TextLimits {
                    max_replacements: Some(2),
                    ..Default::default()
                }
            )
            .unwrap()
            .replacements,
            2
        );
        assert_eq!(
            edit(
                "a",
                &EditArgs {
                    old_string: "a".into(),
                    new_string: String::new(),
                    replace_all: false
                },
                TextLimits {
                    max_output_bytes: Some(0),
                    ..Default::default()
                }
            )
            .unwrap()
            .content,
            ""
        );
    }

    #[test]
    fn raw_json_decode_preserves_duplicate_argument_rejection() {
        for input in [
            r#"{"old_string":"a","old_string":"b","new_string":"c"}"#,
            r#"{"old_string":"a","new_string":"b","replace_all":false,"replace_all":true}"#,
        ] {
            assert!(matches!(
                decode_json::<EditArgs>(input),
                Err(TextError::Decode(_))
            ));
        }
        assert!(matches!(
            decode_json::<WriteArgs>(r#"{"content":"a","content":"b"}"#),
            Err(TextError::Decode(_))
        ));
    }

    #[test]
    fn size_arithmetic_rejects_every_overflow_before_preallocation() {
        // Exercise impossible-to-allocate sizes through the exact helper edit
        // invokes before allocating; no giant strings need to be constructed.
        for (original, old, new, count) in [
            (usize::MAX, usize::MAX, 0, 2),     // removed multiplication
            (2, 1, usize::MAX, 2),              // added multiplication
            (1, 2, 0, 1),                       // subtraction
            (usize::MAX, 1, 2, 1),              // addition
            (1, 1, isize::MAX as usize + 1, 1), // allocation capacity
        ] {
            assert!(matches!(
                edited_len(original, old, new, count),
                Err(TextError::Overflow)
            ));
        }
        assert_eq!(
            edited_len(1, 1, isize::MAX as usize, 1).unwrap(),
            isize::MAX as usize
        );
        assert_eq!(edited_len(8, 2, 1, 3).unwrap(), 5);
    }

    #[test]
    fn read_and_write_preserve_whole_text_and_enforce_both_byte_limits() {
        for operation in [read, write] {
            assert_eq!(
                operation(String::new(), TextLimits::default()).unwrap(),
                TextOutcome {
                    content: String::new(),
                    bytes: 0
                }
            );
            let content = "é\r\n😀\n";
            let limits = TextLimits {
                max_input_bytes: Some(content.len()),
                max_output_bytes: Some(content.len()),
                max_replacements: Some(0),
            };
            let result = operation(content.into(), limits).unwrap();
            assert_eq!(result.content, content);
            assert_eq!(result.bytes, content.len());
            assert!(matches!(
                operation(
                    "😀".into(),
                    TextLimits {
                        max_input_bytes: Some(3),
                        ..Default::default()
                    }
                ),
                Err(TextError::InputTooLarge { bytes: 4, limit: 3 })
            ));
            assert!(matches!(
                operation(
                    "😀".into(),
                    TextLimits {
                        max_output_bytes: Some(3),
                        ..Default::default()
                    }
                ),
                Err(TextError::OutputTooLarge { bytes: 4, limit: 3 })
            ));
            assert!(matches!(
                operation(
                    "a".into(),
                    TextLimits {
                        max_output_bytes: Some(0),
                        ..Default::default()
                    }
                ),
                Err(TextError::OutputTooLarge { bytes: 1, limit: 0 })
            ));
        }
    }

    #[test]
    fn utf8_byte_bounds_preserve_binary_compatibility() {
        for (limit, expected) in [
            (0, 0),
            (1, 1),
            (2, 1),
            (3, 1),
            (4, 1),
            (5, 5),
            (usize::MAX, 6),
        ] {
            assert_eq!(bounded_utf8_len("a😀z".as_bytes(), limit), expected);
        }
        assert_eq!(bounded_utf8_len(&[0xff, 0xfe, 0x00], 2), 2);
    }

    #[test]
    fn operation_names_and_schemas_are_shared_authority() {
        assert_eq!(
            TextOperation::from_name("read", false),
            Some(TextOperation::Read)
        );
        assert_eq!(
            TextOperation::from_name("read", true),
            Some(TextOperation::ReadLines)
        );
        for line_read in [false, true] {
            assert_eq!(
                TextOperation::from_name("edit", line_read),
                Some(TextOperation::Edit)
            );
            assert_eq!(
                TextOperation::from_name("write", line_read),
                Some(TextOperation::Write)
            );
            for name in ["Read", "WRITE", "read_lines", "stat", "", " read"] {
                assert_eq!(TextOperation::from_name(name, line_read), None);
            }
        }
        for (operation, expected) in [
            (TextOperation::Read, schemars::schema_for!(ReadArgs)),
            (
                TextOperation::ReadLines,
                schemars::schema_for!(LineReadArgs),
            ),
            (TextOperation::Edit, schemars::schema_for!(EditArgs)),
            (TextOperation::Write, schemars::schema_for!(WriteArgs)),
        ] {
            assert_eq!(operation.argument_schema(), expected);
        }
        let schema = serde_json::to_value(TextOperation::Edit.argument_schema()).unwrap();
        assert!(
            schema["properties"]["new_string"]["description"]
                .as_str()
                .unwrap()
                .contains("Must differ")
        );
    }

    #[test]
    fn renderer_preserves_numbering_lines_and_provider_metadata() {
        let result = render_numbered("alpha\r\n\nβeta\n", 0, 3, false, 1024);
        assert_eq!(result.body, "     1\talpha\n     2\t\n     3\tβeta\n");
        assert_eq!(result.line_count, 3);
        assert_eq!(result.total_lines, 3);
        assert!(!result.truncated);
        let result = render_numbered("last", 9, 10, false, 1024);
        assert_eq!(result.body, "    10\tlast\n");
        assert!(result.truncated);
        assert!(render_numbered("last", 0, 10, true, 1024).truncated);
        assert_eq!(
            render_numbered("", 0, 0, false, 0),
            Rendered {
                body: String::new(),
                line_count: 0,
                total_lines: 0,
                truncated: false,
            }
        );
        assert!(render_numbered("", 1, 1, false, 0).truncated);
    }

    #[test]
    fn renderer_reserves_marker_and_counts_partial_lines_as_before() {
        let marker = RENDERED_TRUNCATION_MARKER;
        let result = render_numbered("a\nb", 0, 2, false, marker.len() + 9);
        assert_eq!(result.body, format!("     1\ta\n{marker}"));
        assert_eq!(result.line_count, 1);
        assert_eq!(result.total_lines, 2);
        assert!(result.truncated);
        let result = render_numbered("a", 0, 1, false, marker.len() + 1);
        assert_eq!(result.body, format!(" {marker}"));
        assert_eq!(result.line_count, 1);
        assert!(result.truncated);
        let result = render_numbered("a", 0, 1, false, marker.len() + 9);
        assert_eq!(result.body, "     1\ta\n");
        assert_eq!(result.line_count, 1);
        assert!(!result.truncated);
    }

    #[test]
    fn renderer_tiny_bounds_unicode_and_large_offsets_never_overflow() {
        let marker = RENDERED_TRUNCATION_MARKER;
        for max_bytes in 0..=marker.len() {
            let result = render_numbered("😀", 0, 1, false, max_bytes);
            assert_eq!(result.body, marker[..max_bytes]);
            assert_eq!(result.line_count, 0);
            assert!(result.truncated);
        }
        for extra in 1..=4 {
            let result = render_numbered("😀😀", 0, 1, false, marker.len() + 7 + extra);
            let expected = if extra < 4 {
                "     1\t"
            } else {
                "     1\t😀"
            };
            assert_eq!(result.body, format!("{expected}{marker}"));
            assert_eq!(result.line_count, 1);
            assert!(result.body.len() <= marker.len() + 7 + extra);
            assert!(result.truncated);
        }
        let result = render_numbered("a\nb", usize::MAX, usize::MAX, false, usize::MAX);
        assert_eq!(
            result.body,
            format!("{}\ta\n{}\tb\n", usize::MAX, usize::MAX)
        );
        assert_eq!(result.line_count, 2);
        assert_eq!(result.total_lines, usize::MAX);
        assert!(result.truncated);
    }

    #[test]
    fn renderer_matches_original_behavior_for_ordinary_bounds() {
        fn original(
            text: &str,
            start_line: usize,
            total_lines: usize,
            truncated: bool,
            max_bytes: usize,
        ) -> Rendered {
            let marker = RENDERED_TRUNCATION_MARKER;
            let budget = max_bytes - marker.len();
            let mut body = String::with_capacity(text.len().min(max_bytes));
            let mut line_count = 0;
            let mut output_truncated = false;
            for (index, line) in text.lines().enumerate() {
                let numbered = format!("{:>6}\t{}\n", start_line + index + 1, line);
                let remaining = budget.saturating_sub(body.len());
                if remaining > 0 {
                    line_count += 1;
                }
                if numbered.len() > remaining {
                    let mut end = remaining;
                    while !numbered.is_char_boundary(end) {
                        end -= 1;
                    }
                    body.push_str(&numbered[..end]);
                    body.push_str(marker);
                    output_truncated = true;
                    break;
                }
                body.push_str(&numbered);
            }
            Rendered {
                body,
                line_count,
                total_lines,
                truncated: start_line > 0 || truncated || output_truncated,
            }
        }
        for text in ["", "a", "\n", "😀é界\nsecond\n", "first\r\n\r\nlast"] {
            for start in [0, 1, 999_999] {
                for bound in RENDERED_TRUNCATION_MARKER.len()..=160 {
                    let result = render_numbered(text, start, 99, false, bound);
                    assert_eq!(result, original(text, start, 99, false, bound));
                    assert!(result.body.len() <= bound);
                }
            }
        }
    }

    #[test]
    fn errors_keep_actionable_messages_and_decode_source() {
        assert_eq!(
            TextError::EmptyOldString.to_string(),
            "old_string must not be empty"
        );
        assert_eq!(
            TextError::IdenticalStrings.to_string(),
            "old_string and new_string are identical"
        );
        assert_eq!(TextError::NotFound.to_string(), "old_string was not found");
        assert_eq!(
            TextError::MultipleMatches { occurrences: 3 }.to_string(),
            "old_string matched 3 times; set replace_all=true or provide a unique string"
        );
        assert_eq!(
            TextError::Overflow.to_string(),
            "edited content size overflowed"
        );
        let error = decode::<ReadArgs>(json!({"unknown": true})).unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
    }
}
