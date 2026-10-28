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
use orleans_core::{
    CutParams, Document, Job, JobStats, Layer, LayerMode, Op, RasterImage, RasterMode,
};
use orleans_geom::{
    dedup, flatten_path, hatch_contours, offset_closed, point_in_polygon, split_manual_tabs,
    split_tabs, Contour, HatchOptions,
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
        if let Some(raster) = &layer.raster {
            plan_raster(layer, raster, &mut job, &mut stats, &mut pos);
            continue;
        }

        let mut contours = Vec::new();
        let mut origins = Vec::new();
        for shape in &layer.shapes {
            for (contour, path) in flatten_path(&shape.path, opt.tolerance)
                .into_iter()
                .enumerate()
            {
                if path.points.len() < 2 {
                    continue;
                }
                contours.push(path);
                origins.push((shape.id.clone(), contour));
            }
        }
        if contours.is_empty() {
            continue;
        }

        if layer.mode == LayerMode::Fill {
            let mut segments = hatch_contours(
                &contours,
                &HatchOptions {
                    spacing: layer.fill.spacing,
                    angle_deg: layer.fill.angle,
                    overlap: layer.fill.overlap,
                    alternate: layer.fill.alternate,
                },
            );
            if layer.fill.cross {
                segments.extend(hatch_contours(
                    &contours,
                    &HatchOptions {
                        spacing: layer.fill.spacing,
                        angle_deg: layer.fill.angle + 90.0,
                        overlap: layer.fill.overlap,
                        alternate: layer.fill.alternate,
                    },
                ));
            }
            plan_hatch(layer, segments, &mut job, &mut stats, &mut pos);
            if layer.fill.outline {
                plan_contours(
                    layer, &contours, &origins, opt, false, &mut job, &mut stats, &mut pos,
                );
            }
            continue;
        }

        plan_contours(
            layer, &contours, &origins, opt, true, &mut job, &mut stats, &mut pos,
        );
    }

    stats.seconds += stats.travel_mm / opt.rapid_speed.max(1.0) * 60.0;
    job.stats = stats;
    job
}

