//! Taylor polynomial curve tessellation — `ObjDecoder::with_curve_tessellation(N)`
//! evaluates every `cstype taylor` `curv` directive via Horner's-rule
//! polynomial evaluation `P(t) = Σ c_i · t^i` (component-wise per axis,
//! per spec §"Taylor") and emits a real `Topology::LineStrip` primitive
//! on the synthetic `"obj:curves"` mesh. The directive sequence is still
//! preserved on `Scene3D::extras` so the encoder replays the original
//! free-form section unchanged.
//!
//! Spec references: §"Curve and surface type" (cstype), §"Degree" (deg),
//! §"Curve" (curv), §"Taylor" (polynomial coefficients as control
//! points), and §"Taylor curve" example (degree-4 polynomial verbatim).

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder};

/// The spec's degree-4 Taylor example (§"Taylor curve" example) —
/// coefficients for:
///   x(t) =  3.00 +  2.30 t +  7.98 t² +  8.30 t³ +  6.34 t⁴
///   y(t) =  1.00 - 10.10 t +  5.40 t² -  4.70 t³ +  2.03 t⁴
///   z(t) = -2.50 +  0.50 t -  7.00 t² + 18.10 t³ +  0.08 t⁴
/// evaluated between global parameters 0.5 and 1.6.
const SPEC_TAYLOR_EXAMPLE: &str = "\
v 3.000 1.000 -2.500
v 2.300 -10.100 0.500
v 7.980 5.400 -7.000
v 8.300 -4.700 18.100
v 6.340 2.030 0.080
cstype taylor
deg 4
curv 0.5 1.6 1 2 3 4 5
parm u 0.0 2.0
end
";

fn eval_spec_taylor(t: f32) -> [f32; 3] {
    let x = 3.000 + 2.300 * t + 7.980 * t * t + 8.300 * t.powi(3) + 6.340 * t.powi(4);
    let y = 1.000 - 10.100 * t + 5.400 * t * t - 4.700 * t.powi(3) + 2.030 * t.powi(4);
    let z = -2.500 + 0.500 * t - 7.000 * t * t + 18.100 * t.powi(3) + 0.080 * t.powi(4);
    [x, y, z]
}

#[test]
fn spec_taylor_example_matches_analytic_polynomial() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(11)
        .decode(SPEC_TAYLOR_EXAMPLE.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "one synthetic mesh expected");
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("obj:curves"));
    assert_eq!(mesh.primitives.len(), 1);

    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::LineStrip);
    assert_eq!(prim.positions.len(), 12, "samples + 1 vertices");

    // Sample every emitted point against the analytic polynomial at the
    // corresponding parameter value t = 0.5 + i / 11 * (1.6 - 0.5).
    for (i, p) in prim.positions.iter().enumerate() {
        let t = 0.5 + (i as f32 / 11.0) * (1.6 - 0.5);
        let want = eval_spec_taylor(t);
        for k in 0..3 {
            assert!(
                (p[k] - want[k]).abs() < 5e-3,
                "sample {i} (t={t}) axis {k}: got {}, want {}",
                p[k],
                want[k]
            );
        }
    }

    // Provenance extras.
    assert_eq!(
        prim.extras.get("obj:curve_kind").and_then(|v| v.as_str()),
        Some("taylor")
    );
    assert_eq!(
        prim.extras.get("obj:curve_degree").and_then(|v| v.as_u64()),
        Some(4)
    );
    assert_eq!(
        prim.extras
            .get("obj:curve_samples")
            .and_then(|v| v.as_u64()),
        Some(11)
    );
    assert_eq!(
        prim.extras
            .get("obj:tessellated_curve")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    let range = prim
        .extras
        .get("obj:curve_u_range")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!((range[0].as_f64().unwrap() - 0.5).abs() < 1e-6);
    assert!((range[1].as_f64().unwrap() - 1.6).abs() < 1e-6);
}

