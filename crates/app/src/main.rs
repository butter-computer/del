//! Minimal GPUI window: a title, a greeting, and a clickable counter button.
//!
//! All state lives in `del_core::Counter`; this crate only renders it.

use del_core::Counter;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::*;

struct CounterView {
    counter: Counter,
}

impl Render for CounterView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(div().text_2xl().font_weight(FontWeight::BOLD).child("del"))
            .child(self.counter.greeting())
            .child(
                Button::new("increment")
                    .primary()
                    .label(format!("Count: {}", self.counter.count()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.counter.increment();
                        cx.notify();
                    })),
            )
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        // Must run before any gpui-component feature is used.
        gpui_kit::init(cx);

        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("del".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(480.0), px(320.0)),
                cx,
            ))),
            ..Default::default()
        };

        gpui_kit::open_window(options, cx, |_, cx| {
            cx.new(|_| CounterView {
                counter: Counter::new(),
            })
        })
        .expect("failed to open window");

        cx.activate(true);
    });
}
