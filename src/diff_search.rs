use crate::{strings::tr, ui::icon_button};
use gpui_kit::{
    assets::IconName,
    base::TestSupportExt,
    component::{
        ActiveTheme, Disableable, Sizable,
        diff::{Diff, DiffEvent, DiffFile, DiffLinePosition, DiffSide, DiffState},
        h_flex,
        input::{Escape, Input, InputEvent, InputState},
    },
    prelude::FluentBuilder,
    *,
};
use std::{
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const FIND_DELAY: Duration = Duration::from_millis(150);

gpui_kit::actions!(patchlook, [FindInDiff]);

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("secondary-f", FindInDiff, None)]);
}

#[derive(Default)]
struct Matches {
    lines: Vec<MatchedLine>,
    by_position: HashMap<DiffLinePosition, usize>,
    count: usize,
}

struct MatchedLine {
    position: DiffLinePosition,
    first: usize,
    starts: Vec<usize>,
}

impl Matches {
    fn scan(files: &[DiffFile], query: &str, cancelled: &AtomicBool) -> Self {
        let mut matches = Self::default();
        for file in files {
            for [original, modified] in file.source_lines_in_patch_order() {
                let (side, line) = if let Some(line) = modified {
                    (DiffSide::Modified, line)
                } else if let Some(line) = original {
                    (DiffSide::Original, line)
                } else {
                    continue;
                };
                if cancelled.load(Ordering::Relaxed) {
                    return Self::default();
                }
                let (number, text) = line;
                let mut starts = Vec::new();
                for (offset, _) in text.match_indices(query) {
                    if cancelled.load(Ordering::Relaxed) {
                        return Self::default();
                    }
                    starts.push(offset);
                }
                if starts.is_empty() {
                    continue;
                }
                let position = DiffLinePosition::new(file.path().clone(), side, number);
                let row = matches.lines.len();
                matches.by_position.insert(position.clone(), row);
                // An unchanged line has one result and highlights in both panes.
                if let (Some((number, _)), Some(_)) = (original, modified) {
                    matches.by_position.insert(
                        DiffLinePosition::new(file.path().clone(), DiffSide::Original, number),
                        row,
                    );
                }
                matches.lines.push(MatchedLine {
                    position,
                    first: matches.count,
                    starts,
                });
                matches.count += matches.lines[row].starts.len();
            }
        }
        matches
    }
}

struct SearchTask {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for SearchTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

pub struct DiffSearch {
    diff: Entity<DiffState>,
    input: Option<Entity<InputState>>,
    open: bool,
    query: String,
    matches: Rc<Matches>,
    current: Option<usize>,
    pending: Option<SearchTask>,
    _input_subscription: Option<Subscription>,
    _diff_subscription: Subscription,
}

impl DiffSearch {
    pub fn new(diff: Entity<DiffState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&diff, Self::diff_changed);
        Self {
            diff,
            input: None,
            open: false,
            query: String::new(),
            matches: Rc::default(),
            current: None,
            pending: None,
            _input_subscription: None,
            _diff_subscription: subscription,
        }
    }

    pub fn available(&self, cx: &App) -> bool {
        self.diff
            .read(cx)
            .files()
            .iter()
            .any(|file| file.source_lines_in_patch_order().next().is_some())
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.available(cx) {
            return;
        }
        let input = self
            .input
            .get_or_insert_with(|| {
                let input =
                    cx.new(|cx| InputState::new(window, cx).placeholder(tr("find_in_diff")));
                self._input_subscription = Some(cx.subscribe(&input, Self::input_changed));
                input
            })
            .clone();
        if !self.open {
            self.open = true;
            if let Some(selection) = self
                .diff
                .read(cx)
                .selected_inline_text(cx)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
            {
                input.update(cx, |input, cx| input.set_value(selection, window, cx));
            }
            let query = input.read(cx).value().to_string();
            if self.query != query {
                self.search(query, Duration::ZERO, None, cx);
            }
        }
        input.update(cx, |input, cx| {
            window.focus(&input.focus_handle(cx), cx);
            input.select_all(window, cx);
        });
        cx.notify();
    }

    pub fn update_placeholder(&self, window: &mut Window, cx: &mut App) {
        if let Some(input) = &self.input {
            input.update(cx, |input, cx| {
                input.set_placeholder(tr("find_in_diff"), window, cx)
            });
        }
    }

    fn diff_changed(&mut self, diff: Entity<DiffState>, event: &DiffEvent, cx: &mut Context<Self>) {
        match event {
            DiffEvent::TextSelectionFinished if !self.open => {
                let query = diff
                    .read(cx)
                    .selected_inline_text(cx)
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or("");
                if self.query != query {
                    self.search(query.to_owned(), Duration::ZERO, None, cx);
                }
            }
            DiffEvent::TextSelectionCleared if !self.open && !self.query.is_empty() => {
                self.search(String::new(), Duration::ZERO, None, cx);
            }
            DiffEvent::FilesChanged => {
                let query = if self.open {
                    self.query.clone()
                } else {
                    String::new()
                };
                self.search(query, Duration::ZERO, None, cx);
            }
            _ => {}
        }
    }

    fn input_changed(
        &mut self,
        input: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.open {
            return;
        }
        match event {
            InputEvent::Change => {
                self.search(
                    input.read(cx).value().to_string(),
                    FIND_DELAY,
                    Some(true),
                    cx,
                );
            }
            InputEvent::PressEnter { shift, .. } => self.navigate(!shift, cx),
            _ => {}
        }
    }

