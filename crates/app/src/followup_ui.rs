use crate::animated_icons::Glyph;
use crate::keyboard_control::KeyboardControl as _;
use crate::micro_motion::MotionIconExt as _;
use crate::zoom::px as zpx;
use crate::{
    AppShell,
    appearance::{rgb, rgba},
    state::{Selection, Tab},
};
use del_agent::{Attachment, UserInput};
use gpui_kit::component::{
    button::{Button, ButtonCustomVariant, ButtonVariants},
    *,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

#[derive(Clone)]
struct QueueDrag {
    owner: EntityId,
    id: String,
    label: SharedString,
}
/// The row being dragged: its text on the tray's surface, under the pointer.
struct QueueDragPreview(SharedString);
impl Render for QueueDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h(zpx(32.))
            .w(zpx(280.))
            .px(zpx(crate::ui::SPACE_MD))
            .gap(zpx(crate::ui::SPACE_MD))
            .items_center()
            .rounded(zpx(crate::ui::RADIUS_ROW))
            .bg(crate::appearance::popup_surface())
            .border_1()
            .border_color(crate::appearance::separator())
            .shadow_md()
            .opacity(0.94)
            .text_color(crate::ui::primary_text())
            .child(crate::icons::path("icons/queue.svg").text_color(crate::ui::faint_text()))
            .child(div().flex_1().min_w_0().truncate().child(self.0.clone()))
    }
}

/// Imports still being copied into Del's attachment store, by composer
/// owner, and owners whose Send waits for them.
#[derive(Default)]
pub(crate) struct PendingImports {
    labels: std::collections::HashMap<u64, Vec<(u64, String)>>,
    held: std::collections::HashSet<u64>,
    next: u64,
}
impl Global for PendingImports {}

