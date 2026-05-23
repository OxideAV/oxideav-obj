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
fn non_bezier_surface_basis_is_left_captured_only() {
    // A `cstype bspline` `surf` has no surface evaluator yet — the
    // tessellator must not synthesise a mesh for it; the directive
    // sequence still round-trips through `obj:freeform_directives`.
    let text = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bspline
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0 2.0 3.0
parm v 0.0 1.0 2.0 3.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(text.as_bytes())
        .unwrap();
    assert!(
        scene.meshes.is_empty(),
        "B-spline surfaces stay captured-only; no synthetic mesh expected"
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
