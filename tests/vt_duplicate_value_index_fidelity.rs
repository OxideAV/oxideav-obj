//! Regression: a face that references the *later* of two `vt` lines that
//! share a `[u, v]` value but differ in source index keeps its exact `vt`
//! index on round-trip.
//!
//! The typed model stores only the UV *value* on `Primitive::uvs`, so two
//! source `vt` lines with identical `[u, v]` (e.g. `vt 0` padded to
//! `[0, 0]` and `vt 0 0`) collapse to one value. The encoder's
//! value-keyed `tex_map` previously re-resolved every face UV to the
//! *first* matching slot, silently rewriting a `f v/10` reference to
//! `f v/3`. The decoder now records the original `vt` index per vertex
//! (`Primitive::extras["obj:vt_src_index"]`) whenever the texcoord pool
//! holds a duplicate value, and the encoder restores the exact slot.

use oxideav_obj::obj;

#[test]
fn face_referencing_later_duplicate_vt_keeps_its_index() {
    // vt 1 and vt 3 are both value [0, 0] (the 1-D `vt 0` pads to [0,0]).
    // The mixed widths (1-D `vt 0`, 3-D `vt 5 5 5`) trip the
    // `obj:texcoords` source-pool path; the face references vt 3, the
    // *second* [0,0] slot.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
vt 0
vt 0.5 0.5
vt 0 0
f 1/3 2/2 3/1
";
    let scene = obj::parse_obj(src).unwrap();
    let gen1 = String::from_utf8(obj::serialize_obj(&scene, None).unwrap()).unwrap();

    // The face must still reference vt slot 3, not collapse to slot 1.
    assert!(
        gen1.contains("f 1/3 ")
            || gen1
                .lines()
                .any(|l| l.starts_with("f ") && l.contains("/3 ")),
        "face should retain its vt index 3 (the later [0,0] duplicate):\n{gen1}"
    );

    // And the whole thing is a fixed point.
    let scene2 = obj::parse_obj(&gen1).unwrap();
    let gen2 = String::from_utf8(obj::serialize_obj(&scene2, None).unwrap()).unwrap();
    assert_eq!(gen1, gen2, "decode → encode is a fixed point:\n{gen1}");
}

#[test]
fn vt_src_index_extra_present_only_with_duplicate_values() {
    // Distinct values ⇒ no ambiguity ⇒ no `obj:vt_src_index` channel.
    let distinct = "\
v 0 0 0
v 1 0 0
v 0 1 0
vt 0.1
vt 0.2 0.2
vt 0.3 0.3 0.3
f 1/1 2/2 3/3
";
    let scene = obj::parse_obj(distinct).unwrap();
    let has_channel = scene
        .meshes
        .iter()
        .flat_map(|m| &m.primitives)
        .any(|p| p.extras.contains_key("obj:vt_src_index"));
    assert!(
        !has_channel,
        "distinct-valued vt pool needs no source-index channel"
    );

    // Duplicate value ⇒ channel present.
    let dup = "\
v 0 0 0
v 1 0 0
v 0 1 0
vt 0
vt 0.2 0.2
vt 0 0
f 1/3 2/2 3/1
";
    let scene = obj::parse_obj(dup).unwrap();
    let has_channel = scene
        .meshes
        .iter()
        .flat_map(|m| &m.primitives)
        .any(|p| p.extras.contains_key("obj:vt_src_index"));
    assert!(
        has_channel,
        "duplicate-valued vt pool records the source-index channel"
    );
}
