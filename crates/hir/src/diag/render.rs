use annotate_snippets::{Annotation, AnnotationKind, Group, Level, Renderer, Snippet};

use super::{
    span::AbsoluteSpan,
    value::{Diagnostic, DiagnosticLabel, DiagnosticLevel, LabelStyle},
};
use crate::input::SourceFile;

impl Diagnostic {
    /// Converts this diagnostic into `annotate_snippets` groups.
    ///
    /// This is where label spans are resolved to absolute file offsets. Labels
    /// whose files have no available content are skipped, but notes still
    /// render.
    pub fn to_annotate_report<'db>(&self, db: &'db dyn crate::Db) -> Vec<Group<'db>> {
        let mut title = self
            .level
            .to_annotate_level()
            .primary_title(self.message.clone());
        if let Some(code) = &self.code {
            title = title.id(code.clone());
        }

        let mut group = Group::with_title(title);

        let mut by_file: Vec<(SourceFile, Vec<(&DiagnosticLabel, AbsoluteSpan)>)> = Vec::new();
        for label in &self.labels {
            if label.span.file().content(db).is_none() {
                continue;
            }
            let absolute = label.span.resolve_to_absolute(db);
            let file = absolute.file();
            if let Some((_, labels)) = by_file
                .iter_mut()
                .find(|(existing_file, _)| *existing_file == file)
            {
                labels.push((label, absolute));
            } else {
                by_file.push((file, vec![(label, absolute)]));
            }
        }

        for (file, labels) in by_file {
            let url = file.url(db);
            let Some(content) = file.content(db) else {
                continue;
            };

            let source_len = content.len();
            let mut annotations: Vec<Annotation<'_>> = Vec::with_capacity(labels.len());
            let mut visible_ranges = Vec::with_capacity(labels.len());

            for (label, absolute) in labels {
                let span = clamp_span(
                    absolute.start().as_usize(),
                    absolute.end().as_usize(),
                    source_len,
                );
                visible_ranges.push(context_window_span(content.as_str(), &span, 1, 1));
                let mut annotation = label.style.to_annotate_kind().span(span);
                if let Some(message) = &label.message {
                    annotation = annotation.label(message.clone());
                }
                if matches!(label.style, LabelStyle::Primary) {
                    annotation = annotation.highlight_source(true);
                }
                annotations.push(annotation);
            }

            let mut snippet = Snippet::source(content).path(display_url_path(url));
            for range in merge_ranges(visible_ranges) {
                snippet = snippet.annotation(AnnotationKind::Visible.span(range));
            }
            snippet = snippet.annotations(annotations);

            group = group.element(snippet);
        }

        for note in &self.notes {
            group = group.element(Level::NOTE.message(note.clone()));
        }
        for help in &self.helps {
            group = group.element(Level::HELP.message(help.clone()));
        }

        vec![group]
    }

    /// Renders this diagnostic using the default styled terminal renderer.
    pub fn render(&self, db: &dyn crate::Db) -> String {
        self.render_with(db, &Renderer::styled())
    }

    /// Renders this diagnostic using the provided `annotate_snippets` renderer.
    ///
    /// This performs absolute span resolution for labels whose files still have
    /// content, and may panic if such a def-relative label no longer has a
    /// location table entry.
    pub fn render_with(&self, db: &dyn crate::Db, renderer: &Renderer) -> String {
        let report = self.to_annotate_report(db);
        renderer.render(&report)
    }

    /// Renders this diagnostic as a single line:
    /// `path:line:column: error[CODE]: message`.
    ///
    /// Multi-line messages are compacted so short output remains one diagnostic
    /// per line.
    pub fn render_short(&self, db: &dyn crate::Db) -> String {
        let mut output = String::new();
        if let Some(label) = self.primary_label() {
            let absolute = label.span.resolve_to_absolute(db);
            let file = absolute.file();
            let path = display_url_path(file.url(db));
            if let Some(content) = file.content(db) {
                let (line, column) = line_column_for_offset(content, absolute.start().as_usize());
                output.push_str(&format!("{path}:{line}:{column}: "));
            } else {
                output.push_str(&format!("{path}: "));
            }
        }
        output.push_str(self.level.as_str());
        if let Some(code) = &self.code {
            output.push('[');
            output.push_str(code);
            output.push(']');
        }
        output.push_str(": ");
        output.push_str(&compact_diagnostic_message(&self.message));
        output.push('\n');
        output
    }
}

fn display_url_path(url: &url::Url) -> String {
    crate::url_to_file_path(url)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| url.as_str().to_owned())
}

