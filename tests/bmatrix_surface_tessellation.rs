//! Basis-matrix `surf` surface tessellation —
//! `ObjDecoder::with_curve_tessellation(N)` evaluates every `surf`
//! element under a `cstype bmatrix` (or `cstype rat bmatrix`) header
//! into a real `Topology::Triangles` grid on a synthetic mesh named
//! `"obj:surfaces"`, via the bivariate tensor-product polynomial:
//!
//! ```text
//!   S(u, v) = Σ_a Σ_b ( Σ_p B_u[a][p] · u^p )
//!                     ( Σ_q B_v[b][q] · v^q )
//!                     · c_{base_u + a, base_v + b}
//! ```
//!
//! where `B_u` / `B_v` are the per-direction `(n + 1) × (n + 1)`
//! basis matrices from `bmat u` / `bmat v` (row-major, column index
//! varying fastest per spec §"bmat u/v matrix"), and the per-direction
//! segment strides come from `step stepu stepv` (spec §"step stepu
//! stepv" — "For surfaces, the above description applies independently
//! to each parametric direction.").
//!
//! Per-direction control-grid extent is the inverse of the spec
//! §"Basis matrix" `parm = (K − n) / s + 2` relation:
//! `K = (parm − 2) · s + n + 1`. The free-form directive sequence is
//! still preserved on `Scene3D::extras["obj:freeform_directives"]` so
//! the encoder replays the original `cstype` / `deg` / `bmat u` /
//! `bmat v` / `step` / `parm u` / `parm v` / `surf` / `end` block
//! unchanged.
//!
//! Spec references: §"Curve and surface type" (cstype), §"Degree"
//! (deg degu degv), §"bmat u/v matrix", §"step stepu stepv",
//! §"Basis matrix", §"surf s0 s1 t0 t1 v1/vt1/vn1 …",
//! §"Surface vertex data — control points",
//! §"Free-form curve/surface body statements".

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder};

/// Cubic Bezier basis-matrix surface — the spec §"Basis matrix
/// Examples" Example 1 (Cubic Bezier surface made with a basis matrix):
///
/// ```text
///   cstype bmatrix
///   deg 3 3
///   step 3 3
///   bmat u 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
///   bmat v 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
/// ```
///
/// The 4 × 4 control grid sits over the unit square in the xy-plane.
/// A single cubic Bezier patch interpolates its four corner control
/// points exactly (Bernstein basis sums to 1, endpoint coefficients are
/// the only non-zero terms at t ∈ {0, 1}), and the centre sample at
/// (u = 0.5, v = 0.5) is the corner average plus the centre block of
/// the bilinear blend. Since all 16 control points lie on z = 0 every
/// sample also lands on z = 0.
const BEZIER_BMATRIX_SURF: &str = "\
v 0.0 0.0 0.0
v 0.333 0.0 0.0
v 0.666 0.0 0.0
v 1.0 0.0 0.0
v 0.0 0.333 0.0
v 0.333 0.333 0.0
v 0.666 0.333 0.0
v 1.0 0.333 0.0
v 0.0 0.666 0.0
v 0.333 0.666 0.0
v 0.666 0.666 0.0
v 1.0 0.666 0.0
v 0.0 1.0 0.0
v 0.333 1.0 0.0
v 0.666 1.0 0.0
v 1.0 1.0 0.0
cstype bmatrix
deg 3 3
step 3 3
bmat u 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
bmat v 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
parm u 0.0 1.0
parm v 0.0 1.0
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
end
";

#[test]
fn default_decoder_does_not_tessellate_bmatrix_surfaces() {
    let bare = ObjDecoder::new()
        .decode(BEZIER_BMATRIX_SURF.as_bytes())
        .unwrap();
    assert!(
        bare.meshes.is_empty(),
        "default decoder must not synthesise bmatrix-surface meshes"
    );
}

