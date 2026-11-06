//! Lazy, immutable commit reviews for the Changes sidebar.
use super::*;
use crate::workspace_files::{ChangedFile, Commit};
use gpui_kit::component::scroll::ScrollableElement as _;

#[derive(Default)]
pub(super) struct CommitReview {
    expanded: bool,
    copied_at: Option<std::time::Instant>,
    files_loading: bool,
    files: Option<Result<Vec<ChangedFile>, String>>,
    omitted: usize,
    expanded_files: HashSet<PathBuf>,
    // An entry with no result is an in-flight request.
    reviews: HashMap<PathBuf, Option<Result<Rc<ChangeReview>, String>>>,
}

/// A short, reversible reveal; settled closed content is not painted or focusable.
#[derive(IntoElement)]
struct ReviewDisclosure {
    id: ElementId,
    open: bool,
    content: AnyElement,
}
impl RenderOnce for ReviewDisclosure {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let progress = gpui_kit::base::transition(
            (self.id.clone(), "progress"),
            if self.open { 1.0f32 } else { 0.0 },
            gpui_kit::base::Transition::new(std::time::Duration::from_millis(if self.open {
                160
            } else {
                120
            })),
            window,
            cx,
        );
        gpui_kit::base::Collapsible::new()
            .w_full()
            .min_w_0()
            .flex_none()
            .when(progress > 0., |view| {
                view.reveal(self.id, progress).content(self.content)
            })
    }
}

