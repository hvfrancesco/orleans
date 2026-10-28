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

/// The ring vertices, cumulative distances and perimeter of a closed contour.
/// A duplicated closing point is ignored.
fn closed_ring(points: &[Point]) -> Option<(Vec<Point>, Vec<f64>, f64)> {
    if points.len() < 3 {
        return None;
    }
    let mut ring = points.to_vec();
    if let (Some(first), Some(last)) = (ring.first(), ring.last()) {
        if (*first - *last).hypot() < 1e-6 {
            ring.pop();
        }
    }
    if ring.len() < 3 {
        return None;
    }
    let mut cumulative = Vec::with_capacity(ring.len());
    cumulative.push(0.0);
    for index in 1..ring.len() {
        cumulative.push(cumulative[index - 1] + (ring[index] - ring[index - 1]).hypot());
    }
    let total = cumulative[ring.len() - 1] + (ring[0] - ring[ring.len() - 1]).hypot();
    if !total.is_finite() || total <= 1e-9 {
        return None;
    }
    Some((ring, cumulative, total))
}

fn point_at_distance(ring: &[Point], cumulative: &[f64], total: f64, distance: f64) -> Point {
    if distance <= 0.0 || distance >= total {
        return ring[0];
    }
    for index in 0..ring.len() {
        let next = (index + 1) % ring.len();
        let edge_start = cumulative[index];
        let edge_end = if next == 0 { total } else { cumulative[next] };
        if distance <= edge_end {
            let t = (distance - edge_start) / (edge_end - edge_start).max(1e-12);
            let a = ring[index];
            let b = ring[next];
            return Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y));
        }
    }
    ring[0]
}

/// Point at a normalised distance along a closed contour.
pub fn point_at_fraction(points: &[Point], fraction: f64) -> Option<Point> {
    let (ring, cumulative, total) = closed_ring(points)?;
    Some(point_at_distance(
        &ring,
        &cumulative,
        total,
        fraction.rem_euclid(1.0) * total,
    ))
}

/// Nearest point on a closed contour: returns `(fraction, distance)`.
pub fn nearest_along_closed(points: &[Point], point: Point) -> Option<(f64, f64)> {
    let (ring, cumulative, total) = closed_ring(points)?;
    let mut best: Option<(f64, f64)> = None;
    for index in 0..ring.len() {
        let next = (index + 1) % ring.len();
        let a = ring[index];
        let b = ring[next];
        let edge = b - a;
        let length = edge.hypot();
        let t = if length <= 1e-12 {
            0.0
        } else {
            let offset = point - a;
            ((offset.x * edge.x + offset.y * edge.y) / (length * length)).clamp(0.0, 1.0)
        };
        let nearest = Point::new(a.x + t * edge.x, a.y + t * edge.y);
        let distance = (point - nearest).hypot();
        if best.is_none_or(|(_, current)| distance < current) {
            best = Some(((cumulative[index] + t * length) / total, distance));
        }
    }
    best
}

