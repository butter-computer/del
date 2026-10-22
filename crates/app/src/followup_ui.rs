use crate::{
    AppShell,
    appearance::{rgb, rgba},
    composer_ui::QUEUE_SURFACE_RGBA,
    state::{Selection, Tab},
};
use del_agent::{Attachment, UserInput};
use gpui_kit::assets::IconName as ActionIcon;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
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
        div().size(px(1.))
    }
}
fn thumbnail(attachment: &Attachment) -> impl IntoElement {
    div()
        .size(px(24.))
        .flex_none()
        .rounded_sm()
        .overflow_hidden()
        .border_1()
        .border_color(rgba(0xffffff20))
        .child(
            img(attachment.path.clone())
                .size_full()
                .object_fit(ObjectFit::Cover),
        )
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
    pub(super) fn pick_images(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach images".into()),
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
        let work = self
            .state
            .read(cx)
            .backend()
            .runtime
            .spawn_blocking(move || {
                let directory = del_agent::data_dir()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("attachments");
                std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
                paths
                    .into_iter()
                    .map(|path| {
                        let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                        if metadata.len() > 20 * 1024 * 1024 {
                            return Err("Images must be smaller than 20 MB.".into());
                        }
                        let image = image::ImageReader::open(&path)
                            .map_err(|e| e.to_string())?
                            .with_guessed_format()
                            .map_err(|e| e.to_string())?
                            .decode()
                            .map_err(|e| e.to_string())?;
                        let saved = directory.join(format!("{}.png", uuid::Uuid::new_v4()));
                        image
                            .save_with_format(&saved, image::ImageFormat::Png)
                            .map_err(|e| e.to_string())?;
                        Ok(Attachment {
                            path: saved,
                            mime: Some("image/png".into()),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()
            });
        cx.spawn(async move |this, cx| {
            let result = work
                .await
                .map_err(|e| e.to_string())
                .and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(images) => this
                        .composer_images
                        .entry(owner)
                        .or_default()
                        .extend(images),
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
    pub(super) fn paste_images(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return false;
        };
        let owner = self.composer_owner(cx);
        let mut paths = Vec::new();
        for entry in clipboard.entries {
            match entry {
                ClipboardEntry::ExternalPaths(files) => paths.extend(files.0),
                ClipboardEntry::Image(image) => {
                    let directory = del_agent::data_dir()
                        .unwrap_or_else(std::env::temp_dir)
                        .join("attachments");
                    if std::fs::create_dir_all(&directory).is_err() {
                        continue;
                    }
                    let path = directory.join(format!(
                        "clipboard-{}.{}",
                        uuid::Uuid::new_v4(),
                        image.format().extension()
                    ));
                    if std::fs::write(&path, image.bytes()).is_ok() {
                        paths.push(path);
                    }
                }
                _ => {}
            }
        }
        if paths.is_empty() {
            false
        } else {
            self.attach_paths(owner, paths, cx);
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
            attachments: self.composer_images.remove(&owner).unwrap_or_default(),
        };
        if !current.text.trim().is_empty() || !current.attachments.is_empty() {
            chat.update(cx, |chat, cx| {
                chat.enqueue(current, cx);
            });
        }
        self.composer_images.insert(owner, input.attachments);
        self.composer.update(cx, |composer, cx| {
            composer.set_value(input.text, window, cx);
            composer.focus(window, cx);
        });
        cx.notify();
    }
    pub(super) fn render_images(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.composer_owner(cx);
        h_flex().gap_2().children(
            self.composer_images
                .get(&owner)
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, image)| {
                    h_flex()
                        .gap_1()
                        .items_center()
                        .py_1()
                        .child(thumbnail(image))
                        .child(
                            action(
                                format!("remove-image-{index}").into(),
                                ActionIcon::X,
                                "Remove image",
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(images) = this.composer_images.get_mut(&owner)
                                        && index < images.len()
                                    {
                                        images.remove(index);
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                }),
        )
    }
    pub(super) fn render_followups(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(chat) = self.open_chat(cx) else {
            return div().into_any_element();
        };
        let items = chat.read(cx).queue.items.clone();
        if items.is_empty() {
            return div().into_any_element();
        }
        let owner = self.composer_owner(cx);
        let overflow = items.len() > 4;
        let count = items.len();
        let height = items
            .iter()
            .take(4)
            .map(|item| if item.error.is_some() { 44.0 } else { 32.0 })
            .sum::<f32>()
            + 8.0;
        div()
            .relative()
            .w_full()
            .px_3()
            .child(
                v_flex()
                    .id("followup-queue")
                    .h(px(height))
                    .overflow_y_scroll()
                    .rounded_t(px(14.))
                    .bg(rgba(QUEUE_SURFACE_RGBA))
                    .py_1()
                    .children(items.into_iter().enumerate().map(|(index, item)| {
                        let group: SharedString = format!("followup-{}", item.id).into();
                        let hovered = self.queue_hovered.as_deref() == Some(item.id.as_str());
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
                            .h(px(if item.error.is_some() { 44. } else { 32. }))
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
                                    .w(px(12.))
                                    .h(px(24.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(rgba(0xffffff8c))
                                    .opacity(if hovered { 1. } else { 0. })
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
                                            .w(px(9.))
                                            .h(px(15.))
                                            .text_color(rgba(0xffffff8c)),
                                    ),
                            )
                            .child(
                                svg()
                                    .path("icons/queue.svg")
                                    .size(px(14.))
                                    .text_color(rgba(0xffffff70)),
                            )
                            .children(item.input.attachments.iter().take(3).map(thumbnail))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().truncate().text_color(rgba(0xffffffcc)).child(
                                        if item.input.text.is_empty() {
                                            "Image".to_owned()
                                        } else {
                                            item.input.text.clone()
                                        },
                                    ))
                                    .when_some(item.error.clone(), |row, error| {
                                        row.child(
                                            div()
                                                .truncate()
                                                .text_size(px(10.))
                                                .text_color(rgb(0xff646b))
                                                .child(error),
                                        )
                                    }),
                            )
                            .child(
                                Button::new(SharedString::from(format!("steer-{}", item.id)))
                                    .ghost()
                                    .small()
                                    .compact()
                                    .icon(Icon::default().path("icons/steer.svg"))
                                    .label(if item.sending { "Sending…" } else { "Steer" })
                                    .disabled(item.sending)
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
                                .disabled(item.sending)
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
                                .disabled(item.sending)
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
                        .left(px(12.))
                        .right(px(12.))
                        .bottom_0()
                        .h(px(12.))
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
