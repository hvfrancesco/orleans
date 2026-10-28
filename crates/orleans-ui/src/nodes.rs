//! Editing helpers for the anchors and Bézier controls of a path.
//!
//! A node refers to one element of the [`BezPath`] plus the field inside that
//! element. On-curve anchors are the endpoints of `MoveTo`, `LineTo`, `QuadTo`
//! and `CurveTo`; controls are the extra points of quadratic and cubic
//! segments. A closed subpath whose final endpoint coincides with its `MoveTo`
//! point is treated as a single anchor, so dragging that anchor keeps the
//! contour closed.

use std::collections::BTreeSet;

use kurbo::{BezPath, Line, ParamCurve, ParamCurveNearest, PathEl, PathSeg, Point, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeKind {
    Anchor,
    Control1,
    Control2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeRef {
    pub element: usize,
    pub kind: NodeKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub node: NodeRef,
    pub point: Point,
}

impl Node {
    pub fn is_anchor(self) -> bool {
        self.node.kind == NodeKind::Anchor
    }
}

/// Return the visible anchors and control points of `path`.
pub fn nodes(path: &BezPath) -> Vec<Node> {
    let elements = path.elements();
    let closing_ends = closing_endpoints(elements);
    let mut result = Vec::new();
    for (element, path_el) in elements.iter().enumerate() {
        match *path_el {
            PathEl::MoveTo(point) => result.push(node(element, NodeKind::Anchor, point)),
            PathEl::LineTo(point) => {
                if !closing_ends.contains(&element) {
                    result.push(node(element, NodeKind::Anchor, point));
                }
            }
            PathEl::QuadTo(control, point) => {
                result.push(node(element, NodeKind::Control1, control));
                if !closing_ends.contains(&element) {
                    result.push(node(element, NodeKind::Anchor, point));
                }
            }
            PathEl::CurveTo(control1, control2, point) => {
                result.push(node(element, NodeKind::Control1, control1));
                result.push(node(element, NodeKind::Control2, control2));
                if !closing_ends.contains(&element) {
                    result.push(node(element, NodeKind::Anchor, point));
                }
            }
            PathEl::ClosePath => {}
        }
    }
    result
}

/// Lines that connect Bézier controls to their anchors, for drawing.
pub fn control_links(path: &BezPath) -> Vec<(Point, Point)> {
    let mut links = Vec::new();
    let mut current = None;
    let mut subpath_start = None;
    for element in path.elements() {
        match *element {
            PathEl::MoveTo(point) => {
                current = Some(point);
                subpath_start = Some(point);
            }
            PathEl::LineTo(point) => current = Some(point),
            PathEl::QuadTo(control, point) => {
                if let Some(start) = current {
                    links.push((start, control));
                }
                links.push((point, control));
                current = Some(point);
            }
            PathEl::CurveTo(control1, control2, point) => {
                if let Some(start) = current {
                    links.push((start, control1));
                }
                links.push((point, control2));
                current = Some(point);
            }
            PathEl::ClosePath => current = subpath_start,
        }
    }
    links
}

/// Move one node by `delta`.
///
/// Moving an anchor also moves the Bézier controls attached to it: the incoming
/// control of its own segment and the outgoing control of the next segment.
/// Moving a control changes only that control. Returns true when the node was
/// valid and the path changed.
pub fn move_node(path: &mut BezPath, node: NodeRef, delta: Vec2) -> bool {
    let elements = path.elements();
    if node.element >= elements.len() {
        return false;
    }

    let mut targets = vec![node];
    if node.kind == NodeKind::Anchor {
        match elements[node.element] {
            PathEl::QuadTo(..) => targets.push(NodeRef {
                element: node.element,
                kind: NodeKind::Control1,
            }),
            PathEl::CurveTo(..) => targets.push(NodeRef {
                element: node.element,
                kind: NodeKind::Control2,
            }),
            _ => {}
        }

        if let Some(PathEl::MoveTo(_)) = elements.get(node.element) {
            if let Some(closing_end) = closing_end_for_start(elements, node.element) {
                targets.push(NodeRef {
                    element: closing_end,
                    kind: NodeKind::Anchor,
                });
                match elements[closing_end] {
                    PathEl::QuadTo(..) => targets.push(NodeRef {
                        element: closing_end,
                        kind: NodeKind::Control1,
                    }),
                    PathEl::CurveTo(..) => targets.push(NodeRef {
                        element: closing_end,
                        kind: NodeKind::Control2,
                    }),
                    _ => {}
                }
            }
        }

        if matches!(
            elements.get(node.element + 1),
            Some(PathEl::QuadTo(..) | PathEl::CurveTo(..))
        ) {
            targets.push(NodeRef {
                element: node.element + 1,
                kind: NodeKind::Control1,
            });
        }
    }

    let elements = path.elements_mut();
    let mut moved = false;
    for target in targets.into_iter().collect::<BTreeSet<_>>() {
        moved |= apply_delta(elements, target, delta);
    }
    moved
}

fn apply_delta(elements: &mut [PathEl], node: NodeRef, delta: Vec2) -> bool {
    let Some(element) = elements.get_mut(node.element) else {
        return false;
    };
    match (element, node.kind) {
        (PathEl::MoveTo(point) | PathEl::LineTo(point), NodeKind::Anchor)
        | (PathEl::QuadTo(_, point), NodeKind::Anchor)
        | (PathEl::CurveTo(_, _, point), NodeKind::Anchor) => {
            *point += delta;
            true
        }
        (PathEl::QuadTo(control, _), NodeKind::Control1)
        | (PathEl::CurveTo(control, _, _), NodeKind::Control1) => {
            *control += delta;
            true
        }
        (PathEl::CurveTo(_, control, _), NodeKind::Control2) => {
            *control += delta;
            true
        }
        _ => false,
    }
}

fn node(element: usize, kind: NodeKind, point: Point) -> Node {
    Node {
        node: NodeRef { element, kind },
        point,
    }
}

fn closing_endpoints(elements: &[PathEl]) -> BTreeSet<usize> {
    let mut endpoints = BTreeSet::new();
    for (index, element) in elements.iter().enumerate() {
        if matches!(element, PathEl::MoveTo(_)) {
            if let Some(end) = closing_end_for_start(elements, index) {
                endpoints.insert(end);
            }
        }
    }
    endpoints
}

/// The element index of the last segment endpoint when a closed subpath ends
/// exactly where its `MoveTo` started.
fn closing_end_for_start(elements: &[PathEl], start: usize) -> Option<usize> {
    let PathEl::MoveTo(start_point) = elements.get(start)? else {
        return None;
    };
    let mut index = start + 1;
    while let Some(element) = elements.get(index) {
        match element {
            PathEl::MoveTo(_) => return None,
            PathEl::ClosePath => {
                if index <= start + 1 {
                    return None;
                }
                let end = index - 1;
                let (PathEl::LineTo(point)
                | PathEl::QuadTo(_, point)
                | PathEl::CurveTo(_, _, point)) = elements.get(end)?
                else {
                    return None;
                };
                return same_point(*point, *start_point).then_some(end);
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn same_point(a: Point, b: Point) -> bool {
    (a - b).hypot() <= 1e-6
}

/// A point on a path segment returned by [`nearest_segment`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentHit {
    /// Index of the path element whose endpoint (or `ClosePath`) ends the
    /// segment. This is the element that [`insert_node`] splits.
    pub element: usize,
    /// Parameter along the segment, in `0..=1`.
    pub t: f64,
    pub point: Point,
    pub distance: f64,
}

/// Position of one anchor or control, if the node still exists.
pub fn node_position(path: &BezPath, node: NodeRef) -> Option<Point> {
    let element = path.elements().get(node.element)?;
    match (element, node.kind) {
        (PathEl::MoveTo(point) | PathEl::LineTo(point), NodeKind::Anchor)
        | (PathEl::QuadTo(_, point), NodeKind::Anchor)
        | (PathEl::CurveTo(_, _, point), NodeKind::Anchor) => Some(*point),
        (PathEl::QuadTo(control, _), NodeKind::Control1)
        | (PathEl::CurveTo(control, _, _), NodeKind::Control1) => Some(*control),
        (PathEl::CurveTo(_, control, _), NodeKind::Control2) => Some(*control),
        _ => None,
    }
}

/// Find the segment nearest to `point`, including an implicit closing line.
pub fn nearest_segment(path: &BezPath, point: Point) -> Option<SegmentHit> {
    let elements = path.elements();
    let mut current = None;
    let mut subpath_start = None;
    let mut best = None;

    for (element, path_el) in elements.iter().enumerate() {
        match *path_el {
            PathEl::MoveTo(start) => {
                current = Some(start);
                subpath_start = Some(start);
            }
            PathEl::LineTo(end) => {
                if let Some(start) = current {
                    consider_segment(
                        element,
                        PathSeg::Line(Line::new(start, end)),
                        point,
                        &mut best,
                    );
                }
                current = Some(end);
            }
            PathEl::QuadTo(control, end) => {
                if let Some(start) = current {
                    consider_segment(
                        element,
                        PathSeg::Quad(kurbo::QuadBez::new(start, control, end)),
                        point,
                        &mut best,
                    );
                }
                current = Some(end);
            }
            PathEl::CurveTo(control1, control2, end) => {
                if let Some(start) = current {
                    consider_segment(
                        element,
                        PathSeg::Cubic(kurbo::CubicBez::new(start, control1, control2, end)),
                        point,
                        &mut best,
                    );
                }
                current = Some(end);
            }
            PathEl::ClosePath => {
                if let (Some(start), Some(end)) = (current, subpath_start) {
                    if !same_point(start, end) {
                        consider_segment(
                            element,
                            PathSeg::Line(Line::new(start, end)),
                            point,
                            &mut best,
                        );
                    }
                }
                current = subpath_start;
            }
        }
    }
    best
}

/// Split the segment ending at `element` at parameter `t`, adding an anchor.
///
/// For a normal line, quadratic or cubic segment the node is placed at the
/// segment point and the element is split in two. For a `ClosePath` an explicit
/// line to the new point is inserted before the close. Returns a reference to
/// the new anchor.
pub fn insert_node(path: &mut BezPath, element: usize, t: f64) -> Option<NodeRef> {
    if !(t > 1e-6 && t < 1.0 - 1e-6) {
        return None;
    }
    let (segment, closing) = segment_at(path.elements(), element)?;
    let mut elements = path.elements().to_vec();
    if closing {
        let point = segment.eval(t);
        elements.insert(element, PathEl::LineTo(point));
    } else {
        let first = segment.subsegment(0.0..t);
        let second = segment.subsegment(t..1.0);
        elements[element] = path_el(first);
        elements.insert(element + 1, path_el(second));
    }
    *path = BezPath::from_vec(elements);
    Some(NodeRef {
        element,
        kind: NodeKind::Anchor,
    })
}

/// Delete an anchor and its incoming segment.
///
/// Deleting a subpath's `MoveTo` anchor rotates the start to the next anchor
/// and removes the first outgoing segment (and, for a closed contour that ends
/// at the start, its final incoming segment). Empty subpaths are removed.
pub fn delete_anchor(path: &mut BezPath, node: NodeRef) -> bool {
    if node.kind != NodeKind::Anchor {
        return false;
    }
    let elements = path.elements();
    let Some(path_el) = elements.get(node.element) else {
        return false;
    };
    if matches!(path_el, PathEl::ClosePath) {
        return false;
    }

    let modified = if matches!(path_el, PathEl::MoveTo(_)) {
        delete_start(elements, node.element)
    } else {
        let mut elements = elements.to_vec();
        elements.remove(node.element);
        Some(elements)
    };
    let Some(modified) = modified else {
        return false;
    };
    let cleaned = remove_empty_subpaths(modified);
    if !cleaned.iter().any(|element| {
        matches!(
            element,
            PathEl::LineTo(_) | PathEl::QuadTo(..) | PathEl::CurveTo(..)
        )
    }) {
        return false;
    }
    *path = BezPath::from_vec(cleaned);
    true
}

/// Break a path at an anchor. A closed contour opens at the anchor; an interior
/// anchor of an open contour splits it into two subpaths. Endpoint anchors of
/// open contours are already loose ends, so they cannot be broken.
pub fn break_path(path: &mut BezPath, node: NodeRef) -> bool {
    if node.kind != NodeKind::Anchor {
        return false;
    }
    let elements = path.elements();
    let Some(path_el) = elements.get(node.element) else {
        return false;
    };
    if matches!(path_el, PathEl::ClosePath) {
        return false;
    }
    let Some((start, end)) = subpath_range(elements, node.element) else {
        return false;
    };
    let closed = matches!(elements.get(end - 1), Some(PathEl::ClosePath));
    let content_end = if closed { end - 1 } else { end };
    if content_end <= start + 1 {
        return false;
    }

    let mut updated = elements.to_vec();
    if closed {
        let PathEl::MoveTo(start_point) = elements[start] else {
            return false;
        };
        let Some(break_point) = endpoint(&elements[node.element]) else {
            return false;
        };
        // Rebuild the loop starting at the broken anchor, keeping the implicit
        // closing segment as an explicit line and dropping ClosePath. The anchor
        // becomes both ends of the new open contour.
        let mut subpath = Vec::with_capacity(content_end - start + 2);
        subpath.push(PathEl::MoveTo(break_point));
        subpath.extend_from_slice(&elements[node.element + 1..content_end]);
        let previous = endpoint(elements.get(content_end - 1).unwrap_or(&elements[start]))
            .unwrap_or(start_point);
        if !same_point(previous, start_point) {
            subpath.push(PathEl::LineTo(start_point));
        }
        if node.element > start {
            subpath.extend_from_slice(&elements[start + 1..=node.element]);
        }
        updated.splice(start..end, subpath);
    } else {
        let last = content_end - 1;
        if node.element <= start || node.element >= last {
            return false;
        }
        let Some(break_point) = endpoint(&elements[node.element]) else {
            return false;
        };
        updated.insert(node.element + 1, PathEl::MoveTo(break_point));
    }
    *path = BezPath::from_vec(updated);
    true
}

#[derive(Clone, Copy, Debug)]
struct OpenSubpath {
    start: usize,
    end: usize,
    start_node: NodeRef,
    end_node: NodeRef,
    start_point: Point,
    end_point: Point,
}

/// The open subpath containing `element`, with its two endpoint anchors.
fn open_subpath(elements: &[PathEl], element: usize) -> Option<OpenSubpath> {
    let (start, end) = subpath_range(elements, element)?;
    if matches!(elements.get(end - 1), Some(PathEl::ClosePath)) {
        return None;
    }
    let PathEl::MoveTo(start_point) = *elements.get(start)? else {
        return None;
    };
    let end_point = endpoint(elements.get(end - 1)?)?;
    Some(OpenSubpath {
        start,
        end,
        start_node: NodeRef {
            element: start,
            kind: NodeKind::Anchor,
        },
        end_node: NodeRef {
            element: end - 1,
            kind: NodeKind::Anchor,
        },
        start_point,
        end_point,
    })
}

/// Join two endpoint anchors of open contours. Two ends of the same contour
/// close it; endpoints of different contours are merged, adding a straight line
/// when they do not coincide.
pub fn join_nodes(path: &mut BezPath, a: NodeRef, b: NodeRef) -> bool {
    if a.kind != NodeKind::Anchor || b.kind != NodeKind::Anchor || a == b {
        return false;
    }
    let elements = path.elements();
    let Some(info_a) = open_subpath(elements, a.element) else {
        return false;
    };
    let Some(info_b) = open_subpath(elements, b.element) else {
        return false;
    };
    let a_endpoint = a == info_a.start_node || a == info_a.end_node;
    let b_endpoint = b == info_b.start_node || b == info_b.end_node;
    if !a_endpoint || !b_endpoint {
        return false;
    }

    if info_a.start == info_b.start {
        let ends = (a == info_a.start_node && b == info_a.end_node)
            || (a == info_a.end_node && b == info_a.start_node);
        if !ends {
            return false;
        }
        let mut updated = elements.to_vec();
        let mut insert_at = info_a.end;
        if !same_point(info_a.end_point, info_a.start_point) {
            updated.insert(insert_at, PathEl::LineTo(info_a.start_point));
            insert_at += 1;
        }
        updated.insert(insert_at, PathEl::ClosePath);
        *path = BezPath::from_vec(updated);
        return true;
    }

    // Keep the earlier subpath in place and orient both so the join happens at
    // the end of the left one and the start of the right one.
    let (left, left_node, right, right_node) = if info_a.start < info_b.start {
        (info_a, a, info_b, b)
    } else {
        (info_b, b, info_a, a)
    };
    let Some(left_path) = open_subpath_oriented(elements, &left, left_node, true) else {
        return false;
    };
    let Some(right_path) = open_subpath_oriented(elements, &right, right_node, false) else {
        return false;
    };
    if left_path.len() < 2 || right_path.len() < 2 {
        return false;
    }
    let Some(left_end) = left_path.last().and_then(endpoint) else {
        return false;
    };
    let Some(right_start) = right_path.first().and_then(|element| match element {
        PathEl::MoveTo(point) => Some(*point),
        _ => None,
    }) else {
        return false;
    };

    let mut merged = Vec::new();
    merged.extend_from_slice(&elements[..left.start]);
    merged.extend(left_path);
    if !same_point(left_end, right_start) {
        merged.push(PathEl::LineTo(right_start));
    }
    merged.extend_from_slice(&right_path[1..]);
    merged.extend_from_slice(&elements[left.end..right.start]);
    merged.extend_from_slice(&elements[right.end..]);
    *path = BezPath::from_vec(merged);
    true
}

/// Copy one open subpath, optionally reversing it, so that `selected` is its
/// last anchor (`at_end`) or its first anchor.
fn open_subpath_oriented(
    elements: &[PathEl],
    info: &OpenSubpath,
    selected: NodeRef,
    at_end: bool,
) -> Option<Vec<PathEl>> {
    let selected_end = selected == info.end_node;
    let selected_start = selected == info.start_node;
    if at_end {
        if selected_end {
            Some(elements[info.start..info.end].to_vec())
        } else if selected_start {
            reverse_open_subpath(elements, info.start, info.end)
        } else {
            None
        }
    } else if selected_start {
        Some(elements[info.start..info.end].to_vec())
    } else if selected_end {
        reverse_open_subpath(elements, info.start, info.end)
    } else {
        None
    }
}

/// Reverse an open subpath. Control points keep their positions; cubic controls
/// swap roles because the curve is traversed the other way.
fn reverse_open_subpath(elements: &[PathEl], start: usize, end: usize) -> Option<Vec<PathEl>> {
    let PathEl::MoveTo(first) = *elements.get(start)? else {
        return None;
    };
    let count = end.checked_sub(start + 1)?;
    if count == 0 {
        return Some(vec![PathEl::MoveTo(first)]);
    }
    let mut starts = Vec::with_capacity(count);
    let mut current = first;
    for element in &elements[start + 1..end] {
        starts.push(current);
        current = endpoint(element)?;
    }
    let mut reversed = Vec::with_capacity(end - start);
    reversed.push(PathEl::MoveTo(current));
    for index in (0..count).rev() {
        let previous = starts[index];
        reversed.push(match elements[start + 1 + index] {
            PathEl::LineTo(_) => PathEl::LineTo(previous),
            PathEl::QuadTo(control, _) => PathEl::QuadTo(control, previous),
            PathEl::CurveTo(control1, control2, _) => PathEl::CurveTo(control2, control1, previous),
            _ => return None,
        });
    }
    Some(reversed)
}

/// The half-open element range of the subpath that contains `element`.
fn subpath_range(elements: &[PathEl], element: usize) -> Option<(usize, usize)> {
    if element >= elements.len() {
        return None;
    }
    let start = elements[..=element]
        .iter()
        .rposition(|element| matches!(element, PathEl::MoveTo(_)))?;
    let end = elements[start + 1..]
        .iter()
        .position(|element| matches!(element, PathEl::MoveTo(_)))
        .map(|offset| start + 1 + offset)
        .unwrap_or(elements.len());
    Some((start, end))
}

fn consider_segment(element: usize, segment: PathSeg, point: Point, best: &mut Option<SegmentHit>) {
    let nearest = segment.nearest(point, 1e-6);
    let distance = nearest.distance_sq.sqrt();
    if best
        .as_ref()
        .is_none_or(|current| distance < current.distance)
    {
        *best = Some(SegmentHit {
            element,
            t: nearest.t,
            point: segment.eval(nearest.t),
            distance,
        });
    }
}

/// The segment ending at `element`, and whether it is an implicit close.
fn segment_at(elements: &[PathEl], target: usize) -> Option<(PathSeg, bool)> {
    let mut current = None;
    let mut subpath_start = None;
    for (element, path_el) in elements.iter().enumerate() {
        match *path_el {
            PathEl::MoveTo(start) => {
                current = Some(start);
                subpath_start = Some(start);
            }
            PathEl::LineTo(end) => {
                let start = current?;
                if element == target {
                    return Some((PathSeg::Line(Line::new(start, end)), false));
                }
                current = Some(end);
            }
            PathEl::QuadTo(control, end) => {
                let start = current?;
                if element == target {
                    return Some((
                        PathSeg::Quad(kurbo::QuadBez::new(start, control, end)),
                        false,
                    ));
                }
                current = Some(end);
            }
            PathEl::CurveTo(control1, control2, end) => {
                let start = current?;
                if element == target {
                    return Some((
                        PathSeg::Cubic(kurbo::CubicBez::new(start, control1, control2, end)),
                        false,
                    ));
                }
                current = Some(end);
            }
            PathEl::ClosePath => {
                let start = current?;
                let end = subpath_start?;
                if element == target {
                    if same_point(start, end) {
                        return None;
                    }
                    return Some((PathSeg::Line(Line::new(start, end)), true));
                }
                current = subpath_start;
            }
        }
    }
    None
}

fn path_el(segment: PathSeg) -> PathEl {
    match segment {
        PathSeg::Line(line) => PathEl::LineTo(line.p1),
        PathSeg::Quad(quad) => PathEl::QuadTo(quad.p1, quad.p2),
        PathSeg::Cubic(cubic) => PathEl::CurveTo(cubic.p1, cubic.p2, cubic.p3),
    }
}

fn delete_start(elements: &[PathEl], start: usize) -> Option<Vec<PathEl>> {
    let PathEl::MoveTo(start_point) = elements.get(start)? else {
        return None;
    };
    let subpath_end = elements[start + 1..]
        .iter()
        .position(|element| matches!(element, PathEl::MoveTo(_)))
        .map(|offset| start + 1 + offset)
        .unwrap_or(elements.len());
    let closed = matches!(elements.get(subpath_end - 1), Some(PathEl::ClosePath));
    let content_end = if closed { subpath_end - 1 } else { subpath_end };
    let first = start + 1;
    if first >= content_end {
        return Some(remove_subpath(elements, start, subpath_end));
    }

    let first_end = endpoint(elements.get(first)?)?;
    let last = content_end - 1;
    let merge_last = closed && same_point(endpoint(elements.get(last)?)?, *start_point);

    let mut subpath = Vec::with_capacity(content_end - start + 1);
    subpath.push(PathEl::MoveTo(first_end));
    for (index, element) in elements.iter().enumerate().take(content_end).skip(first) {
        if index == first || (merge_last && index == last) {
            continue;
        }
        subpath.push(*element);
    }
    if closed {
        subpath.push(PathEl::ClosePath);
    }
    if !subpath.iter().any(|element| {
        matches!(
            element,
            PathEl::LineTo(_) | PathEl::QuadTo(..) | PathEl::CurveTo(..)
        )
    }) {
        return Some(remove_subpath(elements, start, subpath_end));
    }

    let mut result = Vec::with_capacity(elements.len() - (subpath_end - start) + subpath.len());
    result.extend_from_slice(&elements[..start]);
    result.extend(subpath);
    result.extend_from_slice(&elements[subpath_end..]);
    Some(result)
}

fn remove_subpath(elements: &[PathEl], start: usize, end: usize) -> Vec<PathEl> {
    let mut result = Vec::with_capacity(elements.len() - (end - start));
    result.extend_from_slice(&elements[..start]);
    result.extend_from_slice(&elements[end..]);
    result
}

fn remove_empty_subpaths(elements: Vec<PathEl>) -> Vec<PathEl> {
    let mut result = Vec::with_capacity(elements.len());
    let mut index = 0;
    while index < elements.len() {
        if !matches!(elements[index], PathEl::MoveTo(_)) {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        let mut drawing = false;
        while index < elements.len() && !matches!(elements[index], PathEl::MoveTo(_)) {
            drawing |= matches!(
                elements[index],
                PathEl::LineTo(_) | PathEl::QuadTo(..) | PathEl::CurveTo(..)
            );
            index += 1;
        }
        if drawing {
            result.extend_from_slice(&elements[start..index]);
        }
    }
    result
}

fn endpoint(element: &PathEl) -> Option<Point> {
    match *element {
        PathEl::MoveTo(point) | PathEl::LineTo(point) => Some(point),
        PathEl::QuadTo(_, point) | PathEl::CurveTo(_, _, point) => Some(point),
        PathEl::ClosePath => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Vec2;

    fn vec2(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn sample() -> BezPath {
        BezPath::from_svg("M0,0 L10,0 Q15,5 20,0 C25,-5 30,5 35,0 Z").expect("path")
    }

    #[test]
    fn lists_anchors_and_controls() {
        let nodes = nodes(&sample());
        assert_eq!(nodes.len(), 7);
        assert_eq!(nodes[0].node.element, 0);
        assert_eq!(nodes[0].node.kind, NodeKind::Anchor);
        assert_eq!(nodes[0].point, Point::new(0.0, 0.0));
        assert_eq!(nodes[1].node.element, 1);
        assert_eq!(nodes[2].node.element, 2);
        assert_eq!(nodes[2].node.kind, NodeKind::Control1);
        assert_eq!(nodes[2].point, Point::new(15.0, 5.0));
        assert_eq!(nodes[3].node.element, 2);
        assert_eq!(nodes[3].node.kind, NodeKind::Anchor);
        assert_eq!(nodes[4].node.element, 3);
        assert_eq!(nodes[4].node.kind, NodeKind::Control1);
        assert_eq!(nodes[5].node.kind, NodeKind::Control2);
        assert_eq!(nodes[6].point, Point::new(35.0, 0.0));
    }

    #[test]
    fn merged_closed_anchor_keeps_the_contour_closed() {
        let mut path = BezPath::from_svg("M0,0 C0,10 10,10 0,0 Z").expect("path");
        let nodes = nodes(&path);
        assert_eq!(
            nodes.iter().filter(|node| node.is_anchor()).count(),
            1,
            "the final endpoint duplicates the MoveTo anchor"
        );
        assert_eq!(nodes.len(), 3);

        assert!(move_node(
            &mut path,
            NodeRef {
                element: 0,
                kind: NodeKind::Anchor,
            },
            vec2(1.0, 2.0)
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(1.0, 2.0)),
                PathEl::CurveTo(
                    Point::new(1.0, 12.0),
                    Point::new(11.0, 12.0),
                    Point::new(1.0, 2.0)
                ),
                PathEl::ClosePath,
            ]
        );
    }

    #[test]
    fn closed_single_quad_moves_its_shared_control_once() {
        let mut path = BezPath::from_svg("M0,0 Q5,5 0,0 Z").expect("path");
        assert!(move_node(
            &mut path,
            NodeRef {
                element: 0,
                kind: NodeKind::Anchor,
            },
            vec2(1.0, 1.0)
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(1.0, 1.0)),
                PathEl::QuadTo(Point::new(6.0, 6.0), Point::new(1.0, 1.0)),
                PathEl::ClosePath,
            ]
        );
    }

    #[test]
    fn moving_an_anchor_moves_its_attached_controls() {
        let mut path = sample();
        assert!(move_node(
            &mut path,
            NodeRef {
                element: 2,
                kind: NodeKind::Anchor,
            },
            vec2(1.0, 2.0)
        ));
        assert_eq!(
            path.elements()[2],
            PathEl::QuadTo(Point::new(16.0, 7.0), Point::new(21.0, 2.0))
        );
        assert_eq!(
            path.elements()[3],
            PathEl::CurveTo(
                Point::new(26.0, -3.0),
                Point::new(30.0, 5.0),
                Point::new(35.0, 0.0)
            )
        );
    }

    #[test]
    fn moving_a_control_touches_only_that_control() {
        let mut path = sample();
        assert!(move_node(
            &mut path,
            NodeRef {
                element: 3,
                kind: NodeKind::Control2,
            },
            vec2(1.0, 1.0)
        ));
        assert_eq!(
            path.elements()[3],
            PathEl::CurveTo(
                Point::new(25.0, -5.0),
                Point::new(31.0, 6.0),
                Point::new(35.0, 0.0)
            )
        );
        assert_eq!(
            path.elements()[2],
            PathEl::QuadTo(Point::new(15.0, 5.0), Point::new(20.0, 0.0))
        );
    }

    #[test]
    fn nearest_segment_finds_lines_and_curves() {
        let path = BezPath::from_svg("M0,0 L10,0 Q20,10 30,0").expect("path");
        let line = nearest_segment(&path, Point::new(5.0, 2.0)).expect("line");
        assert_eq!(line.element, 1);
        assert!((line.t - 0.5).abs() < 1e-6);
        assert!((line.point.x - 5.0).abs() < 1e-6);
        assert!((line.point.y).abs() < 1e-6);
        assert!((line.distance - 2.0).abs() < 1e-6);

        let curve = nearest_segment(&path, Point::new(20.0, 4.6)).expect("curve");
        assert_eq!(curve.element, 2);
        assert!((curve.point.x - 20.0).abs() < 1e-3);
        assert!((curve.point.y - 5.0).abs() < 1e-3);
    }

    #[test]
    fn inserting_a_node_splits_lines_and_curves() {
        let mut line = BezPath::from_svg("M0,0 L10,0").expect("path");
        let node = insert_node(&mut line, 1, 0.5).expect("insert");
        assert_eq!(
            node,
            NodeRef {
                element: 1,
                kind: NodeKind::Anchor
            }
        );
        assert_eq!(
            line.elements(),
            &[
                PathEl::MoveTo(Point::new(0.0, 0.0)),
                PathEl::LineTo(Point::new(5.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 0.0)),
            ]
        );

        let mut quad = BezPath::from_svg("M0,0 Q10,10 20,0").expect("path");
        assert!(insert_node(&mut quad, 1, 0.5).is_some());
        assert_eq!(
            quad.elements(),
            &[
                PathEl::MoveTo(Point::new(0.0, 0.0)),
                PathEl::QuadTo(Point::new(5.0, 5.0), Point::new(10.0, 5.0)),
                PathEl::QuadTo(Point::new(15.0, 5.0), Point::new(20.0, 0.0)),
            ]
        );

        let mut cubic = BezPath::from_svg("M0,0 C0,10 10,10 10,0").expect("path");
        assert!(insert_node(&mut cubic, 1, 0.5).is_some());
        let elements = cubic.elements();
        assert_eq!(elements.len(), 3);
        assert_eq!(
            elements[1],
            PathEl::CurveTo(
                Point::new(0.0, 5.0),
                Point::new(2.5, 7.5),
                Point::new(5.0, 7.5)
            )
        );
        assert_eq!(
            elements[2],
            PathEl::CurveTo(
                Point::new(7.5, 7.5),
                Point::new(10.0, 5.0),
                Point::new(10.0, 0.0)
            )
        );
    }

    #[test]
    fn inserting_on_a_closing_line_adds_an_explicit_anchor() {
        let mut path = BezPath::from_svg("M0,0 L10,0 L10,10 Z").expect("path");
        let node = insert_node(&mut path, 3, 0.5).expect("insert");
        assert_eq!(
            node,
            NodeRef {
                element: 3,
                kind: NodeKind::Anchor
            }
        );
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(0.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 10.0)),
                PathEl::LineTo(Point::new(5.0, 5.0)),
                PathEl::ClosePath,
            ]
        );
    }

    #[test]
    fn deleting_an_anchor_removes_its_incoming_segment() {
        let mut path = BezPath::from_svg("M0,0 L10,0 L10,10 L0,10 Z").expect("path");
        assert!(delete_anchor(
            &mut path,
            NodeRef {
                element: 1,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(0.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 10.0)),
                PathEl::LineTo(Point::new(0.0, 10.0)),
                PathEl::ClosePath,
            ]
        );
    }

    #[test]
    fn nearest_segment_includes_the_closing_line() {
        let path = BezPath::from_svg("M0,0 L10,0 L10,10 Z").expect("path");
        let hit = nearest_segment(&path, Point::new(5.0, 5.0)).expect("closing line");
        assert_eq!(hit.element, 3);
        assert!((hit.t - 0.5).abs() < 1e-6);
        assert!((hit.point - Point::new(5.0, 5.0)).hypot() < 1e-6);
    }

    #[test]
    fn deleting_removes_an_empty_subpath() {
        let mut path = BezPath::from_svg("M0,0 L10,0 M20,0 L30,0 L40,0").expect("path");
        assert!(delete_anchor(
            &mut path,
            NodeRef {
                element: 1,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(20.0, 0.0)),
                PathEl::LineTo(Point::new(30.0, 0.0)),
                PathEl::LineTo(Point::new(40.0, 0.0)),
            ]
        );
    }

    #[test]
    fn deleting_a_closed_start_rotates_to_the_next_anchor() {
        let mut path = BezPath::from_svg("M0,0 L10,0 L10,10 L0,0 Z").expect("path");
        assert!(delete_anchor(
            &mut path,
            NodeRef {
                element: 0,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(10.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 10.0)),
                PathEl::ClosePath,
            ]
        );
    }

    #[test]
    fn deleting_an_open_start_rotates_to_the_next_anchor() {
        let mut path = BezPath::from_svg("M0,0 L10,0 L20,0").expect("path");
        assert!(delete_anchor(
            &mut path,
            NodeRef {
                element: 0,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(10.0, 0.0)),
                PathEl::LineTo(Point::new(20.0, 0.0)),
            ]
        );
    }

    #[test]
    fn refuses_to_delete_the_last_geometry() {
        let mut path = BezPath::from_svg("M0,0 L10,0").expect("path");
        assert!(!delete_anchor(
            &mut path,
            NodeRef {
                element: 0,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(path.elements().len(), 2);
    }

    #[test]
    fn breaking_a_closed_contour_opens_at_the_anchor() {
        let mut path = BezPath::from_svg("M0,0 L10,0 L10,10 L0,10 Z").expect("path");
        assert!(break_path(
            &mut path,
            NodeRef {
                element: 1,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(10.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 10.0)),
                PathEl::LineTo(Point::new(0.0, 10.0)),
                PathEl::LineTo(Point::new(0.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 0.0)),
            ]
        );
    }

    #[test]
    fn breaking_at_the_start_keeps_the_closing_segment() {
        let mut path = BezPath::from_svg("M0,0 L10,0 L10,10 Z").expect("path");
        assert!(break_path(
            &mut path,
            NodeRef {
                element: 0,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(0.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 10.0)),
                PathEl::LineTo(Point::new(0.0, 0.0)),
            ]
        );
    }

    #[test]
    fn breaking_an_interior_anchor_splits_an_open_contour() {
        let mut path = BezPath::from_svg("M0,0 L10,0 L20,0 L30,0").expect("path");
        assert!(break_path(
            &mut path,
            NodeRef {
                element: 2,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(0.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 0.0)),
                PathEl::LineTo(Point::new(20.0, 0.0)),
                PathEl::MoveTo(Point::new(20.0, 0.0)),
                PathEl::LineTo(Point::new(30.0, 0.0)),
            ]
        );
    }

    #[test]
    fn breaking_an_endpoint_of_an_open_contour_is_rejected() {
        let mut path = BezPath::from_svg("M0,0 L10,0").expect("path");
        assert!(!break_path(
            &mut path,
            NodeRef {
                element: 0,
                kind: NodeKind::Anchor
            }
        ));
        assert!(!break_path(
            &mut path,
            NodeRef {
                element: 1,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(path.elements().len(), 2);
    }

    #[test]
    fn joining_the_ends_of_an_open_contour_closes_it() {
        let mut path = BezPath::from_svg("M0,0 L10,0 L10,10").expect("path");
        assert!(join_nodes(
            &mut path,
            NodeRef {
                element: 0,
                kind: NodeKind::Anchor
            },
            NodeRef {
                element: 2,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(0.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 10.0)),
                PathEl::LineTo(Point::new(0.0, 0.0)),
                PathEl::ClosePath,
            ]
        );
    }

    #[test]
    fn joining_two_open_contours_adds_a_connecting_line() {
        let mut path = BezPath::from_svg("M0,0 L10,0 M20,0 L30,0").expect("path");
        assert!(join_nodes(
            &mut path,
            NodeRef {
                element: 1,
                kind: NodeKind::Anchor
            },
            NodeRef {
                element: 2,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(0.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 0.0)),
                PathEl::LineTo(Point::new(20.0, 0.0)),
                PathEl::LineTo(Point::new(30.0, 0.0)),
            ]
        );
    }

    #[test]
    fn joining_reversed_endpoints_swaps_cubic_controls() {
        let mut path = BezPath::from_svg("M0,0 C0,10 10,10 10,0 M20,0 L30,0").expect("path");
        assert!(join_nodes(
            &mut path,
            NodeRef {
                element: 0,
                kind: NodeKind::Anchor
            },
            NodeRef {
                element: 2,
                kind: NodeKind::Anchor
            }
        ));
        assert_eq!(
            path.elements(),
            &[
                PathEl::MoveTo(Point::new(10.0, 0.0)),
                PathEl::CurveTo(
                    Point::new(10.0, 10.0),
                    Point::new(0.0, 10.0),
                    Point::new(0.0, 0.0)
                ),
                PathEl::LineTo(Point::new(20.0, 0.0)),
                PathEl::LineTo(Point::new(30.0, 0.0)),
            ]
        );
    }

    #[test]
    fn joining_rejects_interior_and_closed_nodes() {
        let mut open = BezPath::from_svg("M0,0 L10,0 L20,0 L30,0").expect("path");
        assert!(!join_nodes(
            &mut open,
            NodeRef {
                element: 1,
                kind: NodeKind::Anchor
            },
            NodeRef {
                element: 2,
                kind: NodeKind::Anchor
            }
        ));
        let mut closed = BezPath::from_svg("M0,0 L10,0 L0,0 Z").expect("path");
        assert!(!join_nodes(
            &mut closed,
            NodeRef {
                element: 0,
                kind: NodeKind::Anchor
            },
            NodeRef {
                element: 1,
                kind: NodeKind::Anchor
            }
        ));
    }

    #[test]
    fn control_links_follow_each_curve() {
        let links = control_links(&sample());
        assert_eq!(links.len(), 4);
        assert_eq!(links[0], (Point::new(10.0, 0.0), Point::new(15.0, 5.0)));
        assert_eq!(links[1], (Point::new(20.0, 0.0), Point::new(15.0, 5.0)));
        assert_eq!(links[2], (Point::new(20.0, 0.0), Point::new(25.0, -5.0)));
        assert_eq!(links[3], (Point::new(35.0, 0.0), Point::new(30.0, 5.0)));
    }
}