#[test]
fn bezier_bmatrix_surface_tessellates_into_a_triangle_grid() {
    // 4 samples ⇒ a 5×5 vertex lattice = 25 vertices, 4×4 cells × 2
    // triangles × 3 indices = 96 indices.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(BEZIER_BMATRIX_SURF.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "one synthetic surface mesh expected");
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("obj:surfaces"));
    assert_eq!(mesh.primitives.len(), 1);

    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::Triangles);
    assert_eq!(prim.positions.len(), 25, "(samples + 1)^2 lattice vertices");
    let indices = prim.indices.as_ref().expect("triangle indices");
    assert_eq!(indices.len(), 96, "4*4 cells * 2 tris * 3 verts");

    // All control points are at z = 0; every sample must land on z = 0.
    for p in &prim.positions {
        assert!(p[2].abs() < 1e-5, "vertex off the z=0 plane: {p:?}");
    }

    let stride = 5usize;
    // Corner interpolation: lattice corner (su, sv) lands at index
    // `sv * stride + su`. A single-patch cubic Bezier interpolates its
    // four corner control points exactly.
    let c00 = prim.positions[0]; // (u=0, v=0) ⇒ control point 1
    let c10 = prim.positions[stride - 1]; // (u=1, v=0) ⇒ point 4
    let c01 = prim.positions[(stride - 1) * stride]; // (u=0, v=1) ⇒ point 13
    let c11 = prim.positions[stride * stride - 1]; // (u=1, v=1) ⇒ point 16
    assert!(
        (c00[0] - 0.0).abs() < 1e-4 && (c00[1] - 0.0).abs() < 1e-4,
        "c00 = {c00:?}, want (0, 0, 0)"
    );
    assert!(
        (c10[0] - 1.0).abs() < 1e-4 && (c10[1] - 0.0).abs() < 1e-4,
        "c10 = {c10:?}, want (1, 0, 0)"
    );
    assert!(
        (c01[0] - 0.0).abs() < 1e-4 && (c01[1] - 1.0).abs() < 1e-4,
        "c01 = {c01:?}, want (0, 1, 0)"
    );
    assert!(
        (c11[0] - 1.0).abs() < 1e-4 && (c11[1] - 1.0).abs() < 1e-4,
        "c11 = {c11:?}, want (1, 1, 0)"
    );

    // Centre sample: the four interior control points form a regular
    // 2×2 subgrid centred at (0.5, 0.5), so the cubic Bezier centre is
    // also (0.5, 0.5, 0) up to a small float-rounding tolerance.
    let centre = prim.positions[2 * stride + 2];
    assert!(
        (centre[0] - 0.5).abs() < 5e-3 && (centre[1] - 0.5).abs() < 5e-3,
        "centre sample mismatch: {centre:?}"
    );

    // Provenance extras.
    assert_eq!(
        prim.extras
            .get("obj:tessellated_curve")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        prim.extras
            .get("obj:tessellated_surface")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        prim.extras.get("obj:surface_kind").and_then(|v| v.as_str()),
        Some("bmatrix")
    );
    let degree = prim
        .extras
        .get("obj:surface_degree")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(degree[0].as_u64(), Some(3));
    assert_eq!(degree[1].as_u64(), Some(3));
    assert_eq!(
        prim.extras
            .get("obj:surface_samples")
            .and_then(|v| v.as_u64()),
        Some(4)
    );
}

/// Bezier basis-matrix surface raised off the plane. The four corner
/// control points stay at z = 0 (they're the bilinear corners), but the
/// 2×2 interior block sits at z = 1 — a classic Bezier "bump" patch.
/// The corners must interpolate exactly; the centre sample lifts but
/// stays well below 1 (Bezier-style attenuation).
const RAISED_BEZIER_BMATRIX_SURF: &str = "\
v 0.0 0.0 0.0
v 0.333 0.0 0.0
v 0.666 0.0 0.0
v 1.0 0.0 0.0
v 0.0 0.333 0.0
v 0.333 0.333 1.0
v 0.666 0.333 1.0
v 1.0 0.333 0.0
v 0.0 0.666 0.0
v 0.333 0.666 1.0
v 0.666 0.666 1.0
v 1.0 0.666 0.0
v 0.0 1.0 0.0
v 0.333 1.0 0.0
v 0.666 1.0 0.0
v 1.0 1.0 0.0
cstype bmatrix
deg 3 3
step 3 3
bmat u 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
bmat v 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
parm u 0.0 1.0
parm v 0.0 1.0
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
end
";

