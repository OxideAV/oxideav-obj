//! Multi-patch Bezier `surf` surface tessellation —
//! `ObjDecoder::with_curve_tessellation(N)` now decomposes a `surf`
//! whose control mesh spans more than one Bezier patch per direction
//! (spec §"Bezier": "the number of global parameter values given with
//! the parm statement must be K/n + 1, where K is the number of
//! control points. For surfaces, this requirement applies independently
//! for the u and v parametric directions.") into a per-patch tensor-
//! product de Casteljau evaluation on the shared boundary control mesh
//! (spec §"Surface vertex data — Control points": "the control points
//! are ordered as if the surface were a single large patch").
//!
//! Spec references: §"Bezier" (K/n + 1 parm formula), §"Surface vertex
//! data — Control points" (single-large-patch ordering), §"surf s0 s1
//! t0 t1 v1/vt1/vn1 …" (element statement).

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_mesh3d::Topology;
use oxideav_obj::ObjDecoder;

/// Two-patch bilinear (`deg 1 1`) Bezier surface — three control points
/// along u, two along v. Spec §"Bezier" `parm u 0 1 2` (length 3) means
/// `K/n + 1 = 3` ⇒ `K = 2 × 1 = 2` ⇒ `K + 1 = 3` total control points
/// along u. With `parm v 0 1` (length 2) we have one v-patch and two v
/// control points. Total grid: 3 × 2 = 6 control points across two
/// adjacent unit-square patches sharing their middle column.
///
/// Layout (1-based v indices):
///   j = 0:  v1 = (0,0,0)   v2 = (1,0,0)   v3 = (2,0,0)
///   j = 1:  v4 = (0,1,0)   v5 = (1,1,0)   v6 = (2,1,0)
///
/// A bilinear Bezier surface is the plane through its four corners, so
/// both patches together describe a planar 2×1 rectangle. Every sample
/// must land on z = 0; sample (u, v) = (k/N · 2, m/N · 1) must equal
/// (2k/N, m/N, 0).
const TWO_PATCH_BILINEAR_U: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
v 2.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 2.0 0.0 1.0 1 2 3 4 5 6
parm u 0.0 1.0 2.0
parm v 0.0 1.0
end
";

#[test]
fn two_patch_bilinear_u_tessellates_into_a_flat_rectangle() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(TWO_PATCH_BILINEAR_U.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "one synthetic surface mesh expected");
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("obj:surfaces"));
    assert_eq!(mesh.primitives.len(), 1);

    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::Triangles);
    // 4 samples ⇒ 5×5 = 25 lattice vertices, 4×4 cells × 2 tris × 3 = 96 indices.
    assert_eq!(prim.positions.len(), 25);
    assert_eq!(prim.indices.as_ref().unwrap().len(), 96);

    // Every sample sits on the z = 0 plane (both bilinear patches are
    // planar; the patch seam at u = 1 contributes nothing in z).
    for p in &prim.positions {
        assert!(p[2].abs() < 1e-5, "vertex off the z=0 plane: {p:?}");
    }

    // Spec §"Bezier" provenance: 2 u-patches, 1 v-patch.
    let patches = prim
        .extras
        .get("obj:surface_patches")
        .and_then(|v| v.as_array())
        .expect("multi-patch marker present");
    assert_eq!(patches[0].as_u64(), Some(2));
    assert_eq!(patches[1].as_u64(), Some(1));

    let stride = 5usize;
    // The four corners of the global parameter rectangle must
    // interpolate v1, v3, v4, v6 (spec: Bezier surfaces pass through
    // their corner control points; the multi-patch concatenation
    // preserves the global corners because the seam falls strictly
    // inside the rectangle).
    let c00 = prim.positions[0]; // (u=0, v=0) ⇒ v1
    let c10 = prim.positions[stride - 1]; // (u=2, v=0) ⇒ v3
    let c01 = prim.positions[(stride - 1) * stride]; // (u=0, v=1) ⇒ v4
    let c11 = prim.positions[stride * stride - 1]; // (u=2, v=1) ⇒ v6
    assert!((c00[0]).abs() < 1e-5 && (c00[1]).abs() < 1e-5);
    assert!((c10[0] - 2.0).abs() < 1e-5 && (c10[1]).abs() < 1e-5);
    assert!((c01[0]).abs() < 1e-5 && (c01[1] - 1.0).abs() < 1e-5);
    assert!((c11[0] - 2.0).abs() < 1e-5 && (c11[1] - 1.0).abs() < 1e-5);

    // Mid-edge sample (su = 2, sv = 0): the global parameter is
    // (2 × 2/4, 0) = (1.0, 0), which is exactly the patch seam at v2.
    let seam = prim.positions[2];
    assert!(
        (seam[0] - 1.0).abs() < 1e-5 && (seam[1]).abs() < 1e-5,
        "patch seam should hit v2 = (1, 0, 0), got {seam:?}"
    );
}

