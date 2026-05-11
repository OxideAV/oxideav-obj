//! Bezier curve tessellation — `ObjDecoder::with_curve_tessellation(N)`
//! evaluates every `cstype bezier` (or `cstype rat bezier`) `curv`
//! directive into a real `LineStrip` primitive on a synthetic mesh
//! named `"obj:curves"`. The directive sequence is still preserved on
//! `Scene3D::extras` so the encoder replays the original free-form
//! section unchanged.
//!
//! Spec references: §"Curve and surface type" (cstype), §"Degree"
//! (deg), §"Curve" (curv), §"Free-form curve/surface body statements"
//! (end), §"Curve" (rational form weights).

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder, obj};

/// A degree-2 Bezier curve with three control points laid out as a
/// right-angle triangle. de Casteljau on a degree-2 basis is the
/// classic `(1-t)^2 P0 + 2(1-t)t P1 + t^2 P2` blend — we sanity-check
/// the midpoint (t = 0.5) directly.
const QUADRATIC_BEZIER: &str = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
cstype bezier
deg 2
curv 0.0 1.0 1 2 3
parm u 0.0 1.0
end
";

#[test]
fn quadratic_bezier_tessellates_into_a_line_strip_with_correct_endpoints() {
    // Without the option, free-form geometry stays as captured extras.
    let bare = ObjDecoder::new()
        .decode(QUADRATIC_BEZIER.as_bytes())
        .unwrap();
    assert!(
        bare.meshes.is_empty(),
        "default decoder must not synthesise curve meshes"
    );

    // With 8 samples, expect 9 vertices on one LineStrip primitive.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(QUADRATIC_BEZIER.as_bytes())
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
    assert!((p0[0] - 0.0).abs() < 1e-5 && (p0[1] - 0.0).abs() < 1e-5);
    assert!((pn[0] - 2.0).abs() < 1e-5 && (pn[1] - 0.0).abs() < 1e-5);

    // Midpoint check: at t = 0.5 the degree-2 Bezier with P0=(0,0,0),
    // P1=(1,1,0), P2=(2,0,0) evaluates to (1, 0.5, 0).
    let mid = prim.positions[4];
    assert!(
        (mid[0] - 1.0).abs() < 1e-5 && (mid[1] - 0.5).abs() < 1e-5,
        "midpoint mismatch: {mid:?}"
    );

    // Provenance extras: kind, degree, samples, u-range.
    assert_eq!(
        prim.extras.get("obj:curve_kind").and_then(|v| v.as_str()),
        Some("bezier")
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
    let u_range = prim
        .extras
        .get("obj:curve_u_range")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!((u_range[0].as_f64().unwrap() - 0.0).abs() < 1e-6);
    assert!((u_range[1].as_f64().unwrap() - 1.0).abs() < 1e-6);

    assert_eq!(
        prim.extras
            .get("obj:tessellated_curve")
            .and_then(|v| v.as_bool()),
        Some(true)
    );

    // Indices buffer is the implicit 0..N strip.
    let indices = prim.indices.as_ref().expect("strip indices");
    assert_eq!(indices.len(), 9);
}

/// A degree-3 cubic Bezier — four control points laid out as an S-curve.
/// We don't pin the exact midpoint here; we only check that the
/// tessellated polyline visits both endpoints and has 33 vertices for
/// a 32-sample request.
const CUBIC_BEZIER: &str = "\
v 0.0 0.0 0.0
v 1.0 2.0 0.0
v 2.0 -2.0 0.0
v 3.0 0.0 0.0
cstype bezier
deg 3
curv 0.0 1.0 1 2 3 4
parm u 0.0 1.0
end
";

#[test]
fn cubic_bezier_polyline_length_matches_sample_count() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(32)
        .decode(CUBIC_BEZIER.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.positions.len(), 33);
    // Endpoints are exact even with the high sample count.
    assert!((prim.positions[0][0] - 0.0).abs() < 1e-5);
    assert!((prim.positions[32][0] - 3.0).abs() < 1e-5);
    // The curve traverses positive then negative y (S-shape); confirm
    // by scanning for sign changes in the y-coordinate.
    let mut sign_flips = 0;
    let mut prev_sign = prim.positions[0][1].signum();
    for p in &prim.positions[1..] {
        let s = p[1].signum();
        if s != prev_sign && s != 0.0 && prev_sign != 0.0 {
            sign_flips += 1;
            prev_sign = s;
        }
    }
    assert!(sign_flips >= 1, "expected at least one y-axis sign flip");

    assert_eq!(
        prim.extras.get("obj:curve_degree").and_then(|v| v.as_u64()),
        Some(3)
    );
}

