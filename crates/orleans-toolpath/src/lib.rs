//! Toolpath planning: turns a [`Document`] into an ordered [`Job`].
//!
//! The planner is where the "CAM" part lives:
//!
//! * flatten curves to polylines,
//! * apply kerf compensation towards the material,
//! * cut inner contours before outer ones (so parts don't shift),
//! * order contours with a greedy nearest-neighbour pass to cut travel,
//! * repeat each contour for multi-pass layers.

use kurbo::Point;
use orleans_core::{Document, Job, JobStats, LayerMode, Op};
use orleans_geom::{
    dedup, flatten_path, hatch_contours, offset_closed, point_in_polygon, split_tabs, Contour,
    HatchOptions,
};

/// Knobs for [`plan`].
#[derive(Clone, Debug)]
pub struct PlanOptions {
    /// Flattening tolerance in millimetres.
    pub tolerance: f64,
    /// Full kerf width in millimetres. Half of it is applied per side.
    pub kerf: f64,
    /// Rapid (laser-off) speed in mm/min, used for time estimates only.
    pub rapid_speed: f64,
    /// Reorder contours to minimise travel.
    pub optimize: bool,
    /// Where the head is assumed to be when the job starts.
    pub start: Point,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            tolerance: orleans_geom::DEFAULT_TOLERANCE,
            kerf: 0.0,
            rapid_speed: 3000.0,
            optimize: true,
            start: Point::ORIGIN,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("document contains no geometry")]
    Empty,
}

/// Plan every layer of `doc`.
pub fn plan(doc: &Document, opt: &PlanOptions) -> Job {
    let mut job = Job::default();
    let mut pos = opt.start;
    let mut stats = JobStats::default();

    for layer in &doc.layers {
        if !layer.output || layer.is_empty() {
            continue;
        }
        let contours: Vec<Contour> = layer
            .shapes
            .iter()
            .flat_map(|shape| flatten_path(&shape.path, opt.tolerance))
            .filter(|c| c.points.len() >= 2)
            .collect();
        if contours.is_empty() {
            continue;
        }

        if layer.mode == LayerMode::Fill {
            let segments = hatch_contours(
                &contours,
                &HatchOptions {
                    spacing: layer.fill.spacing,
                    angle_deg: layer.fill.angle,
                    overlap: layer.fill.overlap,
                    alternate: layer.fill.alternate,
                },
            );
            for segment in segments {
                if segment.len() < 2 {
                    continue;
                }
                let start = segment[0];
                if (start - pos).hypot() > 1e-6 {
                    stats.travel_mm += (start - pos).hypot();
                    job.ops.push(Op::Rapid { to: start });
                }
                let length = orleans_geom::polyline_length(&segment);
                if length <= 1e-9 {
                    continue;
                }
                for pass in 0..layer.cut.passes.max(1) {
                    if pass > 0 {
                        let last = *segment.last().expect("non-empty");
                        if (last - start).hypot() > 1e-6 {
                            stats.travel_mm += (last - start).hypot();
                            job.ops.push(Op::Rapid { to: start });
                        }
                    }
                    stats.cut_mm += length;
                    stats.seconds += length / layer.cut.speed.max(1.0) * 60.0;
                    job.ops.push(Op::Cut {
                        polyline: segment.clone(),
                        params: layer.cut,
                        layer: layer.index,
                    });
                }
                pos = *segment.last().expect("non-empty");
            }
            continue;
        }

        let depths = nesting_depths(&contours);
        let ordered = order_contours(&contours, &depths, pos, opt.optimize);

        for (contour_idx, points) in ordered {
            let points = apply_kerf(points, depths[contour_idx], opt.kerf);
            if points.len() < 2 {
                continue;
            }
            let parts = if layer.tabs.count > 0
                && contours[contour_idx].closed
                && (layer.tabs.inner || depths[contour_idx].is_multiple_of(2))
            {
                split_tabs(&points, layer.tabs.count, layer.tabs.width)
            } else {
                vec![points]
            };

            for _ in 0..layer.cut.passes.max(1) {
                for part in &parts {
                    if part.len() < 2 {
                        continue;
                    }
                    let start = part[0];
                    if (start - pos).hypot() > 1e-6 {
                        stats.travel_mm += (start - pos).hypot();
                        job.ops.push(Op::Rapid { to: start });
                    }
                    let length = orleans_geom::polyline_length(part);
                    if length <= 1e-9 {
                        continue;
                    }
                    stats.cut_mm += length;
                    stats.seconds += length / layer.cut.speed.max(1.0) * 60.0;
                    job.ops.push(Op::Cut {
                        polyline: part.clone(),
                        params: layer.cut,
                        layer: layer.index,
                    });
                    pos = *part.last().expect("non-empty");
                }
            }
        }
    }

    stats.seconds += stats.travel_mm / opt.rapid_speed.max(1.0) * 60.0;
    job.stats = stats;
    job
}

