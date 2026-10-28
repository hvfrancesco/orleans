//! Geometry helpers: flattening, area, containment and kerf offsetting.
//!
//! Everything here works on polylines (millimetres, Y up) so the planner never
//! has to think about Bézier maths. Offsetting and boolean operations are
//! delegated to Clipper2 (integer-scaled to 0.001 mm), which handles concave
//! contours and self-intersections that a hand-written miter offset cannot.

use clipper2::{EndType, FillRule, JoinType, Milli, Paths};
use kurbo::{BezPath, PathEl, Point};

/// Default flattening tolerance in millimetres. 0.02 mm is below what most
/// laser cutters can resolve, and keeps the G-code small.
pub const DEFAULT_TOLERANCE: f64 = 0.02;

const EPS: f64 = 1e-9;

/// A flattened subpath.
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    pub points: Vec<Point>,
    pub closed: bool,
}

impl Contour {
    /// Length in millimetres, including the closing segment for closed contours.
    pub fn length(&self) -> f64 {
        let mut len: f64 = self.points.windows(2).map(|w| (w[1] - w[0]).hypot()).sum();
        if self.closed {
            if let (Some(first), Some(last)) = (self.points.first(), self.points.last()) {
                len += (*first - *last).hypot();
            }
        }
        len
    }

    /// Signed area in mm². Positive means counter-clockwise (Y up).
    pub fn signed_area(&self) -> f64 {
        signed_area(&self.points)
    }

    /// Centroid of the vertices. Good enough for containment tests and for
    /// choosing a rapid start point.
    pub fn centroid(&self) -> Point {
        let n = self.points.len() as f64;
        if n == 0.0 {
            return Point::ORIGIN;
        }
        let sum = self
            .points
            .iter()
            .fold(Point::ORIGIN, |acc, p| Point::new(acc.x + p.x, acc.y + p.y));
        Point::new(sum.x / n, sum.y / n)
    }

    pub fn bounds(&self) -> Option<kurbo::Rect> {
        if self.points.is_empty() {
            return None;
        }
        let mut rect = kurbo::Rect::new(
            self.points[0].x,
            self.points[0].y,
            self.points[0].x,
            self.points[0].y,
        );
        for p in &self.points[1..] {
            rect = rect.union(kurbo::Rect::new(p.x, p.y, p.x, p.y));
        }
        Some(rect)
    }
}

/// Flatten a Bézier path into polylines, splitting at every `MoveTo`.
pub fn flatten_path(path: &BezPath, tolerance: f64) -> Vec<Contour> {
    let tolerance = if tolerance > 0.0 {
        tolerance
    } else {
        DEFAULT_TOLERANCE
    };

    let mut contours: Vec<Contour> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let mut closed = false;

    let finish = |current: &mut Vec<Point>, closed: bool, contours: &mut Vec<Contour>| {
        if current.len() >= 2 {
            let points = dedup(current, closed);
            if points.len() >= 2 {
                contours.push(Contour { points, closed });
            }
        }
        current.clear();
    };

    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) => {
            finish(&mut current, closed, &mut contours);
            closed = false;
            current.push(p);
        }
        PathEl::LineTo(p) => current.push(p),
        PathEl::ClosePath => {
            closed = true;
            finish(&mut current, true, &mut contours);
            closed = false;
        }
        // `flatten` only emits MoveTo/LineTo/ClosePath.
        _ => {}
    });
    finish(&mut current, closed, &mut contours);

    contours
}

/// Total length of a polyline.
pub fn polyline_length(points: &[Point]) -> f64 {
    points.windows(2).map(|w| (w[1] - w[0]).hypot()).sum()
}

/// Shoelace signed area. Positive is counter-clockwise in a Y-up system.
pub fn signed_area(points: &[Point]) -> f64 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        sum += a.x * b.y - b.x * a.y;
    }
    sum * 0.5
}

