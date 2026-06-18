//! `stech cparma ures vres` / `stech cparmb uvres` surface-approximation
//! resolution — when a free-form `cstype … end` surface block carries a
//! constant-parametric `stech` directive, the tessellator honours the
//! file's requested subdivision density instead of the caller's uniform
//! `with_curve_tessellation(N)` budget. This is the surface analog of the
//! `ctech cparm res` curve override.
//!
//! Spec §"stech technique resolution" (`stech cparma ures vres`):
//!   "Specifies a surface with constant parametric subdivision using
//!    separate resolution parameters for the u and v directions. Each
//!    patch of the surface is subdivided n times in parameter space,
//!    where n is the resolution parameter multiplied by the degree of the
//!    surface. … If you enter a value of 0 for both ures and vres, each
//!    patch is approximated by two triangles."
//!
//! Spec §"stech cparmb uvres":
//!   "Specifies a surface with constant parametric subdivision, with
//!    refinement using one resolution parameter for both the u and v
//!    directions."
//!
//! Only the parametric `cparma` / `cparmb` techniques are honoured for
//! density — the geometric `cspace maxlength` / `curv maxdist maxangle`
//! techniques need iterative spatial / curvature refinement and remain on
//! the caller's uniform sample budget. The shared isotropic surface
//! lattice is driven from the finer of the two per-direction `n` counts.

use oxideav_mesh3d::{Mesh3DDecoder, Topology};
use oxideav_obj::ObjDecoder;

/// A degree-2×2 Bezier patch over a 3×3 planar control grid (z = 0). The
/// control points are listed row-major with u varying fastest (spec
/// §"Surface vertex data — control points"):
///
///   j = 0:  v1=(0,0,0)   v2=(1,0,0)   v3=(2,0,0)
///   j = 1:  v4=(0,1,0)   v5=(1,1,0)   v6=(2,1,0)
///   j = 2:  v7=(0,2,0)   v8=(1,2,0)   v9=(2,2,0)
const BIQUADRATIC_SURF_HEADER: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
v 2.0 1.0 0.0
v 0.0 2.0 0.0
v 1.0 2.0 0.0
v 2.0 2.0 0.0
cstype bezier
deg 2 2
";

const SURF_BODY: &str = "\
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9
parm u 0.0 1.0
parm v 0.0 1.0
end
";

fn with_stech(stech_line: &str) -> String {
    format!("{BIQUADRATIC_SURF_HEADER}{stech_line}{SURF_BODY}")
}

#[test]
fn stech_cparma_drives_lattice_density_from_res_times_degree() {
    // Caller asks for 2 samples; the file's `stech cparma 2 2` overrides
    // it to n = round(2 × deg) = round(2 × 2) = 4 subdivisions per
    // direction ⇒ a 5×5 = 25 vertex lattice (not the caller's 3×3 = 9).
    let text = with_stech("stech cparma 2.0 2.0\n");
    let scene = ObjDecoder::new()
        .with_curve_tessellation(2)
        .decode(text.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1);
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.topology, Topology::Triangles);
    assert_eq!(
        prim.positions.len(),
        25,
        "stech cparma 2 × deg 2 ⇒ 4 subdivisions ⇒ 5×5 lattice, not the caller's 3×3"
    );

    // Every vertex lies on the planar control grid (z = 0).
    for p in &prim.positions {
        assert!(p[2].abs() < 1e-5, "vertex off the z=0 plane: {p:?}");
    }

    // Provenance: surface_samples reports the effective (overridden) count
    // and the source resolution pair is recorded verbatim.
    assert_eq!(
        prim.extras
            .get("obj:surface_samples")
            .and_then(|v| v.as_u64()),
        Some(4)
    );
    let res = prim
        .extras
        .get("obj:surface_stech_cparm_res")
        .and_then(|v| v.as_array())
        .expect("stech override provenance");
    assert_eq!(res.len(), 2);
    assert!((res[0].as_f64().unwrap() - 2.0).abs() < 1e-9);
    assert!((res[1].as_f64().unwrap() - 2.0).abs() < 1e-9);
}

#[test]
fn stech_cparma_anisotropic_uses_finer_direction() {
    // `stech cparma 1 3`: n_u = round(1 × 2) = 2, n_v = round(3 × 2) = 6.
    // The shared isotropic lattice takes the finer (max) ⇒ 6 subdivisions
    // ⇒ 7×7 = 49 vertices, so the coarser u direction is not
    // under-sampled.
    let text = with_stech("stech cparma 1.0 3.0\n");
    let scene = ObjDecoder::new()
        .with_curve_tessellation(2)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.positions.len(),
        49,
        "max(n_u=2, n_v=6) = 6 subdivisions ⇒ 7×7 lattice"
    );
    assert_eq!(
        prim.extras
            .get("obj:surface_samples")
            .and_then(|v| v.as_u64()),
        Some(6)
    );
    let res = prim
        .extras
        .get("obj:surface_stech_cparm_res")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!((res[0].as_f64().unwrap() - 1.0).abs() < 1e-9);
    assert!((res[1].as_f64().unwrap() - 3.0).abs() < 1e-9);
}

