//! Taylor polynomial `surf` surface tessellation —
//! `ObjDecoder::with_curve_tessellation(N)` evaluates every `surf`
//! element under a `cstype taylor` (or `cstype rat taylor`) header into
//! a real `Topology::Triangles` grid on the synthetic `"obj:surfaces"`
//! mesh, via the bivariate tensor-product Horner-rule polynomial
//! evaluation
//! `S(u, v) = Σ_i Σ_j c_{i,j} · u^i · v^j` (spec §"Taylor"). The
//! directive sequence is still preserved on `Scene3D::extras` so the
//! encoder replays the original free-form section unchanged.
//!
//! Spec references: §"Curve and surface type" (cstype), §"Degree"
//! (deg degu degv), §"Surface" (surf s0 s1 t0 t1 …), §"Taylor"
//! (polynomial coefficients as control points), §"Surface vertex data
//! — control points" (row-major u-fastest ordering), §"Free-form
//! curve/surface body statements" (the rational form "does not make
//! sense for Taylor").

use oxideav_mesh3d::{Mesh3DDecoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder, obj};

/// Bilinear (`deg 1 1`) Taylor patch over a planar unit square. The
/// control points are the bivariate polynomial coefficients
/// `c_{i,j}` for `S(u,v) = c_{0,0} + c_{1,0}·u + c_{0,1}·v + c_{1,1}·u·v`,
/// listed row-major with u varying fastest:
///
///   j = 0:  v1 = c_{0,0} = (0,0,0)   v2 = c_{1,0} = (1,0,0)
///   j = 1:  v3 = c_{0,1} = (0,1,0)   v4 = c_{1,1} = (0,0,0)
///
/// This evaluates to `S(u,v) = (u, v, 0)` — the flat unit square
/// parametrised by (u, v).
const BILINEAR_TAYLOR_SURF: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 0.0 0.0 0.0
cstype taylor
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";

#[test]
fn default_decoder_does_not_tessellate_taylor_surfaces() {
    let bare = ObjDecoder::new()
        .decode(BILINEAR_TAYLOR_SURF.as_bytes())
        .unwrap();
    assert!(
        bare.meshes.is_empty(),
        "default decoder must not synthesise Taylor surface meshes"
    );
}

#[test]
fn bilinear_taylor_surface_evaluates_to_the_unit_square_parametrisation() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(BILINEAR_TAYLOR_SURF.as_bytes())
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

    // S(u,v) = (u, v, 0). Sample lattice at (su/4, sv/4).
    let stride = 5usize;
    for sv in 0..stride {
        for su in 0..stride {
            let v = sv as f32 / 4.0;
            let u = su as f32 / 4.0;
            let p = prim.positions[sv * stride + su];
            assert!(
                (p[0] - u).abs() < 1e-5 && (p[1] - v).abs() < 1e-5 && p[2].abs() < 1e-5,
                "lattice (su={su}, sv={sv}) wants ({u},{v},0) got {p:?}"
            );
        }
    }

    // Provenance extras.
    assert_eq!(
        prim.extras
            .get("obj:tessellated_surface")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        prim.extras
            .get("obj:tessellated_curve")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        prim.extras.get("obj:surface_kind").and_then(|v| v.as_str()),
        Some("taylor")
    );
    let deg = prim.extras.get("obj:surface_degree").unwrap();
    assert_eq!(deg.as_array().unwrap()[0].as_u64(), Some(1));
    assert_eq!(deg.as_array().unwrap()[1].as_u64(), Some(1));
    assert_eq!(
        prim.extras
            .get("obj:surface_samples")
            .and_then(|v| v.as_u64()),
        Some(4)
    );
}

