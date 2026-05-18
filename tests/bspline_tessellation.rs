//! B-spline curve tessellation — `ObjDecoder::with_curve_tessellation(N)`
//! evaluates every `cstype bspline` (or `cstype rat bspline`) `curv`
//! directive via the Cox-deBoor recursive basis-function formula and
//! emits a real `Topology::LineStrip` primitive on the synthetic
//! `"obj:curves"` mesh. The directive sequence is still preserved on
//! `Scene3D::extras` so the encoder replays the original free-form
//! section unchanged.
//!
//! Spec references: §"Curve and surface type" (cstype), §"Degree" (deg),
//! §"Curve" (curv), §"Parameter values and knot vectors" (parm),
//! §"B-spline" (Cox-deBoor recursion).

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder, obj};

/// Degree-2 (quadratic) B-spline with the simplest open-uniform knot
/// vector `[0 0 0 1 1 1]`. Spec §"B-spline" condition 6:
///   knots.len() == control_points.len() + degree + 1 = 3 + 2 + 1 = 6.
///
/// With knot multiplicity (n + 1) at both ends, this open-uniform
/// quadratic B-spline degenerates to the equivalent quadratic Bezier
/// over the same control polygon — a well-known result, useful here
/// because the Bezier endpoint and midpoint values are already pinned
/// in the sibling test file.
const QUADRATIC_BSPLINE_OPEN_UNIFORM: &str = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
cstype bspline
deg 2
curv 0.0 1.0 1 2 3
parm u 0.0 0.0 0.0 1.0 1.0 1.0
end
";

#[test]
fn open_uniform_quadratic_b_spline_matches_bezier() {
    // 8-interval tessellation → 9 vertices on the LineStrip.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(QUADRATIC_BSPLINE_OPEN_UNIFORM.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "one synthetic mesh expected");
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("obj:curves"));
    assert_eq!(mesh.primitives.len(), 1);

    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::LineStrip);
    assert_eq!(prim.positions.len(), 9, "samples + 1 vertices on the strip");

    // Endpoint exactness: t = 0 ⇒ P0 = (0,0,0), t = 1 ⇒ P2 = (2,0,0).
    let p0 = prim.positions[0];
    let pn = prim.positions[8];
    assert!(
        (p0[0] - 0.0).abs() < 1e-4 && (p0[1] - 0.0).abs() < 1e-4,
        "start = {p0:?}, want ≈ (0, 0)"
    );
    assert!(
        (pn[0] - 2.0).abs() < 1e-3 && (pn[1] - 0.0).abs() < 1e-3,
        "end = {pn:?}, want ≈ (2, 0)"
    );

    // Midpoint check: open-uniform quadratic B-spline = quadratic Bezier
    // with the same control polygon ⇒ midpoint at t = 0.5 is (1, 0.5).
    let mid = prim.positions[4];
    assert!(
        (mid[0] - 1.0).abs() < 1e-4 && (mid[1] - 0.5).abs() < 1e-4,
        "midpoint mismatch: {mid:?}, want ≈ (1, 0.5)"
    );

    // Provenance extras.
    assert_eq!(
        prim.extras.get("obj:curve_kind").and_then(|v| v.as_str()),
        Some("bspline")
    );
    assert_eq!(
        prim.extras.get("obj:curve_degree").and_then(|v| v.as_u64()),
        Some(2)
    );
    assert_eq!(
        prim.extras
            .get("obj:curve_samples")
            .and_then(|v| v.as_u64()),
        Some(8)
    );
    assert_eq!(
        prim.extras
            .get("obj:tessellated_curve")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
}

/// Linear (degree-1) B-spline with 4 control points and a uniform knot
/// vector `[0, 1, 2, 3, 4, 5]`. The curve degenerates to the polyline
/// connecting the middle control points — a straight piecewise-linear
/// blend. Knot-vector length: 4 + 1 + 1 = 6 ✓.
const LINEAR_BSPLINE_UNIFORM: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
cstype bspline
deg 1
curv 1.0 4.0 1 2 3 4
parm u 0.0 1.0 2.0 3.0 4.0 5.0
end
";

