//! Vertex-normal synthesis from smoothing-group state, per Wavefront
//! OBJ spec §"Grouping" (`s group_number`): "Smoothing group
//! statements let you identify elements over which normals are to be
//! interpolated to give those elements a smooth, non-faceted
//! appearance. This is a quick way to specify vertex normals."
//!
//! The synthesis is opt-in via
//! [`ObjDecoder::with_normal_generation`]; the default leaves `vn`-less
//! primitives with `normals == None` (historical behaviour).

use oxideav_mesh3d::{Indices, Mesh3DDecoder, Topology};
use oxideav_obj::obj::NormalGeneration;
use oxideav_obj::{ObjDecoder, obj};

fn unit_len(n: [f32; 3]) -> f32 {
    (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
}

/// Default decoder leaves `vn`-less faces without normals.
#[test]
fn default_decoder_does_not_synthesise_normals() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
s 1
f 1 2 3
";
    let scene = obj::parse_obj(text).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert!(prim.normals.is_none());
    assert!(!prim.extras.contains_key("obj:generated_normals"));
}

/// A single flat triangle in an active smoothing group gets one
/// unit-length normal per vertex pointing along +Z (CCW winding in
/// the XY plane).
#[test]
fn smooth_group_generates_unit_normals() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
s 1
f 1 2 3
";
    let scene = ObjDecoder::new()
        .with_normal_generation(NormalGeneration::FromSmoothingGroups)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    let normals = prim.normals.as_ref().expect("normals synthesised");
    assert_eq!(normals.len(), prim.positions.len());
    for n in normals {
        assert!((unit_len(*n) - 1.0).abs() < 1e-5, "normal {n:?} not unit");
        // +Z front face for a CCW triangle in the z=0 plane.
        assert!(n[2] > 0.99, "expected +Z normal, got {n:?}");
    }
    assert_eq!(
        prim.extras
            .get("obj:generated_normals")
            .and_then(|v| v.as_str()),
        Some("smooth"),
    );
}

/// Two triangles sharing an edge in one smoothing group share averaged
/// normals at the shared vertices — positions are NOT de-shared.
#[test]
fn smooth_group_shares_vertices() {
    // A "roof": two quads meeting at a ridge, folded so the shared
    // ridge vertices average the two facet normals.
    let text = "\
v -1 0 1
v  1 0 1
v -1 1 0
v  1 1 0
v -1 0 -1
v  1 0 -1
s 1
f 1 2 4 3
f 3 4 6 5
";
    let scene = ObjDecoder::new()
        .with_normal_generation(NormalGeneration::FromSmoothingGroups)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    // Smooth shading keeps the interned vertex set (6 unique corners),
    // not one-per-triangle-corner.
    assert_eq!(prim.positions.len(), 6);
    let normals = prim.normals.as_ref().unwrap();
    // Ridge vertex (index for v3 = [-1,1,0]) is shared by both facets;
    // its normal is the average of the two slope normals → tilts but
    // stays unit length.
    for n in normals {
        assert!((unit_len(*n) - 1.0).abs() < 1e-5);
    }
}

/// `s off` produces faceted normals: each triangle owns three unique
/// vertices so adjacent faces keep a hard edge.
#[test]
fn smoothing_off_generates_faceted_normals() {
    let text = "\
v -1 0 1
v  1 0 1
v -1 1 0
v  1 1 0
v -1 0 -1
v  1 0 -1
s off
f 1 2 4 3
f 3 4 6 5
";
    let scene = ObjDecoder::new()
        .with_normal_generation(NormalGeneration::FromSmoothingGroups)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    // Two quads → fan-triangulated to 4 triangles → 12 unique corners.
    let idx = match prim.indices.as_ref().unwrap() {
        Indices::U16(v) => v.len(),
        Indices::U32(v) => v.len(),
    };
    assert_eq!(idx, 12);
    assert_eq!(prim.positions.len(), 12);
    let normals = prim.normals.as_ref().unwrap();
    assert_eq!(normals.len(), 12);
    // The three corners of any one triangle share an identical normal
    // (flat facet).
    for tri in 0..4 {
        let a = normals[tri * 3];
        let b = normals[tri * 3 + 1];
        let c = normals[tri * 3 + 2];
        assert_eq!(a, b);
        assert_eq!(b, c);
        assert!((unit_len(a) - 1.0).abs() < 1e-5);
    }
    assert_eq!(
        prim.extras
            .get("obj:generated_normals")
            .and_then(|v| v.as_str()),
        Some("flat"),
    );
}

/// No `s` directive at all defaults to faceted (the spec default group
/// is off).
#[test]
fn no_smoothing_directive_is_faceted() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
f 1 2 3
";
    let scene = ObjDecoder::new()
        .with_normal_generation(NormalGeneration::FromSmoothingGroups)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.extras
            .get("obj:generated_normals")
            .and_then(|v| v.as_str()),
        Some("flat"),
    );
}

