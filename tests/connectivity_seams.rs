//! Connectivity (`con`) seam tessellation —
//! `ObjDecoder::with_curve_tessellation(N)` evaluates every `con`
//! connectivity statement (spec §"Connectivity between free-form
//! surfaces", §"con surf_1 q0_1 q1_1 curv2d_1 surf_2 q0_2 q1_2
//! curv2d_2") into a pair of parameter-space `LineStrip` seams — one
//! per joined surface edge — on a synthetic mesh named `"obj:cons"`.
//!
//! A `con` statement carries two `(surf, q0, q1, curv2d)` quadruples
//! that tie two previously-declared `surf` blocks together along a
//! shared trimming-curve segment for edge merging. Where the typed
//! `obj:connectivity` view surfaces the eight raw arguments, this pass
//! draws the seam itself: each `curv2d` is resolved through the same
//! `curv2` table the trim / hole / scrv passes use, and the `[q0, q1]`
//! sub-range is walked with the shared `append_curv2_segment` so a
//! connectivity seam is sampled identically to a special-curve segment.
//!
//! The free-form directive sequence still rides on
//! `Scene3D::extras["obj:freeform_directives"]` so a decode → encode
//! cycle replays the original `con` statement verbatim — the encoder
//! filters the synthetic seams out via the shared
//! `obj:tessellated_curve` sentinel.

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Scene3D, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder};

/// Two adjacent unit-square Bezier surfaces, each with a linear `curv2`
/// trimming loop around its parameter square, joined by a single `con`
/// along the shared trimming-curve segment. The first surface spans
/// x ∈ [0, 1], the second x ∈ [1, 2]; the `con` welds the right edge of
/// surface 1 (curve sub-range [1.0, 2.0]) to the left edge of surface 2
/// (curve sub-range [3.0, 2.0], reversed so the endpoints meet). Shaped
/// after the spec §"Connectivity between free-form surfaces" worked
/// example, but with a non-degenerate parameter range on both sides so
/// each edge resolves to a multi-point seam (the literal spec example's
/// `2.0 2.0 … 4.0 3.0` point-join on side 1 is exercised separately in
/// `con_point_join_emits_single_seam`).
const CON_TWO_SURFACES: &str = "\
cstype bezier
deg 1 1

v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0

vp 0 0
vp 1 0
vp 1 1
vp 0 1

curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0
end

surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
trim 0.0 4.0 1
end

v 1 0 0
v 2 0 0
v 1 1 0
v 2 1 0

surf 0.0 1.0 0.0 1.0 5 6 7 8
parm u 0.0 1.0
parm v 0.0 1.0
trim 0.0 4.0 1
end

con 1 1.0 2.0 1 2 3.0 2.0 1
";

fn con_mesh(scene: &Scene3D) -> &oxideav_mesh3d::Mesh {
    scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:cons"))
        .expect("obj:cons mesh must exist")
}

fn decode_tess(src: &str, samples: u32) -> Scene3D {
    ObjDecoder::new()
        .with_curve_tessellation(samples)
        .decode(src.as_bytes())
        .unwrap()
}

#[test]
fn con_directive_stays_captured_when_tessellation_is_disabled() {
    let scene = ObjDecoder::new()
        .decode(CON_TWO_SURFACES.as_bytes())
        .unwrap();
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.name.as_deref() != Some("obj:cons")),
        "default decoder must not synthesise connectivity meshes"
    );
    // Directive captured verbatim on Scene3D extras for round-trip.
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .and_then(|v| v.as_array())
        .expect("freeform directives captured");
    let keywords: Vec<&str> = dirs
        .iter()
        .filter_map(|d| d.as_array())
        .filter_map(|a| a.first())
        .filter_map(|t| t.as_str())
        .collect();
    assert!(keywords.contains(&"con"), "con directive must be captured");
}

#[test]
fn con_emits_two_seams_one_per_surface_edge() {
    let scene = decode_tess(CON_TWO_SURFACES, 8);
    let mesh = con_mesh(&scene);
    assert_eq!(
        mesh.primitives.len(),
        2,
        "one con must emit exactly two seams (one per surface edge)"
    );
    for prim in &mesh.primitives {
        assert_eq!(prim.topology, Topology::LineStrip);
        assert!(
            prim.positions.len() >= 2,
            "each seam must carry at least two points"
        );
        // Synthetic sentinel + connectivity flag.
        assert_eq!(
            prim.extras
                .get("obj:tessellated_curve")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            prim.extras.get("obj:con").and_then(|v| v.as_bool()),
            Some(true)
        );
    }
}