#[test]
fn stech_cparmb_single_resolution_applies_to_both_directions() {
    // `stech cparmb 3`: one value for both directions ⇒ n = round(3 × 2)
    // = 6 ⇒ 7×7 = 49 vertices. Provenance reports the value in both slots.
    let text = with_stech("stech cparmb 3.0\n");
    let scene = ObjDecoder::new()
        .with_curve_tessellation(2)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.positions.len(),
        49,
        "cparmb 3 × deg 2 ⇒ 6 subdivisions"
    );
    let res = prim
        .extras
        .get("obj:surface_stech_cparm_res")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!((res[0].as_f64().unwrap() - 3.0).abs() < 1e-9);
    assert!((res[1].as_f64().unwrap() - 3.0).abs() < 1e-9);
}

#[test]
fn stech_cparma_zero_collapses_to_two_triangles_per_patch() {
    // Spec: "If you enter a value of 0 for both ures and vres, each patch
    // is approximated by two triangles." ⇒ n = 1 subdivision ⇒ a 2×2 = 4
    // vertex cell, 2 triangles (6 indices), regardless of the caller's
    // larger budget.
    let text = with_stech("stech cparma 0.0 0.0\n");
    let scene = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.positions.len(),
        4,
        "cparma 0 0 ⇒ one subdivision ⇒ two triangles per patch"
    );
    assert_eq!(prim.indices.as_ref().expect("indices").len(), 6);
}

#[test]
fn no_stech_uses_caller_uniform_budget() {
    // No `stech` directive ⇒ the caller's uniform 3-sample budget governs
    // ⇒ 4×4 = 16 vertices, and no override provenance is emitted.
    let text = with_stech("");
    let scene = ObjDecoder::new()
        .with_curve_tessellation(3)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.positions.len(),
        16,
        "(3 + 1)^2 lattice on caller budget"
    );
    assert_eq!(
        prim.extras
            .get("obj:surface_samples")
            .and_then(|v| v.as_u64()),
        Some(3)
    );
    assert!(
        !prim.extras.contains_key("obj:surface_stech_cparm_res"),
        "no stech directive ⇒ no override provenance"
    );
}

#[test]
fn stech_cspace_is_not_honoured_and_keeps_uniform_budget() {
    // The geometric `stech cspace maxlength` technique requires iterative
    // real-space refinement we don't perform ⇒ the caller's uniform
    // budget still governs and no override provenance is emitted.
    let text = with_stech("stech cspace 0.01\n");
    let scene = ObjDecoder::new()
        .with_curve_tessellation(3)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.positions.len(),
        16,
        "cspace technique unsupported ⇒ caller's 3 + 1 lattice"
    );
    assert!(!prim.extras.contains_key("obj:surface_stech_cparm_res"));
}

#[test]
fn stech_resolution_does_not_leak_across_blocks() {
    // `stech cparma` density applies independently per `cstype … end`
    // block: a `stech`-bearing block and a plain block in the same file
    // get their own lattice counts (the override does not leak past
    // `end`).
    let block_a = format!("{BIQUADRATIC_SURF_HEADER}stech cparma 2.0 2.0\n{SURF_BODY}");
    // Second block reuses the same control grid (indices 1..9 are still
    // valid — the `v` lines accumulate, but a fresh deg/surf re-reads the
    // first 9 positions).
    let block_b = "\
cstype bezier
deg 2 2
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let text = format!("{block_a}{block_b}");
    let scene = ObjDecoder::new()
        .with_curve_tessellation(2)
        .decode(text.as_bytes())
        .unwrap();
    let prims = &scene.meshes[0].primitives;
    assert_eq!(prims.len(), 2, "two tessellated surface primitives");

    // Block A: stech cparma 2 × deg 2 = 4 subdivisions ⇒ 5×5 = 25.
    assert_eq!(prims[0].positions.len(), 25);
    assert!(prims[0].extras.contains_key("obj:surface_stech_cparm_res"));

    // Block B: no stech ⇒ caller's 2 + 1 = 3 ⇒ 3×3 = 9, no override.
    assert_eq!(prims[1].positions.len(), 9);
    assert!(!prims[1].extras.contains_key("obj:surface_stech_cparm_res"));
}
