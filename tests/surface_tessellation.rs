//! Bezier `surf` surface tessellation —
//! `ObjDecoder::with_curve_tessellation(N)` evaluates every `surf`
//! element under a `cstype bezier` (or `cstype rat bezier`) header into a
//! real `Topology::Triangles` grid on a synthetic mesh named
//! `"obj:surfaces"`, via the tensor-product de Casteljau algorithm. The
//! directive sequence is still preserved on `Scene3D::extras` so the
//! encoder replays the original free-form section unchanged.
//!
//! Spec references: §"surf s0 s1 t0 t1 v1/vt1/vn1 …" (element statement),
//! §"Degree" (deg degu degv), §"Rational and non-rational curves and
//! surfaces" (bivariate basis), §"Bezier" (basis function), §"Surface
//! vertex data — control points" (row-major u-fastest ordering),
//! §"Free-form curve/surface body statements" (end).

use oxideav_mesh3d::{Mesh3DDecoder, Topology};
use oxideav_obj::{ObjDecoder, obj};

/// Bilinear (`deg 1 1`) Bezier patch over a planar unit square. Control
/// points are listed row-major with u varying fastest (spec §"Surface
/// vertex data — control points"):
///
///   j = 0:  v1 = (0,0,0)  v2 = (1,0,0)
///   j = 1:  v3 = (0,1,0)  v4 = (1,1,0)
///
/// A bilinear Bezier patch IS the plane through those four corners, so
/// every sample lies on z = 0, the four corners interpolate the control
/// points exactly, and the centre sample is the corner average
/// (0.5, 0.5, 0).
const BILINEAR_SURF: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";

#[test]
fn default_decoder_does_not_tessellate_surfaces() {
    let bare = ObjDecoder::new().decode(BILINEAR_SURF.as_bytes()).unwrap();
    assert!(
        bare.meshes.is_empty(),
        "default decoder must not synthesise surface meshes"
    );
}

#[test]
fn bilinear_surface_tessellates_into_a_triangle_grid() {
    // 4 samples ⇒ a 5×5 vertex lattice = 25 vertices, 4×4 cells × 2
    // triangles × 3 indices = 96 indices.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(BILINEAR_SURF.as_bytes())
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

    // Every vertex lies on the z = 0 plane.
    for p in &prim.positions {
        assert!(p[2].abs() < 1e-5, "vertex off the z=0 plane: {p:?}");
    }

    let stride = 5usize;
    // Corner interpolation: lattice corner (su, sv) lands at
    // index sv * stride + su.
    let c00 = prim.positions[0]; // (u=0, v=0) ⇒ v1
    let c10 = prim.positions[stride - 1]; // (u=1, v=0) ⇒ v2
    let c01 = prim.positions[(stride - 1) * stride]; // (u=0, v=1) ⇒ v3
    let c11 = prim.positions[stride * stride - 1]; // (u=1, v=1) ⇒ v4
    assert!((c00[0] - 0.0).abs() < 1e-5 && (c00[1] - 0.0).abs() < 1e-5);
    assert!((c10[0] - 1.0).abs() < 1e-5 && (c10[1] - 0.0).abs() < 1e-5);
    assert!((c01[0] - 0.0).abs() < 1e-5 && (c01[1] - 1.0).abs() < 1e-5);
    assert!((c11[0] - 1.0).abs() < 1e-5 && (c11[1] - 1.0).abs() < 1e-5);

    // Centre of the lattice (su = 2, sv = 2) is the corner average.
    let centre = prim.positions[2 * stride + 2];
    assert!(
        (centre[0] - 0.5).abs() < 1e-5 && (centre[1] - 0.5).abs() < 1e-5,
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
        Some("bezier")
    );
    let deg = prim
        .extras
        .get("obj:surface_degree")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(deg[0].as_u64(), Some(1));
    assert_eq!(deg[1].as_u64(), Some(1));
    assert_eq!(
        prim.extras
            .get("obj:surface_samples")
            .and_then(|v| v.as_u64()),
        Some(4)
    );
    let u_range = prim
        .extras
        .get("obj:surface_u_range")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!((u_range[0].as_f64().unwrap() - 0.0).abs() < 1e-6);
    assert!((u_range[1].as_f64().unwrap() - 1.0).abs() < 1e-6);
    let v_range = prim
        .extras
        .get("obj:surface_v_range")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!((v_range[0].as_f64().unwrap() - 0.0).abs() < 1e-6);
    assert!((v_range[1].as_f64().unwrap() - 1.0).abs() < 1e-6);
}

/// A non-planar bilinear patch — lifting one corner in z makes the
/// surface a hyperbolic paraboloid (the classic bilinear "saddle"). At
/// the centre the surface dips to the average z = 0.25.
const SADDLE_SURF: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 1.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";