#[test]
fn con_seam_provenance_pairs_the_two_surfaces() {
    let scene = decode_tess(CON_TWO_SURFACES, 8);
    let mesh = con_mesh(&scene);

    let side1 = mesh
        .primitives
        .iter()
        .find(|p| p.extras.get("obj:con_side").and_then(|v| v.as_u64()) == Some(1))
        .expect("side-1 seam present");
    let side2 = mesh
        .primitives
        .iter()
        .find(|p| p.extras.get("obj:con_side").and_then(|v| v.as_u64()) == Some(2))
        .expect("side-2 seam present");

    // con 1 1.0 2.0 1 2 3.0 2.0 1
    // side 1: surf_1 = 1, curv2d_1 = 1, q0_1 = 1.0, q1_1 = 2.0
    assert_eq!(
        side1.extras.get("obj:con_surf").and_then(|v| v.as_i64()),
        Some(1)
    );
    assert_eq!(
        side1
            .extras
            .get("obj:con_peer_surf")
            .and_then(|v| v.as_i64()),
        Some(2)
    );
    assert_eq!(
        side1.extras.get("obj:con_curv2d").and_then(|v| v.as_i64()),
        Some(1)
    );
    assert_eq!(
        side1.extras.get("obj:con_q0").and_then(|v| v.as_f64()),
        Some(1.0)
    );
    assert_eq!(
        side1.extras.get("obj:con_q1").and_then(|v| v.as_f64()),
        Some(2.0)
    );

    // side 2: surf_2 = 2, peer = surf_1 = 1, curv2d_2 = 1
    assert_eq!(
        side2.extras.get("obj:con_surf").and_then(|v| v.as_i64()),
        Some(2)
    );
    assert_eq!(
        side2
            .extras
            .get("obj:con_peer_surf")
            .and_then(|v| v.as_i64()),
        Some(1)
    );
    assert_eq!(
        side2.extras.get("obj:con_q0").and_then(|v| v.as_f64()),
        Some(3.0)
    );
    assert_eq!(
        side2.extras.get("obj:con_q1").and_then(|v| v.as_f64()),
        Some(2.0)
    );
}

/// The spec §"Connectivity between free-form surfaces" worked example
/// joins surface 1 at a single parameter point (`q0_1 == q1_1 == 2.0`)
/// to a range on surface 2 (`4.0 3.0`). A zero-length parameter range
/// can't form a polyline seam, so side 1 drops while side 2 emits — one
/// seam total. This documents the point-join edge case without failing
/// the parse.
#[test]
fn con_point_join_emits_single_seam() {
    const SRC: &str = "\
cstype bezier
deg 1 1
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
vp 0 0
vp 1 0
vp 1 1
vp 0 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0
end
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
v 1 0 0
v 2 0 0
v 1 1 0
v 2 1 0
surf 0.0 1.0 0.0 1.0 5 6 7 8
parm u 0.0 1.0
parm v 0.0 1.0
end
con 1 2.0 2.0 1 2 4.0 3.0 1
";
    let scene = decode_tess(SRC, 8);
    let mesh = con_mesh(&scene);
    assert_eq!(
        mesh.primitives.len(),
        1,
        "point-join side drops; only the ranged side emits"
    );
    assert_eq!(
        mesh.primitives[0]
            .extras
            .get("obj:con_side")
            .and_then(|v| v.as_u64()),
        Some(2)
    );
}

/// The seam geometry lives in parameter space (z = 0), matching the
/// `curv2` / `scrv` synthetic layout. Both endpoints of a seam over the
/// closed perimeter loop `curv2 1 2 3 4 1` at parameter 2.0 (side 1) and
/// 3.0 (side 2) must land on the unit square corners.
#[test]
fn con_seam_lives_in_parameter_space_z_zero() {
    let scene = decode_tess(CON_TWO_SURFACES, 8);
    let mesh = con_mesh(&scene);
    for prim in &mesh.primitives {
        for p in &prim.positions {
            assert_eq!(p[2], 0.0, "connectivity seam is parameter-space (z = 0)");
            // The trimming loop walks the unit square, so every sampled
            // point stays within [0, 1] in u and v.
            assert!((0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]));
        }
    }
}

