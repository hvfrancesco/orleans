//! Editing helpers for the anchors and Bézier controls of a path.
//!
//! A node refers to one element of the [`BezPath`] plus the field inside that
//! element. On-curve anchors are the endpoints of `MoveTo`, `LineTo`, `QuadTo`
//! and `CurveTo`; controls are the extra points of quadratic and cubic
//! segments. A closed subpath whose final endpoint coincides with its `MoveTo`
//! point is treated as a single anchor, so dragging that anchor keeps the
//! contour closed.

use std::collections::BTreeSet;

use kurbo::{BezPath, PathEl, Point, Vec2};

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
    fn control_links_follow_each_curve() {
        let links = control_links(&sample());
        assert_eq!(links.len(), 4);
        assert_eq!(links[0], (Point::new(10.0, 0.0), Point::new(15.0, 5.0)));
        assert_eq!(links[1], (Point::new(20.0, 0.0), Point::new(15.0, 5.0)));
        assert_eq!(links[2], (Point::new(20.0, 0.0), Point::new(25.0, -5.0)));
        assert_eq!(links[3], (Point::new(35.0, 0.0), Point::new(30.0, 5.0)));
    }
}
