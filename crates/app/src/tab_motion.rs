//! Small, interruptible tab-strip transitions. Closed tabs retain only their
//! appearance; their tools and processes are released immediately.
use crate::zoom::px as zpx;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::{Duration, Instant};

pub const ENTER: Duration = Duration::from_millis(220);
pub const EXIT: Duration = Duration::from_millis(160);

#[derive(Clone)]
pub struct Visual {
    pub label: String,
    pub icon: Icon,
    pub active: bool,
}

#[derive(Clone)]
pub struct Slot<K> {
    pub key: K,
    visual: Visual,
    pub closing: bool,
    started: Option<Instant>,
    from: f32,
}
impl<K> Slot<K> {
    fn fraction(&self, now: Instant) -> f32 {
        let Some(start) = self.started else {
            return 1.;
        };
        let duration = if self.closing { EXIT } else { ENTER };
        let t = (now.duration_since(start).as_secs_f32() / duration.as_secs_f32()).min(1.);
        let eased = 1. - (1. - t).powi(3);
        if self.closing {
            self.from * (1. - eased)
        } else {
            eased
        }
    }

    pub fn wrap(&self, id: SharedString, child: impl IntoElement) -> AnyElement {
        let width = zpx(crate::icons::TOOL_TAB_WIDTH);
        let slot = div().flex_none().overflow_hidden().child(child);
        if let Some(started) = self.started {
            let closing = self.closing;
            let from = self.from;
            let duration = if closing { EXIT } else { ENTER };
            slot.with_animation(id, Animation::new(duration), move |slot, _| {
                let t = (started.elapsed().as_secs_f32() / duration.as_secs_f32()).min(1.);
                let eased = 1. - (1. - t).powi(3);
                let value = if closing { from * (1. - eased) } else { eased };
                slot.w(width * value).opacity(value)
            })
            .into_any_element()
        } else {
            slot.w(width).into_any_element()
        }
    }

    pub fn ghost(&self, id: SharedString) -> AnyElement {
        self.wrap(
            id,
            h_flex()
                .w(zpx(crate::icons::TOOL_TAB_WIDTH))
                .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
                .px_2()
                .gap_2()
                .rounded_md()
                .when(self.visual.active, |tab| {
                    tab.bg(crate::appearance::rgba(0xffffff14))
                })
                .text_color(crate::appearance::rgba(0xffffff8c))
                .text_size(zpx(del_core::typography::TEXT_MD))
                .child(crate::icons::icon(self.visual.icon.clone()).with_size(zpx(15.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(self.visual.label.clone()),
                ),
        )
    }
}

pub struct Strip<K> {
    initialized: bool,
    slots: Vec<Slot<K>>,
}
impl<K> Default for Strip<K> {
    fn default() -> Self {
        Self {
            initialized: false,
            slots: Vec::new(),
        }
    }
}
impl<K: Clone + PartialEq> Strip<K> {
    pub fn transitioning(&self) -> bool {
        self.slots.iter().any(|slot| slot.started.is_some())
    }

    pub fn settle(&mut self) {
        self.initialized = false;
    }
    pub fn sync<T: 'static>(
        &mut self,
        live: Vec<(K, Visual)>,
        cx: &mut Context<T>,
    ) -> Vec<Slot<K>> {
        let now = Instant::now();
        let animate = self.initialized && !cx.reduce_motion() && !cx.has_active_drag();
        let mut changed = false;
        let mut next: Vec<_> = live
            .into_iter()
            .map(|(key, visual)| {
                let old = self
                    .slots
                    .iter()
                    .find(|slot| slot.key == key && !slot.closing);
                let started = if animate {
                    old.map_or_else(
                        || {
                            changed = true;
                            Some(now)
                        },
                        |old| {
                            old.started
                                .filter(|start| now.duration_since(*start) < ENTER)
                        },
                    )
                } else {
                    None
                };
                Slot {
                    key,
                    visual,
                    closing: false,
                    started,
                    from: 0.,
                }
            })
            .collect();
        if animate {
            for (index, old) in self.slots.iter().enumerate() {
                if next.iter().any(|slot| slot.key == old.key) {
                    continue;
                }
                let mut ghost = old.clone();
                if !ghost.closing {
                    ghost.from = ghost.fraction(now);
                    ghost.closing = true;
                    ghost.started = Some(now);
                    changed = true;
                }
                if ghost.fraction(now) > 0. {
                    next.insert(index.min(next.len()), ghost);
                }
            }
        }
        self.initialized = true;
        self.slots = next.clone();
        if changed {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(ENTER + Duration::from_millis(16))
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    #[::gpui::test]
    fn initial_layout_and_transfers_settle_and_reduced_motion_removes_ghosts(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        let entity = cx.new(|_| ());
        entity.update(cx, |_, cx| {
            let visual = Visual {
                label: "New tab".into(),
                icon: crate::right_panel::Pane::NewTab.icon(),
                active: true,
            };
            let mut strip = Strip::default();
            assert!(
                strip.sync(vec![(1, visual.clone())], cx)[0]
                    .started
                    .is_none()
            );
            assert!(
                strip.sync(vec![(1, visual.clone()), (2, visual.clone())], cx)[1]
                    .started
                    .is_some()
            );
            strip.settle();
            assert!(
                strip.sync(vec![(2, visual.clone())], cx)[0]
                    .started
                    .is_none()
            );
            cx.set_reduce_motion(true);
            assert!(strip.sync(vec![], cx).is_empty());
            assert!(strip.sync(vec![(3, visual)], cx)[0].started.is_none());
        });
    }

    #[test]
    fn closing_an_opening_tab_starts_at_its_current_width() {
        let now = Instant::now();
        let mut slot = Slot {
            key: 1,
            visual: Visual {
                label: "New tab".into(),
                icon: crate::right_panel::Pane::NewTab.icon(),
                active: true,
            },
            closing: false,
            started: Some(now - Duration::from_millis(80)),
            from: 0.,
        };
        let width = slot.fraction(now);
        assert!(width > 0. && width < 1.);
        slot.from = width;
        slot.closing = true;
        slot.started = Some(now);
        assert_eq!(slot.fraction(now), width);
        assert_eq!(slot.fraction(now + EXIT), 0.);
    }
}