impl RightPanel {
    fn toggle_commit(&mut self, id: String, cx: &mut Context<Self>) {
        let review = self.commit_reviews.entry(id.clone()).or_default();
        review.expanded = !review.expanded;
        if !review.expanded || review.files.as_ref().is_some_and(Result::is_ok) {
            cx.notify();
            return;
        }
        // Only new entries or failed loads need a request. Collapsing while a
        // request runs retains it, so reopening never duplicates the work.
        if review.files_loading {
            cx.notify();
            return;
        }
        review.files = None;
        review.files_loading = true;
        let root = self.change_root.clone();
        let source = self.root.clone();
        let work = cx.background_executor().spawn({
            let root = root.clone();
            let id = id.clone();
            async move { crate::workspace_files::commit_files(&root, &id) }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.root != source || this.change_root != root {
                    return;
                }
                let Some(review) = this.commit_reviews.get_mut(&id) else {
                    return;
                };
                review.files_loading = false;
                review.files = Some(result.map(|mut files| {
                    review.omitted = files.len().saturating_sub(MAX_REVIEW_FILES);
                    files.truncate(MAX_REVIEW_FILES);
                    files
                }));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn commit_copy_button(
        &self,
        commit: &Commit,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copied = self
            .commit_reviews
            .get(&commit.id)
            .is_some_and(|r| r.copied_at.is_some());
        let id = commit.id.clone();
        let group = SharedString::from(format!("commit-copy-{}", commit.id));
        h_flex()
            .id(group.clone())
            .group(group.clone())
            .debug_selector(move || format!("history-copy-{index}"))
            .keyboard_control()
            .role(Role::Button)
            .aria_label(if copied {
                "Copied commit hash"
            } else {
                "Copy commit hash"
            })
            .flex_none()
            .gap(zpx(crate::ui::SPACE_XS))
            .items_center()
            .font_family(MONO)
            .cursor_pointer()
            .child(commit.short_id.clone())
            .child(
                div()
                    .opacity(if copied { 1. } else { 0. })
                    .group_hover(group, |icon| icon.opacity(1.))
                    .child(
                        crate::micro_motion::MorphIcon::new(
                            ("commit-copy-icon", index),
                            crate::animated_icons::Morph::CopyCheck,
                            copied,
                        )
                        .with_size(zpx(13.)),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                let at = std::time::Instant::now();
                this.commit_reviews.entry(id.clone()).or_default().copied_at = Some(at);
                let id = id.clone();
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1500))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(review) = this.commit_reviews.get_mut(&id)
                            && review.copied_at == Some(at)
                        {
                            review.copied_at = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
                cx.notify();
            }))
            .into_any_element()
    }

    fn toggle_commit_file(&mut self, id: String, file: ChangedFile, cx: &mut Context<Self>) {
        let Some(review) = self.commit_reviews.get_mut(&id) else {
            return;
        };
        if review.expanded_files.remove(&file.path) {
            cx.notify();
            return;
        }
        review.expanded_files.insert(file.path.clone());
        if matches!(review.reviews.get(&file.path), Some(None | Some(Ok(_)))) {
            cx.notify();
            return;
        }
        review.reviews.insert(file.path.clone(), None);
        let root = self.change_root.clone();
        let source = self.root.clone();
        let path = file.path.clone();
        let work = cx.background_executor().spawn({
            let root = root.clone();
            let id = id.clone();
            async move { crate::workspace_files::commit_diff(&root, &id, &file) }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.root != source || this.change_root != root {
                    return;
                }
                let Some(review) = this.commit_reviews.get_mut(&id) else {
                    return;
                };
                review.reviews.insert(
                    path,
                    Some(result.map(|text| Rc::new(ChangeReview::parse(&text)))),
                );
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_commit(
        &self,
        commit: &Commit,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let empty = CommitReview::default();
        let review = self.commit_reviews.get(&commit.id).unwrap_or(&empty);
        let expanded = review.expanded;
        let id = commit.id.clone();
        let date = crate::workspace_files::commit_date_label(&commit.date, chrono::Local::now());
        let full_date = chrono::DateTime::parse_from_rfc3339(&commit.date)
            .map(|date| {
                date.with_timezone(&chrono::Local)
                    .format("%-d %B %Y, %-I:%M %p %:z")
                    .to_string()
            })
            .unwrap_or_else(|_| commit.date.clone());
        v_flex()
            .w_full()
            .min_w_0()
            .flex_none()
            .border_t_1()
            .border_color(crate::appearance::separator())
            .child(
                v_flex()
                    .id(SharedString::from(format!("commit-{}", commit.id)))
                    .debug_selector(move || format!("history-commit-{index}"))
                    .keyboard_control()
                    .role(gpui::Role::Button)
                    .aria_label(format!(
                        "{} commit {}: {}",
                        if expanded { "Collapse" } else { "Expand" },
                        commit.short_id,
                        commit.subject
                    ))
                    .px(zpx(crate::ui::SPACE_MD))
                    .py(zpx(crate::ui::SPACE_SM))
                    .gap(zpx(crate::ui::SPACE_XS))
                    .cursor_pointer()
                    .hover(|row| row.bg(crate::ui::row_hover()))
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap(zpx(crate::ui::SPACE_SM))
                            .items_start()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(zpx(TEXT_MD))
                                    .whitespace_normal()
                                    .child(commit.subject.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap(zpx(crate::ui::SPACE_SM))
                            .items_baseline()
                            .text_size(zpx(TEXT_SM))
                            .text_color(crate::appearance::secondary_text())
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(ActionTone::Success.foreground())
                                    .child(format!("+{}", commit.added)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(ActionTone::Danger.foreground())
                                    .child(format!("−{}", commit.removed)),
                            )
                            .child(self.commit_copy_button(commit, index, cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_right()
                                    .child(commit.author.clone()),
                            )
                            .child(
                                div()
                                    .id(("commit-date", index))
                                    .flex_none()
                                    .child(date)
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            full_date.clone(),
                                        )
                                        .build(window, cx)
                                    }),
                            ),
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_commit(id.clone(), cx)),
                    ),
            )
            .map(|view| {
                let body = v_flex()
                    .w_full()
                    .pl(zpx(22.))
                    .pr(zpx(crate::ui::SPACE_SM))
                    .pb(zpx(crate::ui::SPACE_SM))
                    .min_w_0();
                let content = match &review.files {
                    None => body.child(review_note("Loading files…")),
                    Some(Err(error)) => body.child(review_error("this commit’s files", error)),
                    Some(Ok(files)) => body
                        .when(files.is_empty(), |body| {
                            body.child(review_note("No files changed in this commit."))
                        })
                        .children(files.iter().enumerate().map(|(file_index, file)| {
                            self.render_commit_file(&commit.id, index, file_index, file, review, cx)
                        }))
                        .when(review.omitted > 0, |body| {
                            body.child(review_note(format!(
                                "{} more files not shown",
                                review.omitted
                            )))
                        }),
                };
                view.child(ReviewDisclosure {
                    id: SharedString::from(format!("commit-reveal-{}", commit.id)).into(),
                    open: expanded,
                    content: content.into_any_element(),
                })
            })
            .into_any_element()
    }

    fn render_commit_file(
        &self,
        id: &str,
        commit_index: usize,
        index: usize,
        file: &ChangedFile,
        review: &CommitReview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = review.expanded_files.contains(&file.path);
        let result = review.reviews.get(&file.path).and_then(Option::as_ref);
        let path = file.path.display().to_string();
        let tooltip = file
            .previous
            .as_ref()
            .map(|old| format!("{} → {path}", old.display()))
            .unwrap_or_else(|| path.clone());
        let id = id.to_string();
        let clicked = file.clone();
        let reveal_id = SharedString::from(format!("commit-file-reveal-{id}-{index}"));
        v_flex()
            .w_full()
            .min_w_0()
            .flex_none()
            .child(
                h_flex()
                    .id(SharedString::from(format!("commit-file-{id}-{index}")))
                    .w_full()
                    .debug_selector(move || format!("history-file-{commit_index}-{index}"))
                    .keyboard_control()
                    .role(gpui::Role::Button)
                    .aria_label(format!(
                        "{} {} ({})",
                        if expanded { "Collapse" } else { "Expand" },
                        path,
                        file.status
                    ))
                    .px(zpx(crate::ui::SPACE_SM))
                    .py(zpx(crate::ui::SPACE_SM))
                    .gap(zpx(crate::ui::SPACE_SM))
                    .items_center()
                    .cursor_pointer()
                    .text_size(zpx(TEXT_SM))
                    .hover(|row| row.bg(crate::ui::row_hover()))
                    .child(
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .with_size(zpx(11.)),
                    )
                    .child(file_icon(&file.path))
                    .child(div().flex_1().min_w_0().truncate().child(
                        StyledText::new(path.clone()).with_highlights(path.rfind('/').map(|end| {
                            (
                                0..end + 1,
                                HighlightStyle {
                                    color: Some(crate::appearance::secondary_text().into()),
                                    ..Default::default()
                                },
                            )
                        })),
                    ))
                    .when_some(result.and_then(|r| r.as_ref().ok()), |row, diff| {
                        row.when(diff.added > 0, |row| {
                            row.child(
                                div()
                                    .text_color(ActionTone::Success.foreground())
                                    .child(format!("+{}", diff.added)),
                            )
                        })
                        .when(diff.removed > 0, |row| {
                            row.child(
                                div()
                                    .text_color(ActionTone::Danger.foreground())
                                    .child(format!("−{}", diff.removed)),
                            )
                        })
                    })
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                            .build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_commit_file(id.clone(), clicked.clone(), cx)
                    })),
            )
            .child(ReviewDisclosure {
                id: reveal_id.into(),
                open: expanded,
                content: div()
                    .w_full()
                    .pl(zpx(12.))
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .debug_selector(move || format!("history-diff-{commit_index}-{index}"))
                            .child(match result {
                                None => review_note("Loading diff…"),
                                Some(Err(error)) => review_error("this diff", error),
                                Some(Ok(diff)) => render_review(
                                    SharedString::from(format!(
                                        "commit-diff-{commit_index}-{index}"
                                    ))
                                    .into(),
                                    diff.clone(),
                                ),
                            }),
                    )
                    .into_any_element(),
            })
            .into_any_element()
    }
}

