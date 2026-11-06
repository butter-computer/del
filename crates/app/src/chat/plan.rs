//! Shared transcript checklist for main and Side chats.
use crate::{appearance, zoom::px as zpx};
use del_agent::{Plan, PlanStatus};
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::{component::*, prelude::*, *};

pub fn card(index: usize, plan: &Plan, running: bool, reduced: bool) -> AnyElement {
    if plan.steps.is_empty() {
        return div().h_0().into_any_element();
    }
    let complete = plan
        .steps
        .iter()
        .filter(|step| step.status == PlanStatus::Completed)
        .count();
    v_flex()
        .id(("plan-checklist", index))
        .debug_selector(|| "plan-checklist".into())
        .role(Role::List)
        .aria_label(format!("Plan: {complete} of {} complete", plan.steps.len()))
        .w_full()
        .min_w_0()
        .p(zpx(14.))
        .gap(zpx(10.))
        .rounded(zpx(crate::ui::RADIUS_CARD))
        .border_1()
        .border_color(appearance::separator())
        .child(
            h_flex()
                .justify_between()
                .gap(zpx(crate::ui::SPACE_MD))
                .text_size(zpx(TEXT_SM))
                .text_color(appearance::secondary_text())
                .child("Plan")
                .child(format!("{complete} of {} complete", plan.steps.len())),
        )
        .when_some(
            plan.explanation.clone().filter(|text| !text.is_empty()),
            |card, text| {
                card.child(
                    div()
                        .text_size(zpx(TEXT_SM))
                        .text_color(appearance::secondary_text())
                        .child(text),
                )
            },
        )
        .children(plan.steps.iter().map(|step| {
            let active = step.status == PlanStatus::InProgress;
            let color = if step.status == PlanStatus::Completed {
                appearance::ActionTone::Success.foreground()
            } else if active {
                appearance::rgb(0xe8e8e8)
            } else {
                appearance::secondary_text()
            };
            let marker = div()
                .relative()
                .size(zpx(16.))
                .child(
                    div()
                        .opacity(
                            if active && running && !reduced || step.status == PlanStatus::Cancelled
                            {
                                0.
                            } else {
                                1.
                            },
                        )
                        .child(
                            crate::micro_motion::MorphIcon::new(
                                SharedString::from(format!("plan-check-{index}-{}", step.id)),
                                crate::animated_icons::Morph::PlanCheck,
                                step.status == PlanStatus::Completed,
                            )
                            .text_color(color),
                        ),
                )
                .when(active && running && !reduced, |marker| {
                    marker.child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(crate::icons::working_spinner()),
                    )
                })
                .when(active && (!running || reduced), |marker| {
                    marker.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(zpx(4.)).rounded_full().bg(color)),
                    )
                })
                .when(step.status == PlanStatus::Cancelled, |marker| {
                    marker.child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(crate::icons::path("icons/dash.svg").text_color(color)),
                    )
                });
            let label = if active && running {
                step.active_text.as_ref().unwrap_or(&step.text)
            } else {
                &step.text
            };
            let status = match step.status {
                PlanStatus::Pending => "Pending",
                PlanStatus::InProgress => "In progress",
                PlanStatus::Completed => "Completed",
                PlanStatus::Cancelled => "Cancelled",
            };
            let row = h_flex()
                .id(SharedString::from(format!("plan-step-{}", step.id)))
                .w_full()
                .min_w_0()
                .items_start()
                .gap(zpx(10.))
                .role(Role::ListItem)
                .debug_selector({
                    let id = format!("plan-step-{}", step.id);
                    move || id.clone()
                })
                .aria_label(format!("{status}: {}", step.text))
                .child(
                    div()
                        .w(zpx(16.))
                        .h(zpx(22.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(marker),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .text_size(zpx(TEXT_MD))
                        .line_height(zpx(22.))
                        .text_color(
                            if matches!(step.status, PlanStatus::Pending | PlanStatus::InProgress) {
                                appearance::rgb(0xe8e8e8)
                            } else {
                                appearance::secondary_text()
                            },
                        )
                        .child(label.clone()),
                );
            if running && !reduced {
                row.with_animation(
                    SharedString::from(format!("plan-change-{index}-{}-{status}-{label}", step.id)),
                    Animation::new(std::time::Duration::from_millis(180))
                        .with_easing(ease_out_quint()),
                    |row, t| row.opacity(0.45 + 0.55 * t),
                )
                .into_any_element()
            } else {
                row.into_any_element()
            }
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    struct Preview {
        plan: Plan,
        width: f32,
    }
    impl Render for Preview {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(self.width))
                .child(card(0, &self.plan, true, cx.reduce_motion()))
        }
    }
    #[::gpui::test]
    fn checklist_wraps_in_side_width_and_clears_without_an_empty_card(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let plan = Plan {
            explanation: None,
            steps: vec![del_agent::PlanStep {
                id: "long".into(),
                text: "Map backend data models and storage, including persistence and restoration after reopening a conversation".into(),
                active_text: None,
                status: PlanStatus::InProgress,
            }],
        };
        let (view, cx) = cx.add_window_view(|_, _| Preview { plan, width: 320. });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let narrow = cx.debug_bounds("plan-step-long").unwrap();
        assert!(narrow.size.height > px(44.));
        assert!(narrow.size.width <= px(320.));
        view.update(cx, |view, cx| {
            view.width = 800.;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let wide = cx.debug_bounds("plan-step-long").unwrap();
        assert!(wide.size.height < narrow.size.height);
        view.update(cx, |view, cx| {
            view.plan.steps.clear();
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("plan-checklist").is_none());
    }
}