#[test]
fn raised_bezier_bmatrix_surface_corners_match_centre_lifts() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(RAISED_BEZIER_BMATRIX_SURF.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    let stride = 9usize;

    // Corners pinned to z = 0.
    for (label, idx) in [
        ("c00", 0usize),
        ("c10", stride - 1),
        ("c01", (stride - 1) * stride),
        ("c11", stride * stride - 1),
    ] {
        let p = prim.positions[idx];
        assert!(p[2].abs() < 1e-4, "{label} z = {}, want 0", p[2]);
    }

    // Centre lifts but Bezier-attenuates well below 1.
    let centre = prim.positions[4 * stride + 4];
    assert!(
        centre[2] > 0.3 && centre[2] < 0.8,
        "centre z = {} out of plausible 0.3..0.8 band",
        centre[2]
    );
}

/// Round-trip stability: the `cstype bmatrix` surface block (including
/// `bmat u`, `bmat v`, `step stepu stepv`, `parm u`, `parm v`) survives
/// a decode → encode → decode cycle. Synthetic tessellated meshes are
/// filtered by the encoder so the polygonal section stays empty.
#[test]
fn bmatrix_surface_directives_round_trip_verbatim() {
    let scene1 = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(BEZIER_BMATRIX_SURF.as_bytes())
        .unwrap();
    let bytes = ObjEncoder::new().encode(&scene1).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.lines().any(|l| l.starts_with("cstype bmatrix")),
        "missing `cstype bmatrix` in output:\n{text}"
    );
    assert!(
        text.lines().any(|l| l.starts_with("step 3 3")),
        "missing `step 3 3` in output:\n{text}"
    );
    assert!(
        text.lines().any(|l| l.starts_with("bmat u ")),
        "missing `bmat u` line:\n{text}"
    );
    assert!(
        text.lines().any(|l| l.starts_with("bmat v ")),
        "missing `bmat v` line:\n{text}"
    );
    assert!(
        text.lines().any(|l| l.starts_with("surf ")),
        "missing `surf` line:\n{text}"
    );

    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let d1 = scene1.extras.get("obj:freeform_directives").unwrap();
    let d2 = scene2.extras.get("obj:freeform_directives").unwrap();
    assert_eq!(d1, d2, "round-trip not stable");
}

/// Malformed bmatrix surface — missing `bmat v` — is silently skipped.
/// No synthetic mesh appears, but the directive sequence still rides on
/// `Scene3D::extras` so the encoder still replays the captured block.
const MISSING_BMAT_V_SURF: &str = "\
v 0.0 0.0 0.0
v 0.333 0.0 0.0
v 0.666 0.0 0.0
v 1.0 0.0 0.0
v 0.0 0.333 0.0
v 0.333 0.333 0.0
v 0.666 0.333 0.0
v 1.0 0.333 0.0
v 0.0 0.666 0.0
v 0.333 0.666 0.0
v 0.666 0.666 0.0
v 1.0 0.666 0.0
v 0.0 1.0 0.0
v 0.333 1.0 0.0
v 0.666 1.0 0.0
v 1.0 1.0 0.0
cstype bmatrix
deg 3 3
step 3 3
bmat u 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
parm u 0.0 1.0
parm v 0.0 1.0
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
end
";

#[test]
fn bmatrix_surface_missing_bmat_v_skipped_silently() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(MISSING_BMAT_V_SURF.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty() || scene.meshes.iter().all(|m| m.primitives.is_empty()),
        "expected no synthetic mesh, got {} mesh(es)",
        scene.meshes.len()
    );
    // Directive block still captured for round-trip.
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

/// Missing `step` (mandatory for surfaces per spec §"step stepu stepv":
/// "stepv is required only for surfaces. There is no default. A value
/// must be supplied.") is silently skipped.
const MISSING_STEP_SURF: &str = "\
v 0.0 0.0 0.0
v 0.333 0.0 0.0
v 0.666 0.0 0.0
v 1.0 0.0 0.0
v 0.0 0.333 0.0
v 0.333 0.333 0.0
v 0.666 0.333 0.0
v 1.0 0.333 0.0
v 0.0 0.666 0.0
v 0.333 0.666 0.0
v 0.666 0.666 0.0
v 1.0 0.666 0.0
v 0.0 1.0 0.0
v 0.333 1.0 0.0
v 0.666 1.0 0.0
v 1.0 1.0 0.0
cstype bmatrix
deg 3 3
bmat u 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
bmat v 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
parm u 0.0 1.0
parm v 0.0 1.0
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
end
";