impl PendingImports {
    pub(crate) fn any(owner: u64, cx: &App) -> bool {
        cx.try_global::<Self>()
            .is_some_and(|imports| imports.labels.get(&owner).is_some_and(|l| !l.is_empty()))
    }
    fn labels(owner: u64, cx: &App) -> Vec<String> {
        cx.try_global::<Self>()
            .and_then(|imports| imports.labels.get(&owner))
            .map(|labels| labels.iter().map(|(_, label)| label.clone()).collect())
            .unwrap_or_default()
    }
    /// Hold `owner`'s Send until its imports land. Returns false when none are pending.
    pub(crate) fn hold(owner: u64, cx: &mut App) -> bool {
        if !Self::any(owner, cx) {
            return false;
        }
        cx.default_global::<Self>().held.insert(owner);
        true
    }
}
fn thumbnail(attachment: &Attachment) -> impl IntoElement {
    div()
        .size(zpx(24.))
        .flex_none()
        .rounded(zpx(crate::ui::RADIUS_XS))
        .overflow_hidden()
        .border_1()
        .border_color(rgba(0xffffff20))
        .flex()
        .items_center()
        .justify_center()
        .when(crate::attachments::is_image(attachment), |el| {
            el.child(
                img(attachment.path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
        })
        .when(!crate::attachments::is_image(attachment), |el| {
            el.child(
                crate::icons::icon(IconName::File).text_color(crate::appearance::secondary_text()),
            )
        })
}
fn action(id: SharedString, label: &'static str) -> Button {
    Button::new(id.clone())
        .debug_selector(move || id.to_string())
        .size(crate::icons::button_size())
        .flex_none()
        .p_0()
        .ghost()
        .text_color(crate::ui::secondary_text())
        .tooltip(label)
        .accessibility_label(label)
}

/// One shared tray for queued follow-ups and the goal bar: the same inset,
/// surface and top corners, sitting on the composer.
pub(crate) fn tray() -> Div {
    v_flex()
        .w_full()
        .rounded_t(zpx(crate::ui::RADIUS_CARD))
        .bg(crate::appearance::queue_surface())
}

/// A pasted or dropped file still importing: its name and a spinner.
pub(crate) fn pending_chip(index: usize, label: String) -> impl IntoElement {
    h_flex()
        .id(("pending-attachment", index))
        .flex_none()
        .h(zpx(56.))
        .w(zpx(148.))
        .p(zpx(crate::ui::SPACE_SM))
        .gap(zpx(crate::ui::SPACE_SM))
        .items_center()
        .rounded(zpx(crate::ui::RADIUS_CARD))
        .bg(rgba(0xffffff08))
        .border_1()
        .border_color(crate::appearance::separator())
        .aria_label(format!("Adding {label}"))
        .child(
            div()
                .size(zpx(40.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(crate::icons::spinner()),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(zpx(crate::ui::SPACE_XS))
                .child(
                    div()
                        .truncate()
                        .text_size(zpx(12.))
                        .text_color(crate::ui::primary_text())
                        .child(label),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(zpx(del_core::typography::TEXT_XS))
                        .text_color(crate::ui::faint_text())
                        .child("Adding…"),
                ),
        )
}

/// One attachment preview chip, shared by main and Side chat composers:
/// an image thumbnail or a file card, opening on click, with a hover remove.
pub(crate) fn attachment_chip(
    scope: &'static str,
    index: usize,
    file: &Attachment,
    cx: &App,
    on_open: impl Fn(&mut Window, &mut App) + 'static,
    on_remove: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let group = SharedString::from(format!("{scope}-attachment-{index}"));
    let is_image = crate::attachments::is_image(file);
    h_flex()
        .id(SharedString::from(format!(
            "{scope}-attachment-chip-{index}"
        )))
        .keyboard_control()
        .role(gpui::Role::Button)
        .aria_label(format!("Preview {}", crate::attachments::filename(file)))
        .group(group.clone())
        .relative()
        .flex_none()
        .h(zpx(56.))
        .w(zpx(if is_image { 56. } else { 148. }))
        .gap(zpx(crate::ui::SPACE_SM))
        .items_center()
        .rounded(zpx(crate::ui::RADIUS_CARD))
        .cursor_pointer()
        .when(!is_image, |row| {
            row.p(zpx(crate::ui::SPACE_SM))
                .bg(rgba(0xffffff08))
                .border_1()
                .border_color(crate::appearance::separator())
        })
        .on_click(move |_, window, cx| on_open(window, cx))
        .when(is_image, |row| {
            row.child(
                div()
                    .size(zpx(56.))
                    .rounded(zpx(crate::ui::RADIUS_CARD))
                    .overflow_hidden()
                    .child(
                        img(file.path.clone())
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    ),
            )
        })
        .when(!is_image, |row| {
            row.child(
                div()
                    .size(zpx(40.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if let Some(asset) = crate::attachments::file_mark(file) {
                        crate::icons::path(asset)
                            .text_color(crate::ui::secondary_text())
                            .into_any_element()
                    } else {
                        crate::icons::icon(IconName::File)
                            .text_color(crate::ui::faint_text())
                            .into_any_element()
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(zpx(crate::ui::SPACE_XS))
                    .child(
                        div()
                            .truncate()
                            .text_size(zpx(12.))
                            .text_color(crate::ui::primary_text())
                            .child(crate::attachments::compact_filename(file)),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(zpx(del_core::typography::TEXT_XS))
                            .text_color(crate::ui::faint_text())
                            .child(crate::attachments::file_type(file)),
                    ),
            )
        })
        .tooltip({
            let name = crate::attachments::filename(file);
            move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(name.clone()).build(window, cx)
            }
        })
        .child(
            Button::new(SharedString::from(format!(
                "{scope}-remove-attachment-{index}"
            )))
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(rgb(0xf5f5f5).into())
                    .foreground(rgb(0x202020).into())
                    .hover(rgb(0xffffff).into())
                    .active(rgb(0xe5e5e5).into()),
            )
            .accessibility_label("Remove attachment")
            .tooltip("Remove attachment")
            .absolute()
            .right(zpx(crate::ui::SPACE_2XS))
            .top(zpx(crate::ui::SPACE_2XS))
            .size(crate::icons::button_size())
            .p_0()
            .rounded_full()
            .opacity(0.0)
            .group_hover(group, |button| button.opacity(1.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                on_remove(window, cx);
            })
            .motion_child(true, move |feedback| {
                feedback.glyph(Glyph::Close).text_color(rgb(0x202020))
            }),
        )
}

/// Grow a newly mounted row from nothing over `ui::REVEAL`, so a tray or strip
/// never jumps to its new height. Reduce Motion shows it at once.
pub(crate) fn reveal_row(
    id: impl Into<ElementId>,
    height: f32,
    row: impl IntoElement,
) -> impl IntoElement {
    div()
        .w_full()
        .flex_none()
        .overflow_hidden()
        .child(row)
        .with_animation(
            id,
            Animation::new(crate::ui::REVEAL).with_easing(crate::ui::ease_out),
            move |row, t| row.h(zpx(height * t)).opacity(t),
        )
}
impl AppShell {
    pub(super) fn composer_owner(&self, cx: &App) -> u64 {
        let state = self.state.read(cx);
        state.uid_of(&match &state.selection {
            Selection::Draft(id) => Tab::Draft(*id),
            Selection::Session(id) => Tab::Session(id.clone()),
            Selection::Terminal(id) => Tab::Terminal(id.clone()),
        })
    }
    pub(super) fn sync_composer_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = self.composer_owner(cx);
        if owner != self.composer_owner {
            self.attachment_scroll.set_offset(point(zpx(0.), zpx(0.)));
            self.composer_drafts
                .insert(self.composer_owner, self.composer.read(cx).content());
            let text = self
                .composer_drafts
                .remove(&owner)
                .unwrap_or_else(|| gpui_kit::component::input::InputContent::new(""));
            self.composer
                .update(cx, |input, cx| input.set_value(text, window, cx));
            self.composer_owner = owner;
            self.sync_goal_placeholder(window, cx);
        }
    }
    pub(super) fn pick_attachments(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        let owner = self.composer_owner(cx);
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                let _ = this.update(cx, |this, cx| this.attach_paths(owner, paths, cx));
            }
        })
        .detach();
    }
    pub(super) fn attach_paths(&mut self, owner: u64, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.attach_sources(
            owner,
            paths
                .into_iter()
                .map(crate::attachments::ImportSource::File)
                .collect(),
            cx,
        );
    }
    fn attach_sources(
        &mut self,
        owner: u64,
        sources: Vec<crate::attachments::ImportSource>,
        cx: &mut Context<Self>,
    ) {
        // A placeholder chip shows at once; Send waits for it rather than
        // leaving the file behind.
        let ticket = {
            let imports = cx.default_global::<PendingImports>();
            imports.next += 1;
            let ticket = imports.next;
            let labels = imports.labels.entry(owner).or_default();
            labels.extend(sources.iter().map(|source| (ticket, source.label())));
            ticket
        };
        cx.notify();
        let work = self
            .state
            .read(cx)
            .backend()
            .runtime
            .spawn_blocking(move || {
                let directory = del_agent::data_dir()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("attachments");
                let mut files = Vec::new();
                let mut errors = Vec::new();
                for source in sources {
                    let label = source.label();
                    match source.import(&directory) {
                        Ok(file) => files.push(file),
                        Err(error) => errors.push(format!("{label}: {error}")),
                    }
                }
                (files, errors)
            });
        cx.spawn(async move |this, cx| {
            let result = work.await.map_err(|e| e.to_string());
            let send = this.update(cx, |this, cx| {
                let error = match result {
                    Ok((files, errors)) => {
                        this.composer_attachments
                            .entry(owner)
                            .or_default()
                            .extend(files);
                        (!errors.is_empty()).then(|| errors.join("\n"))
                    }
                    Err(error) => Some(error),
                };
                // Report only this import's failures; never clear another error.
                if let Some(error) = error {
                    this.state.update(cx, |s, cx| {
                        s.report_error(format!("Could not attach {error}"));
                        cx.notify();
                    });
                }
                let imports = cx.default_global::<PendingImports>();
                let done = imports.labels.get_mut(&owner).is_none_or(|labels| {
                    labels.retain(|(at, _)| *at != ticket);
                    labels.is_empty()
                });
                let held = done && imports.held.remove(&owner);
                cx.notify();
                held && this.composer_owner(cx) == owner
            });
            // A Send pressed during the import goes out now, with the files.
            if send.unwrap_or(false)
                && let Some(window) = cx.update(|cx| cx.active_window())
            {
                let _ = cx.update_window(window, |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.submit(window, cx));
                });
            }
        })
        .detach();
    }
    pub(super) fn paste_attachments(
        &mut self,
        clipboard: &ClipboardItem,
        cx: &mut Context<Self>,
    ) -> bool {
        let sources = crate::attachments::clipboard_sources(clipboard);
        if sources.is_empty() {
            false
        } else {
            self.attach_sources(self.composer_owner(cx), sources, cx);
            true
        }
    }
    pub(super) fn edit_queued(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.open_chat(cx) else {
            return;
        };
        let Some(input) = chat.update(cx, |chat, cx| chat.take_queued(id, cx)) else {
            return;
        };
        let owner = self.composer_owner(cx);
        // The draft being replaced is kept: it moves to the end of the queue,
        // where its row grows in so the move is visible.
        let current = UserInput {
            command: self
                .command_menu
                .read(cx)
                .selected_command(self.composer.read(cx).value().as_ref(), cx),
            skills: crate::command_menu::selected_skills(&self.composer.read(cx).content()),
            text: self.composer.read(cx).value().to_string(),
            attachments: self.composer_attachments.remove(&owner).unwrap_or_default(),
        };
        if !current.text.trim().is_empty() || !current.attachments.is_empty() {
            chat.update(cx, |chat, cx| {
                chat.enqueue(current, cx);
            });
        }
        let content = crate::command_menu::restore_skill_tokens(&input);
        self.composer_attachments.insert(owner, input.attachments);
        self.composer.update(cx, |composer, cx| {
            composer.set_value(content, window, cx);
            composer.focus(window, cx);
        });
        cx.notify();
    }
    pub(super) fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.composer_owner(cx);
        let pending = PendingImports::labels(owner, cx);
        let strip = h_flex()
            .id("composer-attachments")
            .w_full()
            .min_w_0()
            .h(zpx(64.))
            .overflow_x_scroll()
            .overflow_y_hidden()
            .track_scroll(&self.attachment_scroll)
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height());
                let dx = if delta.x.abs() > delta.y.abs() {
                    delta.x
                } else {
                    delta.y
                };
                let scroll = &this.attachment_scroll;
                let x = (scroll.offset().x + dx).clamp(-scroll.max_offset().x, zpx(0.));
                scroll.set_offset(point(x, zpx(0.)));
                cx.stop_propagation();
                cx.notify();
            }))
            .p(zpx(crate::ui::SPACE_XS))
            .gap(zpx(crate::ui::SPACE_MD))
            .children(
                self.composer_attachments
                    .get(&owner)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, file)| {
                        let path = file.path.clone();
                        let shell = cx.entity().downgrade();
                        let remove = cx.entity().downgrade();
                        attachment_chip(
                            "composer",
                            index,
                            file,
                            cx,
                            move |_, cx| {
                                let _ = shell.update(cx, |this, cx| {
                                    this.set_right(true, cx);
                                    this.right.update(cx, |panel, cx| {
                                        panel.preview_file(path.clone(), cx)
                                    });
                                });
                            },
                            move |_, cx| {
                                let _ = remove.update(cx, |this, cx| {
                                    if let Some(files) = this.composer_attachments.get_mut(&owner)
                                        && index < files.len()
                                    {
                                        files.remove(index);
                                    }
                                    cx.notify();
                                });
                            },
                        )
                    }),
            )
            .children(
                pending
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| pending_chip(index, label)),
            );
        // The strip grows in when the first attachment arrives.
        reveal_row(
            ("composer-attachments-reveal", owner),
            64.,
            div()
                .relative()
                .w_full()
                .min_w_0()
                .h(zpx(64.))
                .flex_none()
                .overflow_hidden()
                .child(strip)
                .children([true, false].into_iter().map(|left| {
                    let scroll = self.attachment_scroll.clone();
                    canvas(
                        |bounds, _, _| bounds,
                        move |_, bounds, window, _| {
                            let (has_left, has_right) = attachment_overflow(
                                scroll.offset().x.as_f32(),
                                scroll.max_offset().x.as_f32(),
                            );
                            if if left { has_left } else { has_right } {
                                window.paint_quad(fill(
                                    bounds,
                                    linear_gradient(
                                        if left { 90. } else { 270. },
                                        linear_color_stop(rgba(0x22222299), 0.),
                                        linear_color_stop(rgba(0x22222200), 1.),
                                    ),
                                ));
                            }
                        },
                    )
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w(zpx(40.))
                    .when(left, |fade| fade.left_0())
                    .when(!left, |fade| fade.right_0())
                })),
        )
    }
    pub(super) fn render_followups(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(chat) = self.open_chat(cx) else {
            return div().into_any_element();
        };
        let shell = cx.entity().downgrade();
        render_queue(&chat, cx, move |id, window, cx| {
            let _ = shell.update(cx, |shell, cx| shell.edit_queued(id, window, cx));
        })
    }
}

