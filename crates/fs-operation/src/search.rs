use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::FsAccessPolicy;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8 as UTF8Sink;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::WalkBuilder;
use ignore::overrides::{Override, OverrideBuilder};
use ignore::types::{Types, TypesBuilder};

use crate::{FsError, GrepOutputMode, GrepRequest, GrepResult, direct_symlink};

struct ContentLine {
    path: PathBuf,
    line_number: Option<u64>,
    text: String,
    is_match: bool,
}

struct GrepReport {
    mode: GrepOutputMode,
    show_line_numbers: bool,
    files: Vec<PathBuf>,
    counts: Vec<(PathBuf, usize)>,
    lines: Vec<ContentLine>,
    truncated: bool,
}

impl GrepReport {
    fn into_result(self, root: &Path) -> GrepResult {
        let (match_count, matched_files) = match self.mode {
            GrepOutputMode::FilesWithMatches => (self.files.len(), self.files.len()),
            GrepOutputMode::Count => (
                self.counts.iter().map(|(_, count)| *count).sum(),
                self.counts.len(),
            ),
            GrepOutputMode::Content => (
                self.lines.iter().filter(|line| line.is_match).count(),
                self.lines
                    .iter()
                    .map(|line| line.path.as_path())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
            ),
        };
        let mut output = String::new();
        match self.mode {
            GrepOutputMode::FilesWithMatches => {
                for path in &self.files {
                    output.push_str(&logical_display(root, path));
                    output.push('\n');
                }
            }
            GrepOutputMode::Count => {
                for (path, count) in &self.counts {
                    output.push_str(&format!("{}:{count}\n", logical_display(root, path)));
                }
            }
            GrepOutputMode::Content => {
                output.push_str(&render_content_lines(
                    root,
                    &self.lines,
                    self.show_line_numbers,
                ));
            }
        }
        GrepResult {
            output,
            match_count,
            matched_files,
            truncated: self.truncated,
        }
    }
}

fn render_content_lines(root: &Path, lines: &[ContentLine], show_line_numbers: bool) -> String {
    let mut grouped = BTreeMap::<&Path, Vec<&ContentLine>>::new();
    for line in lines {
        grouped.entry(&line.path).or_default().push(line);
    }

    let mut output = String::new();
    for (file_index, (path, file_lines)) in grouped.into_iter().enumerate() {
        if file_index > 0 {
            output.push('\n');
        }
        let _ = writeln!(output, "{}", logical_display(root, path));

        let number_width = file_lines
            .iter()
            .filter_map(|line| line.line_number)
            .map(|number| number.to_string().len())
            .max()
            .unwrap_or(1);
        let mut previous_line_end = None;
        for line in file_lines {
            if let (Some(previous_end), Some(number)) = (previous_line_end, line.line_number)
                && number > previous_end
            {
                let _ = writeln!(output, "   …");
            }

            let marker = if line.is_match { '>' } else { ' ' };
            if show_line_numbers && let Some(number) = line.line_number {
                let _ = writeln!(output, " {marker} {number:>number_width$} │ {}", line.text);
            } else {
                let _ = writeln!(output, " {marker} │ {}", line.text);
            }
            previous_line_end = line
                .line_number
                .map(|number| number + line.text.split('\n').count() as u64);
        }
    }

    output
}

fn logical_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

const DEFAULT_HEAD_LIMIT: usize = 250;

fn build_overrides(base: &Path, glob: Option<&str>) -> Result<Option<Override>, FsError> {
    let Some(glob) = glob else {
        return Ok(None);
    };
    let mut builder = OverrideBuilder::new(base);
    builder
        .add(glob)
        .map_err(|error| FsError::InvalidGlob(error.to_string()))?;
    builder
        .build()
        .map(Some)
        .map_err(|error| FsError::InvalidGlob(error.to_string()))
}

fn build_types(file_type: Option<&str>) -> Result<Option<Types>, FsError> {
    let Some(file_type) = file_type else {
        return Ok(None);
    };
    let mut builder = TypesBuilder::new();
    builder.add_defaults();
    builder.select(file_type);
    builder
        .build()
        .map(Some)
        .map_err(|error| FsError::InvalidArgument(format!("invalid type {file_type}: {error}")))
}

fn direct_file_selected(path: &Path, overrides: Option<&Override>, types: Option<&Types>) -> bool {
    !overrides.is_some_and(|filter| filter.matched(path, false).is_ignore())
        && !types.is_some_and(|filter| filter.matched(path, false).is_ignore())
}

