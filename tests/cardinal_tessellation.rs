//! Cardinal (Catmull-Rom) curve tessellation — `ObjDecoder::with_curve_tessellation(N)`
//! evaluates every `cstype cardinal` `curv` directive via the spec's
//! conversion-to-Bezier formulation (b0 = c1, b1 = c1 + (c2 - c0)/6,
//! b2 = c2 - (c3 - c1)/6, b3 = c2, then cubic Bezier blend) and emits a
//! real `Topology::LineStrip` primitive on the synthetic
//! `"obj:curves"` mesh. The directive sequence is still preserved on
//! `Scene3D::extras` so the encoder replays the original free-form
//! section unchanged.
//!
//! Spec references: §"Curve and surface type" (cstype), §"Degree" (deg),
//! §"Curve" (curv), §"Cardinal" (Catmull-Rom basis, cubic-only).

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder};

/// Four colinear control points along the X axis. Cardinal interpolates
/// all but the first and last, so the single resulting segment runs
/// from c1 = (1, 0, 0) to c2 = (2, 0, 0). With the tangent vectors
/// (c2 - c0) / 6 = (1/3, 0, 0) and (c3 - c1) / 6 = (1/3, 0, 0), every
/// Bezier control point ends up on the same X axis ⇒ the curve is the
/// straight segment [1, 2].
const COLINEAR_X: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
cstype cardinal
deg 3
curv 0.0 1.0 1 2 3 4
parm u 0.0 1.0
end
";

#[test]
fn colinear_cardinal_traces_straight_segment() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(COLINEAR_X.as_bytes())
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

    // Endpoints: must interpolate c1 and c2 exactly per spec.
    let p0 = prim.positions[0];
    let pn = prim.positions[16];
    assert!((p0[0] - 1.0).abs() < 1e-5, "start = {p0:?}, want (1, 0, 0)");
    assert!((pn[0] - 2.0).abs() < 1e-5, "end = {pn:?}, want (2, 0, 0)");

    // All intermediate samples sit on the X axis (Y ≈ Z ≈ 0).
    for (i, p) in prim.positions.iter().enumerate() {
        assert!(
            p[1].abs() < 1e-5 && p[2].abs() < 1e-5,
            "sample {i} = {p:?}, expected on X axis"
        );
        assert!(
            (1.0..=2.0).contains(&p[0]) || (p[0] - 1.0).abs() < 1e-5 || (p[0] - 2.0).abs() < 1e-5,
            "sample {i} X = {} outside [1, 2]",
            p[0]
        );
    }

    // Midpoint of the straight segment at t = 0.5 is x = 1.5.
    let mid = prim.positions[8];
    assert!(
        (mid[0] - 1.5).abs() < 1e-5,
        "midpoint mismatch: {mid:?}, want (1.5, 0, 0)"
    );

    // Provenance extras.
    assert_eq!(
        prim.extras.get("obj:curve_kind").and_then(|v| v.as_str()),
        Some("cardinal")
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

/// Five Cardinal control points form two cubic segments (K - 3 = 2).
/// The curve must interpolate c1, c2, c3 exactly — c2 is the join point
/// between the two segments.
const FIVE_POINT_CARDINAL: &str = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
v 3.0 1.0 0.0
v 4.0 0.0 0.0
cstype cardinal
deg 3
curv 0.0 2.0 1 2 3 4 5
parm u 0.0 1.0 2.0
end
";

#[test]
fn cardinal_interpolates_interior_control_points() {
    // 32 intervals across 2 segments ⇒ sample 16 ends up on the segment
    // boundary at c2 = (2, 0, 0).
    let scene = ObjDecoder::new()
        .with_curve_tessellation(32)
        .decode(FIVE_POINT_CARDINAL.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.positions.len(), 33);

    // First sample interpolates c1 = (1, 1, 0).
    let start = prim.positions[0];
    assert!(
        (start[0] - 1.0).abs() < 1e-4 && (start[1] - 1.0).abs() < 1e-4,
        "start = {start:?}, want (1, 1, 0)"
    );

    // Last sample interpolates c3 = (3, 1, 0).
    let end = prim.positions[32];
    assert!(
        (end[0] - 3.0).abs() < 1e-4 && (end[1] - 1.0).abs() < 1e-4,
        "end = {end:?}, want (3, 1, 0)"
    );

    // The segment join sits at sample 16 (halfway). Expect c2 = (2, 0).
    let join = prim.positions[16];
    assert!(
        (join[0] - 2.0).abs() < 1e-4 && (join[1] - 0.0).abs() < 1e-4,
        "segment join = {join:?}, want (2, 0, 0)"
    );
}

/// Non-cubic Cardinal is illegal per spec — the directive is captured
/// but the synthetic mesh is not emitted (no tessellation).
const NON_CUBIC_CARDINAL: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
cstype cardinal
deg 2
curv 0.0 1.0 1 2 3 4
parm u 0.0 1.0
end
";

#[test]
fn non_cubic_cardinal_is_rejected() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(NON_CUBIC_CARDINAL.as_bytes())
        .unwrap();
    // No synthetic mesh emitted — directive is still in extras.
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.name.as_deref() != Some("obj:curves")),
        "non-cubic cardinal must not tessellate"
    );
    // But the directive must still ride the free-form extras for
    // round-trip preservation.
    assert!(
        scene.extras.contains_key("obj:freeform_directives"),
        "directives must survive even when tessellation skips"
    );
}