#[test]
fn non_planar_bilinear_centre_is_corner_average() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(2)
        .decode(SADDLE_SURF.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    // 2 samples ⇒ 3×3 lattice, centre at index 1 * 3 + 1 = 4.
    let centre = prim.positions[4];
    // Bilinear blend at (0.5, 0.5): z = 0.25·(0 + 0 + 0 + 1) = 0.25.
    assert!(
        (centre[2] - 0.25).abs() < 1e-5,
        "saddle centre z mismatch: {centre:?}"
    );
}

/// Bicubic (`deg 3 3`) Bezier surface patch — 16 control points, single
/// patch, mirroring the spec's two-Bezier merging-group example
/// (§"Free-form geometry — merging group example"). Control points form
/// a 4×4 planar grid in z = 0 except the four interior points are lifted
/// in z, so the patch interpolates its flat boundary but bulges in the
/// middle. We assert vertex/index counts, exact corner interpolation
/// (Bezier surfaces pass through their corner control points), and that
/// the boundary stays in-plane while the interior lifts.
const BICUBIC_SURF: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 1.0
v 2.0 1.0 1.0
v 3.0 1.0 0.0
v 0.0 2.0 0.0
v 1.0 2.0 1.0
v 2.0 2.0 1.0
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
fn bicubic_surface_interpolates_corners_and_bulges_interior() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(BICUBIC_SURF.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1);
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.topology, Topology::Triangles);
    // 8 samples ⇒ 9×9 = 81 vertices, 8×8 cells × 6 = 384 indices.
    assert_eq!(prim.positions.len(), 81);
    assert_eq!(prim.indices.as_ref().unwrap().len(), 384);

    let stride = 9usize;
    // Corner control points are interpolated exactly by a Bezier patch.
    let c00 = prim.positions[0]; // v1 = (0,0,0)
    let c10 = prim.positions[stride - 1]; // v4 = (3,0,0)
    let c01 = prim.positions[(stride - 1) * stride]; // v13 = (0,3,0)
    let c11 = prim.positions[stride * stride - 1]; // v16 = (3,3,0)
    assert!((c00[0]).abs() < 1e-4 && (c00[1]).abs() < 1e-4 && (c00[2]).abs() < 1e-4);
    assert!((c10[0] - 3.0).abs() < 1e-4 && (c10[1]).abs() < 1e-4 && (c10[2]).abs() < 1e-4);
    assert!((c01[0]).abs() < 1e-4 && (c01[1] - 3.0).abs() < 1e-4 && (c01[2]).abs() < 1e-4);
    assert!((c11[0] - 3.0).abs() < 1e-4 && (c11[1] - 3.0).abs() < 1e-4 && (c11[2]).abs() < 1e-4);

    // The whole u = 0 boundary curve lies in z = 0 (its four control
    // points are all flat). Check the v-direction edge column.
    for sv in 0..stride {
        let edge = prim.positions[sv * stride];
        assert!(
            edge[2].abs() < 1e-4,
            "u=0 boundary should stay flat, got {edge:?}"
        );
    }
    // The centre sample lifts above the plane because the interior
    // control points were raised in z.
    let centre = prim.positions[4 * stride + 4];
    assert!(
        centre[2] > 0.1,
        "interior should bulge above z=0, got {centre:?}"
    );

    let deg = prim
        .extras
        .get("obj:surface_degree")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(deg[0].as_u64(), Some(3));
    assert_eq!(deg[1].as_u64(), Some(3));
}

/// Rational Bezier surface (`cstype rat bezier`) — a flat bilinear patch
/// where the (1,1) corner carries weight 3. The rational projection
/// pulls the centre sample toward that heavy corner relative to the
/// non-rational midpoint of (0.5, 0.5).
const RAT_SURF: &str = "\
v 0.0 0.0 0.0 1.0
v 1.0 0.0 0.0 1.0
v 0.0 1.0 0.0 1.0
v 1.0 1.0 0.0 3.0
cstype rat bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";

#[test]
fn rational_surface_centre_pulls_toward_heavy_corner() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(2)
        .decode(RAT_SURF.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.extras.get("obj:surface_kind").and_then(|v| v.as_str()),
        Some("rat_bezier")
    );
    // Centre of a 3×3 lattice ⇒ index 4.
    let centre = prim.positions[4];
    // Bilinear weights at (0.5, 0.5): each basis = 0.25.
    //   numerator_x = 0.25·(1·0 + 1·1 + 1·0 + 3·1) = 0.25·4 = 1.0
    //   denominator = 0.25·(1 + 1 + 1 + 3) = 1.5
    //   ⇒ x = 1.0 / 1.5 ≈ 0.6667 (pulled past the unweighted 0.5)
    assert!(
        (centre[0] - (1.0 / 1.5)).abs() < 1e-4,
        "rational centre x = {centre:?}, expected ≈ 0.6667"
    );
    assert!(
        (centre[1] - (1.0 / 1.5)).abs() < 1e-4,
        "rational centre y = {centre:?}, expected ≈ 0.6667"
    );
}

