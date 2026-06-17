//! `ctech cparm res` curve-approximation resolution — when a free-form
//! `cstype … end` block carries a `ctech cparm res` directive, the
//! tessellator honours the file's requested subdivision density instead
//! of the caller's uniform `with_curve_tessellation(N)` budget.
//!
//! Spec §"ctech technique resolution" (`ctech cparm res`):
//!   "Specifies a curve with constant parametric subdivision using one
//!    resolution parameter. Each polynomial segment of the curve is
//!    subdivided n times in parameter space, where n is the resolution
//!    parameter multiplied by the degree of the curve. … If res has a
//!    value of 0, each polynomial curve segment is represented by a
//!    single line segment."
//!
//! Only the parametric `cparm` technique is honoured here — the
//! geometric `cspace maxlength` / `curv maxdist maxangle` techniques
//! need iterative chord-length / curvature refinement and remain on the
//! uniform sample budget.

use oxideav_mesh3d::{Mesh3DDecoder, Topology};
use oxideav_obj::ObjDecoder;

/// A degree-2 Bezier with a `ctech cparm 4` directive. Per spec the
/// subdivision count is `n = res × degree = 4 × 2 = 8`, so the strip
/// carries `8 + 1 = 9` vertices regardless of the caller passing a
/// different `with_curve_tessellation` budget.
const CPARM_BEZIER: &str = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
cstype bezier
deg 2
ctech cparm 4
curv 0.0 1.0 1 2 3
parm u 0.0 1.0
end
";

#[test]
fn ctech_cparm_drives_sample_count_from_res_times_degree() {
    // Caller asks for 3 samples; the file's `ctech cparm 4` overrides it
    // to 4 × 2 = 8 subdivisions ⇒ 9 vertices.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(3)
        .decode(CPARM_BEZIER.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1);
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.topology, Topology::LineStrip);
    assert_eq!(
        prim.positions.len(),
        9,
        "ctech cparm 4 × deg 2 ⇒ 8 subdivisions ⇒ 9 vertices, not the caller's 3"
    );

    // Endpoint exactness is unaffected.
    assert!((prim.positions[0][0] - 0.0).abs() < 1e-5);
    assert!((prim.positions[8][0] - 2.0).abs() < 1e-5);

    // The midpoint (sample 4 of 8) still lands on the degree-2 Bezier.
    let mid = prim.positions[4];
    assert!(
        (mid[0] - 1.0).abs() < 1e-5 && (mid[1] - 0.5).abs() < 1e-5,
        "midpoint mismatch: {mid:?}"
    );

    // Provenance: curve_samples reports the effective (overridden) count
    // and the source resolution is recorded.
    assert_eq!(
        prim.extras
            .get("obj:curve_samples")
            .and_then(|v| v.as_u64()),
        Some(8)
    );
    assert_eq!(
        prim.extras
            .get("obj:curve_ctech_cparm_res")
            .and_then(|v| v.as_f64()),
        Some(4.0)
    );
}

/// `ctech cparm 0` ⇒ each segment is a single line segment ⇒ one
/// subdivision ⇒ two vertices (just the endpoints).
#[test]
fn ctech_cparm_zero_collapses_to_a_single_line_segment() {
    let text = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
cstype bezier
deg 2
ctech cparm 0
curv 0.0 1.0 1 2 3
parm u 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.positions.len(),
        2,
        "res 0 ⇒ single line segment ⇒ two endpoint vertices"
    );
    assert!((prim.positions[0][0] - 0.0).abs() < 1e-5);
    assert!((prim.positions[1][0] - 2.0).abs() < 1e-5);
}

/// A block with no `ctech` directive falls back to the caller's uniform
/// `with_curve_tessellation` budget unchanged, and emits no
/// `obj:curve_ctech_cparm_res` provenance.
#[test]
fn no_ctech_uses_caller_uniform_budget() {
    let text = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
cstype bezier
deg 2
curv 0.0 1.0 1 2 3
parm u 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(5)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.positions.len(),
        6,
        "5 + 1 vertices on the caller budget"
    );
    assert_eq!(
        prim.extras
            .get("obj:curve_samples")
            .and_then(|v| v.as_u64()),
        Some(5)
    );
    assert!(
        !prim.extras.contains_key("obj:curve_ctech_cparm_res"),
        "no ctech directive ⇒ no override provenance"
    );
}

/// The geometric `ctech cspace` / `ctech curv` techniques are NOT
/// honoured for density — they require iterative refinement — so the
/// caller's uniform budget still governs and no override provenance is
/// emitted.
#[test]
fn ctech_cspace_is_not_honoured_and_keeps_uniform_budget() {
    let text = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
cstype bezier
deg 2
ctech cspace 0.01
curv 0.0 1.0 1 2 3
parm u 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(7)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.positions.len(),
        8,
        "cspace technique unsupported ⇒ caller's 7 + 1 vertices"
    );
    assert!(!prim.extras.contains_key("obj:curve_ctech_cparm_res"));
}

/// `ctech cparm` density applies independently per `cstype … end`
/// block: a `ctech`-bearing block and a plain block in the same file get
/// their own sample counts (the override does not leak across `end`).
#[test]
fn ctech_resolution_does_not_leak_across_blocks() {
    let text = "\
v 0.0 0.0 0.0
v 1.0 1.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
v 4.0 1.0 0.0
v 5.0 0.0 0.0
cstype bezier
deg 2
ctech cparm 3
curv 0.0 1.0 1 2 3
parm u 0.0 1.0
end
cstype bezier
deg 2
curv 0.0 1.0 4 5 6
parm u 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    let prims = &scene.meshes[0].primitives;
    assert_eq!(prims.len(), 2, "two tessellated curve primitives");

    // Block 1: ctech cparm 3 × deg 2 = 6 subdivisions ⇒ 7 vertices.
    assert_eq!(prims[0].positions.len(), 7);
    assert_eq!(
        prims[0]
            .extras
            .get("obj:curve_ctech_cparm_res")
            .and_then(|v| v.as_f64()),
        Some(3.0)
    );

    // Block 2: no ctech ⇒ caller's 4 + 1 = 5 vertices, no override.
    assert_eq!(prims[1].positions.len(), 5);
    assert!(!prims[1].extras.contains_key("obj:curve_ctech_cparm_res"));
}
