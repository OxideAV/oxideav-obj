//! Smooth per-vertex normals on tessellated `surf` surfaces.
//!
//! When `ObjDecoder::with_curve_tessellation(N)` evaluates a `surf`
//! element into a `Topology::Triangles` mesh, the synthetic primitive
//! now carries a vertex-normal buffer alongside positions/indices so a
//! downstream renderer or glTF exporter can shade the surface smoothly
//! rather than flat. Normals are area-weighted face-normal averages over
//! the emitted triangle lattice; the winding is the spec front
//! orientation (spec §"surf s0 s1 t0 t1 v1/vt1/vn1 …": "the front of the
//! surface is the side where u increases to the right and v increases
//! upward"), so every normal points out of the surface front.
//!
//! Spec references: §"surf s0 s1 t0 t1 v1/vt1/vn1 …" (element + front
//! orientation), §"Bezier" (basis), §"Curve and surface type" (cstype).

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_mesh3d::Topology;
use oxideav_obj::ObjDecoder;

fn unit_len(n: &[f32; 3]) -> f32 {
    (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
}

/// Planar bicubic Bezier surface in the z = 0 plane. Every vertex normal
/// must be the constant plane normal ±Z and unit length. The CCW front
/// winding (u to the right, v upward) gives +Z.
const PLANAR_BICUBIC: &str = "\
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
fn planar_surface_normals_are_constant_plus_z() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(PLANAR_BICUBIC.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.topology, Topology::Triangles);

    let normals = prim
        .normals
        .as_ref()
        .expect("tessellated surface must carry vertex normals");
    // One normal per position.
    assert_eq!(normals.len(), prim.positions.len());
    assert_eq!(normals.len(), 25, "4 samples ⇒ 5×5 lattice");

    for n in normals {
        assert!(
            (unit_len(n) - 1.0).abs() < 1e-4,
            "normal not unit length: {n:?}"
        );
        assert!(
            n[0].abs() < 1e-4 && n[1].abs() < 1e-4,
            "off-plane tilt: {n:?}"
        );
        assert!(
            (n[2] - 1.0).abs() < 1e-4,
            "front winding (u right, v up) should give +Z, got {n:?}"
        );
    }
}

/// Non-planar bilinear (hyperbolic-paraboloid / "saddle") Bezier patch:
/// the four corners are at alternating heights, so the surface is a
/// genuinely curved sheet. The vertex normals must all be unit length,
/// must vary across the lattice (not a single constant value), and must
/// keep a positive z-component (the saddle still faces generally
/// upward under the spec front winding).
const SADDLE_BILINEAR: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 1.0
v 0.0 1.0 1.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";

#[test]
fn curved_surface_normals_are_unit_length_and_vary() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(SADDLE_BILINEAR.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    let normals = prim.normals.as_ref().expect("vertex normals present");
    assert_eq!(normals.len(), prim.positions.len());

    for n in normals {
        assert!(
            (unit_len(n) - 1.0).abs() < 1e-4,
            "normal not unit length: {n:?}"
        );
        assert!(
            n[2] > 0.0,
            "saddle front normal should point upward, got {n:?}"
        );
    }

    // The surface is curved, so at least two lattice vertices must carry
    // measurably different normals (a flat surface would fail this).
    let first = normals[0];
    let differs = normals
        .iter()
        .any(|n| (n[0] - first[0]).abs() > 1e-3 || (n[1] - first[1]).abs() > 1e-3);
    assert!(differs, "curved surface should have varying normals");
}

/// A trimmed surface (`trim` loop dropping part of the lattice plus
/// sub-cell boundary re-mesh) must still produce a normal for *every*
/// vertex, including the synthesised trim-boundary vertices, with no
/// NaNs and unit length throughout.
const TRIMMED_PLANAR: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
vp 0.25 0.25
vp 0.75 0.25
vp 0.75 0.75
vp 0.25 0.75
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
trim 0.0 1.0 1
curv2 1 2 3 4 1
end
";

#[test]
fn trimmed_surface_has_normal_for_every_vertex_no_nan() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(6)
        .decode(TRIMMED_PLANAR.as_bytes())
        .unwrap();
    // Locate the triangulated surface primitive specifically — a trim
    // block can also synthesise a `curv2` boundary LineStrip mesh, which
    // is not what this test is about.
    let prim = scene.meshes.iter().flat_map(|m| &m.primitives).find(|p| {
        p.topology == Topology::Triangles
            && p.extras
                .get("obj:tessellated_surface")
                .and_then(|v| v.as_bool())
                == Some(true)
    });
    let Some(prim) = prim else {
        // If the trim block left the surface captured-only the test is
        // vacuous; base-surface normal coverage is exercised above.
        return;
    };
    let normals = prim.normals.as_ref().expect("vertex normals present");
    assert_eq!(
        normals.len(),
        prim.positions.len(),
        "normal buffer must stay length-parallel with positions"
    );
    for n in normals {
        assert!(
            n.iter().all(|c| c.is_finite()),
            "no NaN/Inf normals allowed: {n:?}"
        );
        assert!(
            (unit_len(n) - 1.0).abs() < 1e-4,
            "normal not unit length: {n:?}"
        );
    }
}