/// Rational Bezier — `cstype rat bezier`. With weights `[1, 2, 1]` on a
/// quadratic basis the curve gets pulled toward the middle control
/// point compared to the non-rational form. We test this by comparing
/// the midpoint of the rational variant against the midpoint of the
/// non-rational one.
const RAT_BEZIER: &str = "\
v 0.0 0.0 0.0 1.0
v 1.0 1.0 0.0 2.0
v 2.0 0.0 0.0 1.0
cstype rat bezier
deg 2
curv 0.0 1.0 1 2 3
parm u 0.0 1.0
end
";

#[test]
fn rational_bezier_midpoint_differs_from_non_rational() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(RAT_BEZIER.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.positions.len(), 9);

    // For the standard Bezier midpoint with these control points,
    // y = 0.5. With the middle weight = 2 the rational variant pulls
    // the curve toward P1 → y becomes 0.5 · 2/1.5 = 0.6667 (the
    // homogeneous-form blend, projected back to 3D).
    //
    // At t = 0.5 with weights w = [1, 2, 1]:
    //   numerator   = (1-t)^2 · w0 · y0 + 2(1-t)t · w1 · y1 + t^2 · w2 · y2
    //               = 0.25·1·0 + 0.5·2·1 + 0.25·1·0 = 1.0
    //   denominator = 0.25·1 + 0.5·2 + 0.25·1 = 1.5
    //   ⇒ y_mid = 1.0 / 1.5 ≈ 0.6667
    let mid = prim.positions[4];
    assert!(
        (mid[1] - (1.0 / 1.5)).abs() < 1e-4,
        "rational midpoint = {:?}, expected y ≈ 0.6667",
        mid
    );

    assert_eq!(
        prim.extras.get("obj:curve_kind").and_then(|v| v.as_str()),
        Some("rat_bezier")
    );
}

#[test]
fn tessellated_curve_is_not_emitted_as_v_lines_by_encoder() {
    // Decoder tessellates → encoder must skip the synthetic geometry
    // and replay the original directives from
    // `Scene3D::extras["obj:freeform_directives"]`. The re-encoded OBJ
    // emits the original cstype/curv/end section unchanged; the 17
    // tessellation sample points must NOT leak into the `v` pool.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(QUADRATIC_BEZIER.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "synthetic curve mesh present");

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();

    // Zero or three `v` lines — not 17. The current encoder doesn't
    // surface the source `v` pool when no polygonal element references
    // it (free-form-only OBJ pre-existing limitation, see
    // `freeform_geometry::cstype_deg_curv_parm_end_round_trip`); the
    // post-condition this test asserts is that the *tessellated*
    // sample points (17 of them) do NOT pollute the output.
    let v_lines = text.lines().filter(|l| l.starts_with("v ")).count();
    assert!(
        v_lines <= 3,
        "tessellation samples leaked as `v` lines; got {v_lines}:\n{text}"
    );

    // No `o obj:curves` header — the synthetic mesh is skipped.
    assert!(
        !text.contains("o obj:curves"),
        "synthetic curve mesh should not be re-emitted as a polygonal `o` block"
    );

    // The directive sequence comes back verbatim.
    for keyword in ["cstype bezier", "deg 2", "curv 0", "parm u 0", "end"] {
        assert!(
            text.lines().any(|l| l.starts_with(keyword)),
            "missing `{keyword}` line in:\n{text}"
        );
    }

    // No `l ` polyline lines (the tessellated strip is filtered out).
    assert!(
        !text.lines().any(|l| l.starts_with("l ")),
        "encoder must not emit `l` lines for tessellated curves; got:\n{text}"
    );
}