/// Two-patch bilinear (`deg 1 1`) Bezier surface — one patch along u,
/// two patches along v. Mirror of the u-test to exercise the v
/// decomposition path independently.
const TWO_PATCH_BILINEAR_V: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
v 0.0 2.0 0.0
v 1.0 2.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 2.0 1 2 3 4 5 6
parm u 0.0 1.0
parm v 0.0 1.0 2.0
end
";

#[test]
fn two_patch_bilinear_v_tessellates_into_a_flat_rectangle() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(TWO_PATCH_BILINEAR_V.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.positions.len(), 25);

    let patches = prim
        .extras
        .get("obj:surface_patches")
        .and_then(|v| v.as_array())
        .expect("multi-patch marker present");
    assert_eq!(patches[0].as_u64(), Some(1));
    assert_eq!(patches[1].as_u64(), Some(2));

    let stride = 5usize;
    // Corners of the 1×2 rectangle.
    let c00 = prim.positions[0];
    let c10 = prim.positions[stride - 1];
    let c01 = prim.positions[(stride - 1) * stride];
    let c11 = prim.positions[stride * stride - 1];
    assert!((c00[0]).abs() < 1e-5 && (c00[1]).abs() < 1e-5);
    assert!((c10[0] - 1.0).abs() < 1e-5 && (c10[1]).abs() < 1e-5);
    assert!((c01[0]).abs() < 1e-5 && (c01[1] - 2.0).abs() < 1e-5);
    assert!((c11[0] - 1.0).abs() < 1e-5 && (c11[1] - 2.0).abs() < 1e-5);

    // Mid-column sample (su = 0, sv = 2): global v = 1.0 (the seam) at
    // u = 0 lands on v3 = (0, 1, 0).
    let seam = prim.positions[2 * stride];
    assert!(
        (seam[0]).abs() < 1e-5 && (seam[1] - 1.0).abs() < 1e-5,
        "patch seam should hit v3 = (0, 1, 0), got {seam:?}"
    );
}

/// Spec §"Surface vertex data — Control points" — the canonical four-
/// patch bicubic Bezier example. Each patch needs `(degu + 1) × (degv + 1)
/// = 4 × 4 = 16` control points; four patches arranged 2×2 share boundary
/// rows and columns, so the global grid is `(3 × 2 + 1) × (3 × 2 + 1) =
/// 7 × 7 = 49` control points. The surface is built as a planar
/// 2 × 2 rectangle in the z = 0 plane (all 49 control points at z = 0,
/// laid out on a uniform 7 × 7 lattice from (0,0) to (2,2)).
///
/// Spec §"Bezier" `parm` formula: `K/n + 1 = 6/3 + 1 = 3` per direction,
/// so `parm u 0 1 2` and `parm v 0 1 2` (length 3 each ⇒ two patches).
fn four_patch_planar_source() -> String {
    let mut s = String::new();
    // 7 × 7 = 49 vertices on a uniform planar grid.
    for j in 0..7 {
        for i in 0..7 {
            let x = i as f32 / 3.0;
            let y = j as f32 / 3.0;
            s.push_str(&format!("v {x} {y} 0.0\n"));
        }
    }
    s.push_str("cstype bezier\n");
    s.push_str("deg 3 3\n");
    s.push_str("surf 0.0 2.0 0.0 2.0");
    for i in 1..=49 {
        s.push_str(&format!(" {i}"));
    }
    s.push('\n');
    s.push_str("parm u 0.0 1.0 2.0\n");
    s.push_str("parm v 0.0 1.0 2.0\n");
    s.push_str("end\n");
    s
}

