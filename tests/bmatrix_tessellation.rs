//! Basis-matrix curve tessellation — `ObjDecoder::with_curve_tessellation(N)`
//! evaluates every `cstype bmatrix` `curv` directive (with the
//! companion `deg`, `bmat u`, and `step` body statements) per spec
//! §"Basis matrix" and §"bmat u/v matrix":
//!
//! ```text
//!   P(t) = Σ_{i=0..n} Σ_{j=0..n} B[i][j] · t^j · p_{base + i}
//! ```
//!
//! where `B` is the (n + 1) × (n + 1) basis stored row-major (column
//! `j` varying fastest, per spec §"bmat u/v matrix"), and segment `i`
//! uses control points `c_{i·step + 1} .. c_{i·step + n + 1}` (1-based)
//! per spec §"step stepu stepv".
//!
//! The free-form directive sequence is still preserved on
//! `Scene3D::extras["obj:freeform_directives"]` so the encoder replays
//! the original `cstype` / `deg` / `bmat u` / `step` / `curv` / `end`
//! block unchanged.
//!
//! Spec references: §"Curve and surface type" (cstype), §"Degree"
//! (deg), §"bmat u/v matrix", §"step stepu stepv",
//! §"Basis matrix", §"Free-form curve/surface body statements".

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder};

/// Cubic Bezier expressed as a basis-matrix curve — the spec's
/// §"Basis matrix Examples" / "Cubic Bezier surface made with a basis
/// matrix" example, demoted to a 1D curve. The basis matrix is the
/// standard Bernstein basis re-expressed in spec form (row index = i,
/// column index = j varies fastest, B[i][j] is the coefficient of
/// p_i · t^j):
///
///   row 0:  1  -3   3  -1     ⇒  (1-t)^3
///   row 1:  0   3  -6   3     ⇒  3t(1-t)^2
///   row 2:  0   0   3  -3     ⇒  3t^2(1-t)
///   row 3:  0   0   0   1     ⇒  t^3
///
/// Four colinear control points along X ⇒ a straight segment from
/// P0 = (0, 0, 0) to P3 = (3, 0, 0).
const COLINEAR_BMATRIX_OBJ: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
cstype bmatrix
deg 3
step 3
bmat u 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
curv 0.0 1.0 1 2 3 4
end
";

#[test]
fn cubic_bezier_via_bmatrix_traces_straight_segment() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(COLINEAR_BMATRIX_OBJ.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "one synthetic mesh expected");
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("obj:curves"));
    assert_eq!(mesh.primitives.len(), 1);

    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::LineStrip);
    assert_eq!(
        prim.positions.len(),
        17,
        "samples + 1 vertices on the strip"
    );

    // Endpoints: Bernstein basis sums to 1, so t=0 → P0 and t=1 → P3
    // exactly.
    let p0 = prim.positions[0];
    let pn = prim.positions[16];
    assert!((p0[0] - 0.0).abs() < 1e-5, "start = {p0:?}, want (0, 0, 0)");
    assert!((pn[0] - 3.0).abs() < 1e-5, "end = {pn:?}, want (3, 0, 0)");

    // All intermediate samples sit on the X axis since the four
    // control points are colinear.
    for (i, p) in prim.positions.iter().enumerate() {
        assert!(
            p[1].abs() < 1e-5 && p[2].abs() < 1e-5,
            "sample {i} = {p:?}, expected on X axis"
        );
    }

    // Midpoint of a cubic Bernstein-blend of 4 colinear control points
    // P0=0, P1=1, P2=2, P3=3 at t=0.5 is 0.125·0 + 0.375·1 + 0.375·2 +
    // 0.125·3 = 0 + 0.375 + 0.75 + 0.375 = 1.5.
    let mid = prim.positions[8];
    assert!(
        (mid[0] - 1.5).abs() < 1e-5,
        "midpoint mismatch: {mid:?}, want (1.5, 0, 0)"
    );

    // Provenance extras.
    assert_eq!(
        prim.extras.get("obj:curve_kind").and_then(|v| v.as_str()),
        Some("bmatrix")
    );
    assert_eq!(
        prim.extras.get("obj:curve_degree").and_then(|v| v.as_u64()),
        Some(3)
    );
    assert_eq!(
        prim.extras
            .get("obj:curve_samples")
            .and_then(|v| v.as_u64()),
        Some(16)
    );
    assert_eq!(
        prim.extras
            .get("obj:tessellated_curve")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
}

