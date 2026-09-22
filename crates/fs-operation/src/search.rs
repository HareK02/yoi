use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use crate::FsAccessPolicy;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8 as UTF8Sink;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::overrides::{Override, OverrideBuilder};
use ignore::types::{Types, TypesBuilder};

use crate::{FsError, GrepOutputMode, GrepRequest, GrepResult, direct_symlink};

struct SourceBoundedReader<'a, R> {
    inner: R,
    remaining: &'a mut u64,
    access: &'a dyn FsAccessPolicy,
}

impl<R: Read> Read for SourceBoundedReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.access.check_cancelled()?;
        if buffer.is_empty() {
            return Ok(0);
        }
        if *self.remaining == 0 {
            let mut probe = [0_u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "grep source exceeded provider byte limit",
                )),
            };
        }
        let limit = usize::try_from(*self.remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = self.inner.read(&mut buffer[..limit])?;
        *self.remaining = (*self.remaining).saturating_sub(read as u64);
        Ok(read)
    }
}

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
    retained_bytes: usize,
    truncated: bool,
}

const MAX_GREP_RESULT_RETAINED_BYTES: usize = 512 * 1024;

fn reserve_report_bytes(report: &mut GrepReport, bytes: usize) -> bool {
    let Some(next) = report.retained_bytes.checked_add(bytes) else {
        report.truncated = true;
        return false;
    };
    if next > MAX_GREP_RESULT_RETAINED_BYTES {
        report.truncated = true;
        return false;
    }
    report.retained_bytes = next;
    true
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

#[derive(Default)]
struct DescriptorIgnoreMatchers {
    loaded_directories: HashSet<PathBuf>,
    by_directory: BTreeMap<PathBuf, Gitignore>,
}

impl DescriptorIgnoreMatchers {
    fn load_directory(
        &mut self,
        directory: &Path,
        explicit_base: &Path,
        source_bytes_remaining: &mut u64,
        access: &dyn FsAccessPolicy,
    ) -> Result<(), FsError> {
        if !self.loaded_directories.insert(directory.to_path_buf()) {
            return Ok(());
        }
        let mut builder = GitignoreBuilder::new(directory);
        let mut has_patterns = false;
        for name in [".gitignore", ".ignore"] {
            let path = directory.join(name);
            let resolved = access
                .resolve_access_path(&path)
                .map_err(|error| FsError::io(&path, error))?;
            if !access.is_readable_paths(&path, &resolved) {
                continue;
            }
            let file = match access.open_read_file(&path, &resolved) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(FsError::io(&path, error)),
            };
            if !file
                .metadata()
                .map_err(|error| FsError::io(&path, error))?
                .is_file()
            {
                continue;
            }
            let bounded = SourceBoundedReader {
                inner: file,
                remaining: source_bytes_remaining,
                access,
            };
            let mut reader = BufReader::new(bounded);
            let mut line = String::new();
            let mut line_number = 0_u64;
            loop {
                line.clear();
                let read = reader
                    .read_line(&mut line)
                    .map_err(|error| FsError::io(&path, error))?;
                if read == 0 {
                    break;
                }
                line_number = line_number.saturating_add(1);
                if line.len() > 1024 * 1024 {
                    return Err(FsError::InvalidArgument(format!(
                        "ignore pattern line {line_number} exceeds provider limit"
                    )));
                }
                let line = line.trim_end_matches(['\r', '\n']);
                // Match the ignore crate's partial-error behavior: one invalid
                // pattern does not discard the remaining valid lines.
                if builder.add_line(Some(path.clone()), line).is_ok() {
                    has_patterns = true;
                }
            }
        }
        if !has_patterns {
            return Ok(());
        }
        let matcher = builder.build().map_err(|_| {
            FsError::InvalidArgument("ignore patterns could not be compiled".to_string())
        })?;
        // An explicitly selected search root is traversed even when an ancestor
        // ignores that directory, matching path-backed Grep behavior.
        if directory != explicit_base
            && matcher
                .matched_path_or_any_parents(explicit_base, true)
                .is_ignore()
        {
            return Ok(());
        }
        self.by_directory.insert(directory.to_path_buf(), matcher);
        Ok(())
    }

    fn is_ignored(&self, root: &Path, path: &Path) -> bool {
        let mut directories = path
            .parent()
            .into_iter()
            .flat_map(Path::ancestors)
            .take_while(|directory| directory.starts_with(root))
            .collect::<Vec<_>>();
        directories.reverse();
        let mut ignored = false;
        for directory in directories {
            let Some(matcher) = self.by_directory.get(directory) else {
                continue;
            };
            let matched = matcher.matched_path_or_any_parents(path, false);
            if matched.is_ignore() {
                ignored = true;
            } else if matched.is_whitelist() {
                ignored = false;
            }
        }
        ignored
    }
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
    access
        .check_cancelled()
        .map_err(|error| FsError::io(&base, error))?;
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
    let resolved_base = match access.resolve_access_path(&base) {
        Ok(resolved) => resolved,
        Err(error) => {
            if let Some(info) = direct_symlink(&base).filter(|info| !info.target_exists) {
                return Err(FsError::BrokenSymlink {
                    path: base.clone(),
                    link: info.link_path,
                    target: info.resolved_path,
                });
            }
            return Err(FsError::io(&base, error));
        }
    };
    let symlink = (resolved_base != base)
        .then(|| direct_symlink(&base))
        .flatten();
    if let Some(info) = symlink.as_ref()
        && !info.target_exists
    {
        return Err(FsError::BrokenSymlink {
            path: base.clone(),
            link: info.link_path.clone(),
            target: info.resolved_path.clone(),
        });
    }
    if !access.is_readable_paths(&base, &resolved_base) {
        return Err(if let Some(info) = symlink.as_ref() {
            let link_parent_readable = info
                .link_path
                .parent()
                .and_then(|parent| {
                    access
                        .resolve_access_path(parent)
                        .ok()
                        .map(|resolved| access.is_readable_paths(parent, &resolved))
                })
                .unwrap_or(false);
            if link_parent_readable {
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
    let base_meta = access
        .read_metadata(&base, &resolved_base)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => FsError::NotFound(base.clone()),
            _ => FsError::io(&base, e),
        })?;
    if !base_meta.is_file() && !base_meta.is_dir() {
        return Err(FsError::InvalidArgument(format!(
            "grep search path must be a regular file or directory: {}",
            base.display()
        )));
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
        retained_bytes: 0,
        truncated: false,
    };
    let mut matching_files_seen = 0;
    let mut matches_seen = 0;

    if base_meta.is_file() {
        let mut source_bytes_remaining = crate::MAX_GREP_SOURCE_BYTES;
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
                &mut source_bytes_remaining,
                access,
            )?;
        }
        return Ok(report.into_result(root));
    }

    let traversal = access
        .open_traversal_root(&base, &resolved_base)
        .map_err(|error| FsError::io(&base, error))?;
    let walker_root = traversal
        .as_ref()
        .map(|traversal| traversal.path())
        .unwrap_or(&base);
    let mut walker = WalkBuilder::new(walker_root);
    if traversal.is_some() {
        walker
            .hidden(false)
            // Path-backed ignore discovery may follow an ignore-file symlink.
            // Descriptor providers load ignore files through `access` below.
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .ignore(false)
            .parents(false)
            .follow_links(false);
        let search_relative = base.strip_prefix(root).map_err(|_| {
            FsError::InvalidArgument("grep base is outside its provider root".to_string())
        })?;
        let search_relative = search_relative.to_path_buf();
        let filter_root = walker_root.to_path_buf();
        walker.filter_entry(move |entry| {
            entry
                .path()
                .strip_prefix(&filter_root)
                .is_ok_and(|relative| {
                    relative.starts_with(&search_relative) || search_relative.starts_with(relative)
                })
        });
    } else {
        walker
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .ignore(true)
            .parents(true)
            .follow_links(false);
        if let Some(types) = types.clone() {
            walker.types(types);
        }
        if let Some(overrides) = overrides.clone() {
            walker.overrides(overrides);
        }
    }

    let mut visited = 0_usize;
    let mut source_bytes_remaining = crate::MAX_GREP_SOURCE_BYTES;
    let mut descriptor_ignores = DescriptorIgnoreMatchers::default();
    if traversal.is_some() {
        let mut directory = root.to_path_buf();
        descriptor_ignores.load_directory(
            &directory,
            &base,
            &mut source_bytes_remaining,
            access,
        )?;
        for component in base
            .strip_prefix(root)
            .map_err(|_| {
                FsError::InvalidArgument("grep base is outside its provider root".to_string())
            })?
            .components()
        {
            directory.push(component);
            descriptor_ignores.load_directory(
                &directory,
                &base,
                &mut source_bytes_remaining,
                access,
            )?;
        }
    }
    for entry in walker.build().flatten() {
        access
            .check_cancelled()
            .map_err(|error| FsError::io(&base, error))?;
        visited = visited.saturating_add(1);
        if visited > crate::MAX_TRAVERSAL_ENTRIES {
            return Err(FsError::InvalidArgument(format!(
                "grep traversal exceeds provider limit {}",
                crate::MAX_TRAVERSAL_ENTRIES
            )));
        }
        let walked_path = entry.path();
        let path = if traversal.is_some() {
            let relative = walked_path.strip_prefix(walker_root).map_err(|_| {
                FsError::InvalidArgument(
                    "descriptor traversal returned a path outside its root".to_string(),
                )
            })?;
            root.join(relative)
        } else {
            walked_path.to_path_buf()
        };
        let file_type = entry.file_type();
        if traversal.is_some() && file_type.as_ref().is_some_and(|kind| kind.is_dir()) {
            descriptor_ignores.load_directory(&path, &base, &mut source_bytes_remaining, access)?;
            continue;
        }
        if !file_type.map(|kind| kind.is_file()).unwrap_or(false) {
            continue;
        }
        if traversal.is_some() {
            let relative = path.strip_prefix(&base).map_err(|_| {
                FsError::InvalidArgument(
                    "descriptor traversal returned a path outside its search base".to_string(),
                )
            })?;
            if relative
                .components()
                .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
                || descriptor_ignores.is_ignored(root, &path)
                || !direct_file_selected(&path, overrides.as_ref(), types.as_ref())
            {
                continue;
            }
        }
        let readable = access
            .resolve_access_path(&path)
            .is_ok_and(|resolved| access.is_readable_paths(&path, &resolved));
        if !readable {
            continue;
        }
        if scan_path(
            &mut searcher,
            &matcher,
            &path,
            mode,
            &mut report,
            &mut matching_files_seen,
            &mut matches_seen,
            offset,
            head_limit,
            &mut source_bytes_remaining,
            access,
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
    source_bytes_remaining: &mut u64,
    access: &dyn FsAccessPolicy,
) -> Result<bool, FsError> {
    let resolved = access
        .resolve_access_path(path)
        .map_err(|error| FsError::io(path, error))?;
    let reader = access
        .open_read_file(path, &resolved)
        .map_err(|error| FsError::io(path, error))?;
    let metadata = reader
        .metadata()
        .map_err(|error| FsError::io(path, error))?;
    if !metadata.is_file() {
        return Err(FsError::InvalidArgument(
            "grep source must be a regular file".to_string(),
        ));
    }
    if metadata.len() > *source_bytes_remaining {
        return Err(FsError::InvalidArgument(
            "grep sources exceed provider byte limit".to_string(),
        ));
    }
    let mut reader = SourceBoundedReader {
        inner: reader,
        remaining: source_bytes_remaining,
        access,
    };
    match mode {
        GrepOutputMode::FilesWithMatches => {
            if !scan_any_match(searcher, matcher, &mut reader, path)? {
                return Ok(false);
            }
            if *matching_files_seen >= offset {
                if !reserve_report_bytes(report, path.to_string_lossy().len().saturating_add(1)) {
                    return Ok(true);
                }
                report.files.push(path.to_path_buf());
                if report.files.len() >= head_limit {
                    report.truncated = true;
                    return Ok(true);
                }
            }
            *matching_files_seen += 1;
        }
        GrepOutputMode::Count => {
            let count = scan_count(searcher, matcher, &mut reader, path)?;
            if count == 0 {
                return Ok(false);
            }
            if *matching_files_seen >= offset {
                if !reserve_report_bytes(report, path.to_string_lossy().len().saturating_add(32)) {
                    return Ok(true);
                }
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
                retained_bytes: &mut report.retained_bytes,
                truncated: &mut report.truncated,
                matches_seen,
                offset,
                head_limit,
            };
            searcher
                .search_reader(matcher, &mut reader, &mut sink)
                .map_err(|error| FsError::io(path, error))?;
            if report.truncated {
                return Ok(true);
            }
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
    reader: &mut dyn Read,
    path: &Path,
) -> Result<bool, FsError> {
    let mut hit = false;
    let sink = UTF8Sink(|_, _| {
        hit = true;
        Ok(false) // stop searching this file immediately
    });
    searcher
        .search_reader(matcher, reader, sink)
        .map_err(|e| FsError::io(path, e))?;
    Ok(hit)
}

fn scan_count(
    searcher: &mut Searcher,
    matcher: &grep_regex::RegexMatcher,
    reader: &mut dyn Read,
    path: &Path,
) -> Result<usize, FsError> {
    let mut count = 0usize;
    let sink = UTF8Sink(|_, _| {
        count += 1;
        Ok(true)
    });
    searcher
        .search_reader(matcher, reader, sink)
        .map_err(|e| FsError::io(path, e))?;
    Ok(count)
}

struct ContentSink<'a> {
    path: PathBuf,
    lines: &'a mut Vec<ContentLine>,
    retained_bytes: &'a mut usize,
    truncated: &'a mut bool,
    matches_seen: &'a mut usize,
    offset: usize,
    head_limit: usize,
}

impl ContentSink<'_> {
    fn reserve(&mut self, content_bytes: usize) -> bool {
        let retained = self
            .path
            .to_string_lossy()
            .len()
            .saturating_add(content_bytes)
            .saturating_add(64);
        let Some(next) = (*self.retained_bytes).checked_add(retained) else {
            *self.truncated = true;
            return false;
        };
        if next > MAX_GREP_RESULT_RETAINED_BYTES {
            *self.truncated = true;
            return false;
        }
        *self.retained_bytes = next;
        true
    }
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

        if !self.reserve(mat.bytes().len()) {
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
        if !self.reserve(ctx.bytes().len()) {
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

#[cfg(test)]
mod bounded_reader_tests {
    use std::io::{Cursor, Read as _};
    use std::path::Path;

    use super::SourceBoundedReader;
    use crate::FsAccessPolicy;

    struct Permit;

    impl FsAccessPolicy for Permit {
        fn is_readable(&self, _path: &Path) -> bool {
            true
        }

        fn is_writable(&self, _path: &Path) -> bool {
            true
        }
    }

    #[test]
    fn source_budget_is_shared_across_candidate_files() {
        let mut remaining = 3_u64;
        let mut first = SourceBoundedReader {
            inner: Cursor::new(b"ab"),
            remaining: &mut remaining,
            access: &Permit,
        };
        let mut output = Vec::new();
        first.read_to_end(&mut output).unwrap();
        drop(first);
        assert_eq!(remaining, 1);

        let mut second = SourceBoundedReader {
            inner: Cursor::new(b"cd"),
            remaining: &mut remaining,
            access: &Permit,
        };
        let error = second.read_to_end(&mut output).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(output, b"abc");
    }
}
