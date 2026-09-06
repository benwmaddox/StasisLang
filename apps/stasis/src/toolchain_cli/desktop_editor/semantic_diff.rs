use eframe::egui::{self, Color32, RichText};
use stasis_compiler::frontend::workshop::{WorkshopSemanticEditPlan, WorkshopSemanticFileChange};
use std::hash::Hash;
use std::ops::Range;

const CONTEXT_LINES: usize = 3;
const COMPACT_MAX_ROWS: usize = 10;
const MAX_LCS_CELLS: usize = 2_000_000;

const ADDED_TEXT: Color32 = Color32::from_rgb(124, 226, 145);
const REMOVED_TEXT: Color32 = Color32::from_rgb(255, 137, 137);
const ADDED_BACKGROUND: Color32 = Color32::from_rgb(22, 48, 33);
const REMOVED_BACKGROUND: Color32 = Color32::from_rgb(45, 26, 30);
const HUNK_TEXT: Color32 = Color32::from_rgb(135, 180, 230);

/// Render the source changes held by one compiler-owned semantic edit plan.
///
/// The plan is intentionally the only input to this module. In particular, no
/// semantic-edit payload is reparsed or treated as a text-edit instruction here.
pub(super) fn render(ui: &mut egui::Ui, plan: &WorkshopSemanticEditPlan, id: impl Hash) {
    let base_id = ui.make_persistent_id(id);
    let file_diffs = plan
        .changed_files
        .iter()
        .enumerate()
        .map(|(index, change)| {
            let file_id = base_id.with(("semantic-file", index, change.file.as_str()));
            (change, cached_file_diff(ui, file_id, change), file_id)
        })
        .collect::<Vec<_>>();
    let total_added = file_diffs
        .iter()
        .map(|(_, diff, _)| diff.added)
        .sum::<usize>();
    let total_removed = file_diffs
        .iter()
        .map(|(_, diff, _)| diff.removed)
        .sum::<usize>();

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Semantic source diff").strong());
        ui.label(format!(
            "{} changed file{}",
            plan.changed_files.len(),
            if plan.changed_files.len() == 1 {
                ""
            } else {
                "s"
            }
        ));
        ui.colored_label(ADDED_TEXT, format!("+{total_added}"));
        ui.colored_label(REMOVED_TEXT, format!("-{total_removed}"));
        if ui.small_button("Copy full diff").clicked() {
            ui.ctx().copy_text(unified_diff(plan));
        }
    });

    if file_diffs.is_empty() {
        ui.label(RichText::new("No semantic source changes.").weak());
        return;
    }

    for (change, diff, file_id) in file_diffs {
        render_file(ui, &change.file, &diff, file_id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffLine {
    kind: LineKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
    ending: LineEnding,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEnding {
    None,
    Lf,
    CrLf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceLine<'a> {
    text: &'a str,
    ending: LineEnding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileDiff {
    lines: Vec<DiffLine>,
    hunks: Vec<Range<usize>>,
    added: usize,
    removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedFileDiff {
    before_hash: String,
    after_hash: String,
    diff: FileDiff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffOp {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

fn cached_file_diff(ui: &egui::Ui, id: egui::Id, change: &WorkshopSemanticFileChange) -> FileDiff {
    let cache_id = id.with("cache");
    if let Some(cached) = ui
        .ctx()
        .data(|data| data.get_temp::<CachedFileDiff>(cache_id))
    {
        if cached.before_hash == change.before_hash && cached.after_hash == change.after_hash {
            return cached.diff;
        }
    }
    let diff = build_file_diff(&change.before_source, &change.after_source);
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            cache_id,
            CachedFileDiff {
                before_hash: change.before_hash.clone(),
                after_hash: change.after_hash.clone(),
                diff: diff.clone(),
            },
        )
    });
    diff
}

fn render_file(ui: &mut egui::Ui, file: &str, diff: &FileDiff, id: egui::Id) {
    let added = diff.added;
    let removed = diff.removed;
    let hunk_count = diff.hunks.len();
    let compact_range = diff.hunks.first().map(|range| {
        let end = range.start.saturating_add(COMPACT_MAX_ROWS).min(range.end);
        range.start..end
    });
    let compact_truncated = compact_range
        .as_ref()
        .zip(diff.hunks.first())
        .is_some_and(|(compact, full)| compact.end < full.end);
    let state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let show_compact = !state.is_open();
    let header = state.show_header(ui, |ui| {
        ui.vertical(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(file).strong().monospace());
                ui.colored_label(ADDED_TEXT, format!("+{added} added"));
                ui.colored_label(REMOVED_TEXT, format!("-{removed} removed"));
                if ui.small_button("Copy file diff").clicked() {
                    ui.ctx().copy_text(unified_file_diff(file, &diff));
                }
            });
            if !show_compact {
                return;
            }
            match compact_range {
                Some(range) => {
                    render_hunks(ui, diff, std::slice::from_ref(&range), id.with("compact"));
                    if hunk_count > 1 || compact_truncated {
                        ui.label(
                            RichText::new(format!(
                                "... {}{}; expand for the complete diff",
                                if hunk_count > 1 {
                                    format!(
                                        "{} more hunk{}",
                                        hunk_count - 1,
                                        if hunk_count == 2 { "" } else { "s" }
                                    )
                                } else {
                                    String::from("compact preview truncated")
                                },
                                if hunk_count > 1 && compact_truncated {
                                    "; compact preview truncated"
                                } else {
                                    ""
                                }
                            ))
                            .weak(),
                        );
                    }
                }
                None => {
                    ui.label(RichText::new("No line changes.").weak());
                }
            }
        });
    });
    let _ = header.body(|ui| {
        if diff.hunks.is_empty() {
            ui.label(RichText::new("No line changes.").weak());
        } else {
            render_hunks(ui, diff, &diff.hunks, id.with("expanded"));
        }
    });
}