/// Bezier-as-bmatrix with a non-colinear control polygon. The four
/// control points (0,0,0), (1,2,0), (2,2,0), (3,0,0) form a symmetric
/// arch; at t=0.5 the cubic Bezier evaluates to:
///   x = 0.125·0 + 0.375·1 + 0.375·2 + 0.125·3 = 1.5
///   y = 0.125·0 + 0.375·2 + 0.375·2 + 0.125·0 = 1.5
const ARCH_BMATRIX_OBJ: &str = "\
v 0.0 0.0 0.0
v 1.0 2.0 0.0
v 2.0 2.0 0.0
v 3.0 0.0 0.0
cstype bmatrix
deg 3
step 3
bmat u 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
curv 0.0 1.0 1 2 3 4
end
";

#[test]
fn cubic_bezier_arch_via_bmatrix_matches_bernstein() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(ARCH_BMATRIX_OBJ.as_bytes())
        .unwrap();
    let mesh = &scene.meshes[0];
    let prim = &mesh.primitives[0];

    // Endpoints — Bezier interpolates P0 and P3 exactly.
    let p0 = prim.positions[0];
    let pn = prim.positions[16];
    assert!(
        (p0[0]).abs() < 1e-5 && (p0[1]).abs() < 1e-5,
        "start = {p0:?}, want (0, 0, 0)"
    );
    assert!(
        (pn[0] - 3.0).abs() < 1e-5 && (pn[1]).abs() < 1e-5,
        "end = {pn:?}, want (3, 0, 0)"
    );

    // Midpoint matches the closed-form Bernstein evaluation.
    let mid = prim.positions[8];
    assert!(
        (mid[0] - 1.5).abs() < 1e-5,
        "midpoint X = {}, want 1.5",
        mid[0]
    );
    assert!(
        (mid[1] - 1.5).abs() < 1e-5,
        "midpoint Y = {}, want 1.5",
        mid[1]
    );
}

/// Hermite curve via basis matrix — spec example 2: degree 3, step 2,
/// with the Hermite basis matrix (row-major, j varies fastest):
///
///   row 0:  1  0  -3   2     (1 - 3t^2 + 2t^3 — H_00 for p0)
///   row 1:  0  1  -2   1     (t - 2t^2 + t^3 — H_10 for tangent v0)
///   row 2:  0  0   3  -2     (3t^2 - 2t^3 — H_01 for p1)
///   row 3:  0  0  -1   1     (-t^2 + t^3 — H_11 for tangent v1)
///
/// Step 2 means a 4-point Hermite "segment" but with stride 2 between
/// successive segments (so the second-half tangent of segment i is the
/// first-half tangent of segment i+1).
///
/// Four control points: p0 = (0,0,0), v0 = (1,0,0), p1 = (1,1,0),
/// v1 = (0,1,0). This is a single Hermite segment from (0,0,0) to
/// (1,1,0); ⇒ the polyline endpoints land on those points.
const HERMITE_BMATRIX_OBJ: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 1.0 1.0 0.0
v 0.0 1.0 0.0
cstype bmatrix
deg 3
step 2
bmat u 1 0 -3 2 0 1 -2 1 0 0 3 -2 0 0 -1 1
curv 0.0 1.0 1 2 3 4
end
";

#[test]
fn hermite_via_bmatrix_interpolates_endpoints() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(HERMITE_BMATRIX_OBJ.as_bytes())
        .unwrap();
    let mesh = &scene.meshes[0];
    let prim = &mesh.primitives[0];

    // Hermite interpolates the position control points exactly at the
    // segment endpoints — t=0 → p0 = (0, 0, 0), t=1 → p1 = (1, 1, 0).
    let p0 = prim.positions[0];
    let pn = *prim.positions.last().unwrap();
    assert!(
        p0[0].abs() < 1e-5 && p0[1].abs() < 1e-5,
        "start = {p0:?}, want (0, 0, 0)"
    );
    assert!(
        (pn[0] - 1.0).abs() < 1e-5 && (pn[1] - 1.0).abs() < 1e-5,
        "end = {pn:?}, want (1, 1, 0)"
    );

    // Provenance extras still carry the bmatrix kind.
    assert_eq!(
        prim.extras.get("obj:curve_kind").and_then(|v| v.as_str()),
        Some("bmatrix")
    );
}