/// How many other contours contain each contour. 0 = outermost.
pub fn nesting_depths(contours: &[Contour]) -> Vec<usize> {
    let areas: Vec<f64> = contours.iter().map(|c| c.signed_area().abs()).collect();
    (0..contours.len())
        .map(|i| {
            let probe = contours[i].centroid();
            contours
                .iter()
                .enumerate()
                .filter(|(j, other)| {
                    *j != i && areas[*j] > areas[i] && point_in_polygon(probe, &other.points)
                })
                .count()
        })
        .collect()
}

/// Kerf compensation: shrink outer contours (`depth` even) and grow holes
/// (`depth` odd) by half the kerf, so the finished part matches the drawing.
fn apply_kerf(points: Vec<Point>, depth: usize, kerf: f64) -> Vec<Point> {
    if kerf <= 0.0 {
        return points;
    }
    let signed = if depth & 1 == 0 {
        kerf * 0.5
    } else {
        -kerf * 0.5
    };
    let offset = offset_closed(&points, signed, 4.0);
    dedup(&offset, true)
}

/// Greedy ordering: deepest contours first, nearest contour next.
fn order_contours(
    contours: &[Contour],
    depths: &[usize],
    mut pos: Point,
    optimize: bool,
) -> Vec<(usize, Vec<Point>)> {
    let mut remaining: Vec<usize> = (0..contours.len()).collect();
    let mut out = Vec::with_capacity(contours.len());

    while !remaining.is_empty() {
        let max_depth = remaining.iter().map(|&i| depths[i]).max().unwrap_or(0);
        let mut best: Option<(usize, usize, Vec<Point>, f64)> = None;

        for &i in remaining.iter().filter(|&&i| depths[i] == max_depth) {
            let contour = &contours[i];
            let (start_idx, dist) = nearest_start(&contour.points, pos, contour.closed);
            let candidate = rotate(&contour.points, start_idx, contour.closed);
            let better = match &best {
                None => true,
                Some((_, _, _, best_dist)) => dist < *best_dist,
            };
            if better || !optimize {
                best = Some((i, start_idx, candidate, dist));
                if !optimize {
                    break;
                }
            }
        }

        let (idx, _, points, _) = best.expect("remaining is non-empty");
        remaining.retain(|&i| i != idx);
        if let Some(last) = points.last() {
            pos = *last;
        }
        out.push((idx, points));
    }
    out
}

fn nearest_start(points: &[Point], from: Point, closed: bool) -> (usize, f64) {
    let mut best = (0usize, f64::INFINITY);
    for (i, p) in points.iter().enumerate() {
        let d = (*p - from).hypot();
        if d < best.1 {
            best = (i, d);
        }
        // For a closed contour the return segment starts at the last vertex,
        // so the first vertex is a candidate anyway.
        let _ = closed;
    }
    best
}