/// A primitive with fewer than 4 control points is invalid for cardinal
/// (degree-3 requires 4 points per segment). Skip silently.
const TOO_FEW_CARDINAL: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
cstype cardinal
deg 3
curv 0.0 1.0 1 2 3
parm u 0.0 1.0
end
";

#[test]
fn cardinal_with_fewer_than_four_control_points_is_skipped() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(TOO_FEW_CARDINAL.as_bytes())
        .unwrap();
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.name.as_deref() != Some("obj:curves")),
        "cardinal with K < 4 must not tessellate"
    );
}

/// Re-encoding a decoded scene with curve tessellation enabled must
/// produce the original `cstype cardinal` / `curv` / `end` block
/// unchanged — the synthetic primitives are filtered from the polygonal
/// section and only the captured directives are replayed.
#[test]
fn cardinal_directives_round_trip_through_encoder() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(FIVE_POINT_CARDINAL.as_bytes())
        .unwrap();
    let encoded = ObjEncoder::new().encode(&scene).unwrap();
    let text = String::from_utf8(encoded).unwrap();
    assert!(
        text.contains("cstype cardinal"),
        "cstype line missing:\n{text}"
    );
    assert!(
        text.contains("curv 0.0 2.0 1 2 3 4 5"),
        "curv line missing:\n{text}"
    );
    assert!(text.contains("\nend\n"), "end line missing:\n{text}");
    // No synthetic curve vertices leak into the polygonal `v` section —
    // the source had 5 `v` lines, the re-encode should too.
    let v_count = text.lines().filter(|l| l.starts_with("v ")).count();
    assert_eq!(
        v_count, 5,
        "expected 5 source v lines, got {v_count}:\n{text}"
    );
}

/// Re-decoding the encoded output (round-trip) must still produce the
/// same tessellated polyline shape, end-to-end.
#[test]
fn cardinal_full_round_trip_preserves_tessellation() {
    let mut dec = ObjDecoder::new().with_curve_tessellation(16);
    let scene1 = dec.decode(FIVE_POINT_CARDINAL.as_bytes()).unwrap();
    let buf = ObjEncoder::new().encode(&scene1).unwrap();
    let scene2 = dec.decode(&buf).unwrap();
    // Same number of synthetic primitives, same shape (sample-exact
    // within rounding tolerance for the f32 path).
    let curves1 = scene1
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:curves"))
        .unwrap();
    let curves2 = scene2
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:curves"))
        .unwrap();
    assert_eq!(curves1.primitives.len(), curves2.primitives.len());
    let p1 = &curves1.primitives[0].positions;
    let p2 = &curves2.primitives[0].positions;
    assert_eq!(p1.len(), p2.len());
    for (i, (a, b)) in p1.iter().zip(p2.iter()).enumerate() {
        for k in 0..3 {
            assert!(
                (a[k] - b[k]).abs() < 1e-4,
                "sample {i} axis {k}: {} vs {}",
                a[k],
                b[k]
            );
        }
    }
}

/// When `samples == 0` (the default), Cardinal must NOT tessellate —
/// the directive rides on `Scene3D::extras` only, matching the round 7
/// default behaviour for Bezier / B-spline.
#[test]
fn cardinal_default_samples_zero_skips_tessellation() {
    let scene = ObjDecoder::new().decode(COLINEAR_X.as_bytes()).unwrap();
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.name.as_deref() != Some("obj:curves")),
        "samples == 0 must not produce synthetic mesh"
    );
    assert!(scene.extras.contains_key("obj:freeform_directives"));
}