#[test]
fn linear_b_spline_tessellates_into_a_piecewise_linear_strip() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(12)
        .decode(LINEAR_BSPLINE_UNIFORM.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.positions.len(), 13);

    // The y-coordinate must stay at 0 (the control polygon is flat on
    // y = 0) — a numerical sanity check on the basis evaluation.
    for p in &prim.positions {
        assert!(p[1].abs() < 1e-4, "y drifted: {p:?}");
    }
    // The x-coordinate must be monotonically non-decreasing.
    let mut prev_x = prim.positions[0][0];
    for p in &prim.positions[1..] {
        assert!(
            p[0] + 1e-4 >= prev_x,
            "x not monotonic: {prev_x} → {}",
            p[0]
        );
        prev_x = p[0];
    }
}

/// Rational B-spline (NURBS) with non-trivial weights on a degree-2
/// open-uniform basis. With weight `2.0` on the middle control point
/// the curve pulls toward P1 just like the rational-Bezier case in the
/// sibling test file (the open-uniform degree-2 B-spline matches the
/// degree-2 Bezier exactly).
const QUADRATIC_NURBS: &str = "\
v 0.0 0.0 0.0 1.0
v 1.0 1.0 0.0 2.0
v 2.0 0.0 0.0 1.0
cstype rat bspline
deg 2
curv 0.0 1.0 1 2 3
parm u 0.0 0.0 0.0 1.0 1.0 1.0
end
";

#[test]
fn rational_b_spline_with_middle_weight_two_pulls_curve_toward_p1() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(QUADRATIC_NURBS.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.positions.len(), 9);

    // Open-uniform quadratic rational B-spline ≡ quadratic rational
    // Bezier ⇒ midpoint y = 1.0 / 1.5 ≈ 0.6667 (same blend as the
    // sibling Bezier test).
    let mid = prim.positions[4];
    assert!(
        (mid[1] - (1.0 / 1.5)).abs() < 1e-3,
        "rational midpoint = {mid:?}, expected y ≈ 0.6667"
    );

    assert_eq!(
        prim.extras.get("obj:curve_kind").and_then(|v| v.as_str()),
        Some("rat_bspline")
    );
}