pub(super) fn file_icon(path: &Path) -> AnyElement {
    use gpui_kit::assets::IconName as FileIcon;
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (icon, color) = match extension.as_str() {
        "rs" => (FileIcon::FileCode, 0xe7a06fff),
        "swift" => (FileIcon::FileCode, 0xf7a266ff),
        "js" | "jsx" | "ts" | "tsx" | "py" | "go" | "c" | "cpp" | "h" | "java" => {
            (FileIcon::FileCode, 0x7cb8deff)
        }
        "json" | "toml" | "yaml" | "yml" | "plist" | "xml" => (FileIcon::FileCog, 0xb5a1dfff),
        "png" | "jpg" | "jpeg" | "svg" | "gif" | "webp" | "ico" => {
            (FileIcon::FileImage, 0xcfa3cfff)
        }
        "sh" | "zsh" | "bash" => (FileIcon::FileTerminal, 0x91bca0ff),
        _ => (FileIcon::FileText, 0xaaaaaaff),
    };
    Icon::new(icon)
        .with_size(zpx(14.))
        .text_color(rgba(color))
        .into_any_element()
}

fn review_note(text: impl Into<SharedString>) -> AnyElement {
    div()
        .px(zpx(crate::ui::SPACE_SM))
        .py(zpx(crate::ui::SPACE_SM))
        .text_size(zpx(TEXT_SM))
        .text_color(crate::appearance::secondary_text())
        .whitespace_normal()
        .child(text.into())
        .into_any_element()
}

