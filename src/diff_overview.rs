use crate::strings::tr;
use gpui_kit::{
    base::TestSupportExt,
    component::{
        ActiveTheme,
        diff::{Diff, DiffFile, DiffLinePosition, DiffParseError, DiffSide, DiffState},
        h_flex,
        tooltip::Tooltip,
    },
    prelude::FluentBuilder,
    *,
};
use std::ops::Range;

const MARK_MIN_HEIGHT: f32 = 3.;

#[derive(Default)]
pub struct DiffOverview {
    rows: usize,
    marks: Vec<Mark>,
}

struct Mark {
    rows: Range<usize>,
    target: DiffLinePosition,
    removed: bool,
    added: bool,
}

impl DiffOverview {
    pub fn parse<'a>(
        patches: impl IntoIterator<Item = &'a str>,
    ) -> Result<(Vec<DiffFile>, Self), DiffParseError> {
        let mut files = Vec::new();
        let mut overview = Self::default();
        for patch in patches {
            let parsed = DiffFile::parse(patch)?;
            for (section, file) in patch.split("\ndiff --git ").zip(&parsed) {
                overview.append_file(section, file.path());
            }
            files.extend(parsed);
        }
        Ok((files, overview))
    }

    // Count full, aligned rows. Folding and wrapping only affect the detail view.
    fn append_file(&mut self, patch: &str, path: &SharedString) {
        let mut previous = [0; 2];
        for hunk in patch.split("\n@@ ").skip(1) {
            let Some((header, body)) = hunk.split_once('\n') else {
                continue;
            };
            let mut ranges = header.split_whitespace();
            let Some((old, new)) =
                hunk_range(ranges.next(), '-').zip(hunk_range(ranges.next(), '+'))
            else {
                continue;
            };
            self.rows += old
                .start
                .saturating_sub(previous[0])
                .max(new.start.saturating_sub(previous[1]));
            let mut cursor = [old.start, new.start];
            let mut block = cursor;
            for line in body.lines() {
                if cursor == [old.end, new.end] {
                    break;
                }
                match line.as_bytes().first().copied().unwrap_or(b' ') {
                    b'-' => cursor[0] += 1,
                    b'+' => cursor[1] += 1,
                    b' ' => {
                        self.append_mark(path, block, cursor);
                        cursor = [cursor[0] + 1, cursor[1] + 1];
                        block = cursor;
                        self.rows += 1;
                    }
                    _ => {} // No-newline metadata does not consume a source row.
                }
            }
            self.append_mark(path, block, cursor);
            previous = cursor;
        }
    }

    fn append_mark(&mut self, path: &SharedString, start: [usize; 2], end: [usize; 2]) {
        let removed = end[0] - start[0];
        let added = end[1] - start[1];
        let height = removed.max(added);
        if height == 0 {
            return;
        }
        let target = if removed > 0 {
            DiffLinePosition::new(path.clone(), DiffSide::Original, start[0] + 1)
        } else {
            DiffLinePosition::new(path.clone(), DiffSide::Modified, start[1] + 1)
        };
        self.marks.push(Mark {
            rows: self.rows..self.rows + height,
            target,
            removed: removed > 0,
            added: added > 0,
        });
        self.rows += height;
    }

    pub fn view(&self, diff: Diff, state: &Entity<DiffState>, cx: &App) -> AnyElement {
        if self.marks.is_empty() {
            return diff.size_full().into_any_element();
        }
        let removed = cx.theme().red;
        let added = cx.theme().green;
        let rows = self.rows as f32;
        h_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(diff.h_full().flex_1().min_w_0())
            .child(
                div()
                    .id("diff-overview")
                    .test_support()
                    .relative()
                    .flex_none()
                    .h_full()
                    .w(px(12.))
                    .overflow_hidden()
                    .border_l_1()
                    .border_color(cx.theme().border.opacity(0.65))
                    .bg(cx.theme().background)
                    .child(
                        div()
                            .absolute()
                            .left(px(2.))
                            .right(px(2.))
                            .top(px(1.))
                            .bottom(px(MARK_MIN_HEIGHT + 1.))
                            .children(self.marks.iter().enumerate().map(|(index, mark)| {
                                let state = state.clone();
                                let target = mark.target.clone();
                                let kind = match (mark.removed, mark.added) {
                                    (true, true) => "modified",
                                    (true, false) => "deleted",
                                    _ => "added",
                                };
                                let label = format!("{} · {}", tr(kind), target.line());
                                h_flex()
                                    .id(("diff-overview-mark", index))
                                    .test_support()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .top(relative(mark.rows.start as f32 / rows))
                                    .h(relative(mark.rows.len() as f32 / rows))
                                    .min_h(px(MARK_MIN_HEIGHT))
                                    .overflow_hidden()
                                    .rounded(px(1.5))
                                    .opacity(0.72)
                                    .hover(|style| style.opacity(1.))
                                    .cursor_pointer()
                                    .role(accesskit::Role::Button)
                                    .aria_label(label.clone())
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(label.clone()).build(window, cx)
                                    })
                                    .when(mark.removed, |bar| {
                                        bar.child(div().flex_1().h_full().bg(removed))
                                    })
                                    .when(mark.added, |bar| {
                                        bar.child(div().flex_1().h_full().bg(added))
                                    })
                                    .on_click(move |_, _, cx| {
                                        state.update(cx, |diff, cx| {
                                            diff.scroll_to_line(target.clone(), cx)
                                        });
                                    })
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn hunk_range(field: Option<&str>, prefix: char) -> Option<Range<usize>> {
    let field = field?.strip_prefix(prefix)?;
    let (start, count) = field.split_once(',').unwrap_or((field, "1"));
    let start: usize = start.parse().ok()?;
    let count: usize = count.parse().ok()?;
    // In a zero-count side, the header names the line before the change.
    let before = start.saturating_sub(usize::from(count > 0));
    Some(before..before.checked_add(count)?)
}
