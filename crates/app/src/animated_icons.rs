//! Action-specific vector parts, drawn natively at the control's existing size.
use crate::zoom::px as zpx;
use gpui_kit::base::StyledExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, Debug)]
pub enum Glyph {
    Add,
    Settings,
    Refresh,
    Archive,
    Pin,
    Trash,
    Restore,
    Close,
    Pencil,
    Folder,
    Terminal,
    Changes,
    SideChat,
    PanelLeft,
    PanelRight,
    Microphone,
    Steer,
    Activity,
}

#[derive(Clone, Copy, Debug)]
pub enum Morph {
    CopyCheck,
    SendStop,
    PlayPause,
    Disclosure,
    Activity,
}

pub(crate) struct Stroke {
    pub points: Vec<[f32; 2]>,
    pub opacity: f32,
}
fn line(points: &[[f32; 2]]) -> Stroke {
    Stroke {
        points: points.to_vec(),
        opacity: 1.,
    }
}
fn arc(x: f32, y: f32, radius: f32, start: f32, sweep: f32) -> Stroke {
    Stroke {
        points: (0..=32)
            .map(|i| {
                let a = start + sweep * i as f32 / 32.;
                [x + radius * a.cos(), y + radius * a.sin()]
            })
            .collect(),
        opacity: 1.,
    }
}
fn rect(x: f32, y: f32, w: f32, h: f32) -> Stroke {
    line(&[[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y]])
}

pub(crate) fn geometry(kind: Glyph, hover: f32, press: f32) -> Vec<Stroke> {
    let h = hover * (1. - press * 0.5);
    let pulse = (PI * hover).sin();
    match kind {
        Glyph::Add => {
            let d = h - press;
            vec![
                line(&[[3. - d, 8.], [13. + d, 8.]]),
                line(&[[8., 3. - d], [8., 13. + d]]),
            ]
        }
        Glyph::Settings => {
            let teeth = (0..=64)
                .map(|i| {
                    let a = i as f32 * TAU / 64.;
                    let r = if i % 8 < 4 { 6.1 } else { 4.9 };
                    [8. + r * a.cos(), 8. + r * a.sin()]
                })
                .collect();
            vec![
                Stroke {
                    points: teeth,
                    opacity: 1.,
                },
                arc(8., 8., 2. + 0.7 * h, 0., TAU),
            ]
        }
        Glyph::Refresh => vec![
            arc(8., 8., 5.2, -0.65, -4.8 + 2.8 * pulse),
            line(&[[13., 3.], [13., 7.], [9., 7.]]),
        ],
        Glyph::Archive => vec![
            rect(3., 6., 10., 7.),
            rect(2., 3. - h * 1.7, 12., 3.),
            line(&[[6., 9. + h], [10., 9. + h]]),
        ],
        Glyph::Pin => vec![
            line(&[
                [5., 2. + h],
                [11., 2. + h],
                [10., 7. + h],
                [12., 9. + h],
                [4., 9. + h],
                [6., 7. + h],
                [5., 2. + h],
            ]),
            line(&[[8., 9. + h], [8., 14. - press]]),
        ],
        Glyph::Trash => vec![
            line(&[[4., 6.], [5., 14.], [11., 14.], [12., 6.]]),
            line(&[[3., 4. - h * 1.5], [13., 4. - h * 1.5]]),
            line(&[
                [6., 4. - h * 1.5],
                [6., 2. - h],
                [10., 2. - h],
                [10., 4. - h * 1.5],
            ]),
            line(&[[7., 7. + h], [7., 11. + h]]),
            line(&[[9., 7. + h], [9., 11. + h]]),
        ],
        Glyph::Restore => vec![
            arc(8., 8., 5., -PI / 2., 4.8 - 2. * pulse),
            line(&[[2., 3. + h], [2., 7. + h], [6., 7. + h]]),
        ],
        Glyph::Close => {
            let d = 0.8 * press;
            vec![
                line(&[[4. + d, 4. + d], [12. - d, 12. - d]]),
                line(&[[4. + d, 12. - d], [12. - d, 4. + d]]),
            ]
        }
        Glyph::Pencil => vec![
            line(&[
                [3. + h, 10. - h],
                [10. + h, 3. - h],
                [13. + h, 6. - h],
                [6. + h, 13. - h],
                [3. + h, 13. - h],
                [3. + h, 10. - h],
            ]),
            line(&[[9. + h, 4. - h], [12. + h, 7. - h]]),
            line(&[[3., 14.], [6. + 5. * h, 14.]]),
        ],
        Glyph::Folder => vec![
            line(&[
                [2., 6.],
                [2., 3.],
                [6., 3.],
                [8., 5.],
                [14., 5.],
                [14., 13.],
                [2., 13.],
                [2., 6.],
            ]),
            line(&[
                [2., 13.],
                [2. + h * 1.8, 6. + h],
                [14. + h * 0.8, 6. + h],
                [14., 13.],
            ]),
        ],
        Glyph::Terminal => vec![
            rect(1.5, 3., 13., 10.),
            line(&[[4., 6.], [6., 8.], [4., 10.]]),
            line(&[[8., 10.], [10. + 2. * h, 10.]]),
        ],
        Glyph::Changes => vec![
            line(&[
                [3., 1.5],
                [9., 1.5],
                [13., 5.5],
                [13., 14.5],
                [3., 14.5],
                [3., 1.5],
            ]),
            line(&[[9., 1.5], [9., 5.5], [13., 5.5]]),
            line(&[[5., 8.], [10. + h, 8.]]),
            line(&[[5., 11.], [8. + 2. * h, 11.]]),
        ],
        Glyph::SideChat => vec![
            line(&[
                [3.5, 2.5],
                [12.5, 2.5],
                [14., 4.],
                [14., 9.5],
                [12.5, 11.],
                [8., 11.],
                [4.5, 14.],
                [4.5, 11.],
                [3.5, 11.],
                [2., 9.5],
                [2., 4.],
                [3.5, 2.5],
            ]),
            line(&[[5.5 - h * 0.4, 6.75], [10.5 + h * 0.4, 6.75]]),
            line(&[[8., 4.25 - h * 0.4], [8., 9.25 + h * 0.4]]),
        ],
        Glyph::PanelLeft | Glyph::PanelRight => {
            let x = if matches!(kind, Glyph::PanelLeft) {
                5. + h * 1.6
            } else {
                11. - h * 1.6
            };
            vec![rect(1.5, 3., 13., 10.), line(&[[x, 3.], [x, 13.]])]
        }
        Glyph::Microphone => vec![
            line(&[
                [6., 3.],
                [6., 8.],
                [7., 9.],
                [9., 9.],
                [10., 8.],
                [10., 3.],
                [9., 2.],
                [7., 2.],
                [6., 3.],
            ]),
            arc(8., 8., 4. + h * 0.6, 0., PI),
            line(&[[8., 12.], [8., 14.]]),
            line(&[[5., 14.], [11., 14.]]),
        ],
        Glyph::Steer => vec![
            line(&[[3., 13.], [3., 8.], [5., 6.], [13., 6.]]),
            line(&[[10. - h, 3.], [13., 6.], [10. - h, 9.]]),
        ],
        Glyph::Activity => vec![line(&[
            [1., 8.],
            [4., 8.],
            [6., 3. - h],
            [9., 13. + h],
            [11., 8.],
            [15., 8.],
        ])],
    }
}