#[test]
fn surf_control_points_accept_slash_references_and_negatives() {
    // `surf` control vertices use `v/vt/vn` syntax; only the leading
    // position index is consumed. Negative (relative-from-end) indices
    // resolve the same way faces do.
    let text = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
vt 0.0 0.0
vt 1.0 0.0
vt 0.0 1.0
vt 1.0 1.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 -4/1 -3/2 -2/3 -1/4
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(2)
        .decode(text.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1);
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.positions.len(), 9);
    // Corner (0,0) interpolates v1.
    assert!((prim.positions[0][0]).abs() < 1e-5 && (prim.positions[0][1]).abs() < 1e-5);
}

#[test]
fn tessellated_surface_is_not_emitted_as_v_lines_by_encoder() {
    // Decoder tessellates → encoder must skip the synthetic triangle
    // mesh and replay the original directives from
    // `Scene3D::extras["obj:freeform_directives"]`. The 25 sample points
    // must NOT leak into the `v` pool, and no `o obj:surfaces` block /
    // `f` lines from the synthetic grid may appear.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(BILINEAR_SURF.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "synthetic surface mesh present");

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();

    // The 25 tessellation sample points must not pollute the `v` pool
    // (free-form-only OBJ surfaces its source pool through
    // `obj:positions`, which is at most the 4 control points here).
    let v_lines = text.lines().filter(|l| l.starts_with("v ")).count();
    assert!(
        v_lines <= 4,
        "tessellation samples leaked as `v` lines; got {v_lines}:\n{text}"
    );

    assert!(
        !text.contains("o obj:surfaces"),
        "synthetic surface mesh must not be re-emitted as a polygonal `o` block"
    );

    // The directive sequence comes back verbatim.
    for keyword in [
        "cstype bezier",
        "deg 1 1",
        "surf 0",
        "parm u 0",
        "parm v 0",
        "end",
    ] {
        assert!(
            text.lines().any(|l| l.starts_with(keyword)),
            "missing `{keyword}` line in:\n{text}"
        );
    }
}

#[test]
fn unsupported_surface_basis_is_left_captured_only() {
    // A `cstype bmatrix` `surf` has no surface evaluator yet — the
    // tessellator must not synthesise a mesh for it; the directive
    // sequence still round-trips through `obj:freeform_directives`.
    // (Bezier / B-spline / Cardinal / Taylor surfaces ARE now
    // tessellated — see the dedicated test groups below and the
    // `taylor_surface_tessellation` test file.)
    let text = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bmatrix
deg 1 1
bmat u 1 0 -1 1
bmat v 1 0 -1 1
step 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "basis-matrix surfaces stay captured-only; no synthetic mesh expected"
    );
    // The directives are still preserved for round-trip.
    assert!(scene.extras.contains_key("obj:freeform_directives"));
}

#[test]
fn malformed_surf_with_wrong_control_count_is_skipped() {
    // `deg 2 2` declares a 3×3 = 9-point single patch, but only 4
    // control points are listed. Single-patch tessellation can't proceed
    // (multi-patch decomposition needs `step`, which Bezier doesn't
    // carry), so the surface is left captured-only rather than guessed.
    let text = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 2 2
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "control-point count mismatch should skip tessellation"
    );
}

// ---------------------------------------------------------------------------
// B-spline / NURBS `surf` surface tessellation (round 12).
//
// Spec references: §"B-spline" (Cox-deBoor recursion + the six knot-vector
// conditions), §"parm u/v" (knot vectors for surfaces), §"deg degu degv"
// (per-direction degree), §"surf s0 s1 t0 t1 v1/vt1/vn1 …" (element
// statement), §"Surface vertex data — control points" (row-major u-fastest
// ordering), §"Rational and non-rational curves and surfaces" (NURBS
// projection).
// ---------------------------------------------------------------------------

/// Planar bilinear (`deg 1 1`) B-spline patch with the simplest clamped
/// knot vectors (`0 0 1 1`). A degree-1 clamped B-spline over a single
/// span is just the bilinear interpolant of its four corners — identical
/// to the bilinear Bezier patch — so every sample stays on z = 0, the
/// corners interpolate the control points exactly, and the centre is the
/// corner average.
const BSPLINE_BILINEAR: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bspline
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 0.0 1.0 1.0
parm v 0.0 0.0 1.0 1.0
end
";

#[test]
fn default_decoder_does_not_tessellate_bspline_surfaces() {
    let bare = ObjDecoder::new()
        .decode(BSPLINE_BILINEAR.as_bytes())
        .unwrap();
    assert!(
        bare.meshes.is_empty(),
        "default decoder must not synthesise B-spline surface meshes"
    );
}