/// A non-degenerate sub-range produces a multi-point seam tracking the
/// curve. Walking [1.0, 3.0] of the square perimeter spans the bottom-
/// right and right edges, so the seam must contain more than the two
/// endpoints.
#[test]
fn con_subrange_samples_the_curve() {
    const SRC: &str = "\
cstype bezier
deg 1 1
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
vp 0 0
vp 1 0
vp 1 1
vp 0 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0
end
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
v 1 0 0
v 2 0 0
v 1 1 0
v 2 1 0
surf 0.0 1.0 0.0 1.0 5 6 7 8
parm u 0.0 1.0
parm v 0.0 1.0
end
con 1 1.0 3.0 1 2 3.0 1.0 1
";
    let scene = decode_tess(SRC, 16);
    let mesh = con_mesh(&scene);
    assert_eq!(mesh.primitives.len(), 2);
    for prim in &mesh.primitives {
        assert!(
            prim.positions.len() > 2,
            "a non-trivial sub-range must sample intermediate curve points"
        );
    }
}

#[test]
fn malformed_con_is_dropped_without_failing_parse() {
    // con with seven args (missing curv2d_2) — typed/geometry views drop
    // it, the verbatim channel keeps it.
    const SRC: &str = "\
cstype bezier
deg 1 1
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
vp 0 0
vp 1 0
vp 1 1
vp 0 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0
end
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
con 1 2.0 2.0 1 2 4.0 3.0
";
    let scene = decode_tess(SRC, 8);
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.name.as_deref() != Some("obj:cons")),
        "a malformed con must not synthesise a seam"
    );
    // Still captured verbatim.
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(
        dirs.iter()
            .filter_map(|d| d.as_array())
            .filter_map(|a| a.first())
            .filter_map(|t| t.as_str())
            .any(|k| k == "con")
    );
}

#[test]
fn con_side_drops_independently_when_one_curve_is_missing() {
    // curv2d_2 = 7 references a curve that was never defined; side 2
    // drops on its own while side 1 still emits.
    const SRC: &str = "\
cstype bezier
deg 1 1
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
vp 0 0
vp 1 0
vp 1 1
vp 0 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0
end
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
v 1 0 0
v 2 0 0
v 1 1 0
v 2 1 0
surf 0.0 1.0 0.0 1.0 5 6 7 8
parm u 0.0 1.0
parm v 0.0 1.0
end
con 1 1.0 3.0 1 2 3.0 1.0 7
";
    let scene = decode_tess(SRC, 8);
    let mesh = con_mesh(&scene);
    assert_eq!(
        mesh.primitives.len(),
        1,
        "only the resolvable side emits a seam"
    );
    assert_eq!(
        mesh.primitives[0]
            .extras
            .get("obj:con_side")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
}

/// The synthetic connectivity seam must NOT leak into a re-encode: the
/// encoder filters it out via the shared `obj:tessellated_curve`
/// sentinel, replaying the original `con` from the captured directive
/// stream so the output round-trips back to a single con statement.
#[test]
fn con_seam_is_filtered_on_re_encode() {
    let scene = decode_tess(CON_TWO_SURFACES, 8);
    // The con mesh exists in the decoded scene.
    assert!(
        scene
            .meshes
            .iter()
            .any(|m| m.name.as_deref() == Some("obj:cons"))
    );

    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let text = String::from_utf8(bytes).unwrap();

    // Exactly one con statement, no synthetic seam geometry, and the
    // original argument list preserved verbatim.
    let con_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("con ")).collect();
    assert_eq!(con_lines.len(), 1, "one con line re-emitted");
    assert_eq!(con_lines[0].trim(), "con 1 1.0 2.0 1 2 3.0 2.0 1");

    // Re-decoding the output reproduces the seam mesh — the geometry is
    // derived, not stored.
    let round = decode_tess(&text, 8);
    let mesh = con_mesh(&round);
    assert_eq!(mesh.primitives.len(), 2);
}