pub(crate) fn morph_geometry(kind: Morph, t: f32) -> Vec<Stroke> {
    let mix = |a: [f32; 2], b: [f32; 2]| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    match kind {
        Morph::CopyCheck => {
            let from = [[5., 5.], [13., 5.], [13., 13.], [5., 13.], [5., 5.]];
            let to = [[3., 8.], [4.5, 9.5], [6., 11.], [9.5, 7.5], [13., 4.]];
            let mut back = line(&[[3., 10.], [2., 10.], [2., 2.], [10., 2.], [10., 3.]]);
            back.opacity = 1. - t;
            vec![
                back,
                Stroke {
                    points: from.into_iter().zip(to).map(|(a, b)| mix(a, b)).collect(),
                    opacity: 1.,
                },
            ]
        }
        Morph::SendStop => {
            let from = [
                [3.5, 7.75],
                [8., 2.75],
                [12.5, 7.75],
                [8.85, 7.75],
                [8.85, 13.5],
                [7.15, 13.5],
                [7.15, 7.75],
                [3.5, 7.75],
            ];
            let to = [
                [3., 3.],
                [8., 3.],
                [13., 3.],
                [13., 8.],
                [13., 13.],
                [3., 13.],
                [3., 8.],
                [3., 3.],
            ];
            vec![Stroke {
                points: from.into_iter().zip(to).map(|(a, b)| mix(a, b)).collect(),
                opacity: 1.,
            }]
        }
        Morph::PlayPause => {
            let a = [[5., 3.], [8., 5.5], [8., 10.5], [5., 13.], [5., 3.]];
            let b = [[8., 5.5], [12., 8.], [12., 8.], [8., 10.5], [8., 5.5]];
            let end_a = [[4., 3.], [6., 3.], [6., 13.], [4., 13.], [4., 3.]];
            let end_b = [[10., 3.], [12., 3.], [12., 13.], [10., 13.], [10., 3.]];
            vec![
                Stroke {
                    points: a.into_iter().zip(end_a).map(|(a, b)| mix(a, b)).collect(),
                    opacity: 1.,
                },
                Stroke {
                    points: b.into_iter().zip(end_b).map(|(a, b)| mix(a, b)).collect(),
                    opacity: 1.,
                },
            ]
        }
        Morph::Activity => geometry(Glyph::Activity, t, 0.),
        Morph::Disclosure => vec![line(&[
            [4., 6. + 4. * t],
            [8., 10. - 4. * t],
            [12., 6. + 4. * t],
        ])],
    }
}

