//! Face / line / point elements interleaved under one `usemtl` split
//! into one primitive per topology.
//!
//! A single primitive maps to a single `Topology`, so a document that
//! mixes `f` / `l` / `p` elements without an intervening state-setting
//! directive must be decoded by splitting into consecutive per-topology
//! primitives — not rejected. This mirrors the behaviour the `p`
//! handler always had; the `f` / `l` handlers now share the same
//! `push_element` split path.
//!
//! Regression: the serialiser could emit an OBJ whose state directives
//! collapsed to no-ops on re-parse, leaving an `f` element adjacent to
//! an `l` element under one primitive. The old decoder rejected that
//! with `Unsupported("OBJ primitive mixes face / line / point elements
//! under one usemtl")`, so `serialize(parse(x))` was not always
//! re-parseable. Auto-splitting on topology makes the decode succeed and
//! the round-trip a fixed point.

use oxideav_obj::obj;

fn ser(text: &str) -> String {
    let scene = obj::parse_obj(text).expect("parses");
    String::from_utf8(obj::serialize_obj(&scene, None).expect("serialises")).expect("utf8")
}

#[test]
fn face_then_line_then_point_split_without_state_change() {
    // No usemtl / group / smoothing directive separates the three
    // element kinds — the split must happen purely on topology.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
f 1 2 3
l 1 2
p 4
";
    let scene = obj::parse_obj(src).expect("mixed-topology document decodes");
    let mesh = &scene.meshes[0];
    assert_eq!(
        mesh.primitives.len(),
        3,
        "one primitive per topology (face / line / point), got {}",
        mesh.primitives.len()
    );
}

#[test]
fn line_then_face_does_not_reject() {
    // The specific ordering that previously tripped the mix error.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
l 1 2
f 1 2 3
";
    let scene = obj::parse_obj(src).expect("line-then-face decodes rather than erroring");
    assert_eq!(scene.meshes[0].primitives.len(), 2);
}

#[test]
fn mixed_topology_round_trip_is_a_fixed_point() {
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
usemtl mat0
f 1 2 3
l 2 4
p 4
f 1 4 3
";
    let once = ser(src);
    let twice = ser(&once);
    assert_eq!(
        once, twice,
        "a document mixing element kinds must re-serialise identically"
    );
    // And the encoder's own output must always re-parse cleanly.
    obj::parse_obj(&once).expect("serialised mixed-topology OBJ re-parses");
}

#[test]
fn point_split_still_inherits_state() {
    // The `p` handler's original state-inheritance behaviour is
    // preserved through the shared helper: the point primitive keeps the
    // active material / smoothing group.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
usemtl steel
s 1
f 1 2 3
p 2
";
    let scene = obj::parse_obj(src).expect("parses");
    let prims = &scene.meshes[0].primitives;
    assert_eq!(prims.len(), 2, "face and point split into two primitives");
    // Both primitives carry the same material id (inherited state).
    assert_eq!(
        prims[0].material, prims[1].material,
        "the point primitive inherits the active material"
    );
}