/// A plain sentence for a failed history load; Git's own words are on hover.
fn review_error(what: &str, raw: &str) -> AnyElement {
    let problem = crate::workspace_files::explain_git_error(raw);
    let summary = if problem.details.is_some() || problem.quiet {
        format!("Could not load {what}. Close and reopen to retry.")
    } else {
        format!("{} Close and reopen to retry.", problem.summary)
    };
    let details: SharedString = raw.to_owned().into();
    div()
        .id(SharedString::from(format!("review-error-{what}")))
        .px(zpx(crate::ui::SPACE_SM))
        .py(zpx(crate::ui::SPACE_SM))
        .whitespace_normal()
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(details.clone()).build(window, cx)
        })
        .child(crate::ui::inline_error(summary))
        .into_any_element()
}

/// The same bounded, syntax-highlighted diff for working and historical files.
pub(super) fn render_review(id: ElementId, review: Rc<ChangeReview>) -> AnyElement {
    ReviewView { id, review }.into_any_element()
}

#[derive(IntoElement)]
struct ReviewView {
    id: ElementId,
    review: Rc<ChangeReview>,
}
impl RenderOnce for ReviewView {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        use gpui_kit::component::diff::{Diff, DiffChangeIndicator, DiffHunkSeparator, DiffState};
        let fallback_height = self
            .review
            .files
            .is_empty()
            .then(|| zpx((self.review.lines.len() as f32 * TEXT_SM * 1.6).clamp(48., 320.)));
        let content = if self.review.files.is_empty() {
            render_plain_review((self.id.clone(), "plain").into(), self.review.clone())
        } else {
            let state = self
                .review
                .viewer
                .borrow_mut()
                .get_or_insert_with(|| cx.new(|cx| DiffState::new(self.review.files.clone(), cx)))
                .clone();
            Diff::new(&state)
                .header_visible(false)
                .single_gutter(true)
                .horizontal_scrollbar(false)
                .fit_height(zpx(320.))
                .hunk_separator(DiffHunkSeparator::LineInfo)
                .change_indicator(DiffChangeIndicator::Bars)
                .border_0()
                .size_full()
                .into_any_element()
        };
        div()
            .id(self.id)
            .debug_selector(|| "diff-scroll-area".into())
            .w_full()
            .min_w_0()
            .when_some(fallback_height, |view, height| view.h(height))
            .flex_none()
            .overflow_hidden()
            .child(content)
            // The inner viewer owns wheel input, including diagonal gestures and
            // edges. Never chain unused deltas to the surrounding commit list.
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
    }
}

