//! 2D trimming-curve (`curv2`) tessellation —
//! `ObjDecoder::with_curve_tessellation(N)` evaluates every `curv2`
//! directive (the parameter-space trimming / special / connectivity
//! curve, spec §"curv2") into a `LineStrip` polyline on a synthetic
//! mesh named `"obj:curves2"`. The curve uses the active `cstype`
//! basis but operates on `vp` parameter vertices in 2D parameter
//! space, so the output positions are `(u, v, 0)`. The directive
//! sequence is still preserved on `Scene3D::extras` so the encoder
//! replays the original free-form section unchanged.
//!
//! Spec references: §"curv2" (2D curve on a surface), §"vp u v w"
//! (parameter vertices + optional rational weight), §"Curve and
//! surface type" (cstype), §"parm u …" (knot vector for the B-spline
//! window), §"Trimming loops and holes" (trim / hole referencing the
//! curv2 index).

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder, obj};

/// A degree-2 Bezier 2D curve in parameter space. The three `vp`
/// control points form a right-angle triangle in `(u, v)`, so the
/// de Casteljau midpoint (t = 0.5) lands at `(1, 0.5)`. This is the
/// 2D analogue of the 3D `quadratic_bezier` test but driven off `vp`
/// parameter vertices via `curv2`.
const QUADRATIC_CURV2: &str = "\
vp 0.0 0.0
vp 1.0 1.0
vp 2.0 0.0
cstype bezier
deg 2
curv2 1 2 3
parm u 0.0 1.0
end
";

#[test]
fn quadratic_curv2_tessellates_into_a_parameter_space_line_strip() {
    // Without the option, the curv2 stays as a captured directive only.
    let bare = ObjDecoder::new()
        .decode(QUADRATIC_CURV2.as_bytes())
        .unwrap();
    assert!(
        bare.meshes.is_empty(),
        "default decoder must not synthesise curv2 meshes"
    );

    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(QUADRATIC_CURV2.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "one synthetic mesh expected");
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("obj:curves2"));
    assert_eq!(mesh.primitives.len(), 1);

    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::LineStrip);
    assert_eq!(prim.positions.len(), 9, "samples + 1 vertices on the strip");

    // Endpoints in parameter space: t = 0 ⇒ (0,0), t = 1 ⇒ (2,0).
    let p0 = prim.positions[0];
    let pn = prim.positions[8];
    assert!((p0[0]).abs() < 1e-5 && (p0[1]).abs() < 1e-5);
    assert!((pn[0] - 2.0).abs() < 1e-5 && (pn[1]).abs() < 1e-5);
    // The curve is flat in parameter space — z stays 0 everywhere.
    assert!(prim.positions.iter().all(|p| p[2].abs() < 1e-6));

    // Midpoint at t = 0.5: (1, 0.5).
    let mid = prim.positions[4];
    assert!(
        (mid[0] - 1.0).abs() < 1e-5 && (mid[1] - 0.5).abs() < 1e-5,
        "midpoint mismatch: {mid:?}"
    );

    // Provenance extras, incl. the 2D-parameter marker.
    assert_eq!(
        prim.extras.get("obj:curve2").and_then(|v| v.as_bool()),
        Some(true)
    );
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
    assert_eq!(
        prim.extras
            .get("obj:tessellated_curve")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
}

/// Rational `curv2` — the spec §"Special point" example shape: a
/// `rat bezier` 2D curve whose `vp` parameter vertices carry an
/// optional 3rd `w` weight. With the middle weight = 2 the curve is
/// pulled toward the middle control point exactly as the 3D rational
/// Bezier is.
const RAT_CURV2: &str = "\
vp 0.0 0.0 1.0
vp 1.0 1.0 2.0
vp 2.0 0.0 1.0
cstype rat bezier
deg 2
curv2 1 2 3
parm u 0.0 1.0
end
";

#[test]
fn rational_curv2_midpoint_uses_the_vp_weight() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(RAT_CURV2.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.positions.len(), 9);

    // Same homogeneous blend as the 3D rational Bezier test:
    //   y_mid = (0.5·2·1) / (0.25·1 + 0.5·2 + 0.25·1) = 1.0 / 1.5.
    let mid = prim.positions[4];
    assert!(
        (mid[1] - (1.0 / 1.5)).abs() < 1e-4,
        "rational curv2 midpoint = {mid:?}, expected v ≈ 0.6667"
    );
    assert_eq!(
        prim.extras.get("obj:curve_kind").and_then(|v| v.as_str()),
        Some("rat_bezier")
    );
}

/// B-spline `curv2` — the parameter range / knot window comes from the
/// `parm u` body statement (a `curv2` line carries no inline `u0 u1`).
/// A clamped quadratic knot vector over three control points produces a
/// curve whose endpoints interpolate the first / last control point.
const BSPLINE_CURV2: &str = "\
vp 0.0 0.0
vp 1.0 2.0
vp 2.0 0.0
cstype bspline
deg 2
curv2 1 2 3
parm u 0.0 0.0 0.0 1.0 1.0 1.0
end
";