#[test]
fn bspline_bilinear_surface_tessellates_into_a_flat_grid() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(BSPLINE_BILINEAR.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "one synthetic surface mesh expected");
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("obj:surfaces"));
    assert_eq!(mesh.primitives.len(), 1);

    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::Triangles);
    assert_eq!(prim.positions.len(), 25, "(samples + 1)^2 lattice vertices");
    assert_eq!(prim.indices.as_ref().unwrap().len(), 96);

    // Flat: every sample lies on z = 0.
    for p in &prim.positions {
        assert!(p[2].abs() < 1e-5, "vertex off the z=0 plane: {p:?}");
    }

    let stride = 5usize;
    let c00 = prim.positions[0];
    let c10 = prim.positions[stride - 1];
    let c01 = prim.positions[(stride - 1) * stride];
    let c11 = prim.positions[stride * stride - 1];
    assert!((c00[0]).abs() < 1e-5 && (c00[1]).abs() < 1e-5);
    assert!((c10[0] - 1.0).abs() < 1e-5 && (c10[1]).abs() < 1e-5);
    assert!((c01[0]).abs() < 1e-5 && (c01[1] - 1.0).abs() < 1e-5);
    assert!((c11[0] - 1.0).abs() < 1e-5 && (c11[1] - 1.0).abs() < 1e-5);

    // Centre of the lattice is the corner average (bilinear blend).
    let centre = prim.positions[2 * stride + 2];
    assert!(
        (centre[0] - 0.5).abs() < 1e-5 && (centre[1] - 0.5).abs() < 1e-5,
        "centre sample mismatch: {centre:?}"
    );

    // Provenance.
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
        Some("bspline")
    );
    let deg = prim
        .extras
        .get("obj:surface_degree")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(deg[0].as_u64(), Some(1));
    assert_eq!(deg[1].as_u64(), Some(1));
}

/// A clamped quadratic (`deg 2 2`) B-spline patch with knots
/// `0 0 0 1 1 1` over a single span is mathematically IDENTICAL to the
/// quadratic Bezier patch with the same 3×3 control grid (a clamped
/// open-uniform B-spline with `degree + 1` end-knot multiplicity and one
/// interior span reduces to the Bernstein/Bezier basis). We assert the
/// sampled surface matches a directly-evaluated bivariate quadratic
/// Bezier at the same lattice — a hard cross-check of the Cox-deBoor
/// surface evaluator against the round-11 de Casteljau path.
const BSPLINE_QUADRATIC_CLAMPED: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.6
v 2.0 0.0 0.0
v 0.0 1.0 0.6
v 1.0 1.0 1.4
v 2.0 1.0 0.6
v 0.0 2.0 0.0
v 1.0 2.0 0.6
v 2.0 2.0 0.0
cstype bspline
deg 2 2
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9
parm u 0.0 0.0 0.0 1.0 1.0 1.0
parm v 0.0 0.0 0.0 1.0 1.0 1.0
end
";

#[test]
fn clamped_quadratic_bspline_matches_quadratic_bezier() {
    // Control grid, row-major u-fastest (matches the surf order above).
    let ctrl: [[f32; 3]; 9] = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.6],
        [2.0, 0.0, 0.0],
        [0.0, 1.0, 0.6],
        [1.0, 1.0, 1.4],
        [2.0, 1.0, 0.6],
        [0.0, 2.0, 0.0],
        [1.0, 2.0, 0.6],
        [2.0, 2.0, 0.0],
    ];
    // Quadratic Bernstein basis on [0,1].
    fn bern2(t: f32) -> [f32; 3] {
        let s = 1.0 - t;
        [s * s, 2.0 * s * t, t * t]
    }
    fn bezier_eval(ctrl: &[[f32; 3]; 9], u: f32, v: f32) -> [f32; 3] {
        let bu = bern2(u);
        let bv = bern2(v);
        let mut acc = [0.0f32; 3];
        for (j, &cv) in bv.iter().enumerate() {
            for (i, &cu) in bu.iter().enumerate() {
                let w = cu * cv;
                let p = ctrl[j * 3 + i];
                acc[0] += w * p[0];
                acc[1] += w * p[1];
                acc[2] += w * p[2];
            }
        }
        acc
    }

    let samples = 4u32;
    let scene = ObjDecoder::new()
        .with_curve_tessellation(samples)
        .decode(BSPLINE_QUADRATIC_CLAMPED.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    let n = samples as usize + 1;
    assert_eq!(prim.positions.len(), n * n);

    for sv in 0..n {
        let v = sv as f32 / (n - 1) as f32;
        for su in 0..n {
            let u = su as f32 / (n - 1) as f32;
            let expect = bezier_eval(&ctrl, u, v);
            let got = prim.positions[sv * n + su];
            for k in 0..3 {
                assert!(
                    (got[k] - expect[k]).abs() < 2e-3,
                    "clamped B-spline ≠ Bezier at (u={u}, v={v}) axis {k}: \
                     got {got:?}, expected {expect:?}"
                );
            }
        }
    }
}

/// Rational B-spline (NURBS) surface from the spec's Example 5
/// (§"Free-form curve/surface body statements" — "Rational B-spline
/// surface"): a degree-2 patch over a clamped open uniform knot vector,
/// with per-vertex weights on the `v` lines. We verify the corner samples
/// interpolate the corner control points (clamped end knots force
/// endpoint interpolation regardless of weight) and that the surface
/// tessellates into the expected lattice with the rational kind tag.
const NURBS_SURFACE_SPEC_EX5: &str = "\
v -1.3 -1.0  0.0
v  0.1 -1.0  0.4  7.6
v  1.4 -1.0  0.0  2.3
v -1.4  0.0  0.2
v  0.1  0.0  0.9  0.5
v  1.3  0.0  0.4  1.5
v -1.4  1.0  0.0  2.3
v  0.1  1.0  0.3  6.1
v  1.1  1.0  0.0  3.3
cstype rat bspline
deg 2 2
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9
parm u 0.0 0.0 0.0 1.0 1.0 1.0
parm v 0.0 0.0 0.0 1.0 1.0 1.0
end
";