#[test]
fn polygonal_geometry_and_tessellated_curves_coexist() {
    let text = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 2.0 0.0 0.0
v 3.0 1.0 0.0
v 4.0 0.0 0.0
f 1 2 3
cstype bezier
deg 2
curv 0.0 1.0 4 5 6
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
    // Polygonal: one Triangles primitive with one face.
    assert_eq!(polygonal.primitives.len(), 1);
    assert_eq!(polygonal.primitives[0].topology, Topology::Triangles);
    // Curves: one LineStrip with 5 vertices.
    assert_eq!(curves.primitives.len(), 1);
    assert_eq!(curves.primitives[0].topology, Topology::LineStrip);
    assert_eq!(curves.primitives[0].positions.len(), 5);
}

#[test]
fn b_spline_and_other_cstype_basis_kinds_are_left_alone() {
    // Only `cstype bezier` / `cstype rat bezier` get tessellated.
    // Spline / cardinal / Taylor / basis-matrix / NURBS forms stay as
    // captured directives so a future evaluator can pick them up.
    let text = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
cstype bspline
deg 2
curv 0.0 1.0 1 2 3
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(text.as_bytes())
        .unwrap();
    // No synthetic curve mesh — B-spline isn't a Bezier variant.
    assert!(
        scene.meshes.is_empty(),
        "B-spline curves must not produce a tessellated mesh"
    );
    // Directives still captured for round-trip.
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(!dirs.is_empty());
}

#[test]
fn cstype_end_terminates_active_basis() {
    // After `end`, subsequent `curv` directives outside any `cstype`
    // header are ignored by the tessellator. The directive sequence
    // is still captured (the parser doesn't filter free-form keywords
    // by context).
    let text = "\
v 0 0 0
v 1 0 0
v 0 1 0
cstype bezier
deg 1
curv 0 1 1 2
end
curv 0 1 1 3
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    // Only one tessellated primitive (the second `curv` ran after `end`).
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.meshes[0].primitives.len(), 1);
}

#[test]
fn curv_with_negative_indices_resolves_against_position_count() {
    // Negative `curv` control-point indices count from the end of the
    // position pool, same convention as `f`.
    let text = "\
v 0 0 0
v 1 0 0
v 2 0 0
v 3 0 0
cstype bezier
deg 1
curv 0 1 -2 -1
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(2)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    // Linear Bezier from v3 (x=2) to v4 (x=3): endpoints are 2 and 3.
    assert!((prim.positions[0][0] - 2.0).abs() < 1e-5);
    assert!((prim.positions[2][0] - 3.0).abs() < 1e-5);
}

#[test]
fn double_decoded_round_trip_preserves_directives() {
    // Decode (tessellate) → encode → decode (tessellate) for an OBJ
    // that mixes polygonal geometry + free-form curves. The polygonal
    // `v` lines survive the re-encode, so the second tessellation
    // produces the same curve geometry as the first.
    let text = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
v 4.0 0.0 0.0
v 5.0 0.0 0.0
f 4 5 6
cstype bezier
deg 2
curv 0.0 1.0 1 2 3
parm u 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(text.as_bytes())
        .unwrap();
    let bytes = ObjEncoder::new().encode(&scene).unwrap();

    let scene2 = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(&bytes)
        .unwrap();
    // Two meshes: polygonal triangle + tessellated curve.
    assert_eq!(scene2.meshes.len(), 2);

    // Find the curve mesh on each scene (mesh order is preserved but
    // matched by name to be robust).
    let curve_mesh_a = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:curves"))
        .unwrap();
    let curve_mesh_b = scene2
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:curves"))
        .unwrap();

    let p0 = &curve_mesh_a.primitives[0];
    let p1 = &curve_mesh_b.primitives[0];
    assert_eq!(p0.positions.len(), p1.positions.len());
    for (a, b) in p0.positions.iter().zip(p1.positions.iter()) {
        assert!(
            (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5 && (a[2] - b[2]).abs() < 1e-5,
            "tessellation drifted on round-trip: {:?} vs {:?}",
            a,
            b
        );
    }
}