fn render_hunks(ui: &mut egui::Ui, diff: &FileDiff, ranges: &[Range<usize>], id: egui::Id) {
    ui.scope(|ui| {
        ui.style_mut().spacing.scroll.floating = false;
        egui::ScrollArea::horizontal()
            .id_source(id)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (hunk_index, range) in ranges.iter().enumerate() {
                    if hunk_index > 0 {
                        ui.separator();
                    }
                    render_hunk(ui, &diff.lines, range.clone());
                }
            });
    });
}

fn render_hunk(ui: &mut egui::Ui, lines: &[DiffLine], range: Range<usize>) {
    ui.label(
        RichText::new(unified_hunk_header(&lines[range.clone()]))
            .monospace()
            .color(HUNK_TEXT),
    );
    for line in &lines[range] {
        render_line(ui, line);
    }
}

fn render_line(ui: &mut egui::Ui, line: &DiffLine) {
    let (background, text_color, prefix) = match line.kind {
        LineKind::Context => (Color32::TRANSPARENT, ui.visuals().text_color(), ' '),
        LineKind::Added => (ADDED_BACKGROUND, ADDED_TEXT, '+'),
        LineKind::Removed => (REMOVED_BACKGROUND, REMOVED_TEXT, '-'),
    };
    egui::Frame::default()
        .fill(background)
        .inner_margin(egui::Margin::symmetric(4.0, 1.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let old_line = line
                    .old_line
                    .map_or_else(|| "     ".to_string(), |number| format!("{number:>5}"));
                let new_line = line
                    .new_line
                    .map_or_else(|| "     ".to_string(), |number| format!("{number:>5}"));
                ui.add(egui::Label::new(RichText::new(old_line).monospace().weak()).wrap(false));
                ui.add(egui::Label::new(RichText::new(new_line).monospace().weak()).wrap(false));
                ui.colored_label(text_color, RichText::new(prefix.to_string()).monospace());
                ui.add(
                    egui::Label::new(RichText::new(&line.text).monospace().color(text_color))
                        .wrap(false)
                        .selectable(true),
                );
                if line.kind != LineKind::Context {
                    let ending_hint = match line.ending {
                        LineEnding::None => " [no newline]",
                        LineEnding::CrLf => " [CRLF]",
                        LineEnding::Lf => "",
                    };
                    if !ending_hint.is_empty() {
                        ui.label(RichText::new(ending_hint).monospace().weak());
                    }
                }
            });
        });
}

