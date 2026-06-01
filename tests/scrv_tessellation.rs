//! Special-curve (`scrv`) tessellation —
//! `ObjDecoder::with_curve_tessellation(N)` evaluates every `scrv`
//! directive (the special curve on a surface, spec §"Special curve",
//! §"scrv u0 u1 curv2d u0 u1 curv2d …") into a parameter-space
//! `LineStrip` polyline on a synthetic mesh named `"obj:scrvs"`.
//!
//! A `scrv` line has the same `(u0, u1, curv2d)` triple shape as
//! `trim` / `hole`: each triple selects a sub-range of a previously
//! defined `curv2` parameter-space curve. Unlike `trim` / `hole` the
//! resulting polyline is NOT a closed polygon — the spec describes it
//! as a "sequence of curves which lie on a given surface to build a
//! single special curve" that the surface triangulator must include
//! as a sequence of triangle edges. Surface-aware triangulation
//! against that constraint remains future work; this round emits the
//! special curve as a stand-alone parameter-space polyline so
//! consumers that care can resolve it without re-walking the
//! directive stream.
//!
//! The free-form directive sequence still rides on
//! `Scene3D::extras["obj:freeform_directives"]` so a decode → encode
//! cycle replays the original `cstype` / `surf` / `scrv` / `end`
//! block verbatim — the encoder filters the synthetic polyline out
//! via the shared `obj:tessellated_curve` sentinel.

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder};

/// A linear `curv2` through four `vp` corners of the unit square plus
/// a `scrv` that walks the perimeter as a single special curve. The
/// surface is a unit-square bilinear Bezier patch in xy.
const SCRV_ON_SQUARE: &str = "\
vp 0.0 0.0
vp 1.0 0.0
vp 1.0 1.0
vp 0.0 1.0
cstype bspline
deg 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0 5.0 6.0
end
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
scrv 0.0 6.0 1
end
";

fn scrv_prim(scene: &oxideav_mesh3d::Scene3D) -> &oxideav_mesh3d::Primitive {
    let mesh = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:scrvs"))
        .expect("obj:scrvs mesh must exist");
    assert_eq!(mesh.primitives.len(), 1, "one scrv primitive expected");
    &mesh.primitives[0]
}

#[test]
fn scrv_directive_stays_captured_when_tessellation_is_disabled() {
    let scene = ObjDecoder::new().decode(SCRV_ON_SQUARE.as_bytes()).unwrap();
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.name.as_deref() != Some("obj:scrvs")),
        "default decoder must not synthesise scrv meshes"
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
    assert!(keywords.contains(&"scrv"));
}

#[test]
fn scrv_tessellates_into_a_parameter_space_line_strip() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(6)
        .decode(SCRV_ON_SQUARE.as_bytes())
        .unwrap();
    let prim = scrv_prim(&scene);
    assert_eq!(prim.topology, Topology::LineStrip);
    // The curv2 (degree-1, 5-point control list including the closing
    // vertex) tessellates to `samples + 1` = 7 points; the scrv slices
    // the full curve range `[0, 6]`, so the entire polyline survives.
    assert_eq!(prim.positions.len(), 7);

    // Curve walks the unit-square perimeter in `(u, v)` parameter
    // space — first vertex at (0,0), last vertex back at (0,0).
    let p0 = prim.positions[0];
    let p_end = prim.positions[6];
    assert!(p0[0].abs() < 1e-4 && p0[1].abs() < 1e-4, "p0={p0:?}");
    assert!(
        p_end[0].abs() < 1e-4 && p_end[1].abs() < 1e-4,
        "p_end={p_end:?}"
    );
    // The curve is parameter-space — z stays 0.
    assert!(prim.positions.iter().all(|p| p[2].abs() < 1e-6));

    // Provenance extras.
    assert_eq!(
        prim.extras.get("obj:scrv").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        prim.extras
            .get("obj:scrv_segments")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        prim.extras
            .get("obj:tessellated_curve")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    let refs = prim
        .extras
        .get("obj:scrv_curv2_refs")
        .and_then(|v| v.as_array())
        .expect("scrv_curv2_refs present");
    assert_eq!(refs.len(), 1);
    let triple = refs[0].as_array().unwrap();
    assert_eq!(triple[0].as_i64(), Some(1));
    assert!((triple[1].as_f64().unwrap() - 0.0).abs() < 1e-6);
    assert!((triple[2].as_f64().unwrap() - 6.0).abs() < 1e-6);
}