/// Round-trip stability: a `cstype bmatrix` block survives a
/// decode → encode → decode cycle, including the `bmat u` and `step`
/// body statements (recorded in `obj:freeform_directives` and replayed
/// verbatim).
#[test]
fn bmatrix_directives_round_trip_verbatim() {
    let scene1 = ObjDecoder::new()
        .decode(COLINEAR_BMATRIX_OBJ.as_bytes())
        .unwrap();
    let bytes = ObjEncoder::new().encode(&scene1).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.lines().any(|l| l.starts_with("cstype bmatrix")),
        "missing `cstype bmatrix` line:\n{text}"
    );
    assert!(
        text.lines().any(|l| l.starts_with("step 3")),
        "missing `step 3` line:\n{text}"
    );
    assert!(
        text.lines().any(|l| l.starts_with("bmat u 1 -3")),
        "missing `bmat u` line:\n{text}"
    );

    // Re-decoding the encoded bytes yields the same directive sequence.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let d1 = scene1.extras.get("obj:freeform_directives").unwrap();
    let d2 = scene2.extras.get("obj:freeform_directives").unwrap();
    assert_eq!(d1, d2, "round-trip not stable");
}

/// Multi-segment basis-matrix curve: 7 control points + step 3 (cubic
/// Bezier) ⇒ 2 segments. Segment 0 uses points 1..4, segment 1 uses
/// points 4..7. Joining at p3 = p4 (control point 4) the polyline
/// passes through that shared point at the segment boundary.
const MULTI_SEGMENT_BMATRIX_OBJ: &str = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 1.0 0.0
v 3.0 0.0 0.0
v 4.0 -1.0 0.0
v 5.0 -1.0 0.0
v 6.0 0.0 0.0
cstype bmatrix
deg 3
step 3
bmat u 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
curv 0.0 2.0 1 2 3 4 5 6 7
end
";

#[test]
fn multi_segment_bmatrix_passes_through_join() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(20)
        .decode(MULTI_SEGMENT_BMATRIX_OBJ.as_bytes())
        .unwrap();
    let mesh = &scene.meshes[0];
    let prim = &mesh.primitives[0];

    // Endpoints: t=0 → P0, t = end → P6.
    let p0 = prim.positions[0];
    let pn = *prim.positions.last().unwrap();
    assert!(p0[0].abs() < 1e-5, "start = {p0:?}, want (0, 0, 0)");
    assert!((pn[0] - 6.0).abs() < 1e-5, "end = {pn:?}, want (6, 0, 0)");

    // 21 samples total (samples + 1).
    assert_eq!(prim.positions.len(), 21);
}

/// Malformed basis-matrix block (missing `bmat u`) is silently
/// dropped — no synthetic mesh appears, but the directive sequence
/// still rides on `Scene3D::extras` for downstream consumers.
const MISSING_BMAT_OBJ: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
cstype bmatrix
deg 3
step 3
curv 0.0 1.0 1 2 3 4
end
";

#[test]
fn missing_bmat_skipped_silently() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(MISSING_BMAT_OBJ.as_bytes())
        .unwrap();
    // No tessellated primitives — directive block is captured but
    // incomplete so no synthetic mesh appears.
    assert!(
        scene.meshes.is_empty() || scene.meshes.iter().all(|m| m.primitives.is_empty()),
        "expected no synthetic mesh, got {} mesh(es)",
        scene.meshes.len()
    );
    // Directives still captured.
    let arr = scene
        .extras
        .get("obj:freeform_directives")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(arr.iter().any(|e| {
        e.as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            == Some("cstype")
    }));
}

/// Wrong-size `bmat u` (3 floats for a degree-3 curve which expects
/// 16) is silently dropped — guards against malformed input rather
/// than aborting the decode.
const WRONG_BMAT_SIZE_OBJ: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
cstype bmatrix
deg 3
step 3
bmat u 1 0 0
curv 0.0 1.0 1 2 3 4
end
";

#[test]
fn wrong_size_bmat_skipped_silently() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(WRONG_BMAT_SIZE_OBJ.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty() || scene.meshes.iter().all(|m| m.primitives.is_empty()),
        "expected no synthetic mesh, got {} mesh(es)",
        scene.meshes.len()
    );
}

/// Tessellation disabled (the default) ⇒ no synthetic primitives even
/// when the basis-matrix block is well-formed; the directive sequence
/// still round-trips.
#[test]
fn tessellation_disabled_skips_bmatrix_evaluation() {
    let scene = ObjDecoder::new()
        .decode(COLINEAR_BMATRIX_OBJ.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "expected no synthetic mesh when tessellation off, got {}",
        scene.meshes.len()
    );
    // Directives present.
    let arr = scene
        .extras
        .get("obj:freeform_directives")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(arr.iter().any(|e| {
        e.as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            == Some("bmat")
    }));
    assert!(arr.iter().any(|e| {
        e.as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            == Some("step")
    }));
}