#[test]
fn nurbs_surface_interpolates_corners_with_clamped_knots() {
    let samples = 6u32;
    let scene = ObjDecoder::new()
        .with_curve_tessellation(samples)
        .decode(NURBS_SURFACE_SPEC_EX5.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1);
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.extras.get("obj:surface_kind").and_then(|v| v.as_str()),
        Some("rat_bspline")
    );
    let n = samples as usize + 1;
    assert_eq!(prim.positions.len(), n * n);

    // Clamped (multiplicity degree+1) end knots ⇒ the four corner samples
    // interpolate the four corner control points exactly, with weight
    // dropping out of the projection at the corners (only one basis is
    // non-zero there).
    let c00 = prim.positions[0]; // v1 = (-1.3, -1.0, 0.0)
    let c10 = prim.positions[n - 1]; // v3 = (1.4, -1.0, 0.0)
    let c01 = prim.positions[(n - 1) * n]; // v7 = (-1.4, 1.0, 0.0)
    let c11 = prim.positions[n * n - 1]; // v9 = (1.1, 1.0, 0.0)
    assert!((c00[0] - -1.3).abs() < 2e-3 && (c00[1] - -1.0).abs() < 2e-3);
    assert!((c10[0] - 1.4).abs() < 2e-3 && (c10[1] - -1.0).abs() < 2e-3);
    assert!((c01[0] - -1.4).abs() < 2e-3 && (c01[1] - 1.0).abs() < 2e-3);
    assert!((c11[0] - 1.1).abs() < 2e-3 && (c11[1] - 1.0).abs() < 2e-3);
}

/// Spec Example 3 — a cubic (`deg 3 3`) non-rational B-spline surface
/// with a 4×4 control grid and 8-entry knot vectors in each direction
/// (`-3 -2 -1 0 1 2 3 4`). We assert it tessellates (knot length
/// `cols + degu + 1 = 4 + 3 + 1 = 8` matches), produces the expected
/// lattice, and that the control points' bounding box contains every
/// sample (a B-spline surface lies in the convex hull of its control
/// net).
const BSPLINE_CUBIC_SPEC_EX3: &str = "\
v -5.000000 -5.000000 -7.808327
v -5.000000 -1.666667 -7.808327
v -5.000000 1.666667 -7.808327
v -5.000000 5.000000 -7.808327
v -1.666667 -5.000000 -7.808327
v -1.666667 -1.666667 11.977780
v -1.666667 1.666667 11.977780
v -1.666667 5.000000 -7.808327
v 1.666667 -5.000000 -7.808327
v 1.666667 -1.666667 11.977780
v 1.666667 1.666667 11.977780
v 1.666667 5.000000 -7.808327
v 5.000000 -5.000000 -7.808327
v 5.000000 -1.666667 -7.808327
v 5.000000 1.666667 -7.808327
v 5.000000 5.000000 -7.808327
cstype bspline
deg 3 3
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
parm u -3.0 -2.0 -1.0 0.0 1.0 2.0 3.0 4.0
parm v -3.0 -2.0 -1.0 0.0 1.0 2.0 3.0 4.0
end
";

#[test]
fn cubic_bspline_surface_lies_in_control_hull() {
    let samples = 8u32;
    let scene = ObjDecoder::new()
        .with_curve_tessellation(samples)
        .decode(BSPLINE_CUBIC_SPEC_EX3.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1);
    let prim = &scene.meshes[0].primitives[0];
    let n = samples as usize + 1;
    assert_eq!(prim.positions.len(), n * n);
    assert_eq!(prim.indices.as_ref().unwrap().len(), 8 * 8 * 6);

    // Convex-hull property: every sample is within the control-net AABB
    // (with a small epsilon for float slack).
    for p in &prim.positions {
        assert!(
            p[0] >= -5.0 - 1e-3 && p[0] <= 5.0 + 1e-3,
            "x out of hull: {p:?}"
        );
        assert!(
            p[1] >= -5.0 - 1e-3 && p[1] <= 5.0 + 1e-3,
            "y out of hull: {p:?}"
        );
        assert!(p[2] >= -7.81 && p[2] <= 11.98, "z out of hull: {p:?}");
    }
    // The dished control net (interior points lifted to z ≈ 11.98, corners
    // at z ≈ -7.81) should pull the surface interior above the corners.
    let centre = prim.positions[(n / 2) * n + n / 2];
    let corner = prim.positions[0];
    assert!(
        centre[2] > corner[2],
        "interior should ride above the dished corners: centre {centre:?}, corner {corner:?}"
    );
}