/// Even-odd point-in-polygon test. Points exactly on an edge count as inside,
/// which makes nesting detection stable for shapes that touch.
pub fn point_in_polygon(p: Point, polygon: &[Point]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let n = polygon.len();
    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];
        if point_on_segment(p, a, b) {
            return true;
        }
        let crosses = (a.y > p.y) != (b.y > p.y);
        if crosses {
            let t = (p.y - a.y) / (b.y - a.y);
            let x = a.x + t * (b.x - a.x);
            if x > p.x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Offset a closed contour by `distance` millimetres towards its interior
/// (positive) or exterior (negative), regardless of winding direction.
///
/// Clipper2 does the work: it is integer-scaled (0.001 mm), handles concave
/// contours and resolves the self-intersections that a naive miter offset
/// produces. Sharp corners are mitered up to `miter_limit` times the offset.
pub fn offset_closed(points: &[Point], distance: f64, miter_limit: f64) -> Vec<Point> {
    let pts = ensure_ccw(dedup(points, true));
    if pts.len() < 3 || distance.abs() < EPS {
        return close(&pts);
    }

    // Clipper's sign is the opposite of ours: positive inflates.
    let result = to_clipper(&[pts]).inflate(
        -distance,
        JoinType::Miter,
        EndType::Polygon,
        miter_limit.max(1.0),
    );

    // A self-intersecting input can yield several rings; keep the largest,
    // which is the actual offset of the material boundary.
    let rings = from_clipper(result);
    rings
        .into_iter()
        .max_by(|a, b| signed_area(a).abs().total_cmp(&signed_area(b).abs()))
        .map(|ring| close(&ring))
        .unwrap_or_default()
}

/// Union of a set of rings (even-odd holes are preserved by Clipper).
pub fn union(rings: &[Vec<Point>]) -> Vec<Vec<Point>> {
    to_clipper(rings)
        .to_clipper_subject()
        .add_clip(CPaths::new(Vec::new()))
        .union(FillRule::NonZero)
        .map(from_clipper)
        .unwrap_or_default()
}

/// Everything covered by `subject` but not by `clip`.
pub fn difference(subject: &[Vec<Point>], clip: &[Vec<Point>]) -> Vec<Vec<Point>> {
    to_clipper(subject)
        .to_clipper_subject()
        .add_clip(to_clipper(clip))
        .difference(FillRule::NonZero)
        .map(from_clipper)
        .unwrap_or_default()
}

/// The area covered by both `subject` and `clip`.
pub fn intersection(subject: &[Vec<Point>], clip: &[Vec<Point>]) -> Vec<Vec<Point>> {
    to_clipper(subject)
        .to_clipper_subject()
        .add_clip(to_clipper(clip))
        .intersect(FillRule::NonZero)
        .map(from_clipper)
        .unwrap_or_default()
}

/// Clipper2 works on scaled integers; 0.001 mm resolution.
type CPaths = Paths<Milli>;

fn to_clipper(rings: &[Vec<Point>]) -> CPaths {
    let paths: Vec<Vec<(f64, f64)>> = rings
        .iter()
        .map(|ring| ring.iter().map(|p| (p.x, p.y)).collect())
        .collect();
    paths.into()
}

fn from_clipper(paths: CPaths) -> Vec<Vec<Point>> {
    let paths: Vec<Vec<(f64, f64)>> = paths.into();
    paths
        .into_iter()
        .filter(|ring| ring.len() >= 3)
        .map(|ring| ring.into_iter().map(|(x, y)| Point::new(x, y)).collect())
        .collect()
}

fn ensure_ccw(mut points: Vec<Point>) -> Vec<Point> {
    if signed_area(&points) < 0.0 {
        points.reverse();
    }
    points
}

fn close(points: &[Point]) -> Vec<Point> {
    let mut out = points.to_vec();
    if let (Some(first), Some(last)) = (out.first().copied(), out.last().copied()) {
        if (first - last).hypot() > EPS {
            out.push(first);
        }
    }
    out
}

/// Remove the duplicated closing point (when `closed`) and consecutive
/// duplicates within 1 µm.
pub fn dedup(points: &[Point], closed: bool) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        let dup = match out.last() {
            Some(last) => (*last - *p).hypot() < 1e-6,
            None => false,
        };
        if !dup {
            out.push(*p);
        }
    }
    if closed && out.len() > 1 {
        let first = out[0];
        let last = out[out.len() - 1];
        if (first - last).hypot() < 1e-6 {
            out.pop();
        }
    }
    out
}

