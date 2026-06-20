//! Tessellation of the superseded `cdc` (Cardinal curve) and `bzp`
//! (Bezier patch) statements via
//! `ObjDecoder::with_curve_tessellation(N)`.
//!
//! Spec §"Superseded statements" §"cdc v1 v2 v3 v4 …" (Cardinal curve,
//! min four control points, cubic Catmull-Rom basis), §"bzp v1 … v16"
//! (16-point bicubic Bezier patch), and §"res useg vseg" (per-direction
//! segment-count reference/display statement, clamped 3..=120). The
//! geometry of the `bzp` form is pinned by §"Comparison of 2.11 and 3.0
//! syntax" §"Bezier patch", which writes the same patch as both
//! `bzp 1 … 16` and the modern `cstype bezier` / `surf` form.
//!
//! Both passes emit synthetic primitives carrying the shared
//! `obj:tessellated_curve = true` sentinel so the encoder drops them on
//! re-emit while the verbatim `cdc` / `bzp` / `res` lines remain the
//! round-trip source of truth.

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder};

fn mesh_named<'a>(
    scene: &'a oxideav_mesh3d::Scene3D,
    name: &str,
) -> Option<&'a oxideav_mesh3d::Mesh> {
    scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some(name))
}

// ---------------------------------------------------------------------------
// cdc — Cardinal curve
// ---------------------------------------------------------------------------

/// The spec's §"Comparison" §"Cardinal curve" listing: six planar
/// control points, `cdc 1 2 3 4 5 6`. The 3.0 equivalent is
/// `cstype cardinal` / `curv 0 3 1 2 3 4 5 6`, so the `cdc` form must
/// trace the identical cubic Catmull-Rom polyline interpolating the
/// interior control points c2..c5.
const SPEC_CDC: &str = "\
v 2.570000 1.280000 0.000000
v 0.940000 1.340000 0.000000
v -0.670000 0.820000 0.000000
v -0.770000 -0.940000 0.000000
v 1.030000 -1.350000 0.000000
v 3.070000 -1.310000 0.000000
cdc 1 2 3 4 5 6
";

#[test]
fn cdc_tessellates_to_linestrip_interpolating_interior_points() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(12)
        .decode(SPEC_CDC.as_bytes())
        .unwrap();

    let mesh = mesh_named(&scene, "obj:superseded_curves")
        .expect("synthetic superseded-curve mesh expected");
    assert_eq!(mesh.primitives.len(), 1);
    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::LineStrip);
    assert_eq!(
        prim.extras.get("obj:curve_kind").and_then(|v| v.as_str()),
        Some("cdc")
    );
    assert_eq!(
        prim.extras.get("obj:curve_degree").and_then(|v| v.as_u64()),
        Some(3),
        "Cardinal is cubic-only per spec"
    );

    // Cardinal interpolates every interior control point exactly. The
    // curve must pass through c2 = (0.94, 1.34) and c5 = (1.03, -1.35).
    let hits_c2 = prim
        .positions
        .iter()
        .any(|p| (p[0] - 0.94).abs() < 1e-3 && (p[1] - 1.34).abs() < 1e-3);
    let hits_c5 = prim
        .positions
        .iter()
        .any(|p| (p[0] - 1.03).abs() < 1e-3 && (p[1] - (-1.35)).abs() < 1e-3);
    assert!(hits_c2, "curve must interpolate c2");
    assert!(hits_c5, "curve must interpolate c5");

    // Planar input ⇒ planar output.
    for p in &prim.positions {
        assert!(p[2].abs() < 1e-5, "z must stay 0, got {p:?}");
    }
}

#[test]
fn cdc_matches_modern_cardinal_curve_geometry() {
    // The spec asserts `cdc 1..6` and `cstype cardinal / curv 0 3 1..6`
    // describe the SAME curve. Tessellate both and compare vertex-for-
    // vertex at the same sample budget.
    let modern = "\
v 2.570000 1.280000 0.000000
v 0.940000 1.340000 0.000000
v -0.670000 0.820000 0.000000
v -0.770000 -0.940000 0.000000
v 1.030000 -1.350000 0.000000
v 3.070000 -1.310000 0.000000
cstype cardinal
deg 3
curv 0.0 3.0 1 2 3 4 5 6
parm u 0.0 1.0 2.0 3.0
end
";
    let s_old = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(SPEC_CDC.as_bytes())
        .unwrap();
    let s_new = ObjDecoder::new()
        .with_curve_tessellation(16)
        .decode(modern.as_bytes())
        .unwrap();

    let p_old = &mesh_named(&s_old, "obj:superseded_curves")
        .unwrap()
        .primitives[0];
    let p_new = &mesh_named(&s_new, "obj:curves").unwrap().primitives[0];
    assert_eq!(p_old.positions.len(), p_new.positions.len());
    for (a, b) in p_old.positions.iter().zip(p_new.positions.iter()) {
        for k in 0..3 {
            assert!(
                (a[k] - b[k]).abs() < 1e-4,
                "cdc vs modern Cardinal mismatch: {a:?} vs {b:?}"
            );
        }
    }
}

