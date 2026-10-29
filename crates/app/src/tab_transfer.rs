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
    slot: Option<Bounds<Pixels>>,
}
impl Placement {
    pub fn visit(&self, target: EntityId, point: Point<Pixels>, bounds: Bounds<Pixels>) -> bool {
        let mut state = self.0.borrow_mut();
        let inside = bounds.contains(&point);
        if inside {
            state.target = Some(target);
        } else if state.target == Some(target) {
            state.target = None;
            state.slot = None;
        }
        inside
    }
    pub fn slot(&self, mut bounds: Bounds<Pixels>, viewport: Bounds<Pixels>) {
        // The drag is painted above the rows, so keep a complete tab inside the
        // destination viewport while its strip scrolls to the insertion point.
        bounds.origin.x = bounds.left().clamp(
            viewport.left(),
            (viewport.right() - bounds.size.width).max(viewport.left()),
        );
        self.0.borrow_mut().slot = Some(bounds);
    }
    pub fn preview(
        &self,
        natural: Point<Pixels>,
        source_y: Pixels,
        width: Pixels,
    ) -> Bounds<Pixels> {
        let state = self.0.borrow();
        let height = zpx(crate::icons::TOOL_TAB_HEIGHT);
        if let Some(slot) = state.slot.filter(|_| state.target.is_some()) {
            Bounds::new(
                point(slot.left(), slot.center().y - height / 2.),
                size(width, height),
            )
        } else {
            // Pointer movement only changes X; retain the source row until docking.
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
                    let alpha = (distance / zpx(28.)).clamp(0., 1.);
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
            .w(zpx(28.))
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
    fn preview_stays_on_its_row_then_settles_in_a_complete_destination_slot() {
        let placement = Placement::default();
        let width = zpx(140.);
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
        placement.slot(
            Bounds::new(
                point(zpx(450.), viewport.top()),
                size(width, viewport.size.height),
            ),
            viewport,
        );
        let preview = placement.preview(pointer, source_y, width);
        assert_eq!(preview.size.width, width);
        assert_eq!(preview.right(), viewport.right());
        assert_eq!(preview.center().y, viewport.center().y);
        assert!(!placement.visit(target, pointer, viewport));
        assert_eq!(
            placement.preview(pointer, source_y, width).origin,
            point(pointer.x, source_y)
        );
    }
}