fn build_file_diff(before: &str, after: &str) -> FileDiff {
    let old_lines = source_lines(before);
    let new_lines = source_lines(after);
    let ops = line_diff(&old_lines, &new_lines);
    let lines = ops
        .into_iter()
        .map(|op| match op {
            DiffOp::Equal(old, new) => DiffLine {
                kind: LineKind::Context,
                old_line: Some(old + 1),
                new_line: Some(new + 1),
                ending: old_lines[old].ending,
                text: old_lines[old].text.to_string(),
            },
            DiffOp::Delete(old) => DiffLine {
                kind: LineKind::Removed,
                old_line: Some(old + 1),
                new_line: None,
                ending: old_lines[old].ending,
                text: old_lines[old].text.to_string(),
            },
            DiffOp::Insert(new) => DiffLine {
                kind: LineKind::Added,
                old_line: None,
                new_line: Some(new + 1),
                ending: new_lines[new].ending,
                text: new_lines[new].text.to_string(),
            },
        })
        .collect::<Vec<_>>();
    let added = lines
        .iter()
        .filter(|line| line.kind == LineKind::Added)
        .count();
    let removed = lines
        .iter()
        .filter(|line| line.kind == LineKind::Removed)
        .count();
    let hunks = hunk_ranges(&lines, CONTEXT_LINES);
    FileDiff {
        lines,
        hunks,
        added,
        removed,
    }
}

fn source_lines(source: &str) -> Vec<SourceLine<'_>> {
    if source.is_empty() {
        Vec::new()
    } else {
        source
            .split_inclusive('\n')
            .map(|line| {
                if let Some(text) = line.strip_suffix("\r\n") {
                    SourceLine {
                        text,
                        ending: LineEnding::CrLf,
                    }
                } else if let Some(text) = line.strip_suffix('\n') {
                    SourceLine {
                        text,
                        ending: LineEnding::Lf,
                    }
                } else {
                    SourceLine {
                        text: line,
                        ending: LineEnding::None,
                    }
                }
            })
            .collect()
    }
}

fn hunk_ranges(lines: &[DiffLine], context: usize) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut current: Option<Range<usize>> = None;
    for changed in lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.kind != LineKind::Context)
        .map(|(index, _)| index)
    {
        let start = changed.saturating_sub(context);
        let end = changed
            .saturating_add(context.saturating_add(1))
            .min(lines.len());
        if let Some(range) = &mut current {
            if start <= range.end {
                range.end = range.end.max(end);
                continue;
            }
        }
        if let Some(range) = current.take() {
            ranges.push(range);
        }
        current = Some(start..end);
    }
    if let Some(range) = current {
        ranges.push(range);
    }
    ranges
}

fn unified_hunk_header(lines: &[DiffLine]) -> String {
    let old_start = lines.iter().find_map(|line| line.old_line).unwrap_or(0);
    let new_start = lines.iter().find_map(|line| line.new_line).unwrap_or(0);
    let old_count = lines
        .iter()
        .filter(|line| line.kind != LineKind::Added)
        .count();
    let new_count = lines
        .iter()
        .filter(|line| line.kind != LineKind::Removed)
        .count();
    format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@")
}

fn unified_file_diff(file: &str, diff: &FileDiff) -> String {
    let mut output = format!("diff --git a/{file} b/{file}\n--- a/{file}\n+++ b/{file}\n");
    for range in &diff.hunks {
        output.push_str(&unified_hunk_header(&diff.lines[range.clone()]));
        output.push('\n');
        for line in &diff.lines[range.clone()] {
            let prefix = match line.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Removed => '-',
            };
            output.push(prefix);
            output.push_str(&line.text);
            match line.ending {
                LineEnding::None => {
                    output.push('\n');
                    output.push_str("\\ No newline at end of file\n");
                }
                LineEnding::Lf => output.push('\n'),
                LineEnding::CrLf => output.push_str("\r\n"),
            }
        }
    }
    if diff.hunks.is_empty() {
        output.push_str("(no line changes)\n");
    }
    output
}