/// Shared queue presentation and actions for main and Side chat composers.
/// Drag ownership follows the chat entity so a drop cannot cross conversations.
pub(crate) fn render_queue(
    chat: &Entity<crate::chat::ChatView>,
    cx: &App,
    on_edit: impl Fn(&str, &mut Window, &mut App) + 'static,
) -> AnyElement {
    // In-flight input is already visible in the transcript. Showing it here
    // briefly grows the composer, then shrinks it again on acknowledgement.
    let items: Vec<_> = chat
        .read(cx)
        .queue
        .items
        .iter()
        .filter(|item| !item.sending)
        .cloned()
        .collect();
    if items.is_empty() {
        return div().into_any_element();
    }
    let owner = chat.entity_id();
    let on_edit = std::rc::Rc::new(on_edit);
    let overflow = items.len() > 4;
    let count = items.len();
    // Four rows show; more scroll. New rows grow in, so the tray never jumps.
    let max_height = 4. * 32. + 2. * crate::ui::SPACE_XS;
    div()
        .relative()
        .w_full()
        .px(zpx(crate::ui::SPACE_MD))
        .child(
            tray()
                .id("followup-queue")
                .debug_selector(|| "followup-queue".into())
                .max_h(zpx(max_height))
                .overflow_y_scroll()
                .py(zpx(crate::ui::SPACE_XS))
                .children(items.into_iter().enumerate().map(|(index, item)| {
                    let group: SharedString = format!("followup-{}", item.id).into();
                    let on_edit = on_edit.clone();
                    let edit = item.id.clone();
                    let delete = item.id.clone();
                    let send = item.id.clone();
                    let target = item.id.clone();
                    let delete_chat = chat.clone();
                    let send_chat = chat.clone();
                    let reorder_chat = chat.clone();
                    let preview: SharedString = if item.input.text.is_empty() {
                        item.input
                            .attachments
                            .iter()
                            .map(crate::attachments::filename)
                            .collect::<Vec<_>>()
                            .join(", ")
                            .into()
                    } else {
                        item.input
                            .text
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .to_owned()
                            .into()
                    };
                    let reveal_id = SharedString::from(format!("followup-reveal-{}", item.id));
                    let row = h_flex()
                        .id(group.clone())
                        .group(group.clone())
                        .h(zpx(32.))
                        .w_full()
                        .min_w_0()
                        .flex_none()
                        .px(zpx(crate::ui::SPACE_MD))
                        .gap(zpx(crate::ui::SPACE_MD))
                        .items_center()
                        .when(index + 1 < count, |row| {
                            row.border_b_1()
                                .border_color(crate::appearance::separator())
                        })
                        // Where the dragged row will land.
                        .drag_over::<QueueDrag>(|row, _, _, _| {
                            row.bg(crate::ui::row_selected())
                                .border_t_2()
                                .border_color(crate::appearance::ActionTone::Accent.foreground())
                        })
                        .on_drop(move |drag: &QueueDrag, _, cx| {
                            if drag.owner == owner {
                                reorder_chat.update(cx, |chat, cx| {
                                    chat.reorder_queued(&drag.id, &target, cx)
                                });
                            }
                        })
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .gap(zpx(crate::ui::SPACE_MD))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("drag-{}", item.id)))
                                        .cursor_grab()
                                        .w(crate::icons::size())
                                        .h(crate::icons::button_size())
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(crate::ui::faint_text())
                                        .on_drag(
                                            QueueDrag {
                                                owner,
                                                id: item.id.clone(),
                                                label: preview.clone(),
                                            },
                                            |drag: &QueueDrag, _, _, cx| {
                                                let label = drag.label.clone();
                                                cx.new(|_| QueueDragPreview(label))
                                            },
                                        )
                                        .child(
                                            crate::icons::path("icons/queue.svg")
                                                .text_color(crate::ui::faint_text()),
                                        ),
                                )
                                .children(item.input.attachments.iter().take(3).map(thumbnail))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(crate::ui::primary_text())
                                        .child(if item.input.text.is_empty() {
                                            item.input
                                                .attachments
                                                .iter()
                                                .map(crate::attachments::filename)
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        } else {
                                            item.input.text.clone()
                                        }),
                                ),
                        )
                        .child(
                            Button::new(SharedString::from(format!("steer-{}", item.id)))
                                .debug_selector(|| format!("steer-{}", item.id))
                                .ghost()
                                .small()
                                .compact()
                                .flex_none()
                                .text_color(crate::ui::secondary_text())
                                .accessibility_label("Steer queued message")
                                .on_click(move |_, _, cx| {
                                    send_chat.update(cx, |chat, cx| {
                                        chat.deliver_queued(send.clone(), true, cx)
                                    })
                                })
                                .motion_child(true, move |feedback| {
                                    h_flex()
                                        .items_center()
                                        .gap(zpx(crate::ui::SPACE_XS))
                                        .child(feedback.glyph(Glyph::Steer))
                                        .child("Steer")
                                }),
                        )
                        .child(
                            action(
                                format!("delete-{}", item.id).into(),
                                "Delete queued message",
                            )
                            .on_click(move |_, _, cx| {
                                delete_chat.update(cx, |chat, cx| {
                                    chat.take_queued(&delete, cx);
                                });
                            })
                            .motion_child(true, move |feedback| feedback.glyph(Glyph::Trash)),
                        )
                        .child(
                            action(format!("edit-{}", item.id).into(), "Edit queued message")
                                .on_click(move |_, window, cx| on_edit(&edit, window, cx))
                                .motion_child(true, move |feedback| feedback.glyph(Glyph::Pencil)),
                        );
                    reveal_row(reveal_id, 32., row)
                })),
        )
        .when(overflow, |queue| {
            queue.child(
                div()
                    .absolute()
                    .left(zpx(crate::ui::SPACE_MD))
                    .right(zpx(crate::ui::SPACE_MD))
                    .bottom_0()
                    .h(zpx(12.))
                    .bg(linear_gradient(
                        0.,
                        linear_color_stop(crate::appearance::queue_surface(), 0.),
                        linear_color_stop(crate::appearance::queue_surface().opacity(0.), 1.),
                    )),
            )
        })
        .into_any_element()
}

fn attachment_overflow(offset: f32, max_offset: f32) -> (bool, bool) {
    (-offset > 1., max_offset + offset > 1.)
}

#[cfg(test)]
mod attachment_scroll_tests {
    #[test]
    fn fades_only_show_where_more_attachments_exist() {
        use super::attachment_overflow;
        assert_eq!(attachment_overflow(0., 0.), (false, false));
        assert_eq!(attachment_overflow(0., 300.), (false, true));
        assert_eq!(attachment_overflow(-150., 300.), (true, true));
        assert_eq!(attachment_overflow(-300., 300.), (true, false));
    }
}