#[derive(IntoElement)]
pub struct AnimatedIcon {
    strokes: Vec<Stroke>,
    filled: bool,
    size: Pixels,
    style: StyleRefinement,
}
impl AnimatedIcon {
    pub fn new(kind: Glyph, hover: f32, press: f32) -> Self {
        Self {
            strokes: geometry(kind, hover, press),
            filled: false,
            size: zpx(crate::icons::GLYPH_SIZE),
            style: StyleRefinement::default(),
        }
    }
    pub fn morph(kind: Morph, progress: f32) -> Self {
        Self {
            strokes: morph_geometry(kind, progress),
            filled: matches!(kind, Morph::SendStop | Morph::PlayPause),
            size: zpx(crate::icons::GLYPH_SIZE),
            style: StyleRefinement::default(),
        }
    }
    pub fn with_size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}
impl Styled for AnimatedIcon {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
impl RenderOnce for AnimatedIcon {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .size(self.size)
            .flex_none()
            .refine_style(&self.style)
            .child(
                canvas(
                    |_, window, _| window.text_style().color,
                    move |bounds, color, window, _| {
                        let scale = bounds.size.width.as_f32() / 16.;
                        for stroke in self.strokes {
                            if stroke.opacity <= 0. {
                                continue;
                            }
                            let mut path = if self.filled {
                                PathBuilder::fill()
                            } else {
                                PathBuilder::stroke(px(1.25 * scale))
                            };
                            for (i, p) in stroke.points.into_iter().enumerate() {
                                let p = bounds.origin + point(px(p[0] * scale), px(p[1] * scale));
                                if i == 0 {
                                    path.move_to(p);
                                } else {
                                    path.line_to(p);
                                }
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color.opacity(stroke.opacity));
                            }
                        }
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn animated_parts_stay_inside_their_fixed_icon_box() {
        let kinds = [
            Glyph::Add,
            Glyph::Settings,
            Glyph::Refresh,
            Glyph::Archive,
            Glyph::Pin,
            Glyph::Trash,
            Glyph::Restore,
            Glyph::Close,
            Glyph::Pencil,
            Glyph::Folder,
            Glyph::Terminal,
            Glyph::Changes,
            Glyph::SideChat,
            Glyph::PanelLeft,
            Glyph::PanelRight,
            Glyph::Microphone,
            Glyph::Steer,
            Glyph::Activity,
        ];
        for kind in kinds {
            for step in 0..=20 {
                for press in [0., 0.5, 1.] {
                    for stroke in geometry(kind, step as f32 / 20., press) {
                        assert!((0. ..=1.).contains(&stroke.opacity));
                        assert!(stroke.points.len() >= 2);
                        for point in stroke.points {
                            assert!(
                                point
                                    .into_iter()
                                    .all(|n| n.is_finite() && (0.5..=15.5).contains(&n)),
                                "{kind:?}: {point:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn parts_move_independently_and_morphs_have_real_intermediate_paths() {
        // The trash body and folder back stay anchored while their lids move.
        for kind in [Glyph::Trash, Glyph::Folder] {
            let rest = geometry(kind, 0., 0.);
            let active = geometry(kind, 1., 0.);
            assert_eq!(rest[0].points, active[0].points);
            assert_ne!(rest[1].points, active[1].points);
        }
        for kind in [
            Morph::CopyCheck,
            Morph::SendStop,
            Morph::PlayPause,
            Morph::Disclosure,
            Morph::Activity,
        ] {
            let start = morph_geometry(kind, 0.);
            let halfway = morph_geometry(kind, 0.5);
            let end = morph_geometry(kind, 1.);
            assert!(
                start
                    .iter()
                    .zip(&halfway)
                    .any(|(a, b)| a.points != b.points)
            );
            assert!(halfway.iter().zip(&end).any(|(a, b)| a.points != b.points));
            for step in 0..=20 {
                for stroke in morph_geometry(kind, step as f32 / 20.) {
                    assert!(
                        stroke
                            .points
                            .into_iter()
                            .flatten()
                            .all(|n| n.is_finite() && (0.5..=15.5).contains(&n))
                    );
                }
            }
        }
        assert_eq!(morph_geometry(Morph::CopyCheck, 1.)[0].opacity, 0.);
    }
}