#[test]
fn b_spline_tessellation_round_trips_through_encoder() {
    // Decode (tessellate) → encode → decode (tessellate) on a free-form
    // OBJ with only a B-spline curve. The polygonal `v` lines survive
    // through `Scene3D::extras["obj:positions"]`, so the second
    // tessellation produces the same curve geometry as the first.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(QUADRATIC_BSPLINE_OPEN_UNIFORM.as_bytes())
        .unwrap();
    let bytes = ObjEncoder::new().encode(&scene).unwrap();

    // The cstype/curv/parm/end directives must come back verbatim.
    let text = std::str::from_utf8(&bytes).unwrap();
    for keyword in ["cstype bspline", "deg 2", "curv 0", "parm u 0", "end"] {
        assert!(
            text.lines().any(|l| l.starts_with(keyword)),
            "missing `{keyword}` line in:\n{text}"
        );
    }

    let scene2 = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(&bytes)
        .unwrap();
    let curve_a = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:curves"))
        .unwrap();
    let curve_b = scene2
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:curves"))
        .unwrap();

    let p0 = &curve_a.primitives[0];
    let p1 = &curve_b.primitives[0];
    assert_eq!(p0.positions.len(), p1.positions.len());
    for (a, b) in p0.positions.iter().zip(p1.positions.iter()) {
        assert!(
            (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4 && (a[2] - b[2]).abs() < 1e-4,
            "tessellation drifted on round-trip: {a:?} vs {b:?}"
        );
    }
}

#[test]
fn b_spline_with_wrong_knot_vector_length_is_skipped() {
    // Knot vector too short — 3 control points + degree 2 + 1 = 6
    // knots required; we only provide 5. The directive remains in
    // `freeform_directives` but no synthetic primitive is emitted.
    let text = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
cstype bspline
deg 2
curv 0.0 1.0 1 2 3
parm u 0.0 0.0 0.5 1.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(text.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "B-spline with incomplete knot vector must not tessellate"
    );
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(
        dirs.iter().any(|d| d
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            == Some("curv")),
        "curv directive still captured for round-trip"
    );
}

#[test]
fn polygonal_geometry_and_tessellated_b_spline_coexist() {
    let text = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 2.0 0.0 0.0
v 3.0 1.0 0.0
v 4.0 0.0 0.0
f 1 2 3
cstype bspline
deg 2
curv 0.0 1.0 4 5 6
parm u 0.0 0.0 0.0 1.0 1.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    // Two meshes: polygonal (unnamed) + obj:curves.
    assert_eq!(scene.meshes.len(), 2);
    let polygonal = &scene.meshes[0];
    let curves = &scene.meshes[1];
    assert_eq!(curves.name.as_deref(), Some("obj:curves"));
    assert_eq!(polygonal.primitives.len(), 1);
    assert_eq!(polygonal.primitives[0].topology, Topology::Triangles);
    assert_eq!(curves.primitives.len(), 1);
    assert_eq!(curves.primitives[0].topology, Topology::LineStrip);
    assert_eq!(curves.primitives[0].positions.len(), 5);
}

#[test]
fn cstype_end_resets_parm_u_so_next_curve_needs_fresh_knots() {
    // After `end`, the active `parm u` knot vector is cleared. A
    // following `cstype bspline` block without its own `parm u` must
    // not accidentally reuse the previous one — the curve should
    // silently skip tessellation rather than evaluate against a stale
    // knot vector.
    let text = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
cstype bspline
deg 2
curv 0.0 1.0 1 2 3
parm u 0.0 0.0 0.0 1.0 1.0 1.0
end
cstype bspline
deg 2
curv 0.0 1.0 1 2 3
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    // Exactly one tessellated curve — the second block had no `parm u`
    // and so was skipped.
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.meshes[0].primitives.len(), 1);
}

#[test]
fn default_decoder_without_tessellation_does_not_synthesise_b_spline_mesh() {
    // `with_curve_tessellation` not set ⇒ default 0 ⇒ no synthetic
    // geometry, even for a fully-specified B-spline.
    let scene = ObjDecoder::new()
        .decode(QUADRATIC_BSPLINE_OPEN_UNIFORM.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "default decoder must stay verbatim"
    );
    // Free-form directives are still captured.
    assert!(scene.extras.contains_key("obj:freeform_directives"));
}

#[test]
fn tessellated_b_spline_is_filtered_out_of_v_pool_on_encode() {
    // Decoder tessellates → encoder must skip the synthetic geometry
    // and replay the original directives from
    // `Scene3D::extras["obj:freeform_directives"]`. The re-encoded OBJ
    // emits the original cstype/curv/parm/end section unchanged; the
    // tessellation sample points must NOT leak into the `v` pool.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(QUADRATIC_BSPLINE_OPEN_UNIFORM.as_bytes())
        .unwrap();
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let v_lines = text.lines().filter(|l| l.starts_with("v ")).count();
    assert!(
        v_lines <= 3,
        "tessellation samples leaked as `v` lines; got {v_lines}:\n{text}"
    );
    assert!(
        !text.contains("o obj:curves"),
        "synthetic curve mesh should not be re-emitted as a polygonal `o` block"
    );
    assert!(
        !text.lines().any(|l| l.starts_with("l ")),
        "encoder must not emit `l` lines for tessellated curves; got:\n{text}"
    );
}
