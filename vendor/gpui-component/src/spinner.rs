use crate::{Icon, IconName, Sizable, Size};
use gpui::{
    Animation, AnimationExt as _, App, Hsla, IntoElement, ParentElement, Pixels, RenderOnce,
    Styled as _, Transformation, Window, div, percentage, prelude::FluentBuilder as _, px, rems,
};
use instant::Duration;

/// One full turn. Every spinner shares this period and a clock, so two
/// spinners for the same work (a sidebar row and its top tab) stay in phase.
pub const SPINNER_PERIOD: Duration = Duration::from_millis(800);

/// A cycling loading spinner.
///
/// It turns at a constant rate, phase-locked to the app-wide animation clock.
/// Under Reduce Motion it shows a small static dot rather than a ring frozen
/// mid-turn, which would read as stalled.
#[derive(IntoElement)]
pub struct Spinner {
    size: Size,
    icon: Icon,
    speed: Duration,
    easing: Box<dyn Fn(f32) -> f32>,
    color: Option<Hsla>,
}

impl Spinner {
    /// Create a new loading spinner.
    pub fn new() -> Self {
        Self {
            size: Size::Medium,
            speed: SPINNER_PERIOD,
            easing: Box::new(|t| t),
            icon: Icon::new(IconName::Loader),
            color: None,
        }
    }

    /// Set specified icon for the spinner.
    ///
    /// Default is [`IconName::Loader`].
    ///
    /// Please ensure the icon used is suitable for a loading spinner.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = icon.into();
        self
    }

    /// Set the icon color.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Set the easing function. The default is linear, for constant rotation.
    pub fn ease(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.easing = Box::new(easing);
        self
    }

    /// Set the duration of one turn. The default is [`SPINNER_PERIOD`].
    pub fn period(mut self, period: Duration) -> Self {
        self.speed = period;
        self
    }

    fn side(&self, window: &Window) -> Pixels {
        match self.size {
            Size::Size(side) => side,
            Size::XSmall => rems(0.75).to_pixels(window.rem_size()),
            Size::Small => rems(0.875).to_pixels(window.rem_size()),
            Size::Medium => rems(1.).to_pixels(window.rem_size()),
            Size::Large => rems(1.5).to_pixels(window.rem_size()),
        }
    }
}

impl Sizable for Spinner {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl RenderOnce for Spinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if cx.reduce_motion() {
            let side = self.side(window);
            // The ring would take the inherited text colour, so the dot does too.
            let color = self.color.unwrap_or(window.text_style().color);
            return div()
                .flex_none()
                .size(side)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size((side * 0.36).max(px(4.)))
                        .rounded_full()
                        .bg(color),
                )
                .into_any_element();
        }
        div()
            .child(
                self.icon
                    .with_size(self.size)
                    .when_some(self.color, |this, color| this.text_color(color))
                    .with_animation(
                        "circle",
                        Animation::new(self.speed)
                            .repeat_synced()
                            .with_easing(self.easing),
                        |this, delta| this.transform(Transformation::rotate(percentage(delta))),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Render, TestAppContext, px, size};

    struct SpinnerHost;

    impl Render for SpinnerHost {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            Spinner::new()
        }
    }

    #[gpui::test]
    fn reduced_motion_spinner_is_static_and_requests_no_frame(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.open_window(size(px(100.), px(100.)), |_, _| SpinnerHost);
        cx.run_until_parked();

        assert_eq!(
            window
                .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap(),
            0
        );
    }
}