/// Explicit `vn` data supersedes smoothing groups — the primitive is
/// left untouched (spec §"Vertex normals").
#[test]
fn explicit_normals_are_not_overwritten() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
vn 0 0 1
s 1
f 1//1 2//1 3//1
";
    let scene = ObjDecoder::new()
        .with_normal_generation(NormalGeneration::FromSmoothingGroups)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert!(prim.normals.is_some());
    // Not flagged as generated — these came from the source `vn`.
    assert!(!prim.extras.contains_key("obj:generated_normals"));
    for n in prim.normals.as_ref().unwrap() {
        assert_eq!(*n, [0.0, 0.0, 1.0]);
    }
}

/// Line / point primitives are never touched by normal generation.
#[test]
fn lines_are_not_given_normals() {
    let text = "\
v 0 0 0
v 1 0 0
v 2 0 0
s 1
l 1 2 3
";
    let scene = ObjDecoder::new()
        .with_normal_generation(NormalGeneration::FromSmoothingGroups)
        .decode(text.as_bytes())
        .unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert!(matches!(
        prim.topology,
        Topology::LineStrip | Topology::Lines | Topology::LineLoop
    ));
    assert!(prim.normals.is_none());
}

/// A decode-with-generation → encode round-trip must NOT fabricate
/// `vn` lines: the source file had none, so the re-emitted OBJ stays
/// `vn`-free and the faces keep the plain `f a b c` syntax.
#[test]
fn generated_normals_do_not_leak_into_encode() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
s 1
f 1 2 3
";
    let scene = ObjDecoder::new()
        .with_normal_generation(NormalGeneration::FromSmoothingGroups)
        .decode(text.as_bytes())
        .unwrap();
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(
        !s.contains("\nvn "),
        "synthesised vn leaked into output:\n{s}"
    );
    assert!(!s.contains("//"), "synthesised normal index leaked:\n{s}");
    // The face is still present in plain form.
    assert!(s.contains("\nf 1 2 3\n"), "face missing/altered:\n{s}");
}

/// The faceted path de-shares vertices, but a re-encode must still stay
/// `vn`-free and re-decode cleanly (no synthesised-normal leakage, no
/// panic on the re-expanded geometry).
#[test]
fn faceted_generation_re_encodes_vn_free() {
    let text = "\
v -1 0 1
v  1 0 1
v -1 1 0
v  1 1 0
v -1 0 -1
v  1 0 -1
s off
f 1 2 4 3
f 3 4 6 5
";
    let scene = ObjDecoder::new()
        .with_normal_generation(NormalGeneration::FromSmoothingGroups)
        .decode(text.as_bytes())
        .unwrap();
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(
        !s.contains("\nvn "),
        "faceted normals leaked into output:\n{s}"
    );
    assert!(!s.contains("//"), "faceted normal index leaked:\n{s}");
    // Re-decode (default decoder) must succeed.
    let scene2 = obj::parse_obj(s).unwrap();
    assert!(!scene2.meshes.is_empty());
    assert!(scene2.meshes[0].primitives[0].normals.is_none());
}

/// A primitive that mixes `vn`-bearing and `vn`-less faces (spec-illegal
/// per §"f", but produced by lenient tools) leaves `[0,0,0]` placeholder
/// normals by default. With normal generation enabled, the zero
/// placeholders are backfilled from geometry while explicit normals are
/// preserved exactly.
#[test]
fn partial_normals_backfilled_without_generation_default_leaves_zeros() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 2 0 0
v 2 1 0
vn 0 0 1
f 1//1 2//1 3//1
f 2 4 5
";
    // Default decoder: the `vn`-less face's vertices carry a zero normal.
    let scene = obj::parse_obj(text).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    let normals = prim.normals.as_ref().expect("has_normal primitive");
    assert!(
        normals.contains(&[0.0, 0.0, 0.0]),
        "expected a zero placeholder normal in the default decode",
    );

    // With generation: zeros are backfilled, explicit (0,0,1) preserved.
    let scene2 = ObjDecoder::new()
        .with_normal_generation(NormalGeneration::FromSmoothingGroups)
        .decode(text.as_bytes())
        .unwrap();
    let prim2 = &scene2.meshes[0].primitives[0];
    let normals2 = prim2.normals.as_ref().unwrap();
    assert!(
        !normals2.contains(&[0.0, 0.0, 0.0]),
        "zero placeholders should be backfilled: {normals2:?}",
    );
    // The explicitly-specified normal is still present unchanged.
    assert!(
        normals2.contains(&[0.0, 0.0, 1.0]),
        "explicit normal must be preserved: {normals2:?}",
    );
    // Backfilled mixed primitive is NOT flagged generated (round-trip
    // still emits `v//vn` because explicit normals exist).
    assert!(!prim2.extras.contains_key("obj:generated_normals"));
}