#[test]
fn four_patch_bicubic_planar_surface_stays_flat() {
    let src = four_patch_planar_source();
    let scene = ObjDecoder::new()
        .with_curve_tessellation(6)
        .decode(src.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    // 6 samples ⇒ 7×7 = 49 lattice vertices.
    assert_eq!(prim.positions.len(), 49);

    // Provenance: 2 × 2 patches.
    let patches = prim
        .extras
        .get("obj:surface_patches")
        .and_then(|v| v.as_array())
        .expect("multi-patch marker present");
    assert_eq!(patches[0].as_u64(), Some(2));
    assert_eq!(patches[1].as_u64(), Some(2));

    let stride = 7usize;
    // The planar 7 × 7 control mesh sits at z = 0 everywhere; every
    // sample of every bicubic patch (and across patch seams) must
    // also be z = 0.
    for p in &prim.positions {
        assert!(p[2].abs() < 1e-4, "vertex off the z=0 plane: {p:?}");
    }
    // Corners are interpolated by Bezier — they should match the
    // explicit corner control points (rows 0, 3, 6 × cols 0, 3, 6).
    let c00 = prim.positions[0];
    let c11 = prim.positions[stride * stride - 1];
    assert!((c00[0]).abs() < 1e-4 && (c00[1]).abs() < 1e-4);
    assert!((c11[0] - 2.0).abs() < 1e-4 && (c11[1] - 2.0).abs() < 1e-4);

    // The interior seam sample at (u, v) = (1, 1) lives at lattice
    // index (3, 3) since 6 samples × 1/2 = 3.
    let centre = prim.positions[3 * stride + 3];
    assert!(
        (centre[0] - 1.0).abs() < 1e-4 && (centre[1] - 1.0).abs() < 1e-4,
        "centre seam should hit (1, 1, 0), got {centre:?}"
    );
}

/// Single-patch Bezier surface must still emit no `obj:surface_patches`
/// marker — the marker is reserved for multi-patch decomposition.
const SINGLE_PATCH_BICUBIC: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
v 2.0 1.0 0.0
v 3.0 1.0 0.0
v 0.0 2.0 0.0
v 1.0 2.0 0.0
v 2.0 2.0 0.0
v 3.0 2.0 0.0
v 0.0 3.0 0.0
v 1.0 3.0 0.0
v 2.0 3.0 0.0
v 3.0 3.0 0.0
cstype bezier
deg 3 3
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
parm u 0.0 1.0
parm v 0.0 1.0
end
";

#[test]
fn single_patch_bicubic_does_not_emit_multipatch_marker() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(SINGLE_PATCH_BICUBIC.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert!(
        !prim.extras.contains_key("obj:surface_patches"),
        "single-patch surfaces must not carry the multi-patch marker"
    );
}

/// Multi-patch surface whose control count doesn't match the
/// `parm`-implied grid extent stays captured-only (spec §"Bezier"
/// requires `K = degu × (parm_count − 1)` exactly per direction).
const MULTIPATCH_WITH_WRONG_CONTROL_COUNT: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 2.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0 2.0
parm v 0.0 1.0
end
";

#[test]
fn multipatch_with_wrong_control_count_is_skipped() {
    // `parm u` count 3 with `deg 1 1` implies 3 u-control-points, but
    // only 4 control vertices are given (need 3 × 2 = 6). The surface
    // must be left captured-only — no synthetic mesh emitted, but the
    // directive sequence still rides on Scene3D::extras for round-trip.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(MULTIPATCH_WITH_WRONG_CONTROL_COUNT.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "mismatched control count must leave the surface captured-only"
    );
}

/// Rational multi-patch Bezier (`cstype rat bezier`) — each control
/// point carries its own weight. Same two-patch bilinear topology as
/// `TWO_PATCH_BILINEAR_U` but the middle column (the patch seam) is
/// double-weighted, which pulls the seam sample toward it relative to
/// the unweighted midpoint.
const TWO_PATCH_RAT_BILINEAR: &str = "\
v 0.0 0.0 0.0 1.0
v 1.0 0.0 0.0 2.0
v 2.0 0.0 0.0 1.0
v 0.0 1.0 0.0 1.0
v 1.0 1.0 0.0 2.0
v 2.0 1.0 0.0 1.0
cstype rat bezier
deg 1 1
surf 0.0 2.0 0.0 1.0 1 2 3 4 5 6
parm u 0.0 1.0 2.0
parm v 0.0 1.0
end
";

#[test]
fn rational_multipatch_seam_uses_per_vertex_weights() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(TWO_PATCH_RAT_BILINEAR.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    // 5 × 5 lattice ⇒ 25 vertices.
    assert_eq!(prim.positions.len(), 25);

    // Spec §"Bezier" multi-patch boundary sharing — the seam sample at
    // (u = 1, v = 0) sits exactly on the shared control point v2,
    // which the rational projection still hits exactly because the
    // boundary blend collapses to a single weighted point.
    let seam = prim.positions[2];
    assert!(
        (seam[0] - 1.0).abs() < 1e-4 && (seam[1]).abs() < 1e-4 && (seam[2]).abs() < 1e-4,
        "weighted seam should land on v2 = (1, 0, 0), got {seam:?}"
    );

    assert_eq!(
        prim.extras.get("obj:surface_kind").and_then(|v| v.as_str()),
        Some("rat_bezier")
    );
}