/// Bicubic Taylor patch with a non-trivial mix of monomial coefficients.
/// Coefficient layout (i = u-power 0..3 across, j = v-power 0..3 down,
/// row-major u-fastest):
///
///   j = 0:  c_{0,0}=(0,0,1)  c_{1,0}=(1,0,0)  c_{2,0}=(0,0,2)  c_{3,0}=(0,0,0)
///   j = 1:  c_{0,1}=(0,1,0)  c_{1,1}=(0,0,3)  c_{2,1}=(0,0,0)  c_{3,1}=(0,0,0)
///   j = 2:  c_{0,2}=(0,0,4)  c_{1,2}=(0,0,0)  c_{2,2}=(0,0,0)  c_{3,2}=(0,0,0)
///   j = 3:  c_{0,3}=(0,0,0)  c_{1,3}=(0,0,0)  c_{2,3}=(0,0,0)  c_{3,3}=(0,0,0)
///
/// This is `S(u,v) = (u, v, 1 + 2u² + 3uv + 4v²)`.
const BICUBIC_TAYLOR_SURF: &str = "\
v 0 0 1
v 1 0 0
v 0 0 2
v 0 0 0
v 0 1 0
v 0 0 3
v 0 0 0
v 0 0 0
v 0 0 4
v 0 0 0
v 0 0 0
v 0 0 0
v 0 0 0
v 0 0 0
v 0 0 0
v 0 0 0
cstype taylor
deg 3 3
surf 0.0 1.0 0.0 1.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
parm u 0.0 1.0
parm v 0.0 1.0
end
";

fn eval_bicubic_taylor(u: f32, v: f32) -> [f32; 3] {
    [u, v, 1.0 + 2.0 * u * u + 3.0 * u * v + 4.0 * v * v]
}

#[test]
fn bicubic_taylor_surface_matches_analytic_polynomial() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(6)
        .decode(BICUBIC_TAYLOR_SURF.as_bytes())
        .unwrap();
    let mesh = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:surfaces"))
        .expect("synthetic surface mesh");
    let prim = &mesh.primitives[0];
    assert_eq!(prim.topology, Topology::Triangles);
    assert_eq!(prim.positions.len(), 49, "(6 + 1)^2 lattice vertices");

    let stride = 7usize;
    for sv in 0..stride {
        for su in 0..stride {
            let u = su as f32 / 6.0;
            let v = sv as f32 / 6.0;
            let want = eval_bicubic_taylor(u, v);
            let got = prim.positions[sv * stride + su];
            for k in 0..3 {
                assert!(
                    (got[k] - want[k]).abs() < 5e-5,
                    "(su={su}, sv={sv}) axis {k}: got {} want {}",
                    got[k],
                    want[k]
                );
            }
        }
    }
    assert_eq!(
        prim.extras.get("obj:surface_kind").and_then(|v| v.as_str()),
        Some("taylor")
    );
    let deg = prim.extras.get("obj:surface_degree").unwrap();
    assert_eq!(deg.as_array().unwrap()[0].as_u64(), Some(3));
    assert_eq!(deg.as_array().unwrap()[1].as_u64(), Some(3));
}

/// Taylor surface evaluated over a non-default `s0/s1/t0/t1` window.
/// Same bilinear `S(u,v) = (u, v, 0)` patch as the first test but with
/// `surf 0.25 0.75 0.5 1.0 …` — the corner samples must land at the
/// clipped parameter values, not the original (0, 0)-(1, 1).
const CLIPPED_TAYLOR_SURF: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 0.0 0.0 0.0
cstype taylor
deg 1 1
surf 0.25 0.75 0.5 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";