struct GrepParams {
    pattern: String,
    path: Option<PathBuf>,
    glob: Option<String>,
    file_type: Option<String>,
    case_insensitive: bool,
    before: Option<usize>,
    after: Option<usize>,
    context: Option<usize>,
    line_numbers: Option<bool>,
    multiline: bool,
    output_mode: Option<GrepOutputMode>,
    head_limit: Option<usize>,
    offset: Option<usize>,
}

pub fn run_grep(
    root: &Path,
    base: PathBuf,
    request: GrepRequest,
    access: &dyn FsAccessPolicy,
) -> Result<GrepResult, FsError> {
    let p = GrepParams {
        pattern: request.pattern,
        path: Some(base.clone()),
        glob: request.glob,
        file_type: request.file_type,
        case_insensitive: request.case_insensitive,
        before: Some(request.before_context),
        after: Some(request.after_context),
        context: None,
        line_numbers: Some(true),
        multiline: request.multiline,
        output_mode: Some(request.output_mode),
        head_limit: Some(request.limit),
        offset: Some(request.offset),
    };
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(p.case_insensitive)
        .multi_line(p.multiline)
        .dot_matches_new_line(p.multiline)
        .build(&p.pattern)
        .map_err(|e| FsError::InvalidRegex(e.to_string()))?;

    let (before, after) = match (p.before, p.after, p.context) {
        (_, _, Some(c)) => (c, c),
        (b, a, None) => (b.unwrap_or(0), a.unwrap_or(0)),
    };

    let mut sb = SearcherBuilder::new();
    sb.binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(p.line_numbers.unwrap_or(true))
        .multi_line(p.multiline)
        .before_context(before)
        .after_context(after);
    let mut searcher = sb.build();

    let base = p.path.unwrap_or(base);
    if !base.is_absolute() {
        return Err(FsError::RelativePath(base));
    }
    let symlink = direct_symlink(&base);
    if !access.is_readable(&base) {
        return Err(if let Some(info) = symlink.as_ref() {
            let link_parent_readable = info
                .link_path
                .parent()
                .map(|parent| access.is_readable(parent))
                .unwrap_or(false);
            if info.target_exists && link_parent_readable {
                FsError::SymlinkOutOfScope {
                    path: base.clone(),
                    target: info.resolved_path.clone(),
                    required_permission: "read",
                }
            } else {
                FsError::OutOfScope(base.clone())
            }
        } else {
            FsError::OutOfScope(base.clone())
        });
    }
    if let Some(info) = symlink.as_ref() {
        if !info.target_exists {
            return Err(FsError::BrokenSymlink {
                path: base.clone(),
                link: info.link_path.clone(),
                target: info.target_path.clone(),
            });
        }
    }
    let base_meta = std::fs::metadata(&base).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => FsError::NotFound(base.clone()),
        _ => FsError::io(&base, e),
    })?;
    if !base_meta.is_file() && !base_meta.is_dir() {
        return Err(FsError::InvalidArgument(format!(
            "grep search path must be a regular file or directory: {}",
            base.display()
        )));
    }
    if base_meta.is_dir()
        && let Some(info) = symlink.as_ref()
    {
        return Err(FsError::SymlinkDirectoryNotTraversed {
            tool: "Grep",
            path: base.clone(),
            target: info.resolved_path.clone(),
        });
    }

    let filter_base = if base_meta.is_file() { root } else { &base };
    let types = build_types(p.file_type.as_deref())?;
    let overrides = build_overrides(filter_base, p.glob.as_deref())?;

    let mode = p.output_mode.unwrap_or_default();
    let head_limit = p.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
    let offset = p.offset.unwrap_or(0);
    let show_line_numbers = p.line_numbers.unwrap_or(true);

    let mut report = GrepReport {
        mode,
        show_line_numbers,
        files: Vec::new(),
        counts: Vec::new(),
        lines: Vec::new(),
        truncated: false,
    };
    let mut matching_files_seen = 0;
    let mut matches_seen = 0;

    if base_meta.is_file() {
        if direct_file_selected(&base, overrides.as_ref(), types.as_ref()) {
            scan_path(
                &mut searcher,
                &matcher,
                &base,
                mode,
                &mut report,
                &mut matching_files_seen,
                &mut matches_seen,
                offset,
                head_limit,
            )?;
        }
        return Ok(report.into_result(root));
    }

    let mut walker = WalkBuilder::new(&base);
    walker
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .follow_links(false);
    if let Some(types) = types {
        walker.types(types);
    }
    if let Some(overrides) = overrides {
        walker.overrides(overrides);
    }

    for entry in walker.build().flatten() {
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let path = entry.path();
        if !access.is_readable(path) {
            continue;
        }
        if scan_path(
            &mut searcher,
            &matcher,
            path,
            mode,
            &mut report,
            &mut matching_files_seen,
            &mut matches_seen,
            offset,
            head_limit,
        )? {
            break;
        }
    }

    Ok(report.into_result(root))
}