#[test]
fn cdc_res_overrides_segment_count() {
    // `res useg vseg` sets the per-cubic-segment subdivision count. The
    // SPEC_CDC curve has 6 control points ⇒ 3 cubic segments. With
    // `res 6` the curve is sampled at 6+1 vertices regardless of the
    // caller's uniform budget.
    let text = "v 0 0 0\nv 1 0 0\nv 2 1 0\nv 3 1 0\nv 4 0 0\nres 6\ncdc 1 2 3 4 5\n";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(40)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &mesh_named(&scene, "obj:superseded_curves")
        .unwrap()
        .primitives[0];
    assert_eq!(
        prim.extras
            .get("obj:curve_samples")
            .and_then(|v| v.as_u64()),
        Some(6),
        "res 6 must drive the sample budget"
    );
    assert_eq!(
        prim.extras
            .get("obj:curve_res_useg")
            .and_then(|v| v.as_u64()),
        Some(6)
    );
    assert_eq!(prim.positions.len(), 7);
}

#[test]
fn cdc_res_clamped_to_spec_range() {
    // Spec §"res useg vseg": minimum 3, maximum 120. `res 1` clamps up
    // to 3; `res 999` clamps down to 120.
    let lo = "v 0 0 0\nv 1 0 0\nv 2 0 0\nv 3 0 0\nres 1\ncdc 1 2 3 4\n";
    let s = ObjDecoder::new()
        .with_curve_tessellation(50)
        .decode(lo.as_bytes())
        .unwrap();
    let p = &mesh_named(&s, "obj:superseded_curves").unwrap().primitives[0];
    assert_eq!(
        p.extras.get("obj:curve_samples").and_then(|v| v.as_u64()),
        Some(3)
    );

    let hi = "v 0 0 0\nv 1 0 0\nv 2 0 0\nv 3 0 0\nres 999\ncdc 1 2 3 4\n";
    let s = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(hi.as_bytes())
        .unwrap();
    let p = &mesh_named(&s, "obj:superseded_curves").unwrap().primitives[0];
    assert_eq!(
        p.extras.get("obj:curve_samples").and_then(|v| v.as_u64()),
        Some(120)
    );
}

#[test]
fn cdc_too_few_control_points_is_skipped() {
    // Spec §"cdc": "A minimum of four vertex numbers are required."
    let text = "v 0 0 0\nv 1 0 0\nv 2 0 0\ncdc 1 2 3\n";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(text.as_bytes())
        .unwrap();
    assert!(
        mesh_named(&scene, "obj:superseded_curves").is_none(),
        "3-point cdc is malformed and must not tessellate"
    );
}

#[test]
fn cdc_negative_indices_resolve_from_end() {
    // Negative indices are relative-from-end per spec §"cdc".
    let abs = "v 0 0 0\nv 1 0 0\nv 2 1 0\nv 3 0 0\ncdc 1 2 3 4\n";
    let neg = "v 0 0 0\nv 1 0 0\nv 2 1 0\nv 3 0 0\ncdc -4 -3 -2 -1\n";
    let sa = ObjDecoder::new()
        .with_curve_tessellation(10)
        .decode(abs.as_bytes())
        .unwrap();
    let sn = ObjDecoder::new()
        .with_curve_tessellation(10)
        .decode(neg.as_bytes())
        .unwrap();
    let pa = &mesh_named(&sa, "obj:superseded_curves").unwrap().primitives[0];
    let pn = &mesh_named(&sn, "obj:superseded_curves").unwrap().primitives[0];
    assert_eq!(pa.positions, pn.positions);
}

// ---------------------------------------------------------------------------
// bzp — Bezier patch
// ---------------------------------------------------------------------------