/// Merged tab intervals on a closed contour, in distance units.
fn tab_intervals(total: f64, positions: &[f64], width: f64) -> Vec<(f64, f64)> {
    if !width.is_finite() || width <= 0.0 {
        return Vec::new();
    }
    let half = width * 0.5;
    let mut intervals = Vec::new();
    for &position in positions {
        if !position.is_finite() {
            continue;
        }
        let center = position.rem_euclid(1.0) * total;
        let start = center - half;
        let end = center + half;
        if start < 0.0 {
            intervals.push((start + total, total));
            intervals.push((0.0, end.min(total)));
        } else if end > total {
            intervals.push((start, total));
            intervals.push((0.0, end - total));
        } else {
            intervals.push((start, end));
        }
    }
    intervals.retain(|(start, end)| end - start > 1e-9);
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (start, end) in intervals {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 + 1e-9 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn span_polyline(
    ring: &[Point],
    cumulative: &[f64],
    total: f64,
    start: f64,
    end: f64,
) -> Vec<Point> {
    let mut span = vec![point_at_distance(ring, cumulative, total, start)];
    for (index, &distance) in cumulative.iter().enumerate() {
        if distance > start + 1e-9 && distance < end - 1e-9 {
            span.push(ring[index]);
        }
    }
    span.push(point_at_distance(ring, cumulative, total, end));
    dedup(&span, false)
}

/// Split a closed contour at manually placed tab fractions.
pub fn split_manual_tabs(points: &[Point], positions: &[f64], width: f64) -> Vec<Vec<Point>> {
    let Some((ring, cumulative, total)) = closed_ring(points) else {
        return vec![points.to_vec()];
    };
    let intervals = tab_intervals(total, positions, width);
    if intervals.is_empty() {
        return vec![points.to_vec()];
    }

    let mut cuts = Vec::new();
    let mut cursor = 0.0;
    for (start, end) in intervals {
        if start > cursor + 1e-9 {
            cuts.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if cursor < total - 1e-9 {
        cuts.push((cursor, total));
    }

    let mut parts = Vec::new();
    for (start, end) in cuts {
        let part = span_polyline(&ring, &cumulative, total, start, end);
        if part.len() >= 2 {
            parts.push(part);
        }
    }
    parts
}

/// Polylines covering the tab spans themselves, for canvas previews.
pub fn tab_span_polylines(points: &[Point], positions: &[f64], width: f64) -> Vec<Vec<Point>> {
    let Some((ring, cumulative, total)) = closed_ring(points) else {
        return Vec::new();
    };
    tab_intervals(total, positions, width)
        .into_iter()
        .map(|(start, end)| span_polyline(&ring, &cumulative, total, start, end))
        .filter(|span| span.len() >= 2)
        .collect()
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

/// Options for scanline filling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HatchOptions {
    /// Spacing between hatch lines in millimetres.
    pub spacing: f64,
    /// Hatch angle in degrees. 0 is horizontal, 90 is vertical.
    pub angle_deg: f64,
    /// Extend each hatch segment by this distance at both ends, in
    /// millimetres.
    pub overlap: f64,
    /// Reverse alternate scan lines to reduce travel.
    pub alternate: bool,
}

impl Default for HatchOptions {
    fn default() -> Self {
        Self {
            spacing: 0.25,
            angle_deg: 0.0,
            overlap: 0.0,
            alternate: true,
        }
    }
}

/// Generate scanline hatch segments clipped to a set of closed rings.
///
/// The rings should already be unioned if overlapping shapes should not cancel.
/// An even-odd scanline rule handles nested holes; ring winding is irrelevant.
/// Each returned segment is `[start, end]` in millimetres.
pub fn hatch(rings: &[Vec<Point>], options: &HatchOptions) -> Vec<Vec<Point>> {
    let step = options.spacing;
    if !step.is_finite() || step <= 1e-6 || rings.is_empty() {
        return Vec::new();
    }
    let angle = options.angle_deg.to_radians();
    let (sin, cos) = angle.sin_cos();
    let overlap = options.overlap.max(0.0);

    let rotated: Vec<Vec<Point>> = rings
        .iter()
        .filter(|ring| ring.len() >= 3)
        .map(|ring| {
            ring.iter()
                .map(|point| {
                    Point::new(
                        point.x * cos + point.y * sin,
                        -point.x * sin + point.y * cos,
                    )
                })
                .collect()
        })
        .filter(|ring: &Vec<Point>| ring.len() >= 3)
        .collect();
    if rotated.is_empty() {
        return Vec::new();
    }

    let (min_y, max_y) = rotated.iter().flat_map(|ring| ring.iter()).fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(min_y, max_y), point| (min_y.min(point.y), max_y.max(point.y)),
    );
    if !(min_y.is_finite() && max_y.is_finite()) || max_y <= min_y {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut line = 0usize;
    let mut y = (min_y / step).ceil() * step + step * 0.5;
    let max_lines = (((max_y - min_y) / step).ceil() as usize).saturating_add(2);
    while y < max_y - 1e-9 && line < max_lines {
        let mut crossings = Vec::new();
        for ring in &rotated {
            let mut previous = ring[ring.len() - 1];
            for &point in ring {
                if (previous.y > y) != (point.y > y) {
                    let t = (y - previous.y) / (point.y - previous.y);
                    crossings.push(previous.x + t * (point.x - previous.x));
                }
                previous = point;
            }
        }
        crossings.sort_by(f64::total_cmp);

        let mut line_segments = Vec::new();
        let (pairs, _) = crossings.as_chunks::<2>();
        for pair in pairs {
            let x0 = pair[0] - overlap;
            let x1 = pair[1] + overlap;
            if x1 - x0 <= 1e-9 {
                continue;
            }
            let start = Point::new(x0 * cos - y * sin, x0 * sin + y * cos);
            let end = Point::new(x1 * cos - y * sin, x1 * sin + y * cos);
            line_segments.push(vec![start, end]);
        }
        if options.alternate && line % 2 == 1 {
            line_segments.reverse();
            for segment in &mut line_segments {
                segment.swap(0, 1);
            }
        }
        segments.extend(line_segments);
        y += step;
        line += 1;
    }
    segments
}

/// Fill closed contours with scan lines, unioning them first.
pub fn hatch_contours(contours: &[Contour], options: &HatchOptions) -> Vec<Vec<Point>> {
    let rings: Vec<Vec<Point>> = contours
        .iter()
        .filter(|contour| contour.closed)
        .map(|contour| contour.points.clone())
        .collect();
    hatch(&union(&rings), options)
}

/// Split a closed contour into cut runs around evenly spaced holding tabs.
///
/// The first tab straddles the contour seam, so the returned open runs are
/// ordered from the end of the seam tab around the contour. If tabs are
/// disabled, invalid, or wider than the contour, the original closed polyline
/// is returned unchanged.
pub fn split_tabs(points: &[Point], count: u32, width: f64) -> Vec<Vec<Point>> {
    if count == 0 || !width.is_finite() || width <= 0.0 || points.len() < 4 {
        return vec![points.to_vec()];
    }
    let mut ring = points.to_vec();
    if let (Some(first), Some(last)) = (ring.first(), ring.last()) {
        if (*first - *last).hypot() < 1e-6 {
            ring.pop();
        }
    }
    let n = ring.len();
    if n < 3 {
        return vec![points.to_vec()];
    }

    let mut cumulative = Vec::with_capacity(n);
    cumulative.push(0.0);
    for index in 1..n {
        cumulative.push(cumulative[index - 1] + (ring[index] - ring[index - 1]).hypot());
    }
    let total = cumulative[n - 1] + (ring[0] - ring[n - 1]).hypot();
    let count_f = f64::from(count);
    if !total.is_finite() || total <= 1e-9 || width * count_f >= total {
        return vec![points.to_vec()];
    }

    let point_at = |distance: f64| -> Point {
        if distance <= 0.0 || distance >= total {
            return ring[0];
        }
        for index in 0..n {
            let next = (index + 1) % n;
            let edge_start = cumulative[index];
            let edge_end = if next == 0 { total } else { cumulative[next] };
            if distance <= edge_end {
                let t = (distance - edge_start) / (edge_end - edge_start).max(1e-12);
                let a = ring[index];
                let b = ring[next];
                return Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y));
            }
        }
        ring[0]
    };

    let step = total / count_f;
    let half = width * 0.5;
    let mut parts = Vec::new();
    for index in 0..count {
        let start = if index == 0 {
            half
        } else {
            f64::from(index) * step + half
        };
        let end = if index + 1 == count {
            total - half
        } else {
            f64::from(index + 1) * step - half
        };
        if end - start <= 1e-9 {
            continue;
        }

        let mut part = vec![point_at(start)];
        for (vertex, &point) in ring.iter().enumerate() {
            let distance = cumulative[vertex];
            if distance > start + 1e-9 && distance < end - 1e-9 {
                part.push(point);
            }
        }
        part.push(point_at(end));

        let mut clean = Vec::with_capacity(part.len());
        for point in part {
            if clean
                .last()
                .is_none_or(|last: &Point| (*last - point).hypot() > 1e-9)
            {
                clean.push(point);
            }
        }
        if clean.len() >= 2 {
            parts.push(clean);
        }
    }

    if parts.is_empty() {
        vec![points.to_vec()]
    } else {
        parts
    }
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
    fn tabs_leave_even_uncut_gaps() {
        let parts = split_tabs(&square(10.0), 4, 4.0);
        assert_eq!(parts.len(), 4);
        for part in &parts {
            assert!((polyline_length(part) - 6.0).abs() < 1e-9);
        }
        assert!((parts[0][0] - Point::new(2.0, 0.0)).hypot() < 1e-9);
        assert!((parts[0][1] - Point::new(8.0, 0.0)).hypot() < 1e-9);
        assert!((parts[3][0] - Point::new(0.0, 8.0)).hypot() < 1e-9);
        assert!((parts[3][1] - Point::new(0.0, 2.0)).hypot() < 1e-9);
    }

    #[test]
    fn tabs_can_be_disabled_or_rejected_when_too_wide() {
        let full = split_tabs(&square(10.0), 0, 4.0);
        assert_eq!(full.len(), 1);
        assert_eq!(full[0], square(10.0));

        let full = split_tabs(&square(10.0), 20, 5.0);
        assert_eq!(full.len(), 1);
        assert_eq!(full[0], square(10.0));
    }

    #[test]
    fn manual_tabs_split_at_the_requested_fraction() {
        let parts = split_manual_tabs(&square(10.0), &[0.5], 2.0);
        assert_eq!(parts.len(), 2);
        let cut_length: f64 = parts.iter().map(|part| polyline_length(part)).sum();
        assert!((cut_length - 38.0).abs() < 1e-9);

        let spans = tab_span_polylines(&square(10.0), &[0.5], 2.0);
        assert_eq!(spans.len(), 1);
        assert!((polyline_length(&spans[0]) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_manual_tab_across_the_seam_is_one_gap() {
        let parts = split_manual_tabs(&square(10.0), &[0.0], 2.0);
        assert_eq!(parts.len(), 1);
        assert!((polyline_length(&parts[0]) - 38.0).abs() < 1e-9);
    }

    #[test]
    fn nearest_point_reports_fraction_and_distance() {
        let (fraction, distance) =
            nearest_along_closed(&square(10.0), Point::new(5.0, -2.0)).expect("nearest");
        assert!((fraction - 0.125).abs() < 1e-9);
        assert!((distance - 2.0).abs() < 1e-9);

        let point = point_at_fraction(&square(10.0), 0.125).expect("point");
        assert!((point - Point::new(5.0, 0.0)).hypot() < 1e-9);
    }

    #[test]
    fn hatch_fills_a_square_with_scan_lines() {
        let options = HatchOptions {
            spacing: 2.0,
            angle_deg: 0.0,
            overlap: 0.0,
            alternate: false,
        };
        let segments = hatch(&[square(10.0)], &options);
        assert_eq!(segments.len(), 5);
        for (index, segment) in segments.iter().enumerate() {
            assert_eq!(segment.len(), 2);
            assert!((segment[0].x - 0.0).abs() < 1e-9);
            assert!((segment[1].x - 10.0).abs() < 1e-9);
            let expected_y = 1.0 + index as f64 * 2.0;
            assert!((segment[0].y - expected_y).abs() < 1e-9);
            assert!((segment[1].y - expected_y).abs() < 1e-9);
        }
    }

    #[test]
    fn hatch_uses_even_odd_holes() {
        let outer = square(10.0);
        let hole = vec![
            Point::new(4.0, 4.0),
            Point::new(6.0, 4.0),
            Point::new(6.0, 6.0),
            Point::new(4.0, 6.0),
        ];
        let options = HatchOptions {
            spacing: 2.0,
            angle_deg: 0.0,
            overlap: 0.0,
            alternate: false,
        };
        let segments = hatch(&[outer, hole], &options);
        assert_eq!(segments.len(), 6);
        let middle: Vec<_> = segments
            .iter()
            .filter(|segment| (segment[0].y - 5.0).abs() < 1e-9)
            .collect();
        assert_eq!(middle.len(), 2);
        assert!((middle[0][0].x - 0.0).abs() < 1e-9);
        assert!((middle[0][1].x - 4.0).abs() < 1e-9);
        assert!((middle[1][0].x - 6.0).abs() < 1e-9);
        assert!((middle[1][1].x - 10.0).abs() < 1e-9);
    }

    #[test]
    fn hatch_angle_and_overlap_are_applied() {
        let options = HatchOptions {
            spacing: 2.0,
            angle_deg: 90.0,
            overlap: 0.5,
            alternate: false,
        };
        let segments = hatch(&[square(10.0)], &options);
        assert_eq!(segments.len(), 5);
        for segment in &segments {
            assert!((segment[0].x - segment[1].x).abs() < 1e-9);
            assert!((segment[0].y + 0.5).abs() < 1e-9);
            assert!((segment[1].y - 10.5).abs() < 1e-9);
        }
    }

    #[test]
    fn hatch_alternates_line_direction() {
        let options = HatchOptions {
            spacing: 2.0,
            angle_deg: 0.0,
            overlap: 0.0,
            alternate: true,
        };
        let segments = hatch(&[square(10.0)], &options);
        assert_eq!(segments.len(), 5);
        assert!(segments[0][0].x < segments[0][1].x);
        assert!(segments[1][0].x > segments[1][1].x);
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