#[test]
fn bspline_curv2_uses_parm_u_for_its_window() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(BSPLINE_CURV2.as_bytes())
        .unwrap();
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("obj:curves2"));
    let prim = &mesh.primitives[0];
    assert_eq!(prim.positions.len(), 9);
    // Clamped quadratic: endpoints interpolate first/last control point.
    assert!((prim.positions[0][0]).abs() < 1e-4 && (prim.positions[0][1]).abs() < 1e-4);
    assert!((prim.positions[8][0] - 2.0).abs() < 1e-4 && (prim.positions[8][1]).abs() < 1e-4);
    // u-range echoes the parm-u extents (0 .. 1).
    let r = prim
        .extras
        .get("obj:curve_u_range")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!((r[0].as_f64().unwrap()).abs() < 1e-6);
    assert!((r[1].as_f64().unwrap() - 1.0).abs() < 1e-6);
}

#[test]
fn curv2_with_negative_indices_resolves_against_vp_count() {
    // Negative `curv2` control-point indices count from the end of the
    // `vp` pool, matching the §"curv2" relative-index convention and
    // the §"Special point" example (`curv2 -6 -5 …`).
    let text = "\
vp 0.0 0.0
vp 1.0 0.0
vp 2.0 0.0
vp 3.0 0.0
cstype bezier
deg 1
curv2 -2 -1
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(2)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    // Linear curve from vp3 (u=2) to vp4 (u=3): endpoints are 2 and 3.
    assert!((prim.positions[0][0] - 2.0).abs() < 1e-5);
    assert!((prim.positions[2][0] - 3.0).abs() < 1e-5);
}

#[test]
fn curv2_and_curv_coexist_on_separate_meshes() {
    // A free-form section with both a 3D `curv` (off `v` vertices) and
    // a 2D `curv2` (off `vp` vertices) produces two distinct synthetic
    // meshes: `obj:curves` and `obj:curves2`.
    let text = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
vp 0.0 0.0
vp 1.0 1.0
vp 2.0 0.0
cstype bezier
deg 2
curv 0.0 1.0 1 2 3
curv2 1 2 3
parm u 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    assert!(
        scene
            .meshes
            .iter()
            .any(|m| m.name.as_deref() == Some("obj:curves")),
        "3D curve mesh expected"
    );
    assert!(
        scene
            .meshes
            .iter()
            .any(|m| m.name.as_deref() == Some("obj:curves2")),
        "2D curv2 mesh expected"
    );
}

#[test]
fn tessellated_curv2_is_filtered_out_by_the_encoder() {
    // Decoder tessellates the curv2 → encoder must skip the synthetic
    // polyline and replay the original directives. The sample points
    // must NOT leak into the `v` pool or as `l` lines, and no
    // `o obj:curves2` block is emitted.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(QUADRATIC_CURV2.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "synthetic curv2 mesh present");

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();

    // The 17 tessellation sample points must not surface as `v` lines.
    let v_lines = text.lines().filter(|l| l.starts_with("v ")).count();
    assert_eq!(
        v_lines, 0,
        "tessellation samples leaked as `v` lines:\n{text}"
    );
    assert!(
        !text.contains("o obj:curves2"),
        "synthetic curv2 mesh should not be re-emitted as a polygonal `o` block"
    );
    assert!(
        !text.lines().any(|l| l.starts_with("l ")),
        "encoder must not emit `l` lines for tessellated curv2:\n{text}"
    );

    // The directive sequence (incl. the `vp` pool + curv2) comes back.
    for keyword in [
        "vp 0",
        "cstype bezier",
        "deg 2",
        "curv2 1 2 3",
        "parm u 0",
        "end",
    ] {
        assert!(
            text.lines().any(|l| l.starts_with(keyword)),
            "missing `{keyword}` line in:\n{text}"
        );
    }
}

#[test]
fn curv2_round_trip_is_stable() {
    // Decode (tessellate) → encode → decode (tessellate): the second
    // tessellation must reproduce the first curve geometry exactly,
    // since the `vp` pool + curv2 directives survive the re-encode.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(QUADRATIC_CURV2.as_bytes())
        .unwrap();
    let bytes = ObjEncoder::new().encode(&scene).unwrap();

    let scene2 = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(&bytes)
        .unwrap();

    let a = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:curves2"))
        .unwrap();
    let b = scene2
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:curves2"))
        .unwrap();
    let p0 = &a.primitives[0];
    let p1 = &b.primitives[0];
    assert_eq!(p0.positions.len(), p1.positions.len());
    for (x, y) in p0.positions.iter().zip(p1.positions.iter()) {
        assert!(
            (x[0] - y[0]).abs() < 1e-5 && (x[1] - y[1]).abs() < 1e-5 && (x[2] - y[2]).abs() < 1e-5,
            "curv2 tessellation drifted on round-trip: {x:?} vs {y:?}"
        );
    }
}

#[test]
fn curv2_too_few_control_points_is_left_alone() {
    // A `curv2` with a single control point is malformed (the spec
    // requires a minimum of two); it must not produce a mesh.
    let text = "\
vp 0.0 0.0
cstype bezier
deg 1
curv2 1
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "single-control-point curv2 must not produce a mesh"
    );
    // Still captured for round-trip.
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(!dirs.is_empty());
}