fn unified_diff(plan: &WorkshopSemanticEditPlan) -> String {
    if plan.changed_files.is_empty() {
        return "No semantic source changes.\n".to_string();
    }
    plan.changed_files
        .iter()
        .map(|change| {
            unified_file_diff(
                &change.file,
                &build_file_diff(&change.before_source, &change.after_source),
            )
        })
        .collect()
}

fn line_diff(old: &[SourceLine<'_>], new: &[SourceLine<'_>]) -> Vec<DiffOp> {
    if old == new {
        return (0..old.len())
            .map(|index| DiffOp::Equal(index, index))
            .collect();
    }
    let cells = old.len().checked_mul(new.len()).unwrap_or(usize::MAX);
    if cells <= MAX_LCS_CELLS {
        let mut output = Vec::with_capacity(old.len().saturating_add(new.len()));
        hirschberg(old, new, 0, 0, &mut output);
        return output;
    }
    bounded_line_diff(old, new)
}

/// Keep large previews responsive while retaining every source line in the
/// result. The bounded look-ahead recognizes nearby anchors and otherwise
/// emits a deterministic delete-then-insert pair.
fn bounded_line_diff(old: &[SourceLine<'_>], new: &[SourceLine<'_>]) -> Vec<DiffOp> {
    const LOOKAHEAD: usize = 32;
    let mut output = Vec::with_capacity(old.len().saturating_add(new.len()));
    let mut old_index = 0;
    let mut new_index = 0;
    while old_index < old.len() || new_index < new.len() {
        if old_index < old.len() && new_index < new.len() && old[old_index] == new[new_index] {
            output.push(DiffOp::Equal(old_index, new_index));
            old_index += 1;
            new_index += 1;
            continue;
        }
        let old_end = old_index.saturating_add(LOOKAHEAD).min(old.len());
        let new_end = new_index.saturating_add(LOOKAHEAD).min(new.len());
        let next_new = (new_index + 1..new_end)
            .find(|&index| old_index < old.len() && old[old_index] == new[index]);
        let next_old = (old_index + 1..old_end)
            .find(|&index| new_index < new.len() && old[index] == new[new_index]);
        match (next_new, next_old) {
            (Some(new_anchor), Some(old_anchor))
                if new_anchor - new_index <= old_anchor - old_index =>
            {
                while new_index < new_anchor {
                    output.push(DiffOp::Insert(new_index));
                    new_index += 1;
                }
            }
            (Some(_), Some(old_anchor)) => {
                while old_index < old_anchor {
                    output.push(DiffOp::Delete(old_index));
                    old_index += 1;
                }
            }
            (Some(new_anchor), None) => {
                while new_index < new_anchor {
                    output.push(DiffOp::Insert(new_index));
                    new_index += 1;
                }
            }
            (None, Some(old_anchor)) => {
                while old_index < old_anchor {
                    output.push(DiffOp::Delete(old_index));
                    old_index += 1;
                }
            }
            (None, None) => {
                if old_index < old.len() {
                    output.push(DiffOp::Delete(old_index));
                    old_index += 1;
                }
                if new_index < new.len() {
                    output.push(DiffOp::Insert(new_index));
                    new_index += 1;
                }
            }
        }
    }
    output
}