/// Reorder a polyline to begin at `start`, closing it if needed.
fn rotate(points: &[Point], start: usize, closed: bool) -> Vec<Point> {
    let mut out = Vec::with_capacity(points.len() + 1);
    out.extend_from_slice(&points[start..]);
    out.extend_from_slice(&points[..start]);
    if closed {
        if let Some(first) = out.first().copied() {
            if let Some(last) = out.last() {
                if (first - *last).hypot() > 1e-6 {
                    out.push(first);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::BezPath;
    use orleans_core::{CutParams, Layer, Shape};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((x0, y0));
        p.line_to((x1, y0));
        p.line_to((x1, y1));
        p.line_to((x0, y1));
        p.close_path();
        p
    }

    fn doc_with(shapes: Vec<BezPath>) -> Document {
        let mut doc = Document::new(20.0, 20.0);
        let mut layer = Layer::new(0, "C00", [0, 0, 0], CutParams::new(1000.0, 50.0, 1));
        for (i, p) in shapes.into_iter().enumerate() {
            layer.shapes.push(Shape::new(format!("s{i}"), p, false));
        }
        doc.layers.push(layer);
        doc
    }

    #[test]
    fn cuts_hole_before_outer() {
        let doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0), rect(4.0, 4.0, 6.0, 6.0)]);
        let job = plan(&doc, &PlanOptions::default());

        let cuts: Vec<_> = job.cuts().collect();
        assert_eq!(cuts.len(), 2);
        let first_len = cuts[0].0.len();
        let expected_hole = 4;
        assert_eq!(first_len, expected_hole + 1, "hole should be cut first");

        // The hole is 4x4 and the outer contour 10x10.
        let hole_area = orleans_geom::signed_area(&cuts[0].0[..4]).abs();
        assert!((hole_area - 4.0).abs() < 1e-6);
    }

    #[test]
    fn tabs_split_outer_contours_into_cut_runs() {
        let mut doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        doc.layers[0].tabs = orleans_core::TabParams {
            count: 4,
            width: 2.0,
            inner: false,
        };
        let job = plan(&doc, &PlanOptions::default());
        assert_eq!(job.cuts().count(), 4);
        assert!((job.stats.cut_mm - 32.0).abs() < 1e-6);
        assert!(job.stats.travel_mm > 1.0);
    }

    #[test]
    fn tabs_can_be_enabled_for_inner_contours() {
        let mut doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0), rect(4.0, 4.0, 6.0, 6.0)]);
        doc.layers[0].tabs = orleans_core::TabParams {
            count: 2,
            width: 0.5,
            inner: false,
        };
        assert_eq!(plan(&doc, &PlanOptions::default()).cuts().count(), 3);

        doc.layers[0].tabs.inner = true;
        assert_eq!(plan(&doc, &PlanOptions::default()).cuts().count(), 4);
    }

    #[test]
    fn fill_layers_emit_scan_lines_instead_of_outlines() {
        let mut doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        doc.layers[0].mode = LayerMode::Fill;
        doc.layers[0].fill.spacing = 2.0;
        doc.layers[0].fill.angle = 0.0;
        doc.layers[0].fill.alternate = false;

        let job = plan(&doc, &PlanOptions::default());
        assert_eq!(job.cuts().count(), 5);
        assert!((job.stats.cut_mm - 50.0).abs() < 1e-6);
        for (polyline, _, _) in job.cuts() {
            assert_eq!(polyline.len(), 2);
            assert!((polyline[0].y - polyline[1].y).abs() < 1e-9);
        }
    }

    #[test]
    fn fill_layers_respect_holes_with_opposite_winding() {
        let path =
            BezPath::from_svg("M0,0 L10,0 L10,10 L0,10 Z M2,2 L2,8 L8,8 L8,2 Z").expect("path");
        let mut doc = doc_with(vec![path]);
        doc.layers[0].mode = LayerMode::Fill;
        doc.layers[0].fill.spacing = 2.0;
        doc.layers[0].fill.alternate = false;

        let job = plan(&doc, &PlanOptions::default());
        let middle: Vec<_> = job
            .cuts()
            .filter(|(polyline, _, _)| (polyline[0].y - 5.0).abs() < 1e-6)
            .collect();
        assert_eq!(middle.len(), 2);
        assert!((middle[0].0[0].x - 0.0).abs() < 1e-9);
        assert!((middle[0].0[1].x - 2.0).abs() < 1e-9);
        assert!((middle[1].0[0].x - 8.0).abs() < 1e-9);
        assert!((middle[1].0[1].x - 10.0).abs() < 1e-9);
    }

    #[test]
    fn fill_layers_ignore_open_contours() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        let mut doc = doc_with(vec![path]);
        doc.layers[0].mode = LayerMode::Fill;
        let job = plan(&doc, &PlanOptions::default());
        assert!(job.is_empty());
    }

    #[test]
    fn non_output_layers_are_not_planned() {
        let mut doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        doc.layers[0].output = false;
        let job = plan(&doc, &PlanOptions::default());
        assert!(job.is_empty());
    }

    #[test]
    fn hidden_layers_still_cut_if_output_is_enabled() {
        let mut doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        doc.layers[0].visible = false;
        let job = plan(&doc, &PlanOptions::default());
        assert_eq!(job.cuts().count(), 1);
    }

    #[test]
    fn kerf_shrinks_outer_and_grows_hole() {
        let doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0), rect(4.0, 4.0, 6.0, 6.0)]);
        let opt = PlanOptions {
            kerf: 0.2,
            ..Default::default()
        };
        let job = plan(&doc, &opt);
        let cuts: Vec<_> = job.cuts().collect();

        // A 2x2 hole grows by half the kerf on each side: 2.2^2.
        let hole_area = orleans_geom::signed_area(&cuts[0].0[..4]).abs();
        assert!(
            (hole_area - 2.2 * 2.2).abs() < 1e-3,
            "hole area {hole_area}"
        );

        let outer = &cuts[1].0;
        let outer_area = orleans_geom::signed_area(&outer[..4]).abs();
        assert!(
            (outer_area - 9.8 * 9.8).abs() < 1e-3,
            "outer area {outer_area}"
        );
    }

    #[test]
    fn multiple_passes_repeat_the_contour() {
        let mut doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        doc.layers[0].cut = CutParams::new(1500.0, 40.0, 3);
        let job = plan(&doc, &PlanOptions::default());

        assert_eq!(job.cuts().count(), 3);
        assert!(job.stats.cut_mm > 119.0 && job.stats.cut_mm < 121.0);
    }

    #[test]
    fn closed_contour_starts_and_ends_at_the_same_point() {
        let doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        let job = plan(&doc, &PlanOptions::default());
        let (points, _, _) = job.cuts().next().unwrap();
        assert!((points[0] - *points.last().unwrap()).hypot() < 1e-9);
    }

    #[test]
    fn empty_document_plans_to_nothing() {
        let job = plan(&Document::default(), &PlanOptions::default());
        assert!(job.is_empty());
    }
}
