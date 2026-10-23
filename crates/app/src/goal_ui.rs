//! Goal controls share a provider-neutral contract; continuation stays in the harness.
use crate::{AppShell, appearance::rgba, state::Selection, zoom::px as zpx};
use del_agent::{ComposerCommand, GoalAction, GoalStatus};
use gpui_kit::assets::IconName as GoalIcon;
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        *,
    },
    prelude::*,
    *,
};

pub(crate) const GOAL_INSET: f32 = 24.;

impl AppShell {
    pub(super) fn begin_goal(
        &mut self,
        command: ComposerCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_modes.insert(self.composer_owner(cx), command);
        self.sync_goal_placeholder(window, cx);
        cx.notify();
    }

    pub(super) fn cancel_goal_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.goal_modes.remove(&self.composer_owner(cx));
        self.sync_goal_placeholder(window, cx);
        cx.notify();
    }

    pub(super) fn sync_goal_placeholder(&self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = if self.goal_modes.contains_key(&self.composer_owner(cx)) {
            "Describe your goal, define measurable outcomes for best results"
        } else {
            "Ask anything..."
        };
        self.composer.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx)
        });
    }

    fn goal_action(&mut self, action: GoalAction, cx: &mut Context<Self>) {
        let Selection::Session(id) = self.state.read(cx).selection.clone() else {
            return;
        };
        if !self.goal_controls.insert(id.clone()) {
            return;
        }
        let backend = self.state.read(cx).backend();
        let session = id.clone();
        let task = backend
            .runtime
            .spawn(async move { backend.manager.control_goal(&session, action).await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.goal_controls.remove(&id);
                if let Err(error) = result {
                    this.state.update(cx, |s, cx| {
                        s.error = Some(error);
                        cx.notify();
                    });
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_goal_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(goal) = self.open_chat(cx).and_then(|chat| chat.read(cx).goal()) else {
            return div().into_any_element();
        };
        let owner = self.composer_owner(cx);
        let expanded = self.goal_expanded.contains(&owner);
        let pending = match &self.state.read(cx).selection {
            Selection::Session(id) => self.goal_controls.contains(id),
            _ => false,
        };
        let paused = goal.status != GoalStatus::Active;
        v_flex()
            .mx(zpx(GOAL_INSET))
            .px_2p5()
            .py_1()
            .gap_2()
            .rounded_t_lg()
            .bg(rgba(0xffffff0b))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(GoalIcon::Target)
                            .with_size(zpx(14.))
                            .text_color(rgba(0xffffff88)),
                    )
                    .child(
                        div()
                            .text_size(zpx(12.))
                            .flex_none()
                            .child(goal.status.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(zpx(12.))
                            .text_color(rgba(0xffffff99))
                            .truncate()
                            .child(goal.objective.clone()),
                    )
                    .when(goal.time_used_seconds > 0, |row| {
                        row.child(
                            div()
                                .text_size(zpx(11.))
                                .text_color(rgba(0xffffff77))
                                .child(format!("{}s", goal.time_used_seconds)),
                        )
                    })
                    .child(
                        Button::new("clear-goal")
                            .ghost()
                            .small()
                            .compact()
                            .disabled(pending)
                            .icon(GoalIcon::Trash)
                            .tooltip("Clear goal")
                            .accessibility_label("Clear goal")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.goal_action(GoalAction::Clear, cx)
                            })),
                    )
                    .when(
                        goal.can_pause && goal.status != GoalStatus::Complete,
                        |row| {
                            row.child(
                                Button::new("pause-goal")
                                    .ghost()
                                    .small()
                                    .compact()
                                    .disabled(pending)
                                    .icon(if paused {
                                        GoalIcon::Play
                                    } else {
                                        GoalIcon::Pause
                                    })
                                    .tooltip(if paused { "Resume goal" } else { "Pause goal" })
                                    .accessibility_label(if paused {
                                        "Resume goal"
                                    } else {
                                        "Pause goal"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.goal_action(
                                            if paused {
                                                GoalAction::Resume
                                            } else {
                                                GoalAction::Pause
                                            },
                                            cx,
                                        )
                                    })),
                            )
                        },
                    )
                    .child(
                        Button::new("expand-goal")
                            .ghost()
                            .small()
                            .compact()
                            .icon(if expanded {
                                GoalIcon::ChevronUp
                            } else {
                                GoalIcon::ChevronDown
                            })
                            .tooltip("Goal details")
                            .accessibility_label("Goal details")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.goal_expanded.remove(&owner) {
                                    this.goal_expanded.insert(owner);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .when(expanded, |column| {
                column.child(
                    v_flex()
                        .pb_2()
                        .gap_1()
                        .child(div().text_size(zpx(13.)).child(goal.objective))
                        .when(goal.tokens_used > 0, |column| {
                            column.child(
                                div()
                                    .text_size(zpx(11.))
                                    .text_color(rgba(0xffffff88))
                                    .child(format!("{} tokens used", goal.tokens_used)),
                            )
                        }),
                )
            })
            .into_any_element()
    }
}