/// Two `scrv` directives + multi-segment scrv. Each scrv directive
/// emits one primitive; the second one chains two `(u0, u1, curv2d)`
/// triples into a single polyline.
const TWO_SCRVS: &str = "\
vp 0.0 0.0
vp 1.0 0.0
vp 1.0 1.0
vp 0.0 1.0
cstype bspline
deg 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0 5.0 6.0
end
vp 0.2 0.2
vp 0.8 0.8
cstype bezier
deg 1
curv2 5 6
parm u 0.0 1.0
end
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
scrv 0.0 6.0 1
scrv 0.0 1.0 2 0.0 6.0 1
end
";

#[test]
fn each_scrv_emits_one_primitive_and_multi_segments_chain() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(6)
        .decode(TWO_SCRVS.as_bytes())
        .unwrap();
    let mesh = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:scrvs"))
        .expect("obj:scrvs mesh exists");
    assert_eq!(mesh.primitives.len(), 2, "two scrv primitives expected");

    // First scrv: single segment over curv2 #1, samples + 1 = 7 verts.
    let first = &mesh.primitives[0];
    assert_eq!(first.positions.len(), 7);
    assert_eq!(
        first
            .extras
            .get("obj:scrv_segments")
            .and_then(|v| v.as_u64()),
        Some(1)
    );

    // Second scrv: two segments. First segment (curv2 #2: diagonal from
    // (0.2,0.2) to (0.8,0.8), 7 verts) + second segment (curv2 #1: the
    // closed square perimeter, 7 verts but the first sample is dropped
    // by `append_curv2_segment` to avoid a duplicate at the join), so
    // total 13 verts.
    let second = &mesh.primitives[1];
    assert_eq!(
        second
            .extras
            .get("obj:scrv_segments")
            .and_then(|v| v.as_u64()),
        Some(2)
    );
    assert_eq!(second.positions.len(), 13);
    // First sample of the second scrv comes from the diagonal start.
    let p0 = second.positions[0];
    assert!((p0[0] - 0.2).abs() < 1e-4 && (p0[1] - 0.2).abs() < 1e-4);
}

#[test]
fn scrv_polyline_filters_out_of_the_encoder_replay() {
    // Decode with the tessellation knob ⇒ scrv mesh exists; re-encode
    // and confirm the synthetic mesh's positions don't pollute the `v`
    // pool but the original `scrv` directive replays verbatim.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(6)
        .decode(SCRV_ON_SQUARE.as_bytes())
        .unwrap();
    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    // Count `v` lines (positions): should only be the 4 source surface
    // control vertices, NOT the 7 synthetic scrv polyline points.
    let v_count = text
        .lines()
        .filter(|l| l.starts_with("v ") && !l.starts_with("vp "))
        .count();
    assert_eq!(v_count, 4, "scrv polyline points must not pollute v pool");
    // The original scrv directive must round-trip verbatim.
    assert!(
        text.contains("scrv 0 6 1") || text.contains("scrv 0.0 6.0 1"),
        "scrv directive missing from re-encoded output: {text}"
    );
}

#[test]
fn scrv_referencing_undefined_curv2_skips_the_segment() {
    const BAD_REF: &str = "\
vp 0.0 0.0
vp 1.0 0.0
vp 1.0 1.0
vp 0.0 1.0
cstype bspline
deg 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0 5.0 6.0
end
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
scrv 0.0 6.0 1 0.0 1.0 99
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(6)
        .decode(BAD_REF.as_bytes())
        .unwrap();
    let mesh = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:scrvs"))
        .expect("obj:scrvs mesh exists");
    assert_eq!(mesh.primitives.len(), 1);
    let prim = &mesh.primitives[0];
    // Only the first (valid) segment contributed — the second was
    // silently dropped.
    assert_eq!(
        prim.extras
            .get("obj:scrv_segments")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
}

#[test]
fn scrv_with_zero_segments_emits_no_primitive() {
    // A bare `scrv` with no triples cannot produce a polyline — the
    // primitive must be omitted (not a zero-length LineStrip).
    const BARE_SCRV: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
scrv
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(6)
        .decode(BARE_SCRV.as_bytes())
        .unwrap();
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.name.as_deref() != Some("obj:scrvs")),
        "empty scrv must not produce an obj:scrvs mesh"
    );
}
