//! End-to-end check: SVG bytes in, simulated GRBL motion out.
//!
//! This exercises the exact pipeline the `orleans` binary uses, without touching a
//! serial port, so it is safe to run in CI.

use kurbo::Point;
use orleans_driver::gcode::{self, GcodeOptions};
use orleans_driver::{GrblClient, RunState, SimulatedGrbl};
use orleans_import::{import_svg, ImportOptions};
use orleans_toolpath::{plan, PlanOptions};

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="40mm" viewBox="0 0 60 40">
    <circle cx="30" cy="20" r="18" fill="none" stroke="#000000"/>
    <circle cx="30" cy="20" r="10" fill="none" stroke="#000000"/>
    <circle cx="30" cy="20" r="3" fill="none" stroke="#000000"/>
</svg>"##;

#[test]
fn svg_reaches_the_simulated_controller() {
    let doc = import_svg(SVG.as_bytes(), &ImportOptions::default()).expect("import");
    assert_eq!(doc.shape_count(), 3);
    assert_eq!(doc.layers.len(), 1);

    let job = plan(
        &doc,
        &PlanOptions {
            kerf: 0.2,
            ..Default::default()
        },
    );
    // Three contours, each cut once.
    assert_eq!(job.cuts().count(), 3);
    assert!(job.stats.cut_mm > 100.0);

    // All three circles are on one layer, so the planner can order them.
    // The smallest (innermost) contour must be cut first.
    let first_len = job.cuts().next().unwrap().0.len();
    let last_len = job.cuts().last().unwrap().0.len();
    assert!(
        first_len < last_len,
        "expected inside-out ordering, got {first_len} then {last_len}"
    );

    let program = gcode::write(&job, &GcodeOptions::default());
    assert!(program.contains("G21"));
    assert!(program.contains("M3 S200"));
    assert!(program.trim_end().ends_with("G0 X0 Y0"));

    let mut sim = SimulatedGrbl::new();
    let sent = program
        .lines()
        .filter(|l| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with(';')
        })
        .count();
    {
        let mut client = GrblClient::new(&mut sim);
        let acks = client.run(&program).expect("stream");
        assert_eq!(acks, sent);
    }

    assert_eq!(sim.state, RunState::Idle);
    assert_eq!(sim.pos, [0.0, 0.0, 0.0]);
    assert_eq!(sim.received.len(), sent);
}

#[test]
fn kerf_moves_the_toolpath_away_from_the_nominal_edge() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="20mm" viewBox="0 0 20 20">
        <rect x="5" y="5" width="10" height="10" fill="none" stroke="#000000"/>
    </svg>"##;

    let doc = import_svg(svg.as_bytes(), &ImportOptions::default()).expect("import");
    let plain = plan(&doc, &PlanOptions::default());
    let kerfed = plan(
        &doc,
        &PlanOptions {
            kerf: 0.4,
            ..Default::default()
        },
    );

    let plain_bounds = bounds(plain.cuts().next().unwrap().0);
    let kerfed_bounds = bounds(kerfed.cuts().next().unwrap().0);

    assert!((plain_bounds.0 - 5.0).abs() < 1e-6);
    // Half the kerf per side: 5.2 .. 14.8.
    assert!((kerfed_bounds.0 - 5.2).abs() < 1e-3);
    assert!((kerfed_bounds.2 - 14.8).abs() < 1e-3);
}

fn bounds(points: &[Point]) -> (f64, f64, f64, f64) {
    let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in points {
        b.0 = b.0.min(p.x);
        b.1 = b.1.min(p.y);
        b.2 = b.2.max(p.x);
        b.3 = b.3.max(p.y);
    }
    b
}