/// The spec's §"Comparison" §"Bezier patch" listing verbatim: sixteen
/// planar control points spanning the [-5, 5]² square in the z = 0
/// plane, `bzp 1 … 16`.
const SPEC_BZP: &str = "\
v -5.000000 -5.000000 0.000000
v -5.000000 -1.666667 0.000000
v -5.000000 1.666667 0.000000
v -5.000000 5.000000 0.000000
v -1.666667 -5.000000 0.000000
v -1.666667 -1.666667 0.000000
v -1.666667 1.666667 0.000000
v -1.666667 5.000000 0.000000
v 1.666667 -5.000000 0.000000
v 1.666667 -1.666667 0.000000
v 1.666667 1.666667 0.000000
v 1.666667 5.000000 0.000000
v 5.000000 -5.000000 0.000000
v 5.000000 -1.666667 0.000000
v 5.000000 1.666667 0.000000
v 5.000000 5.000000 0.000000
bzp 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
";

#[test]
fn bzp_tessellates_to_triangles_with_normals() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(SPEC_BZP.as_bytes())
        .unwrap();
    let mesh = mesh_named(&scene, "obj:superseded_surfaces")
        .expect("synthetic superseded-surface mesh expected");
    assert_eq!(mesh.primitives.len(), 1);
    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::Triangles);
    assert_eq!(
        prim.extras.get("obj:surface_kind").and_then(|v| v.as_str()),
        Some("bzp")
    );
    // (4 + 1)² lattice = 25 vertices, 4×4 cells × 2 triangles × 3 = 96
    // indices.
    assert_eq!(prim.positions.len(), 25);
    let idx_len = match prim.indices.as_ref().unwrap() {
        oxideav_mesh3d::Indices::U16(v) => v.len(),
        oxideav_mesh3d::Indices::U32(v) => v.len(),
    };
    assert_eq!(idx_len, 96);
    let normals = prim.normals.as_ref().expect("per-vertex normals");
    assert_eq!(normals.len(), 25);
}

#[test]
fn bzp_flat_patch_stays_planar_and_spans_control_hull() {
    // The spec patch is a flat z = 0 grid. A bicubic Bezier over a planar
    // control mesh is planar, and the corner control points are
    // interpolated exactly, so the tessellated surface must lie in z = 0
    // and reach the [-5, 5]² extent.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(6)
        .decode(SPEC_BZP.as_bytes())
        .unwrap();
    let prim = &mesh_named(&scene, "obj:superseded_surfaces")
        .unwrap()
        .primitives[0];

    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for p in &prim.positions {
        assert!(
            p[2].abs() < 1e-4,
            "flat patch must stay in z = 0, got {p:?}"
        );
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_y = min_y.min(p[1]);
        max_y = max_y.max(p[1]);
    }
    // Corner control points are interpolated ⇒ extent reaches ±5.
    assert!((min_x - (-5.0)).abs() < 1e-3, "min x = {min_x}");
    assert!((max_x - 5.0).abs() < 1e-3, "max x = {max_x}");
    assert!((min_y - (-5.0)).abs() < 1e-3, "min y = {min_y}");
    assert!((max_y - 5.0).abs() < 1e-3, "max y = {max_y}");

    // Flat patch ⇒ all normals point along ±z.
    for n in prim.normals.as_ref().unwrap() {
        assert!(
            n[0].abs() < 1e-3 && n[1].abs() < 1e-3 && (n[2].abs() - 1.0).abs() < 1e-3,
            "flat-patch normal must be ±z, got {n:?}"
        );
    }
}