#[test]
fn bspline_surface_round_trips_directives_without_leaking_samples() {
    // Decoder tessellates → encoder must skip the synthetic triangle mesh
    // and replay the original B-spline directives from
    // `Scene3D::extras["obj:freeform_directives"]`.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(BSPLINE_QUADRATIC_CLAMPED.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "synthetic surface mesh present");

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();

    // The 25 tessellation samples must not pollute the `v` pool (only the
    // 9 source control points may appear).
    let v_lines = text.lines().filter(|l| l.starts_with("v ")).count();
    assert!(
        v_lines <= 9,
        "tessellation samples leaked as `v` lines; got {v_lines}:\n{text}"
    );
    assert!(
        !text.contains("o obj:surfaces"),
        "synthetic surface mesh must not be re-emitted as a polygonal block"
    );

    for keyword in [
        "cstype bspline",
        "deg 2 2",
        "surf 0",
        "parm u 0",
        "parm v 0",
        "end",
    ] {
        assert!(
            text.lines().any(|l| l.starts_with(keyword)),
            "missing `{keyword}` line in:\n{text}"
        );
    }
}

#[test]
fn bspline_surface_with_short_knot_vector_is_skipped() {
    // `deg 2 2` over a 3-control-point direction needs
    // `cols + degu + 1 = 3 + 2 + 1 = 6` knots; supplying only 4 in `parm u`
    // implies a 1-control-point u-direction, which doesn't match the 9
    // listed control vertices, so the surface is left captured-only.
    let text = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
v 2.0 1.0 0.0
v 0.0 2.0 0.0
v 1.0 2.0 0.0
v 2.0 2.0 0.0
cstype bspline
deg 2 2
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9
parm u 0.0 0.0 1.0 1.0
parm v 0.0 0.0 0.0 1.0 1.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "knot-vector/control-count mismatch should skip tessellation"
    );
    assert!(scene.extras.contains_key("obj:freeform_directives"));
}

// ---------------------------------------------------------------------------
// Cardinal (Catmull-Rom) `surf` surface tessellation (round 13).
//
// Spec references: §"Cardinal" (cubic-only; Cardinal→Bezier conversion;
// "For surfaces, all but the first and last row and column of control
// points are interpolated"; "Cardinal splines are only defined for the
// cubic case"), §"deg degu degv" (per-direction degree), §"surf s0 s1 t0
// t1 v1/vt1/vn1 …" (element statement), §"Surface vertex data — control
// points" (row-major u-fastest ordering), §"Free-form curve/surface body
// statements" (Cardinal unit-weight default is reasonable).
// ---------------------------------------------------------------------------

/// Spec Example 4 — a planar Cardinal surface. A 4×4 control grid all at
/// z = 0 (`deg 3 3`), so a Cardinal patch over it must also be flat: every
/// tessellated sample stays on z = 0.
const CARDINAL_SURF_SPEC_EX4: &str = "\
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
cstype cardinal
deg 3 3
surf 0.000000 1.000000 0.000000 1.000000 13 14 15 16 9 10 11 12 5 6 7 8 1 2 3 4
parm u 0.000000 1.000000
parm v 0.000000 1.000000
end
";

#[test]
fn default_decoder_does_not_tessellate_cardinal_surfaces() {
    let bare = ObjDecoder::new()
        .decode(CARDINAL_SURF_SPEC_EX4.as_bytes())
        .unwrap();
    assert!(
        bare.meshes.is_empty(),
        "default decoder must not synthesise Cardinal surface meshes"
    );
}

#[test]
fn cardinal_spec_example4_tessellates_into_a_flat_grid() {
    let samples = 4u32;
    let scene = ObjDecoder::new()
        .with_curve_tessellation(samples)
        .decode(CARDINAL_SURF_SPEC_EX4.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "one synthetic surface mesh expected");
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("obj:surfaces"));
    assert_eq!(mesh.primitives.len(), 1);

    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::Triangles);
    assert_eq!(prim.positions.len(), 25, "(samples + 1)^2 lattice vertices");
    assert_eq!(prim.indices.as_ref().unwrap().len(), 96);

    // The whole control net is z = 0 ⇒ the Cardinal patch is flat.
    for p in &prim.positions {
        assert!(p[2].abs() < 1e-4, "vertex off the z=0 plane: {p:?}");
    }

    // Provenance.
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
        Some("cardinal")
    );
    let deg = prim
        .extras
        .get("obj:surface_degree")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(deg[0].as_u64(), Some(3));
    assert_eq!(deg[1].as_u64(), Some(3));
}

