use crate::zoom::px as zpx;
use crate::{
    AppShell,
    appearance::{rgb, rgba},
    composer_ui::QUEUE_SURFACE_RGBA,
    state::{Selection, Tab},
};
use del_agent::{Attachment, UserInput};
use gpui_kit::assets::IconName as ActionIcon;
use gpui_kit::component::{
    button::{Button, ButtonCustomVariant, ButtonVariants},
    *,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

#[derive(Clone)]
struct QueueDrag {
    owner: u64,
    id: String,
}
struct QueueDragPreview;
impl Render for QueueDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(zpx(1.))
    }
}
fn thumbnail(attachment: &Attachment) -> impl IntoElement {
    div()
        .size(zpx(24.))
        .flex_none()
        .rounded_sm()
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
                Icon::new(IconName::File)
                    .with_size(zpx(15.))
                    .text_color(rgba(0xffffff99)),
            )
        })
}
fn action(id: SharedString, icon: ActionIcon, label: &'static str) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .compact()
        .icon(icon)
        .tooltip(label)
        .accessibility_label(label)
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
            self.composer_drafts.insert(
                self.composer_owner,
                self.composer.read(cx).value().to_string(),
            );
            let text = self.composer_drafts.remove(&owner).unwrap_or_default();
            self.composer
                .update(cx, |input, cx| input.set_value(text, window, cx));
            self.composer_owner = owner;
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
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((files, errors)) => {
                        this.composer_attachments
                            .entry(owner)
                            .or_default()
                            .extend(files);
                        this.state.update(cx, |s, cx| {
                            s.error = (!errors.is_empty()).then(|| errors.join("\n"));
                            cx.notify();
                        });
                    }
                    Err(error) => this.state.update(cx, |s, cx| {
                        s.error = Some(error);
                        cx.notify();
                    }),
                }
                cx.notify();
            });
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
        let current = UserInput {
            text: self.composer.read(cx).value().to_string(),
            attachments: self.composer_attachments.remove(&owner).unwrap_or_default(),
        };
        if !current.text.trim().is_empty() || !current.attachments.is_empty() {
            chat.update(cx, |chat, cx| {
                chat.enqueue(current, cx);
            });
        }
        self.composer_attachments.insert(owner, input.attachments);
        self.composer.update(cx, |composer, cx| {
            composer.set_value(input.text, window, cx);
            composer.focus(window, cx);
        });
        cx.notify();
    }
    pub(super) fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.composer_owner(cx);
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
            .p_1()
            .gap_2()
            .children(
                self.composer_attachments
                    .get(&owner)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, file)| {
                        let group = SharedString::from(format!("attachment-{index}"));
                        let is_image = crate::attachments::is_image(file);
                        let path = file.path.clone();
                        h_flex()
                            .id(("composer-attachment", index))
                            .role(gpui::Role::Button)
                            .aria_label(format!("Preview {}", crate::attachments::filename(file)))
                            .group(group.clone())
                            .relative()
                            .flex_none()
                            .h(zpx(56.))
                            .w(zpx(if is_image { 56. } else { 148. }))
                            .gap_1p5()
                            .items_center()
                            .rounded_lg()
                            .cursor_pointer()
                            .when(!is_image, |row| {
                                row.p_1p5()
                                    .bg(rgba(0xffffff08))
                                    .border_1()
                                    .border_color(rgba(0xffffff12))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_right(true, cx);
                                this.right
                                    .update(cx, |panel, cx| panel.preview_file(path.clone(), cx));
                            }))
                            .when(is_image, |row| {
                                row.child(
                                    div().size(zpx(56.)).rounded_lg().overflow_hidden().child(
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
                                        .child(
                                            if let Some(asset) = crate::attachments::file_mark(file)
                                            {
                                                img(asset)
                                                    .size(zpx(28.))
                                                    .object_fit(ObjectFit::Contain)
                                                    .into_any_element()
                                            } else {
                                                Icon::new(IconName::File)
                                                    .with_size(zpx(28.))
                                                    .text_color(rgba(0xffffff70))
                                                    .into_any_element()
                                            },
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_1()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(zpx(12.))
                                                .text_color(rgba(0xffffffdd))
                                                .child(crate::attachments::compact_filename(file)),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(zpx(11.))
                                                .text_color(rgba(0xffffff70))
                                                .child(crate::attachments::file_type(file)),
                                        ),
                                )
                            })
                            .tooltip({
                                let name = crate::attachments::filename(file);
                                move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(name.clone())
                                        .build(window, cx)
                                }
                            })
                            .child(
                                Button::new(SharedString::from(format!(
                                    "remove-attachment-{index}"
                                )))
                                .custom(
                                    ButtonCustomVariant::new(cx)
                                        .color(rgb(0xf5f5f5).into())
                                        .foreground(rgb(0x202020).into())
                                        .hover(rgb(0xffffff).into())
                                        .active(rgb(0xe5e5e5).into()),
                                )
                                .child(
                                    Icon::new(ActionIcon::Close)
                                        .with_size(zpx(12.))
                                        .text_color(rgb(0x202020)),
                                )
                                .accessibility_label("Remove attachment")
                                .tooltip("Remove attachment")
                                .absolute()
                                .right(zpx(2.))
                                .top(zpx(2.))
                                .size(zpx(20.))
                                .p_0()
                                .rounded_full()
                                .opacity(0.0)
                                .group_hover(group, |button| button.opacity(1.0))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if let Some(files) =
                                            this.composer_attachments.get_mut(&owner)
                                            && index < files.len()
                                        {
                                            files.remove(index);
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                    }),
            );
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
            }))
    }
    pub(super) fn render_followups(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(chat) = self.open_chat(cx) else {
            return div().into_any_element();
        };
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
        let owner = self.composer_owner(cx);
        let overflow = items.len() > 4;
        let count = items.len();
        let height = items.iter().take(4).map(|_| 32.0).sum::<f32>() + 8.0;
        div()
            .relative()
            .w_full()
            .px_3()
            .child(
                v_flex()
                    .id("followup-queue")
                    .h(zpx(height))
                    .overflow_y_scroll()
                    .rounded_t(zpx(14.))
                    .bg(rgba(QUEUE_SURFACE_RGBA))
                    .py_1()
                    .children(items.into_iter().enumerate().map(|(index, item)| {
                        let group: SharedString = format!("followup-{}", item.id).into();
                        let hover_id = item.id.clone();
                        let edit = item.id.clone();
                        let delete = item.id.clone();
                        let send = item.id.clone();
                        let target = item.id.clone();
                        let delete_chat = chat.clone();
                        let send_chat = chat.clone();
                        let reorder_chat = chat.clone();
                        h_flex()
                            .id(group.clone())
                            .group(group.clone())
                            .h(zpx(32.))
                            .flex_none()
                            .px_2()
                            .gap_2()
                            .items_center()
                            .when(index + 1 < count, |row| {
                                row.border_b_1().border_color(rgba(0xffffff0c))
                            })
                            .on_hover(cx.listener(move |this, entered, _, cx| {
                                if *entered {
                                    this.queue_hovered = Some(hover_id.clone());
                                } else if this.queue_hovered.as_ref() == Some(&hover_id) {
                                    this.queue_hovered = None;
                                }
                                cx.notify();
                            }))
                            .on_drop(move |drag: &QueueDrag, _, cx| {
                                if drag.owner == owner {
                                    reorder_chat.update(cx, |chat, cx| {
                                        chat.reorder_queued(&drag.id, &target, cx)
                                    });
                                }
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("drag-{}", item.id)))
                                    .cursor_grab()
                                    .w(zpx(12.))
                                    .h(zpx(24.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(rgba(0xffffff8c))
                                    .opacity(0.0)
                                    .on_drag(
                                        QueueDrag {
                                            owner,
                                            id: item.id.clone(),
                                        },
                                        |_, _, _, cx| cx.new(|_| QueueDragPreview),
                                    )
                                    .child(
                                        svg()
                                            .path("icons/grip.svg")
                                            .w(zpx(9.))
                                            .h(zpx(15.))
                                            .text_color(rgba(0xffffff8c)),
                                    ),
                            )
                            .child(
                                svg()
                                    .path("icons/queue.svg")
                                    .size(zpx(14.))
                                    .text_color(rgba(0xffffff70)),
                            )
                            .children(item.input.attachments.iter().take(3).map(thumbnail))
                            .child(v_flex().flex_1().min_w_0().child(
                                div().truncate().text_color(rgba(0xffffffcc)).child(
                                    if item.input.text.is_empty() {
                                        item.input
                                            .attachments
                                            .iter()
                                            .map(crate::attachments::filename)
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    } else {
                                        item.input.text.clone()
                                    },
                                ),
                            ))
                            .child(
                                Button::new(SharedString::from(format!("steer-{}", item.id)))
                                    .ghost()
                                    .small()
                                    .compact()
                                    .icon(Icon::default().path("icons/steer.svg"))
                                    .label("Steer")
                                    .on_click(move |_, _, cx| {
                                        send_chat.update(cx, |chat, cx| {
                                            chat.deliver_queued(send.clone(), chat.is_running(), cx)
                                        })
                                    }),
                            )
                            .child(
                                action(
                                    format!("delete-{}", item.id).into(),
                                    ActionIcon::Trash,
                                    "Delete queued message",
                                )
                                .on_click(move |_, _, cx| {
                                    delete_chat.update(cx, |chat, cx| {
                                        chat.take_queued(&delete, cx);
                                    });
                                }),
                            )
                            .child(
                                action(
                                    format!("edit-{}", item.id).into(),
                                    ActionIcon::Pencil,
                                    "Edit queued message",
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| this.edit_queued(&edit, window, cx),
                                )),
                            )
                    })),
            )
            .when(overflow, |queue| {
                queue.child(
                    div()
                        .absolute()
                        .left(zpx(12.))
                        .right(zpx(12.))
                        .bottom_0()
                        .h(zpx(12.))
                        .bg(linear_gradient(
                            0.,
                            linear_color_stop(rgba(QUEUE_SURFACE_RGBA), 0.),
                            linear_color_stop(rgba(QUEUE_SURFACE_RGBA & 0xffffff00), 1.),
                        )),
                )
            })
            .into_any_element()
    }
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