fn render_plain_review(id: ElementId, review: Rc<ChangeReview>) -> AnyElement {
    let hidden = review.lines.len().saturating_sub(MAX_REVIEW_LINES);
    v_flex()
        .id(id)
        .size_full()
        .min_w_0()
        .flex_none()
        .overflow_scroll()
        .track_scroll(&review.scroll)
        .vertical_scrollbar(&review.scroll)
        .items_start()
        .children(review.lines.iter().take(MAX_REVIEW_LINES).map(|line| {
            let fill = match line.kind {
                '+' => 0x45cf7845,
                '-' => 0xff6b6b45,
                _ => 0x00000000,
            };
            h_flex()
                .min_w_full()
                .px(zpx(crate::ui::SPACE_SM))
                .gap(zpx(crate::ui::SPACE_SM))
                .flex_none()
                .whitespace_nowrap()
                .bg(rgba(fill))
                .font_family(MONO)
                .text_size(zpx(TEXT_SM))
                .line_height(relative(1.6))
                .child(
                    div()
                        .w(zpx(32.))
                        .flex_none()
                        .text_color(crate::ui::faint_text())
                        .child(line.old.map(|n| n.to_string()).unwrap_or_default()),
                )
                .child(
                    div()
                        .w(zpx(32.))
                        .flex_none()
                        .text_color(crate::ui::faint_text())
                        .child(line.new.map(|n| n.to_string()).unwrap_or_default()),
                )
                .child(
                    div()
                        .flex_none()
                        .child(format!("{} {}", line.kind, line.text)),
                )
        }))
        .when(hidden > 0, |list| {
            list.child(review_note(format!(
                "{hidden} more {} not shown",
                if hidden == 1 { "line" } else { "lines" }
            )))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    struct DiffFixture(Rc<ChangeReview>);
    impl Render for DiffFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(320.))
                .child(render_review("test-diff".into(), self.0.clone()))
        }
    }

    #[::gpui::test]
    fn diff_is_bounded_and_scrolls_on_both_axes(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let source = format!(
            "@@ -0,0 +1,100 @@\n{}",
            ("+".to_string() + &"wide content ".repeat(60) + "\n").repeat(100)
        );
        let review = Rc::new(ChangeReview::parse(&source));
        let (_, cx) = cx.add_window_view(|_, _| DiffFixture(review.clone()));
        cx.run_until_parked();
        let bounds = cx.debug_bounds("diff-scroll-area").unwrap();
        assert!(bounds.size.height <= zpx(320.));
        assert!(bounds.size.width <= px(320.));
        assert!(
            review.scroll.max_offset().x > px(500.),
            "long lines overflow horizontally"
        );
        assert!(
            review.scroll.max_offset().y > px(500.),
            "long diffs overflow vertically"
        );
        review.scroll.set_offset(point(px(-120.), px(-200.)));
        assert_eq!(review.scroll.offset(), point(px(-120.), px(-200.)));
    }
    #[::gpui::test]
    fn short_diff_height_excludes_hidden_patch_metadata(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let source = "diff --git a/test.rs b/test.rs\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ b/test.rs\n@@ -0,0 +1,3 @@\n+one\n+two\n+three\n";
        let review = Rc::new(ChangeReview::parse(source));
        let (_, cx) = cx.add_window_view(|_, _| DiffFixture(review.clone()));
        cx.run_until_parked();
        let expected = cx.update(|window, cx| {
            window.rem_size() * (f32::from(cx.theme().mono_font_size) / 16.) * 1.6 * 3.
        });
        let height = cx.debug_bounds("diff-scroll-area").unwrap().size.height;
        assert!(
            (height - expected).abs() <= px(1.),
            "three displayed rows must have no metadata-sized blank tail: {height:?} vs {expected:?}"
        );
    }

    struct NestedDiffFixture {
        review: Rc<ChangeReview>,
        outer: ScrollHandle,
    }
    impl Render for NestedDiffFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .id("outer-list")
                .w(px(400.))
                .h(px(500.))
                .overflow_y_scroll()
                .track_scroll(&self.outer)
                .child(render_review("nested-diff".into(), self.review.clone()))
                .child(div().h(px(1000.)).flex_none())
        }
    }

    #[::gpui::test]
    fn native_diff_contains_diagonal_and_edge_scrolls(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let source = format!(
            "diff --git a/test.rs b/test.rs\nnew file mode 100644\n--- /dev/null\n+++ b/test.rs\n@@ -0,0 +1,100 @@\n{}",
            ("+let value = ".to_owned() + &"1234 + ".repeat(100) + "0;\n").repeat(100)
        );
        let review = Rc::new(ChangeReview::parse(&source));
        assert_eq!(review.files.len(), 1);
        let outer = ScrollHandle::new();
        let (_, cx) = cx.add_window_view(|_, _| NestedDiffFixture {
            review: review.clone(),
            outer: outer.clone(),
        });
        cx.run_until_parked();
        let bounds = cx.debug_bounds("diff-scroll-area").unwrap();
        assert!(bounds.size.height <= zpx(320.));
        assert_eq!(bounds.size.width, px(400.));
        assert!(
            review.viewer.borrow().is_some(),
            "valid patches use the code diff viewer"
        );
        for (dx, dy) in [
            (-120., -40.),
            (120., 40.),
            (0., -10000.),
            (-100., -10000.),
            (0., 10000.),
        ] {
            cx.simulate_event(ScrollWheelEvent {
                position: bounds.center(),
                delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
                ..Default::default()
            });
            cx.run_until_parked();
            assert_eq!(
                outer.offset(),
                point(px(0.), px(0.)),
                "inner scrolling must not reach the outer list"
            );
        }
    }
}
