//! Shared placement and overflow rendering for transfers between tool-tab rows.
use crate::zoom::px as zpx;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Default)]
pub(crate) struct Placement(Rc<RefCell<Destination>>);
#[derive(Default)]
struct Destination {
    target: Option<EntityId>,
    row: Option<Bounds<Pixels>>,
}
impl Placement {
    pub fn visit(&self, target: EntityId, point: Point<Pixels>, bounds: Bounds<Pixels>) -> bool {
        let mut state = self.0.borrow_mut();
        let inside = bounds.contains(&point);
        if inside {
            state.target = Some(target);
            state.row = Some(bounds);
        } else if state.target == Some(target) {
            state.target = None;
            state.row = None;
        }
        inside
    }
    pub fn preview(
        &self,
        natural: Point<Pixels>,
        source_y: Pixels,
        width: Pixels,
    ) -> Bounds<Pixels> {
        let state = self.0.borrow();
        let height = zpx(crate::icons::TOOL_TAB_HEIGHT);
        if let Some(row) = state.row.filter(|_| state.target.is_some()) {
            Bounds::new(
                // The insertion gap moves neighbors, never the held preview.
                point(natural.x, row.center().y - height / 2.),
                size(width, height),
            )
        } else {
            // Pointer movement only changes X; retain the source row outside a destination.
            Bounds::new(point(natural.x, source_y), size(width, height))
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct InsertionGap {
    pub index: Option<usize>,
    pub width: f32,
}
impl InsertionGap {
    pub fn update(&mut self, index: Option<usize>, width: f32) {
        if index.is_some() && self.index != index {
            crate::haptics::tick();
        }
        self.index = index;
        self.width = width;
    }
    pub fn clear(&mut self) {
        self.index = None;
    }
    pub fn element(&self, selector: &'static str) -> AnyElement {
        div()
            .id(selector)
            .debug_selector(move || selector.into())
            .w(zpx(self.width))
            .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
            .flex_none()
            .into_any_element()
    }
}

pub(crate) fn edge_fades(scroll: &ScrollHandle, surface: Rgba) -> Vec<AnyElement> {
    [true, false]
        .into_iter()
        .map(|left| {
            let scroll = scroll.clone();
            canvas(
                |bounds, _, _| bounds,
                move |_, bounds, window, _| {
                    let distance = if left {
                        -scroll.offset().x
                    } else {
                        scroll.max_offset().x + scroll.offset().x
                    };
                    let alpha = (distance / zpx(16.)).clamp(0., 1.);
                    if alpha > 0. {
                        window.paint_quad(fill(
                            bounds,
                            linear_gradient(
                                if left { 90. } else { 270. },
                                linear_color_stop(surface.opacity(alpha), 0.),
                                linear_color_stop(surface.opacity(0.), 1.),
                            ),
                        ));
                    }
                },
            )
            .absolute()
            .top_0()
            .bottom_0()
            .w(zpx(16.))
            .when(left, |fade| fade.left_0())
            .when(!left, |fade| fade.right_0())
            .into_any_element()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn preview_tracks_pointer_while_held_over_destination_slots() {
        let placement = Placement::default();
        let width = zpx(crate::icons::TOOL_TAB_WIDTH);
        let pointer = point(zpx(40.), zpx(90.));
        let source_y = zpx(25.);
        for y in [-100., 0., 250.] {
            let moved = point(zpx(80.), zpx(y));
            assert_eq!(
                placement.preview(moved, source_y, width).origin,
                point(moved.x, source_y)
            );
        }
        assert_eq!(
            placement.preview(pointer, source_y, width).origin,
            point(pointer.x, source_y)
        );
        let viewport = Bounds::new(point(zpx(200.), zpx(20.)), size(zpx(300.), zpx(40.)));
        let target = EntityId::from(1);
        assert!(placement.visit(target, viewport.center(), viewport));
        let preview = placement.preview(pointer, source_y, width);
        assert_eq!(preview.size.width, width);
        assert_eq!(preview.left(), pointer.x);
        for x in [210., 225., 390., 280.] {
            let moved = point(zpx(x), zpx(90.));
            let preview = placement.preview(moved, source_y, width);
            assert_eq!(
                preview.left(),
                moved.x,
                "the held tab must track the pointer inside a destination slot"
            );
            assert_eq!(preview.center().y, viewport.center().y);
        }
        // Changing destinations must not change the horizontal grab offset.
        let other = Bounds::new(point(zpx(0.), zpx(20.)), viewport.size);
        assert!(placement.visit(EntityId::from(2), other.center(), other));
        assert_eq!(
            placement.preview(pointer, source_y, width).left(),
            pointer.x
        );
        assert_eq!(preview.center().y, viewport.center().y);
        assert!(!placement.visit(EntityId::from(2), viewport.bottom_right(), other));
        assert_eq!(
            placement.preview(pointer, source_y, width).origin,
            point(pointer.x, source_y)
        );
    }
}