#[test]
fn bmatrix_surface_missing_step_skipped_silently() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(MISSING_STEP_SURF.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty() || scene.meshes.iter().all(|m| m.primitives.is_empty()),
        "expected no synthetic mesh, got {} mesh(es)",
        scene.meshes.len()
    );
}

/// Multi-patch basis-matrix surface: a 4×7 control grid with
/// `step 3 3` decomposes into two patches in the u direction (and a
/// single patch in v) per spec §"step stepu stepv" / §"Basis matrix":
/// `K_u = (parm_u − 2) · stepu + degu + 1 = (3 − 2)·3 + 3 + 1 = 7`,
/// `K_v = (parm_v − 2) · stepv + degv + 1 = (2 − 2)·3 + 3 + 1 = 4`.
///
/// Both patches sit in the z = 0 plane, so every sample lands there.
/// The leftmost u column lies on x = 0 and the rightmost on x = 2
/// (each unit-square patch is 1 wide), and the four global corners
/// match the corner control points of patches 0 and 1.
const MULTI_PATCH_BMATRIX_SURF: &str = "\
v 0.0 0.0 0.0
v 0.333 0.0 0.0
v 0.666 0.0 0.0
v 1.0 0.0 0.0
v 1.333 0.0 0.0
v 1.666 0.0 0.0
v 2.0 0.0 0.0
v 0.0 0.333 0.0
v 0.333 0.333 0.0
v 0.666 0.333 0.0
v 1.0 0.333 0.0
v 1.333 0.333 0.0
v 1.666 0.333 0.0
v 2.0 0.333 0.0
v 0.0 0.666 0.0
v 0.333 0.666 0.0
v 0.666 0.666 0.0
v 1.0 0.666 0.0
v 1.333 0.666 0.0
v 1.666 0.666 0.0
v 2.0 0.666 0.0
v 0.0 1.0 0.0
v 0.333 1.0 0.0
v 0.666 1.0 0.0
v 1.0 1.0 0.0
v 1.333 1.0 0.0
v 1.666 1.0 0.0
v 2.0 1.0 0.0
cstype bmatrix
deg 3 3
step 3 3
bmat u 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
bmat v 1 -3 3 -1 0 3 -6 3 0 0 3 -3 0 0 0 1
parm u 0.0 1.0 2.0
parm v 0.0 1.0
surf 0.0 2.0 0.0 1.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28
end
";

#[test]
fn multi_patch_bmatrix_surface_corners_match_endpoints() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(6)
        .decode(MULTI_PATCH_BMATRIX_SURF.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    let stride = 7usize;

    // Every sample on z = 0 (the whole control grid is planar).
    for p in &prim.positions {
        assert!(p[2].abs() < 1e-4, "off-plane sample: {p:?}");
    }

    // Global corners interpolate the four control-grid corners.
    let c00 = prim.positions[0];
    let c10 = prim.positions[stride - 1];
    let c01 = prim.positions[(stride - 1) * stride];
    let c11 = prim.positions[stride * stride - 1];
    assert!(
        (c00[0] - 0.0).abs() < 1e-4 && (c00[1] - 0.0).abs() < 1e-4,
        "c00 = {c00:?}, want (0, 0)"
    );
    assert!(
        (c10[0] - 2.0).abs() < 1e-4 && (c10[1] - 0.0).abs() < 1e-4,
        "c10 = {c10:?}, want (2, 0)"
    );
    assert!(
        (c01[0] - 0.0).abs() < 1e-4 && (c01[1] - 1.0).abs() < 1e-4,
        "c01 = {c01:?}, want (0, 1)"
    );
    assert!(
        (c11[0] - 2.0).abs() < 1e-4 && (c11[1] - 1.0).abs() < 1e-4,
        "c11 = {c11:?}, want (2, 1)"
    );
}

/// Tessellation disabled (default) ⇒ no synthetic primitives; the
/// directive sequence still round-trips.
#[test]
fn tessellation_disabled_skips_bmatrix_surface_evaluation() {
    let scene = ObjDecoder::new()
        .decode(BEZIER_BMATRIX_SURF.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "expected no synthetic mesh when tessellation off, got {}",
        scene.meshes.len()
    );
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