fn hirschberg(
    old: &[SourceLine<'_>],
    new: &[SourceLine<'_>],
    old_offset: usize,
    new_offset: usize,
    output: &mut Vec<DiffOp>,
) {
    if old.is_empty() {
        output.extend((0..new.len()).map(|index| DiffOp::Insert(new_offset + index)));
        return;
    }
    if new.is_empty() {
        output.extend((0..old.len()).map(|index| DiffOp::Delete(old_offset + index)));
        return;
    }
    if old.len() == 1 {
        if let Some(match_index) = new.iter().position(|line| *line == old[0]) {
            output.extend((0..match_index).map(|index| DiffOp::Insert(new_offset + index)));
            output.push(DiffOp::Equal(old_offset, new_offset + match_index));
            output.extend(
                (match_index + 1..new.len()).map(|index| DiffOp::Insert(new_offset + index)),
            );
        } else {
            output.push(DiffOp::Delete(old_offset));
            output.extend((0..new.len()).map(|index| DiffOp::Insert(new_offset + index)));
        }
        return;
    }
    if new.len() == 1 {
        if let Some(match_index) = old.iter().position(|line| *line == new[0]) {
            output.extend((0..match_index).map(|index| DiffOp::Delete(old_offset + index)));
            output.push(DiffOp::Equal(old_offset + match_index, new_offset));
            output.extend(
                (match_index + 1..old.len()).map(|index| DiffOp::Delete(old_offset + index)),
            );
        } else {
            output.extend((0..old.len()).map(|index| DiffOp::Delete(old_offset + index)));
            output.push(DiffOp::Insert(new_offset));
        }
        return;
    }

    let old_mid = old.len() / 2;
    let split = {
        let forward = lcs_prefix_lengths(&old[..old_mid], new);
        let backward = lcs_suffix_lengths(&old[old_mid..], new);
        let mut split = 0;
        let mut best = 0;
        for index in 0..=new.len() {
            let score = forward[index] + backward[index];
            if score > best {
                best = score;
                split = index;
            }
        }
        split
    };
    hirschberg(
        &old[..old_mid],
        &new[..split],
        old_offset,
        new_offset,
        output,
    );
    hirschberg(
        &old[old_mid..],
        &new[split..],
        old_offset + old_mid,
        new_offset + split,
        output,
    );
}

fn lcs_prefix_lengths(old: &[SourceLine<'_>], new: &[SourceLine<'_>]) -> Vec<usize> {
    let mut previous = vec![0; new.len() + 1];
    let mut current = vec![0; new.len() + 1];
    for old_line in old {
        for (new_index, new_line) in new.iter().enumerate() {
            current[new_index + 1] = if old_line == new_line {
                previous[new_index] + 1
            } else {
                previous[new_index + 1].max(current[new_index])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous
}

fn lcs_suffix_lengths(old: &[SourceLine<'_>], new: &[SourceLine<'_>]) -> Vec<usize> {
    let mut next = vec![0; new.len() + 1];
    let mut current = vec![0; new.len() + 1];
    for old_line in old.iter().rev() {
        for new_index in (0..new.len()).rev() {
            current[new_index] = if old_line == &new[new_index] {
                next[new_index + 1] + 1
            } else {
                next[new_index].max(current[new_index + 1])
            };
        }
        std::mem::swap(&mut next, &mut current);
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(diff: &FileDiff) -> Vec<(LineKind, Option<usize>, Option<usize>, &str)> {
        diff.lines
            .iter()
            .map(|line| (line.kind, line.old_line, line.new_line, line.text.as_str()))
            .collect()
    }

    #[test]
    fn add_diff_has_added_line_and_exact_numbers() {
        let diff = build_file_diff("one\n", "one\ntwo\n");
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 0);
        assert_eq!(
            lines(&diff),
            vec![
                (LineKind::Context, Some(1), Some(1), "one"),
                (LineKind::Added, None, Some(2), "two")
            ]
        );
    }

    #[test]
    fn delete_diff_has_removed_line_and_exact_numbers() {
        let diff = build_file_diff("one\ntwo\n", "one\n");
        assert_eq!(diff.added, 0);
        assert_eq!(diff.removed, 1);
        assert_eq!(
            lines(&diff),
            vec![
                (LineKind::Context, Some(1), Some(1), "one"),
                (LineKind::Removed, Some(2), None, "two")
            ]
        );
    }

    #[test]
    fn terminated_add_does_not_create_a_phantom_line() {
        let diff = build_file_diff("one\n", "one\ntwo\n");
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 0);
        assert_eq!(
            lines(&diff),
            vec![
                (LineKind::Context, Some(1), Some(1), "one"),
                (LineKind::Added, None, Some(2), "two")
            ]
        );
    }

    #[test]
    fn trailing_newline_change_is_visible_and_copy_marks_missing_ending() {
        let diff = build_file_diff("one", "one\n");
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
        let copied = unified_file_diff("src/main.stasis", &diff);
        assert!(copied.contains("\\ No newline at end of file"));
        assert!(copied.contains("-one\n\\ No newline at end of file\n+one\n"));
    }

    #[test]
    fn empty_side_hunk_headers_start_at_zero() {
        let added = build_file_diff("", "new\n");
        assert!(unified_file_diff("src/main.stasis", &added).contains("@@ -0,0 +1,1 @@"));
        let removed = build_file_diff("old\n", "");
        assert!(unified_file_diff("src/main.stasis", &removed).contains("@@ -1,1 +0,0 @@"));
    }

    #[test]
    fn update_diff_is_a_removed_then_added_pair() {
        let diff = build_file_diff("old", "new");
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
        assert_eq!(
            lines(&diff),
            vec![
                (LineKind::Removed, Some(1), None, "old"),
                (LineKind::Added, None, Some(1), "new")
            ]
        );
    }

    #[test]
    fn multifile_copy_contains_each_exact_file() {
        let plan = test_plan(vec![
            ("src/a.stasis", "a\n", "b\n"),
            ("src/b.stasis", "x\n", "y\n"),
        ]);
        let copied = unified_diff(&plan);
        assert!(copied.contains("diff --git a/src/a.stasis b/src/a.stasis"));
        assert!(copied.contains("-a\n+b\n"));
        assert!(copied.contains("diff --git a/src/b.stasis b/src/b.stasis"));
        assert!(copied.contains("-x\n+y\n"));
    }

    #[test]
    fn no_op_has_no_hunks_or_counts() {
        let diff = build_file_diff("same\ncontext", "same\ncontext");
        assert_eq!(diff.added, 0);
        assert_eq!(diff.removed, 0);
        assert!(diff.hunks.is_empty());
        assert!(unified_file_diff("src/main.stasis", &diff).contains("(no line changes)"));
    }

    #[test]
    fn hunk_keeps_three_context_lines_around_change() {
        let before = (1..=10)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let after = (1..=10)
            .map(|line| {
                if line == 5 {
                    "changed".to_string()
                } else {
                    format!("line {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let diff = build_file_diff(&before, &after);
        assert_eq!(diff.hunks, vec![1..9]);
        let visible = &diff.lines[diff.hunks[0].clone()];
        assert_eq!(visible.first().and_then(|line| line.old_line), Some(2));
        assert_eq!(visible.last().and_then(|line| line.old_line), Some(8));
        assert!(visible.iter().any(|line| line.text == "changed"));
    }

    #[test]
    fn large_inputs_use_bounded_memory_path_and_preserve_all_lines() {
        let old = (0..2_000)
            .map(|_| SourceLine {
                text: "old",
                ending: LineEnding::None,
            })
            .collect::<Vec<_>>();
        let new = (0..2_000)
            .map(|_| SourceLine {
                text: "new",
                ending: LineEnding::None,
            })
            .collect::<Vec<_>>();
        let diff = line_diff(&old, &new);
        assert_eq!(
            diff.iter()
                .filter(|op| matches!(op, DiffOp::Delete(_)))
                .count(),
            old.len()
        );
        assert_eq!(
            diff.iter()
                .filter(|op| matches!(op, DiffOp::Insert(_)))
                .count(),
            new.len()
        );
    }

    fn test_plan(files: Vec<(&str, &str, &str)>) -> WorkshopSemanticEditPlan {
        use stasis_compiler::frontend::workshop::{
            WorkshopReloadClassification, WorkshopSemanticFileChange,
        };
        WorkshopSemanticEditPlan {
            schema_version: 2,
            edits: Vec::new(),
            changed_files: files
                .into_iter()
                .map(|(file, before, after)| WorkshopSemanticFileChange {
                    file: file.to_string(),
                    before_source: before.to_string(),
                    after_source: after.to_string(),
                    before_hash: String::new(),
                    after_hash: String::new(),
                })
                .collect(),
            reload: WorkshopReloadClassification {
                expected_reload: stasis_compiler::frontend::workshop::ExpectedReload::FastReload,
                reason: String::new(),
                changed_symbols: Vec::new(),
            },
        }
    }
}