#[test]
fn bzp_matches_modern_surf_patch_geometry() {
    // Spec §"Comparison" §"Bezier patch": `bzp 1..16` and the modern
    // `cstype bezier / deg 3 3 / surf … 13 14 15 16 9 10 11 12 5 6 7 8
    // 1 2 3 4` describe the SAME patch. The modern surf re-lists the
    // bzp rows in reverse v order, so the two tessellations cover the
    // identical point set (the v parameterisation direction flips). We
    // assert the bounding boxes and z-planarity agree.
    let modern = "\
v -5.000000 -5.000000 0.000000
v -5.000000 -1.666667 0.000000
v -5.000000 1.666667 0.000000
v -5.000000 5.000000 0.000000
v -1.666667 -5.000000 0.000000
v -1.666667 -1.666667 0.000000
v -1.666667 1.666667 0.000000
v -1.666667 5.000000 0.000000
v 1.666667 -5.000000 0.000000
v 1.666667 -1.666667 0.000000
v 1.666667 1.666667 0.000000
v 1.666667 5.000000 0.000000
v 5.000000 -5.000000 0.000000
v 5.000000 -1.666667 0.000000
v 5.000000 1.666667 0.000000
v 5.000000 5.000000 0.000000
cstype bezier
deg 3 3
surf 0.0 1.0 0.0 1.0 13 14 15 16 9 10 11 12 5 6 7 8 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let s_old = ObjDecoder::new()
        .with_curve_tessellation(5)
        .decode(SPEC_BZP.as_bytes())
        .unwrap();
    let s_new = ObjDecoder::new()
        .with_curve_tessellation(5)
        .decode(modern.as_bytes())
        .unwrap();
    let p_old = &mesh_named(&s_old, "obj:superseded_surfaces")
        .unwrap()
        .primitives[0];
    let p_new = &mesh_named(&s_new, "obj:surfaces").unwrap().primitives[0];

    assert_eq!(p_old.positions.len(), p_new.positions.len());
    // Same point set (order may differ by the v flip) ⇒ same sorted
    // coordinate multiset.
    let sort_pts = |pts: &[[f32; 3]]| -> Vec<[i64; 3]> {
        let mut v: Vec<[i64; 3]> = pts
            .iter()
            .map(|p| {
                [
                    (p[0] * 1000.0).round() as i64,
                    (p[1] * 1000.0).round() as i64,
                    (p[2] * 1000.0).round() as i64,
                ]
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        sort_pts(&p_old.positions),
        sort_pts(&p_new.positions),
        "bzp and modern surf must cover the same point set"
    );
}

#[test]
fn bzp_res_overrides_lattice_density() {
    // `res useg vseg` drives the per-direction lattice; the shared
    // isotropic lattice uses the finer (max) of the two.
    let text = SPEC_BZP.replace("bzp", "res 8 5\nbzp");
    let scene = ObjDecoder::new()
        .with_curve_tessellation(3)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &mesh_named(&scene, "obj:superseded_surfaces")
        .unwrap()
        .primitives[0];
    assert_eq!(
        prim.extras
            .get("obj:surface_samples")
            .and_then(|v| v.as_u64()),
        Some(8),
        "max(useg=8, vseg=5) drives the lattice"
    );
    // (8 + 1)² = 81 lattice vertices.
    assert_eq!(prim.positions.len(), 81);
    let seg = prim
        .extras
        .get("obj:surface_res_seg")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(seg[0].as_u64(), Some(8));
    assert_eq!(seg[1].as_u64(), Some(5));
}

#[test]
fn bzp_wrong_control_point_count_is_skipped() {
    // Spec §"bzp": exactly sixteen vertex numbers are required.
    let text = "v 0 0 0\nv 1 0 0\nv 2 0 0\nbzp 1 2 3\n";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    assert!(
        mesh_named(&scene, "obj:superseded_surfaces").is_none(),
        "non-16-point bzp must not tessellate"
    );
}

// ---------------------------------------------------------------------------
// Round-trip: synthetic geometry must not leak into the re-emit
// ---------------------------------------------------------------------------

#[test]
fn tessellated_superseded_geometry_not_emitted_as_geometry() {
    // Decoding with tessellation on then re-encoding must reproduce the
    // verbatim `cdc` / `bzp` / `res` lines and NOT spill the synthetic
    // lattice vertices into the `v` pool.
    let mut text = String::from(SPEC_BZP);
    text.push_str("res 5 5\n");
    text.push_str("cdc 1 2 3 4 5 6\n");

    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(text.as_bytes())
        .unwrap();
    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let out = String::from_utf8(bytes).unwrap();

    // Verbatim superseded lines survive.
    assert!(
        out.contains("bzp 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16"),
        "bzp line must round-trip:\n{out}"
    );
    assert!(out.contains("cdc 1 2 3 4 5 6"), "cdc line must round-trip");
    assert!(out.contains("res 5 5"), "res line must round-trip");

    // Exactly the original 16 vertices — no synthetic lattice / polyline
    // vertices leaked in.
    let v_lines = out.lines().filter(|l| l.starts_with("v ")).count();
    assert_eq!(v_lines, 16, "only the 16 source vertices re-emit:\n{out}");
}

#[test]
fn tessellation_off_by_default_emits_no_synthetic_mesh() {
    // Without `with_curve_tessellation`, the superseded statements only
    // round-trip verbatim — no synthetic geometry is produced.
    let scene = ObjDecoder::new().decode(SPEC_BZP.as_bytes()).unwrap();
    assert!(mesh_named(&scene, "obj:superseded_surfaces").is_none());
    assert!(mesh_named(&scene, "obj:superseded_curves").is_none());
}