#[allow(clippy::too_many_arguments)]
fn scan_path(
    searcher: &mut Searcher,
    matcher: &grep_regex::RegexMatcher,
    path: &Path,
    mode: GrepOutputMode,
    report: &mut GrepReport,
    matching_files_seen: &mut usize,
    matches_seen: &mut usize,
    offset: usize,
    head_limit: usize,
) -> Result<bool, FsError> {
    match mode {
        GrepOutputMode::FilesWithMatches => {
            if !scan_any_match(searcher, matcher, path)? {
                return Ok(false);
            }
            if *matching_files_seen >= offset {
                report.files.push(path.to_path_buf());
                if report.files.len() >= head_limit {
                    report.truncated = true;
                    return Ok(true);
                }
            }
            *matching_files_seen += 1;
        }
        GrepOutputMode::Count => {
            let count = scan_count(searcher, matcher, path)?;
            if count == 0 {
                return Ok(false);
            }
            if *matching_files_seen >= offset {
                report.counts.push((path.to_path_buf(), count));
                if report.counts.len() >= head_limit {
                    report.truncated = true;
                    return Ok(true);
                }
            }
            *matching_files_seen += 1;
        }
        GrepOutputMode::Content => {
            let before_count = *matches_seen;
            let mut sink = ContentSink {
                path: path.to_path_buf(),
                lines: &mut report.lines,
                matches_seen,
                offset,
                head_limit,
            };
            searcher
                .search_path(matcher, path, &mut sink)
                .map_err(|error| FsError::io(path, error))?;
            if *matches_seen >= offset.saturating_add(head_limit) && *matches_seen > before_count {
                report.truncated = true;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn scan_any_match(
    searcher: &mut Searcher,
    matcher: &grep_regex::RegexMatcher,
    path: &Path,
) -> Result<bool, FsError> {
    let mut hit = false;
    let sink = UTF8Sink(|_, _| {
        hit = true;
        Ok(false) // stop searching this file immediately
    });
    searcher
        .search_path(matcher, path, sink)
        .map_err(|e| FsError::io(path, e))?;
    Ok(hit)
}

fn scan_count(
    searcher: &mut Searcher,
    matcher: &grep_regex::RegexMatcher,
    path: &Path,
) -> Result<usize, FsError> {
    let mut count = 0usize;
    let sink = UTF8Sink(|_, _| {
        count += 1;
        Ok(true)
    });
    searcher
        .search_path(matcher, path, sink)
        .map_err(|e| FsError::io(path, e))?;
    Ok(count)
}

struct ContentSink<'a> {
    path: PathBuf,
    lines: &'a mut Vec<ContentLine>,
    matches_seen: &'a mut usize,
    offset: usize,
    head_limit: usize,
}

impl Sink for ContentSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let idx = *self.matches_seen;
        *self.matches_seen += 1;

        // Skip matches before offset.
        if idx < self.offset {
            return Ok(true);
        }
        // Stop searching this file once we've filled the head_limit.
        if idx >= self.offset.saturating_add(self.head_limit) {
            return Ok(false);
        }

        let text = String::from_utf8_lossy(mat.bytes())
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();
        self.lines.push(ContentLine {
            path: self.path.clone(),
            line_number: mat.line_number(),
            text,
            is_match: true,
        });
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        let seen = *self.matches_seen;
        if seen < self.offset {
            return Ok(true);
        }
        if seen >= self.offset.saturating_add(self.head_limit) {
            return Ok(false);
        }
        let text = String::from_utf8_lossy(ctx.bytes())
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();
        self.lines.push(ContentLine {
            path: self.path.clone(),
            line_number: ctx.line_number(),
            text,
            is_match: false,
        });
        Ok(true)
    }
}