fn plan_hatch(
    layer: &Layer,
    segments: Vec<Vec<Point>>,
    job: &mut Job,
    stats: &mut JobStats,
    pos: &mut Point,
) {
    for segment in segments {
        if segment.len() < 2 {
            continue;
        }
        let start = segment[0];
        if (start - *pos).hypot() > 1e-6 {
            stats.travel_mm += (start - *pos).hypot();
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
        *pos = *segment.last().expect("non-empty");
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_contours(
    layer: &Layer,
    contours: &[Contour],
    origins: &[(String, usize)],
    opt: &PlanOptions,
    use_tabs: bool,
    job: &mut Job,
    stats: &mut JobStats,
    pos: &mut Point,
) {
    let depths = nesting_depths(contours);
    let ordered = order_contours(contours, &depths, *pos, opt.optimize);

    for (contour_idx, points) in ordered {
        let points = apply_kerf(points, depths[contour_idx], opt.kerf);
        if points.len() < 2 {
            continue;
        }
        let closed = contours[contour_idx].closed;
        let manual_positions = use_tabs
            .then(|| origins.get(contour_idx))
            .flatten()
            .and_then(|(shape, contour)| {
                layer
                    .manual_tabs
                    .iter()
                    .find(|tabs| tabs.shape == *shape && tabs.contour == *contour)
                    .map(|tabs| tabs.positions.as_slice())
            });
        let parts = if use_tabs && layer.tabs.manual {
            match manual_positions {
                Some(positions) if closed && !positions.is_empty() => {
                    split_manual_tabs(&points, positions, layer.tabs.width)
                }
                _ => vec![points],
            }
        } else if use_tabs
            && layer.tabs.count > 0
            && closed
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
                if (start - *pos).hypot() > 1e-6 {
                    stats.travel_mm += (start - *pos).hypot();
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
                *pos = *part.last().expect("non-empty");
            }
        }
    }
}

fn plan_raster(
    layer: &Layer,
    raster: &RasterImage,
    job: &mut Job,
    stats: &mut JobStats,
    pos: &mut Point,
) {
    if raster.width == 0
        || raster.height == 0
        || raster.width_mm <= 0.0
        || raster.height_mm <= 0.0
        || raster.grayscale.len() < (raster.width * raster.height) as usize
    {
        return;
    }
    let interval = if raster.line_interval > 1e-6 {
        raster.line_interval
    } else {
        raster.height_mm / f64::from(raster.height)
    };
    if !interval.is_finite() || interval <= 1e-6 {
        return;
    }
    let rows = ((raster.height_mm / interval).ceil() as usize).min(1_000_000);
    let dither = if raster.mode == RasterMode::Dither {
        Some(dither_map(raster))
    } else {
        None
    };
    let pixel_width = raster.width_mm / f64::from(raster.width);
    let threshold = f32::from(raster.threshold);

    for row_index in 0..rows {
        let y = raster.y + (row_index as f64 + 0.5) * interval;
        if y >= raster.y + raster.height_mm {
            continue;
        }
        let from_bottom = ((y - raster.y) / raster.height_mm * f64::from(raster.height))
            .floor()
            .clamp(0.0, f64::from(raster.height - 1)) as u32;
        let source_row = raster.height - 1 - from_bottom;
        let mut runs: Vec<(u32, u32, Option<CutParams>)> = Vec::new();
        match raster.mode {
            RasterMode::Threshold | RasterMode::Dither => {
                let mut start = None;
                for x in 0..raster.width {
                    let active = if let Some(map) = &dither {
                        map[(source_row * raster.width + x) as usize] != 0
                    } else {
                        darkness(raster, x, source_row) >= threshold
                    };
                    match (active, start) {
                        (true, None) => start = Some(x),
                        (false, Some(first)) => {
                            runs.push((first, x, None));
                            start = None;
                        }
                        _ => {}
                    }
                }
                if let Some(first) = start {
                    runs.push((first, raster.width, None));
                }
            }
            RasterMode::Grayscale => {
                let mut start = None;
                let mut level = 0u8;
                for x in 0..raster.width {
                    let current = grayscale_level(darkness(raster, x, source_row));
                    let active = current > 0;
                    if active {
                        if start.is_none() {
                            start = Some(x);
                            level = current;
                        } else if current != level {
                            runs.push((
                                start.expect("started"),
                                x,
                                Some(power_for_level(layer, level)),
                            ));
                            start = Some(x);
                            level = current;
                        }
                    } else if let Some(first) = start.take() {
                        if level > 0 {
                            runs.push((first, x, Some(power_for_level(layer, level))));
                        }
                    }
                }
                if let Some(first) = start {
                    if level > 0 {
                        runs.push((first, raster.width, Some(power_for_level(layer, level))));
                    }
                }
            }
        }

        if raster.bidirectional && row_index % 2 == 1 {
            runs.reverse();
        }
        for (first, end, params) in runs {
            let mut start = raster_point(raster, raster.x + f64::from(first) * pixel_width, y);
            let mut finish = raster_point(raster, raster.x + f64::from(end) * pixel_width, y);
            if raster.bidirectional && row_index % 2 == 1 {
                std::mem::swap(&mut start, &mut finish);
            }
            let length = (finish - start).hypot();
            if length <= 1e-9 {
                continue;
            }
            let params = params.unwrap_or(layer.cut);
            for _ in 0..layer.cut.passes.max(1) {
                if (*pos - start).hypot() > 1e-6 {
                    stats.travel_mm += (*pos - start).hypot();
                    job.ops.push(Op::Rapid { to: start });
                }
                stats.cut_mm += length;
                stats.seconds += length / layer.cut.speed.max(1.0) * 60.0;
                job.ops.push(Op::Cut {
                    polyline: vec![start, finish],
                    params,
                    layer: layer.index,
                });
                *pos = finish;
            }
        }
    }
}

fn raster_point(raster: &RasterImage, x: f64, y: f64) -> Point {
    let center = raster.center();
    let (sin, cos) = raster.angle.to_radians().sin_cos();
    let dx = x - center.x;
    let dy = y - center.y;
    Point::new(
        center.x + dx * cos - dy * sin,
        center.y + dx * sin + dy * cos,
    )
}

fn darkness(raster: &RasterImage, x: u32, y: u32) -> f32 {
    let value = f32::from(raster.pixel(x, y));
    if raster.invert {
        value
    } else {
        255.0 - value
    }
}

fn grayscale_level(darkness: f32) -> u8 {
    ((darkness.max(0.0) / 255.0 * 16.0).round() as u8).min(16)
}

fn power_for_level(layer: &Layer, level: u8) -> CutParams {
    let mut params = layer.cut;
    params.power = layer.cut.power * f64::from(level) / 16.0;
    params
}

fn dither_map(raster: &RasterImage) -> Vec<u8> {
    let width = raster.width as usize;
    let height = raster.height as usize;
    let mut work: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| darkness(raster, x as u32, y as u32))
        .collect();
    let mut out = vec![0u8; work.len()];
    let threshold = f32::from(raster.threshold);
    let at = |x: isize, y: isize, width: usize, height: usize| -> Option<usize> {
        if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
            Some(y as usize * width + x as usize)
        } else {
            None
        }
    };
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let old = work[index];
            let new = if old >= threshold { 255.0 } else { 0.0 };
            let error = old - new;
            if new > 0.0 {
                out[index] = 255;
            }
            work[index] = new;
            for (dx, dy, weight) in [
                (1isize, 0isize, 7.0f32),
                (-1, 1, 3.0),
                (0, 1, 5.0),
                (1, 1, 1.0),
            ] {
                if let Some(target) = at(x as isize + dx, y as isize + dy, width, height) {
                    work[target] += error * weight / 16.0;
                }
            }
        }
    }
    out
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

    fn raster_doc(mode: RasterMode, grayscale: Vec<u8>) -> Document {
        let mut doc = doc_with(Vec::new());
        doc.layers[0].mode = LayerMode::Raster;
        doc.layers[0].raster = Some(RasterImage {
            width: 4,
            height: 2,
            grayscale,
            x: 0.0,
            y: 0.0,
            width_mm: 4.0,
            height_mm: 2.0,
            angle: 0.0,
            mode,
            threshold: 128,
            invert: false,
            line_interval: 1.0,
            bidirectional: false,
        });
        doc
    }

    #[test]
    fn raster_threshold_emits_active_pixel_runs() {
        let doc = raster_doc(RasterMode::Threshold, vec![0, 255, 255, 0, 255, 0, 0, 255]);
        let job = plan(&doc, &PlanOptions::default());
        let cuts: Vec<_> = job.cuts().collect();
        assert_eq!(cuts.len(), 3);
        assert!((job.stats.cut_mm - 4.0).abs() < 1e-6);
        assert!((cuts[0].0[0].y - 0.5).abs() < 1e-9);
        assert!((cuts[0].0[0].x - 1.0).abs() < 1e-9);
        assert!((cuts[0].0[1].x - 3.0).abs() < 1e-9);
    }

    #[test]
    fn raster_grayscale_uses_power_levels() {
        let doc = raster_doc(
            RasterMode::Grayscale,
            vec![0, 128, 255, 255, 255, 255, 255, 255],
        );
        let job = plan(&doc, &PlanOptions::default());
        let cuts: Vec<_> = job.cuts().collect();
        assert!(!cuts.is_empty());
        assert!(cuts.iter().any(|(_, params, _)| params.power < 50.0));
    }

    #[test]
    fn raster_angle_rotates_scan_lines() {
        let mut doc = raster_doc(RasterMode::Threshold, vec![0, 0, 0, 0, 0, 0, 0, 0]);
        doc.layers[0].raster.as_mut().expect("raster").angle = 90.0;
        let job = plan(&doc, &PlanOptions::default());
        for (polyline, _, _) in job.cuts() {
            assert!((polyline[0].x - polyline[1].x).abs() < 1e-9);
            assert!((polyline[0].y - polyline[1].y).abs() > 0.5);
        }
    }

    #[test]
    fn raster_dither_generates_marks() {
        let doc = raster_doc(
            RasterMode::Dither,
            vec![128, 128, 128, 128, 128, 128, 128, 128],
        );
        let job = plan(&doc, &PlanOptions::default());
        assert!(!job.is_empty());
    }

    #[test]
    fn tabs_split_outer_contours_into_cut_runs() {
        let mut doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        doc.layers[0].tabs = orleans_core::TabParams {
            count: 4,
            width: 2.0,
            inner: false,
            manual: false,
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
            manual: false,
        };
        assert_eq!(plan(&doc, &PlanOptions::default()).cuts().count(), 3);

        doc.layers[0].tabs.inner = true;
        assert_eq!(plan(&doc, &PlanOptions::default()).cuts().count(), 4);
    }

    #[test]
    fn manual_tabs_override_evenly_distributed_tabs() {
        let mut doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        doc.layers[0].tabs = orleans_core::TabParams {
            count: 4,
            width: 2.0,
            inner: false,
            manual: true,
        };
        doc.layers[0].manual_tabs.push(orleans_core::ManualTabs {
            shape: "s0".to_owned(),
            contour: 0,
            positions: vec![0.5],
        });

        let job = plan(&doc, &PlanOptions::default());
        assert_eq!(job.cuts().count(), 2);
        assert!((job.stats.cut_mm - 38.0).abs() < 1e-6);
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
    fn cross_fill_adds_a_perpendicular_pass() {
        let mut doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        doc.layers[0].mode = LayerMode::Fill;
        doc.layers[0].fill.spacing = 2.0;
        doc.layers[0].fill.alternate = false;
        doc.layers[0].fill.cross = true;

        let job = plan(&doc, &PlanOptions::default());
        let cuts: Vec<_> = job.cuts().collect();
        assert_eq!(cuts.len(), 10);
        let horizontal = cuts
            .iter()
            .filter(|(line, _, _)| (line[0].y - line[1].y).abs() < 1e-9)
            .count();
        let vertical = cuts
            .iter()
            .filter(|(line, _, _)| (line[0].x - line[1].x).abs() < 1e-9)
            .count();
        assert_eq!(horizontal, 5);
        assert_eq!(vertical, 5);
    }

    #[test]
    fn fill_outline_traces_the_closed_contour() {
        let mut doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0)]);
        doc.layers[0].mode = LayerMode::Fill;
        doc.layers[0].fill.spacing = 2.0;
        doc.layers[0].fill.alternate = false;
        doc.layers[0].fill.outline = true;

        let job = plan(&doc, &PlanOptions::default());
        let cuts: Vec<_> = job.cuts().collect();
        assert_eq!(cuts.len(), 6);
        let outline = cuts
            .iter()
            .find(|(line, _, _)| line.len() > 2)
            .expect("filled contour");
        assert_eq!(outline.0.first(), outline.0.last());
        assert!((orleans_geom::polyline_length(outline.0) - 40.0).abs() < 1e-6);
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