/// A 4×4 Cardinal control grid whose boundary ring is flat (z = 0) but
/// whose interior 2×2 block is lifted in z. Spec §"Cardinal": for
/// surfaces, all but the first and last row and column of control points
/// are interpolated — so a single bicubic Cardinal patch (one segment per
/// direction, parameter domain [0,1]²) has parametric corners that land
/// exactly on the four *interior* control points. We verify those four
/// parametric corners interpolate the lifted interior points exactly.
const CARDINAL_INTERIOR_BULGE: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.5
v 2.0 1.0 0.7
v 3.0 1.0 0.0
v 0.0 2.0 0.0
v 1.0 2.0 0.9
v 2.0 2.0 0.3
v 3.0 2.0 0.0
v 0.0 3.0 0.0
v 1.0 3.0 0.0
v 2.0 3.0 0.0
v 3.0 3.0 0.0
cstype cardinal
deg 3 3
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
parm u 0.0 1.0
parm v 0.0 1.0
end
";

#[test]
fn cardinal_surface_interpolates_interior_control_points() {
    let samples = 6u32;
    let scene = ObjDecoder::new()
        .with_curve_tessellation(samples)
        .decode(CARDINAL_INTERIOR_BULGE.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    let n = samples as usize + 1;
    assert_eq!(prim.positions.len(), n * n);

    // The interior 2×2 control block is, in the row-major grid (cols=4),
    // grid[1][1] = v6 (1,1,0.5), grid[1][2] = v7 (2,1,0.7),
    // grid[2][1] = v10 (1,2,0.9), grid[2][2] = v11 (2,2,0.3).
    // A single Cardinal patch (one u-segment, one v-segment) maps the
    // parametric corners to those interior points:
    //   (u=0, v=0) ⇒ grid[1][1] = v6
    //   (u=1, v=0) ⇒ grid[1][2] = v7
    //   (u=0, v=1) ⇒ grid[2][1] = v10
    //   (u=1, v=1) ⇒ grid[2][2] = v11
    let p00 = prim.positions[0];
    let p10 = prim.positions[n - 1];
    let p01 = prim.positions[(n - 1) * n];
    let p11 = prim.positions[n * n - 1];
    let approx = |a: [f32; 3], b: [f32; 3]| {
        (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4 && (a[2] - b[2]).abs() < 1e-4
    };
    assert!(approx(p00, [1.0, 1.0, 0.5]), "(0,0) corner: {p00:?}");
    assert!(approx(p10, [2.0, 1.0, 0.7]), "(1,0) corner: {p10:?}");
    assert!(approx(p01, [1.0, 2.0, 0.9]), "(0,1) corner: {p01:?}");
    assert!(approx(p11, [2.0, 2.0, 0.3]), "(1,1) corner: {p11:?}");
}

#[test]
fn cardinal_surface_matches_cardinal_to_bezier_reference() {
    // Cross-check the tensor-product evaluator against an independent
    // Cardinal→Bezier reference (spec §"Cardinal" conversion applied per
    // direction). For a single 4×4 patch each parametric direction is one
    // Cardinal segment over c0..c3, converted to a cubic Bezier and
    // evaluated with the Bernstein basis. The tensor product runs the u
    // pass on each v-row, then a v pass on the four collapsed points.
    let grid: [[f32; 3]; 16] = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [3.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 1.0, 0.5],
        [2.0, 1.0, 0.7],
        [3.0, 1.0, 0.0],
        [0.0, 2.0, 0.0],
        [1.0, 2.0, 0.9],
        [2.0, 2.0, 0.3],
        [3.0, 2.0, 0.0],
        [0.0, 3.0, 0.0],
        [1.0, 3.0, 0.0],
        [2.0, 3.0, 0.0],
        [3.0, 3.0, 0.0],
    ];
    // Cardinal→Bezier for a single 4-point segment, then Bernstein eval.
    fn cardinal_seg(c: &[[f32; 3]; 4], t: f32) -> [f32; 3] {
        let mut b = [[0.0f32; 3]; 4];
        for a in 0..3 {
            b[0][a] = c[1][a];
            b[1][a] = c[1][a] + (c[2][a] - c[0][a]) / 6.0;
            b[2][a] = c[2][a] - (c[3][a] - c[1][a]) / 6.0;
            b[3][a] = c[2][a];
        }
        let u = 1.0 - t;
        let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
        let mut p = [0.0f32; 3];
        for a in 0..3 {
            p[a] = w[0] * b[0][a] + w[1] * b[1][a] + w[2] * b[2][a] + w[3] * b[3][a];
        }
        p
    }
    fn reference(grid: &[[f32; 3]; 16], u: f32, v: f32) -> [f32; 3] {
        // u pass: collapse each of the 4 v-rows.
        let mut col = [[0.0f32; 3]; 4];
        for (r, slot) in col.iter_mut().enumerate() {
            let row: [[f32; 3]; 4] = [
                grid[r * 4],
                grid[r * 4 + 1],
                grid[r * 4 + 2],
                grid[r * 4 + 3],
            ];
            *slot = cardinal_seg(&row, u);
        }
        // v pass over the collapsed points.
        cardinal_seg(&col, v)
    }

    let samples = 5u32;
    let scene = ObjDecoder::new()
        .with_curve_tessellation(samples)
        .decode(CARDINAL_INTERIOR_BULGE.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    let n = samples as usize + 1;
    for sv in 0..n {
        let v = sv as f32 / (n - 1) as f32;
        for su in 0..n {
            let u = su as f32 / (n - 1) as f32;
            let expect = reference(&grid, u, v);
            let got = prim.positions[sv * n + su];
            for k in 0..3 {
                assert!(
                    (got[k] - expect[k]).abs() < 1e-4,
                    "Cardinal surface ≠ reference at (u={u}, v={v}) axis {k}: \
                     got {got:?}, expected {expect:?}"
                );
            }
        }
    }
}

#[test]
fn cardinal_surface_round_trips_directives_without_leaking_samples() {
    // Decoder tessellates → encoder must skip the synthetic triangle mesh
    // and replay the original Cardinal directives verbatim. The 25 sample
    // points must not pollute the `v` pool (only the 16 control points may
    // appear), and no `o obj:surfaces` block may be re-emitted.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(CARDINAL_SURF_SPEC_EX4.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "synthetic surface mesh present");

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();

    let v_lines = text.lines().filter(|l| l.starts_with("v ")).count();
    assert!(
        v_lines <= 16,
        "tessellation samples leaked as `v` lines; got {v_lines}:\n{text}"
    );
    assert!(
        !text.contains("o obj:surfaces"),
        "synthetic surface mesh must not be re-emitted as a polygonal `o` block"
    );
    for keyword in ["cstype cardinal", "deg 3 3", "surf 0", "end"] {
        assert!(
            text.lines().any(|l| l.starts_with(keyword)),
            "missing `{keyword}` line in:\n{text}"
        );
    }
}

#[test]
fn non_cubic_cardinal_surface_is_rejected() {
    // Spec §"Cardinal": "Cardinal splines are only defined for the cubic
    // case." A `deg 2 2` Cardinal surface must be left captured-only.
    let text = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
v 2.0 1.0 0.0
v 0.0 2.0 0.0
v 1.0 2.0 0.0
v 2.0 2.0 0.0
cstype cardinal
deg 2 2
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "non-cubic Cardinal surfaces stay captured-only"
    );
    assert!(scene.extras.contains_key("obj:freeform_directives"));
}

/// A 4-col × 5-row Cardinal control grid (one u-segment, two v-segments)
/// exercising the multi-segment path. The grid is laid out row-major with
/// the u index (4 columns, x = 0..3) varying fastest and 5 v-rows
/// (y = 0..4). cols is read from `parm u` (3 values ⇒ `K = parm + 1 = 4`)
/// and rows from `parm v` (4 values ⇒ `K = parm + 1 = 5`). Two genuine
/// interior control points (row 2, cols 1 and 2) lift in z; the surface
/// must tessellate and the bulge must reach into the sampled domain.
const CARDINAL_MULTISEG: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
v 3.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
v 2.0 1.0 0.0
v 3.0 1.0 0.0
v 0.0 2.0 0.0
v 1.0 2.0 0.6
v 2.0 2.0 0.6
v 3.0 2.0 0.0
v 0.0 3.0 0.0
v 1.0 3.0 0.0
v 2.0 3.0 0.0
v 3.0 3.0 0.0
v 0.0 4.0 0.0
v 1.0 4.0 0.0
v 2.0 4.0 0.0
v 3.0 4.0 0.0
cstype cardinal
deg 3 3
surf 0.0 2.0 0.0 1.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20
parm u 0.0 1.0 2.0
parm v 0.0 1.0 2.0 3.0
end
";

#[test]
fn cardinal_multi_segment_surface_tessellates() {
    // cols = parm_u.len() + 1 = 4 (3 parm-u values); rows = parm_v.len() + 1
    // = 5 (4 parm-v values). The 4×5 grid (one u-segment, two v-segments)
    // must tessellate.
    let samples = 4u32;
    let scene = ObjDecoder::new()
        .with_curve_tessellation(samples)
        .decode(CARDINAL_MULTISEG.as_bytes())
        .unwrap();
    assert_eq!(scene.meshes.len(), 1, "multi-segment Cardinal surface");
    let prim = &scene.meshes[0].primitives[0];
    let n = samples as usize + 1;
    assert_eq!(prim.positions.len(), n * n);
    assert_eq!(prim.indices.as_ref().unwrap().len(), 96);
    // Cardinal (Catmull-Rom) is NOT convex-hull-bounded — its tangent
    // construction can overshoot — so we don't impose a hull clamp.
    // Instead, verify the interior bulge surfaces: the v = 1 segment
    // boundary interpolates control row 2 (whose interior points sit at
    // z = 0.6), so the sampled domain must rise above the flat boundary.
    let max_z = prim.positions.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
    assert!(
        max_z > 0.3,
        "interior bulge should lift the surface above z = 0; max_z = {max_z}"
    );
    // The x/y footprint stays roughly inside the [0,3] × [0,4] control net
    // (small Cardinal overshoot at the edges is allowed).
    for p in &prim.positions {
        assert!(p[0] >= -0.5 && p[0] <= 3.5, "x off the net: {p:?}");
        assert!(p[1] >= -0.5 && p[1] <= 4.5, "y off the net: {p:?}");
    }
}