/// Linear (degree-1) Taylor — coefficients (c_0, c_1) mean
/// P(t) = c_0 + c_1 · t. Two control points; the polyline must trace a
/// straight segment from c_0 (at t=0) to c_0 + c_1 (at t=1).
const LINEAR_TAYLOR: &str = "\
v 1.0 2.0 3.0
v 4.0 -1.0 0.5
cstype taylor
deg 1
curv 0.0 1.0 1 2
parm u 0.0 1.0
end
";

#[test]
fn linear_taylor_traces_straight_segment() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(LINEAR_TAYLOR.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.positions.len(), 5);

    // Sample 0: t=0 ⇒ P = c_0 = (1, 2, 3).
    let p0 = prim.positions[0];
    assert!(
        (p0[0] - 1.0).abs() < 1e-5 && (p0[1] - 2.0).abs() < 1e-5 && (p0[2] - 3.0).abs() < 1e-5,
        "t=0 sample = {p0:?}, want (1, 2, 3)"
    );

    // Sample 4: t=1 ⇒ P = c_0 + c_1 = (5, 1, 3.5).
    let p4 = prim.positions[4];
    assert!(
        (p4[0] - 5.0).abs() < 1e-5 && (p4[1] - 1.0).abs() < 1e-5 && (p4[2] - 3.5).abs() < 1e-5,
        "t=1 sample = {p4:?}, want (5, 1, 3.5)"
    );

    // Midpoint: t=0.5 ⇒ P = (1 + 4·0.5, 2 + (-1)·0.5, 3 + 0.5·0.5)
    //                     = (3, 1.5, 3.25).
    let mid = prim.positions[2];
    assert!(
        (mid[0] - 3.0).abs() < 1e-5 && (mid[1] - 1.5).abs() < 1e-5 && (mid[2] - 3.25).abs() < 1e-5,
        "t=0.5 sample = {mid:?}, want (3, 1.5, 3.25)"
    );
}

/// A `curv` whose control-point count doesn't match `deg + 1` is
/// invalid for Taylor — the directive is captured but no synthetic
/// primitive is emitted.
const DEGREE_MISMATCH: &str = "\
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
cstype taylor
deg 4
curv 0.0 1.0 1 2 3
parm u 0.0 1.0
end
";

#[test]
fn taylor_degree_mismatch_is_skipped() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(DEGREE_MISMATCH.as_bytes())
        .unwrap();
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.name.as_deref() != Some("obj:curves")),
        "deg/control-point mismatch must not tessellate"
    );
    assert!(scene.extras.contains_key("obj:freeform_directives"));
}

/// The encoder must filter the synthetic Taylor primitive and replay
/// only the source `cstype taylor` / `curv` block.
#[test]
fn taylor_directives_round_trip_through_encoder() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(SPEC_TAYLOR_EXAMPLE.as_bytes())
        .unwrap();
    let buf = ObjEncoder::new().encode(&scene).unwrap();
    let text = String::from_utf8(buf).unwrap();
    assert!(text.contains("cstype taylor"));
    assert!(text.contains("curv 0.5 1.6 1 2 3 4 5"));
    assert!(text.contains("\nend\n"));
    let v_count = text.lines().filter(|l| l.starts_with("v ")).count();
    assert_eq!(
        v_count, 5,
        "expected 5 source v lines, got {v_count}:\n{text}"
    );
}

/// Default `samples == 0` ⇒ no synthetic tessellation; directive still
/// rides on extras.
#[test]
fn taylor_default_samples_zero_skips_tessellation() {
    let scene = ObjDecoder::new()
        .decode(SPEC_TAYLOR_EXAMPLE.as_bytes())
        .unwrap();
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.name.as_deref() != Some("obj:curves")),
        "samples == 0 must not produce synthetic mesh"
    );
    assert!(scene.extras.contains_key("obj:freeform_directives"));
}