#[test]
fn taylor_surface_honours_surf_parameter_clip() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(2)
        .decode(CLIPPED_TAYLOR_SURF.as_bytes())
        .unwrap();
    let mesh = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:surfaces"))
        .expect("synthetic surface mesh");
    let prim = &mesh.primitives[0];
    assert_eq!(prim.positions.len(), 9, "(2 + 1)^2 lattice vertices");

    let stride = 3usize;
    // Bilinear collapses to S(u,v) = (u, v, 0), so the (su, sv) corners
    // are the clipped (u, v) values.
    let corners = [
        ((0, 0), 0.25_f32, 0.5_f32),
        ((2, 0), 0.75_f32, 0.5_f32),
        ((0, 2), 0.25_f32, 1.0_f32),
        ((2, 2), 0.75_f32, 1.0_f32),
    ];
    for ((su, sv), u, v) in corners {
        let p = prim.positions[sv * stride + su];
        assert!(
            (p[0] - u).abs() < 1e-5 && (p[1] - v).abs() < 1e-5 && p[2].abs() < 1e-5,
            "corner (su={su}, sv={sv}) wants ({u},{v},0) got {p:?}"
        );
    }

    let u_range = prim.extras.get("obj:surface_u_range").unwrap();
    let v_range = prim.extras.get("obj:surface_v_range").unwrap();
    assert!((u_range.as_array().unwrap()[0].as_f64().unwrap() - 0.25).abs() < 1e-6);
    assert!((u_range.as_array().unwrap()[1].as_f64().unwrap() - 0.75).abs() < 1e-6);
    assert!((v_range.as_array().unwrap()[0].as_f64().unwrap() - 0.5).abs() < 1e-6);
    assert!((v_range.as_array().unwrap()[1].as_f64().unwrap() - 1.0).abs() < 1e-6);
}

/// `cstype rat taylor` is accepted for syntactic compatibility but
/// routes to the same evaluator with no weight blending (spec
/// §"Free-form curve/surface body statements" explicitly says the
/// rational form "does not make sense for Taylor"). Two identical
/// bilinear surfaces — one `taylor`, one `rat taylor` whose `v` lines
/// carry distinct `w` weights — should tessellate to identical
/// positions, confirming the weights have no effect.
#[test]
fn rat_taylor_surface_ignores_per_vertex_weights() {
    let plain = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 0.0 0.0 0.0
cstype taylor
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let rat = "\
v 0.0 0.0 0.0 7.0
v 1.0 0.0 0.0 0.5
v 0.0 1.0 0.0 3.0
v 0.0 0.0 0.0 0.25
cstype rat taylor
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let a = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(plain.as_bytes())
        .unwrap();
    let b = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(rat.as_bytes())
        .unwrap();

    let pa = &a.meshes[0].primitives[0];
    let pb = &b.meshes[0].primitives[0];
    assert_eq!(pa.positions.len(), pb.positions.len());
    for (x, y) in pa.positions.iter().zip(pb.positions.iter()) {
        for k in 0..3 {
            assert!((x[k] - y[k]).abs() < 1e-5);
        }
    }
}

/// Encoder filters synthetic Taylor surface primitives out so a
/// decode → encode pass reproduces the original `cstype taylor` block
/// from `Scene3D::extras["obj:freeform_directives"]` unchanged.
#[test]
fn taylor_surface_roundtrip_emits_original_directive_block() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(BILINEAR_TAYLOR_SURF.as_bytes())
        .unwrap();
    let mut encoder = ObjEncoder::new();
    use oxideav_mesh3d::Mesh3DEncoder;
    let bytes = encoder.encode(&scene).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    // The freeform block round-trips verbatim (no synthetic geometry
    // bleeds into the output).
    assert!(text.contains("cstype taylor"), "encoded text:\n{text}");
    assert!(
        text.contains("surf 0 1 0 1 1 2 3 4") || text.contains("surf 0.0 1.0"),
        "surf line missing or reformatted: {text}"
    );
    assert!(text.contains("end"));
    // None of the synthetic-surface sentinel positions should appear in
    // an `f` face line.
    assert!(
        !text.contains("\nf "),
        "synthetic surface triangles must not be emitted as polygonal faces:\n{text}"
    );
}

/// Free-function path also produces a Taylor surface mesh under the
/// `curve_tessellation_samples` parse option.
#[test]
fn parse_obj_with_options_tessellates_taylor_surface() {
    let scene = obj::parse_obj_with_options(
        BILINEAR_TAYLOR_SURF,
        &obj::ParseOptions {
            curve_tessellation_samples: 2,
            ..Default::default()
        },
        |_| Ok(Vec::new()),
    )
    .unwrap();
    let mesh = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:surfaces"))
        .expect("synthetic surface mesh");
    assert_eq!(mesh.primitives[0].positions.len(), 9);
}
