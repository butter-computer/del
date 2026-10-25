//! On-demand process-tree RSS for loaded sessions.
use crate::{
    state::{AppState, Tab},
    zoom::px as zpx,
};
use del_agent::SessionId;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::{component::*, prelude::*, *};
use std::collections::{HashMap, HashSet};

struct Entry {
    id: SessionId,
    terminal: bool,
    bytes: Option<u64>,
}
pub struct Usage {
    state: Entity<AppState>,
    pub open: bool,
    pub terminals: Vec<(SessionId, u32)>,
    entries: Vec<Entry>,
    unloaded: HashSet<SessionId>,
    loading: bool,
    error: Option<String>,
    generation: u64,
    scroll: ScrollHandle,
}
impl Usage {
    pub fn new(state: Entity<AppState>) -> Self {
        Self {
            state,
            open: false,
            terminals: vec![],
            entries: vec![],
            unloaded: HashSet::new(),
            loading: false,
            error: None,
            generation: 0,
            scroll: ScrollHandle::new(),
        }
    }
    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        self.generation += 1;
        if open {
            self.unloaded.clear();
            self.refresh(cx);
        }
        cx.notify();
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let backend = self.state.read(cx).backend();
        let ids: Vec<_> = backend
            .manager
            .list_sessions()
            .into_iter()
            .filter(|s| backend.manager.is_live(&s.id))
            .map(|s| s.id)
            .collect();
        let terminals = self.terminals.clone();
        let generation = self.generation;
        self.loading = self.entries.is_empty();
        let job = backend.runtime.spawn(async move {
            let mut roots = Vec::new();
            for id in ids {
                let process = backend.manager.session_process(&id).await;
                roots.push((id, false, process));
            }
            roots.extend(
                terminals
                    .into_iter()
                    .map(|(id, pid)| (id, true, Some((pid, false)))),
            );
            tokio::task::spawn_blocking(move || {
                let output = std::process::Command::new("/bin/ps")
                    .args(["-axo", "pid=,ppid=,rss="])
                    .output()
                    .map_err(|e| e.to_string())?;
                if !output.status.success() {
                    return Err("Could not read process memory.".to_owned());
                }
                let processes = parse_processes(&String::from_utf8_lossy(&output.stdout));
                let mut entries: Vec<_> = roots
                    .into_iter()
                    .map(|(id, terminal, process)| Entry {
                        id,
                        terminal,
                        bytes: process.and_then(|(pid, _)| tree_bytes(pid, &processes)),
                    })
                    .collect();
                entries.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
                Ok(entries)
            })
            .await
            .map_err(|e| e.to_string())?
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation || !this.open {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(Ok(entries)) => {
                        this.entries = entries
                            .into_iter()
                            .filter(|entry| !this.unloaded.contains(&entry.id))
                            .collect();
                        this.error = None;
                    }
                    _ => this.error = Some("Could not read session memory.".into()),
                }
                cx.notify();
            });
            cx.background_executor()
                .timer(std::time::Duration::from_secs(3))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.open && this.generation == generation {
                    this.refresh(cx);
                }
            });
        })
        .detach();
    }
}
fn parse_processes(text: &str) -> HashMap<u32, (u32, u64)> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            Some((
                words.next()?.parse().ok()?,
                (
                    words.next()?.parse().ok()?,
                    words.next()?.parse::<u64>().ok()? * 1024,
                ),
            ))
        })
        .collect()
}
fn tree_bytes(root: u32, processes: &HashMap<u32, (u32, u64)>) -> Option<u64> {
    processes.get(&root)?;
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    let mut bytes = 0;
    while let Some(pid) = pending.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if let Some((_, rss)) = processes.get(&pid) {
            bytes += rss;
        }
        pending.extend(
            processes
                .iter()
                .filter(|(_, (parent, _))| *parent == pid)
                .map(|(pid, _)| *pid),
        );
    }
    Some(bytes)
}
impl Render for Usage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scroll = self.scroll.clone();
        let background: Rgba = cx.theme().popover.into();
        v_flex()
            .w(zpx(240.))
            .whitespace_normal()
            .gap_2()
            .child(div().text_size(zpx(14.)).child("Session memory"))
            .child(
                div()
                    .relative()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .id("memory-list")
                            .max_h(zpx(144.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .when(self.loading, |list| list.child("Reading memory…"))
                            .when(!self.loading && self.entries.is_empty(), |list| {
                                list.child("No loaded sessions")
                            })
                            .children(self.entries.iter().map(|entry| {
                                let id = entry.id.clone();
                                let unload_id = id.clone();
                                let delete_id = id.clone();
                                let terminal = entry.terminal;
                                let title = self.state.read(cx).current_title(&id);
                                let memory = entry
                                    .bytes
                                    .map(|n| format!("{:.0} MB", n as f64 / 1_048_576.))
                                    .unwrap_or_else(|| "—".into());
                                h_flex()
                                    .id(SharedString::from(format!("memory-row-{}", id.0)))
                                    .group("memory-row")
                                    .h(zpx(36.))
                                    .flex_none()
                                    .gap_2()
                                    .rounded_md()
                                    .hover(|row| row.bg(crate::appearance::rgba(0xffffff0a)))
                                    .child(
                                        h_flex()
                                            .id(SharedString::from(format!(
                                                "memory-session-{}",
                                                id.0
                                            )))
                                            .role(Role::Button)
                                            .cursor_pointer()
                                            .pl_2()
                                            .h(zpx(36.))
                                            .flex_1()
                                            .min_w_0()
                                            .aria_label(format!("Open {title}"))
                                            .child(crate::sidebar::fading_title(title))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.state.update(cx, |s, cx| {
                                                    s.select_tab(
                                                        &if terminal {
                                                            Tab::Terminal(id.clone())
                                                        } else {
                                                            Tab::Session(id.clone())
                                                        },
                                                        cx,
                                                    )
                                                });
                                                this.set_open(false, cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_size(zpx(12.))
                                            .text_color(crate::appearance::rgba(0xffffff99))
                                            .child(memory),
                                    )
                                    .child(
                                        h_flex()
                                            .flex_none()
                                            .gap(zpx(4.))
                                            .pr(zpx(4.))
                                            .opacity(0.)
                                            .group_hover("memory-row", |actions| {
                                                actions.opacity(1.)
                                            })
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "memory-delete-{}",
                                                    delete_id.0
                                                )))
                                                .ghost()
                                                .size(crate::icons::button_size())
                                                .flex_none()
                                                .p_0()
                                                .rounded(zpx(6.))
                                                .tooltip("Permanently delete")
                                                .accessibility_label("Permanently delete session")
                                                .child(
                                                    crate::icons::icon(
                                                        gpui_kit::assets::IconName::Trash,
                                                    )
                                                    .text_color(crate::appearance::rgb(0xff646b)),
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.state.update(cx, |s, cx| {
                                                        s.permanently_delete(delete_id.clone(), cx)
                                                    });
                                                    this.unloaded.insert(delete_id.clone());
                                                    this.entries
                                                        .retain(|entry| entry.id != delete_id);
                                                    cx.notify();
                                                })),
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "memory-unload-{}",
                                                    unload_id.0
                                                )))
                                                .ghost()
                                                .size(crate::icons::button_size())
                                                .flex_none()
                                                .p_0()
                                                .rounded(zpx(6.))
                                                .tooltip("Unload session; keep history")
                                                .accessibility_label("Unload session")
                                                .child(crate::icons::icon(
                                                    gpui_kit::assets::IconName::Close,
                                                ))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.state.update(cx, |s, cx| {
                                                        s.unload(unload_id.clone(), cx)
                                                    });
                                                    this.unloaded.insert(unload_id.clone());
                                                    this.entries
                                                        .retain(|entry| entry.id != unload_id);
                                                    cx.notify();
                                                })),
                                            ),
                                    )
                            })),
                    )
                    .child(
                        canvas(
                            |bounds, _, _| bounds,
                            move |_, bounds, window, _| {
                                if scroll.max_offset().y + scroll.offset().y > zpx(1.) {
                                    let mut transparent = background;
                                    transparent.a = 0.;
                                    window.paint_quad(fill(
                                        bounds,
                                        linear_gradient(
                                            0.,
                                            linear_color_stop(background, 0.),
                                            linear_color_stop(transparent, 1.),
                                        ),
                                    ));
                                }
                            },
                        )
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(zpx(18.)),
                    ),
            )
            .when_some(self.error.clone(), |view, error| view.child(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    #[test]
    fn sums_only_root_and_descendants() {
        let rows = parse_processes("10 1 100\n11 10 30\n12 11 20\n20 1 900\ninvalid");
        assert_eq!(tree_bytes(10, &rows), Some(150 * 1024));
        assert_eq!(tree_bytes(99, &rows), None);
    }
}