    fn search(
        &mut self,
        query: String,
        delay: Duration,
        jump: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        self.pending = None;
        self.matches = Rc::default();
        self.current = None;
        self.query = query;
        cx.notify();
        let files = self.diff.read(cx).files();
        if self.query.is_empty() || files.is_empty() {
            return;
        }
        let files = files.to_vec();
        let query = self.query.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let request = cancelled.clone();
        self.pending = Some(SearchTask {
            cancelled,
            _task: cx.spawn(async move |this, cx| {
                if !delay.is_zero() {
                    cx.background_executor().timer(delay).await;
                }
                let worker = request.clone();
                let matches = cx
                    .background_spawn(async move { Matches::scan(&files, &query, &worker) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this
                        .pending
                        .as_ref()
                        .is_some_and(|task| Arc::ptr_eq(&task.cancelled, &request))
                    {
                        this.pending = None;
                        this.matches = Rc::new(matches);
                        if let Some(forward) = jump {
                            this.navigate(forward, cx);
                        }
                        cx.notify();
                    }
                });
            }),
        });
    }

    fn navigate(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            self.search(self.query.clone(), Duration::ZERO, Some(forward), cx);
            return;
        }
        let count = self.matches.count;
        if count == 0 {
            return;
        }
        let index = match (self.current, forward) {
            (Some(index), true) => (index + 1) % count,
            (Some(index), false) if index > 0 => index - 1,
            (None, true) => 0,
            _ => count - 1,
        };
        self.current = Some(index);
        let row = self
            .matches
            .lines
            .partition_point(|line| line.first <= index)
            - 1;
        let position = self.matches.lines[row].position.clone();
        self.diff
            .update(cx, |diff, cx| diff.scroll_to_line_nearby(position, cx));
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.search(String::new(), Duration::ZERO, None, cx);
        window.focus(&self.diff.read(cx).focus_handle(cx), cx);
    }

    pub fn highlight(&self, diff: Diff, cx: &App) -> Diff {
        if self.matches.count == 0 {
            return diff;
        }
        let matches = self.matches.clone();
        let current = self.current;
        let length = self.query.len();
        let color = cx.theme().yellow;
        diff.text_highlights(move |position| {
            let Some(&row) = matches.by_position.get(position) else {
                return Vec::new();
            };
            let line = &matches.lines[row];
            line.starts
                .iter()
                .enumerate()
                .map(|(index, &start)| {
                    let opacity = if current == Some(line.first + index) {
                        0.68
                    } else {
                        0.34
                    };
                    (
                        start..start + length,
                        HighlightStyle {
                            background_color: Some(color.opacity(opacity)),
                            ..Default::default()
                        },
                    )
                })
                .collect()
        })
    }
}

impl Render for DiffSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(input) = self.input.as_ref().filter(|_| self.open) else {
            return div().into_any_element();
        };
        let status = if self.pending.is_some() {
            "…".into()
        } else {
            format!(
                "{} / {}",
                self.current.map_or(0, |i| i + 1),
                self.matches.count
            )
        };
        h_flex()
            .flex_none()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .on_action(cx.listener(|this, _: &Escape, window, cx| this.close(window, cx)))
            .child(
                Input::new(input)
                    .id("diff-find-input")
                    .aria_label(tr("find_in_diff"))
                    .small()
                    .cleanable(true)
                    .flex_1()
                    .min_w_0(),
            )
            .when(!self.query.is_empty(), |bar| {
                bar.child(
                    div()
                        .id("diff-find-count")
                        .test_support()
                        .aria_label(status.clone())
                        .px_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(status),
                )
            })
            .child(
                icon_button("diff-find-previous", IconName::ChevronUp, "previous_match")
                    .small()
                    .disabled(self.matches.count == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.navigate(false, cx))),
            )
            .child(
                icon_button("diff-find-next", IconName::ChevronDown, "next_match")
                    .small()
                    .disabled(self.matches.count == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.navigate(true, cx))),
            )
            .child(
                icon_button("diff-find-close", IconName::X, "dismiss")
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::test::{TestEventExt, TestWindowExt};

    struct Viewer {
        diff: Entity<DiffState>,
        find: Entity<DiffSearch>,
    }

    impl Render for Viewer {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.find
                .read(cx)
                .highlight(Diff::new(&self.diff), cx)
                .size_full()
        }
    }

    #[gpui_kit::test]
    fn diff_find_matches_selection_when_the_mouse_is_released(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, viewer) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| {
                    let diff = cx.new(|cx| {
                        DiffState::new([DiffFile::unchanged("file.txt", "needle\nneedle\n")], cx)
                    });
                    let find = cx.new(|cx| DiffSearch::new(diff.clone(), cx));
                    Viewer { diff, find }
                })
            })
            .unwrap()
        });
        let end = cx
            .update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                window.render_frame(cx);
                let bounds = window.find_all("source")[0].bounds();
                let start = point(bounds.left() + px(1.), bounds.center().y);
                let end = point(bounds.left() + px(65.), bounds.center().y);
                window.pointer_down(start, MouseButton::Left, cx);
                window.pointer_move(end, Some(MouseButton::Left), cx);
                assert_eq!(
                    viewer.read(cx).diff.read(cx).selected_inline_text(cx),
                    Some("needle")
                );
                end
            })
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(viewer.read(cx).find.read(cx).matches.count, 0));
        cx.update_window(handle, |_, window, cx| {
            window.pointer_up(end, MouseButton::Left, cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(viewer.read(cx).find.read(cx).matches.count, 2));
    }
}
