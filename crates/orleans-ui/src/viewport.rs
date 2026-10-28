//! World (millimetres, Y up) to screen (pixels, Y down) mapping.
//!
//! The viewport stores the world point at the centre of the canvas plus a
//! zoom factor in pixels per millimetre. Keeping the centre (rather than a
//! top-left offset) makes "zoom at the cursor" easy and keeps the drawing
//! centred when the window is resized.

use eframe::egui::{Pos2, Rect, Vec2};
use kurbo::{Point, Rect as WorldRect};

#[derive(Clone, Debug)]
pub struct Viewport {
    /// World point drawn at the centre of the canvas.
    pub center: Point,
    /// Pixels per millimetre.
    pub zoom: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            center: Point::new(50.0, 50.0),
            zoom: 4.0,
        }
    }
}

impl Viewport {
    pub fn world_to_screen(&self, area: Rect, p: Point) -> Pos2 {
        let c = area.center();
        Pos2::new(
            c.x + (p.x - self.center.x) as f32 * self.zoom,
            c.y - (p.y - self.center.y) as f32 * self.zoom,
        )
    }

    pub fn screen_to_world(&self, area: Rect, s: Pos2) -> Point {
        let c = area.center();
        Point::new(
            self.center.x + f64::from((s.x - c.x) / self.zoom),
            self.center.y - f64::from((s.y - c.y) / self.zoom),
        )
    }

    /// Drag the drawing by a screen-space delta.
    pub fn pan(&mut self, delta: Vec2) {
        self.center.x -= f64::from(delta.x / self.zoom);
        self.center.y += f64::from(delta.y / self.zoom);
    }

    /// Zoom by `factor` while keeping the world point under `cursor` fixed.
    pub fn zoom_at(&mut self, area: Rect, cursor: Pos2, factor: f32) {
        let before = self.screen_to_world(area, cursor);
        self.zoom = (self.zoom * factor).clamp(0.01, 800.0);
        let after = self.screen_to_world(area, cursor);
        self.center.x += before.x - after.x;
        self.center.y += before.y - after.y;
    }

    /// Frame `bounds` inside `area`, leaving `padding` pixels around it.
    pub fn fit(&mut self, bounds: WorldRect, area: Rect, padding: f32) {
        let usable_w = (area.width() - 2.0 * padding).max(1.0);
        let usable_h = (area.height() - 2.0 * padding).max(1.0);
        let world_w = bounds.width().max(1e-6);
        let world_h = bounds.height().max(1e-6);
        let zoom = (f64::from(usable_w) / world_w).min(f64::from(usable_h) / world_h);
        self.zoom = (zoom as f32).clamp(0.01, 800.0);
        self.center = Point::new(bounds.x0 + world_w / 2.0, bounds.y0 + world_h / 2.0);
    }

    /// World rectangle currently visible in `area`.
    pub fn visible_world(&self, area: Rect) -> WorldRect {
        let a = self.screen_to_world(area, area.left_top());
        let b = self.screen_to_world(area, area.right_bottom());
        WorldRect::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};

    fn area() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))
    }

    #[test]
    fn screen_world_round_trip() {
        let v = Viewport::default();
        for p in [Point::new(0.0, 0.0), Point::new(12.5, 80.25)] {
            let back = v.screen_to_world(area(), v.world_to_screen(area(), p));
            assert!((back.x - p.x).abs() < 1e-4, "{back:?} != {p:?}");
            assert!((back.y - p.y).abs() < 1e-4, "{back:?} != {p:?}");
        }
    }

    #[test]
    fn y_axis_points_up_on_screen() {
        let v = Viewport::default();
        let low = v.world_to_screen(area(), Point::new(0.0, 0.0));
        let high = v.world_to_screen(area(), Point::new(0.0, 10.0));
        assert!(high.y < low.y);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut v = Viewport::default();
        let cursor = pos2(650.0, 120.0);
        let before = v.screen_to_world(area(), cursor);
        v.zoom_at(area(), cursor, 1.7);
        let after = v.screen_to_world(area(), cursor);
        assert!((before.x - after.x).abs() < 1e-4, "{before:?} != {after:?}");
        assert!((before.y - after.y).abs() < 1e-4, "{before:?} != {after:?}");
    }

    #[test]
    fn fit_centres_and_frames_the_content() {
        let mut v = Viewport::default();
        let bounds = WorldRect::new(10.0, 20.0, 110.0, 70.0);
        v.fit(bounds, area(), 40.0);
        let centre = v.world_to_screen(area(), Point::new(60.0, 45.0));
        assert!((centre.x - area().center().x).abs() < 0.01);
        assert!((centre.y - area().center().y).abs() < 0.01);
        let corner = v.world_to_screen(area(), Point::new(10.0, 20.0));
        assert!(area().contains(corner));
    }

    #[test]
    fn pan_moves_the_drawing_with_the_mouse() {
        let mut v = Viewport::default();
        let p = Point::new(10.0, 10.0);
        let before = v.world_to_screen(area(), p);
        v.pan(vec2(20.0, -10.0));
        let after = v.world_to_screen(area(), p);
        assert!((after.x - (before.x + 20.0)).abs() < 1e-3);
        assert!((after.y - (before.y - 10.0)).abs() < 1e-3);
    }
}
