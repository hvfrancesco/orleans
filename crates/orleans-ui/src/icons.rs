//! Small vector icons used by compact toolbar and selection controls.

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Response, Shape, Stroke, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Undo,
    Redo,
    Fit,
    Group,
    Ungroup,
    AlignLeft,
    AlignCenterH,
    AlignRight,
    AlignTop,
    AlignCenterV,
    AlignBottom,
    DistributeH,
    DistributeV,
    Duplicate,
    Delete,
    Up,
    Down,
    Union,
    Subtract,
    Intersect,
}

/// Compact square icon button with a localized tooltip.
pub fn button(ui: &mut egui::Ui, icon: Icon, size: f32, tooltip: &str, enabled: bool) -> Response {
    let response = ui.add_enabled(enabled, egui::Button::new("").min_size(Vec2::splat(size)));
    let response = response.on_hover_text(tooltip);
    let color = if enabled {
        ui.style().interact(&response).fg_stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    draw(ui.painter(), response.rect, icon, color);
    response
}

fn draw(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let scale = rect.width().min(rect.height()) / 24.0;
    let p = |x: f32, y: f32| {
        Pos2::new(
            rect.center().x + (x - 12.0) * scale,
            rect.center().y + (y - 12.0) * scale,
        )
    };
    let stroke = Stroke::new((1.45 * scale).max(1.0), color);
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|(x, y)| p(*x, *y)).collect(),
            stroke,
        ));
    };
    let circle = |x: f32, y: f32, r: f32| {
        painter.circle_stroke(p(x, y), r * scale, stroke);
    };
    let rect_stroke = |x0: f32, y0: f32, x1: f32, y1: f32| {
        painter.rect_stroke(
            Rect::from_min_max(p(x0, y0), p(x1, y1)),
            1.0,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    let triangle = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        painter.add(Shape::convex_polygon(
            vec![p(a.0, a.1), p(b.0, b.1), p(c.0, c.1)],
            color,
            Stroke::NONE,
        ));
    };

    match icon {
        Icon::Undo | Icon::Redo => {
            let mirror = icon == Icon::Redo;
            let x = |value: f32| if mirror { 24.0 - value } else { value };
            line(&[
                (x(17.0), 7.0),
                (x(10.0), 7.0),
                (x(7.0), 10.0),
                (x(7.0), 14.0),
                (x(10.0), 17.0),
                (x(15.0), 17.0),
            ]);
            triangle((x(7.0), 10.0), (x(5.0), 13.0), (x(10.0), 13.0));
        }
        Icon::Fit => {
            line(&[(3.0, 9.0), (3.0, 3.0), (9.0, 3.0)]);
            line(&[(15.0, 3.0), (21.0, 3.0), (21.0, 9.0)]);
            line(&[(21.0, 15.0), (21.0, 21.0), (15.0, 21.0)]);
            line(&[(9.0, 21.0), (3.0, 21.0), (3.0, 15.0)]);
        }
        Icon::Group | Icon::Ungroup => {
            let gap = if icon == Icon::Group { 1.0 } else { 3.0 };
            rect_stroke(4.0, 5.0, 12.0 - gap, 13.0);
            rect_stroke(12.0 + gap, 11.0, 20.0, 19.0);
            if icon == Icon::Group {
                line(&[(12.0, 9.0), (16.0, 9.0), (16.0, 11.0)]);
            }
        }
        Icon::AlignLeft | Icon::AlignCenterH | Icon::AlignRight => {
            let x = match icon {
                Icon::AlignLeft => 4.0,
                Icon::AlignCenterH => 12.0,
                _ => 20.0,
            };
            line(&[(x, 4.0), (x, 20.0)]);
            let left = if icon == Icon::AlignLeft {
                4.0
            } else if icon == Icon::AlignCenterH {
                6.0
            } else {
                9.0
            };
            line(&[(left, 8.0), (left + 11.0, 8.0)]);
            line(&[(left, 16.0), (left + 7.0, 16.0)]);
        }
        Icon::AlignTop | Icon::AlignCenterV | Icon::AlignBottom => {
            let y = match icon {
                Icon::AlignTop => 4.0,
                Icon::AlignCenterV => 12.0,
                _ => 20.0,
            };
            line(&[(4.0, y), (20.0, y)]);
            let top = if icon == Icon::AlignTop {
                4.0
            } else if icon == Icon::AlignCenterV {
                6.0
            } else {
                9.0
            };
            line(&[(8.0, top), (8.0, top + 11.0)]);
            line(&[(16.0, top), (16.0, top + 7.0)]);
        }
        Icon::DistributeH => {
            line(&[(4.0, 4.0), (4.0, 20.0)]);
            line(&[(12.0, 4.0), (12.0, 20.0)]);
            line(&[(20.0, 4.0), (20.0, 20.0)]);
            line(&[(7.0, 12.0), (17.0, 12.0)]);
            triangle((7.0, 12.0), (10.0, 10.0), (10.0, 14.0));
            triangle((17.0, 12.0), (14.0, 10.0), (14.0, 14.0));
        }
        Icon::DistributeV => {
            line(&[(4.0, 4.0), (20.0, 4.0)]);
            line(&[(4.0, 12.0), (20.0, 12.0)]);
            line(&[(4.0, 20.0), (20.0, 20.0)]);
            line(&[(12.0, 7.0), (12.0, 17.0)]);
            triangle((12.0, 7.0), (10.0, 10.0), (14.0, 10.0));
            triangle((12.0, 17.0), (10.0, 14.0), (14.0, 14.0));
        }
        Icon::Duplicate => {
            rect_stroke(4.0, 7.0, 14.0, 19.0);
            rect_stroke(9.0, 4.0, 20.0, 16.0);
        }
        Icon::Delete => {
            line(&[(5.0, 7.0), (19.0, 7.0)]);
            line(&[(9.0, 7.0), (10.0, 4.0), (14.0, 4.0), (15.0, 7.0)]);
            line(&[(7.0, 7.0), (8.0, 20.0), (16.0, 20.0), (17.0, 7.0)]);
        }
        Icon::Up => {
            line(&[(12.0, 19.0), (12.0, 5.0)]);
            triangle((12.0, 4.0), (7.0, 10.0), (17.0, 10.0));
        }
        Icon::Down => {
            line(&[(12.0, 5.0), (12.0, 19.0)]);
            triangle((12.0, 20.0), (7.0, 14.0), (17.0, 14.0));
        }
        Icon::Union | Icon::Subtract | Icon::Intersect => {
            circle(9.0, 12.0, 6.0);
            circle(15.0, 12.0, 6.0);
            match icon {
                Icon::Subtract => line(&[(15.0, 12.0), (20.0, 12.0)]),
                Icon::Intersect => {
                    painter.circle_filled(p(12.0, 12.0), 3.0 * scale, color);
                }
                _ => {}
            }
        }
    }
}