impl DiagnosticLevel {
    fn to_annotate_level(self) -> Level<'static> {
        match self {
            DiagnosticLevel::Error => Level::ERROR,
            DiagnosticLevel::Warning => Level::WARNING,
            DiagnosticLevel::Note => Level::NOTE,
            DiagnosticLevel::Help => Level::HELP,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            DiagnosticLevel::Error => "error",
            DiagnosticLevel::Warning => "warning",
            DiagnosticLevel::Note => "note",
            DiagnosticLevel::Help => "help",
        }
    }
}

impl LabelStyle {
    fn to_annotate_kind(self) -> AnnotationKind {
        match self {
            LabelStyle::Primary => AnnotationKind::Primary,
            LabelStyle::Secondary => AnnotationKind::Context,
        }
    }
}

fn clamp_span(start: usize, end: usize, source_len: usize) -> core::ops::Range<usize> {
    let start = start.min(source_len);
    let end = end.min(source_len);
    if start <= end { start..end } else { end..start }
}

fn context_window_span(
    source: &str,
    focus: &core::ops::Range<usize>,
    lines_before: usize,
    lines_after: usize,
) -> core::ops::Range<usize> {
    if source.is_empty() {
        return 0..0;
    }

    let focus_start = normalize_line_lookup_offset(source, focus.start);
    let focus_end = normalize_line_lookup_offset(source, focus.end);

    let mut start = line_start_at_or_before(source, focus_start);
    for _ in 0..lines_before {
        if start == 0 {
            break;
        }
        start = line_start_at_or_before(source, start.saturating_sub(1));
    }

    let mut end = line_end_at_or_after(source, focus_end);
    for _ in 0..lines_after {
        if end >= source.len() {
            break;
        }
        end = line_end_at_or_after(source, (end + 1).min(source.len()));
    }

    let target_lines = lines_before + lines_after + 1;
    while count_lines_in_span(source, start, end) < target_lines {
        if start > 0 {
            start = line_start_at_or_before(source, start.saturating_sub(1));
            continue;
        }
        if end < source.len() {
            end = line_end_at_or_after(source, (end + 1).min(source.len()));
        } else {
            break;
        }
    }

    if start == end && !source.is_empty() {
        start..(end + 1).min(source.len())
    } else {
        start..end
    }
}

fn normalize_line_lookup_offset(source: &str, offset: usize) -> usize {
    let mut offset = offset.min(source.len());
    if offset == source.len() {
        offset = floor_char_boundary(source, offset.saturating_sub(1));
    }
    let bytes = source.as_bytes();
    if bytes.get(offset).copied() == Some(b'\n') && offset > 0 {
        offset = floor_char_boundary(source, offset - 1);
    }
    offset
}

fn floor_char_boundary(source: &str, offset: usize) -> usize {
    let mut offset = offset.min(source.len());
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn ceil_char_boundary(source: &str, offset: usize) -> usize {
    let mut offset = offset.min(source.len());
    while offset < source.len() && !source.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

fn line_start_at_or_before(source: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(source, offset);
    source[..offset].rfind('\n').map_or(0, |idx| idx + 1)
}

fn line_end_at_or_after(source: &str, offset: usize) -> usize {
    let offset = ceil_char_boundary(source, offset);
    source[offset..]
        .find('\n')
        .map_or(source.len(), |idx| offset + idx)
}

fn merge_ranges(mut ranges: Vec<core::ops::Range<usize>>) -> Vec<core::ops::Range<usize>> {
    if ranges.len() <= 1 {
        return ranges;
    }

    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<core::ops::Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end {
                if range.end > last.end {
                    last.end = range.end;
                }
            } else {
                merged.push(range);
            }
        } else {
            merged.push(range);
        }
    }
    merged
}

fn count_lines_in_span(source: &str, start: usize, end: usize) -> usize {
    if source.is_empty() {
        return 0;
    }
    let start = start.min(source.len());
    let end = end.min(source.len());
    if start >= end {
        return 1;
    }
    let mut count = source[start..end]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    if end == source.len() && source.ends_with('\n') && count > 0 {
        count -= 1;
    }
    count
}

fn line_column_for_offset(source: &str, offset: usize) -> (usize, usize) {
    let offset = floor_char_boundary(source, offset.min(source.len()));
    let line = source[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let line_start = line_start_at_or_before(source, offset);
    let column = source[line_start..offset].chars().count() + 1;
    (line, column)
}

fn compact_diagnostic_message(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}