fn point_on_segment(p: Point, a: Point, b: Point) -> bool {
    let ab = b - a;
    let ap = p - a;
    let cross = ab.x * ap.y - ab.y * ap.x;
    if cross.abs() > 1e-9 * ab.hypot().max(1.0) {
        return false;
    }
    let dot = ab.x * ap.x + ab.y * ap.y;
    dot >= 0.0 && dot <= ab.x * ab.x + ab.y * ab.y
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::BezPath;

    fn square(size: f64) -> Vec<Point> {
        vec![
            Point::new(0.0, 0.0),
            Point::new(size, 0.0),
            Point::new(size, size),
            Point::new(0.0, size),
        ]
    }

    #[test]
    fn square_signed_area_is_positive_ccw() {
        assert!((signed_area(&square(10.0)) - 100.0).abs() < 1e-9);
        let mut cw = square(10.0);
        cw.reverse();
        assert!((signed_area(&cw) + 100.0).abs() < 1e-9);
    }

    #[test]
    fn offset_shrinks_square_by_one_mm() {
        let out = offset_closed(&square(10.0), 1.0, 4.0);
        assert!(out.len() >= 5);
        let area = signed_area(&out);
        assert!((area - 64.0).abs() < 1e-6, "area was {area}");
    }

    #[test]
    fn offset_is_winding_independent() {
        let mut cw = square(10.0);
        cw.reverse();
        let out = offset_closed(&cw, 1.0, 4.0);
        assert!((signed_area(&out).abs() - 64.0).abs() < 1e-6);
    }

    #[test]
    fn flatten_splits_subpaths() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.line_to((10.0, 10.0));
        path.close_path();
        path.move_to((20.0, 0.0));
        path.line_to((30.0, 0.0));
        path.line_to((30.0, 10.0));
        path.close_path();

        let contours = flatten_path(&path, 0.01);
        assert_eq!(contours.len(), 2);
        assert!(contours.iter().all(|c| c.closed));
        assert_eq!(contours[0].points.len(), 3);
        assert!((contours[0].signed_area() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn boolean_union_of_overlapping_squares() {
        let a = vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
        ];
        let b = vec![
            Point::new(5.0, 0.0),
            Point::new(15.0, 0.0),
            Point::new(15.0, 10.0),
            Point::new(5.0, 10.0),
        ];
        let out = union(&[a, b]);
        let area: f64 = out.iter().map(|r| signed_area(r).abs()).sum();
        assert!((area - 150.0).abs() < 1e-3, "area {area}");
    }

    #[test]
    fn boolean_difference_cuts_a_hole() {
        let outer = square(10.0);
        let hole = vec![
            Point::new(4.0, 4.0),
            Point::new(6.0, 4.0),
            Point::new(6.0, 6.0),
            Point::new(4.0, 6.0),
        ];
        let out = difference(&[outer], &[hole]);
        assert_eq!(out.len(), 2, "outer boundary plus hole");
        let area: f64 = out.iter().map(|r| signed_area(r).abs()).sum();
        // Ring areas are unsigned here: 100 + 4.
        assert!((area - 104.0).abs() < 1e-3, "area {area}");
    }

    #[test]
    fn offset_of_concave_polygon_does_not_explode() {
        let l_shape = vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 5.0),
            Point::new(5.0, 5.0),
            Point::new(5.0, 10.0),
            Point::new(0.0, 10.0),
        ];
        let out = offset_closed(&l_shape, 1.0, 4.0);
        assert!(out.len() >= 6);
        // Eroding the L by 1 mm leaves 8x8 minus the enlarged 5x5 notch.
        let area = signed_area(&out).abs();
        assert!((area - 39.0).abs() < 0.01, "area {area}");
        assert!(out.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
    }

    #[test]
    fn containment() {
        let sq = square(10.0);
        assert!(point_in_polygon(Point::new(5.0, 5.0), &sq));
        assert!(!point_in_polygon(Point::new(15.0, 5.0), &sq));
        assert!(point_in_polygon(Point::new(0.0, 5.0), &sq));
    }
}
