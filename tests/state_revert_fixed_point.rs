//! Transient state directives must not break the round-trip fixed point.
//!
//! A state-setting directive (`s`, `usemtl`, `bevel`, `usemap`, …) that
//! binds no element of its own, followed by a directive that reverts the
//! state, leaves two adjacent primitives whose resolved state matches.
//! The encoder drops empty primitives and emits one state block per
//! surviving primitive, so a naive re-parse would collapse the pair and
//! `serialize(parse(x))` would not be a fixed point. The decoder now
//! coalesces adjacent same-state / same-topology primitives (and drops
//! empty ones), so its output is already normalised.

use oxideav_obj::obj;

fn ser(text: &str) -> String {
    let scene = obj::parse_obj(text).expect("parses");
    String::from_utf8(obj::serialize_obj(&scene, None).expect("serialises")).expect("utf8")
}

fn assert_fixed_point(name: &str, src: &str) {
    let once = ser(src);
    let twice = ser(&once);
    assert_eq!(
        once, twice,
        "{name}: not a fixed point\n--- once ---\n{once}\n--- twice ---\n{twice}"
    );
    obj::parse_obj(&once)
        .unwrap_or_else(|e| panic!("{name}: serialised output must re-parse: {e:?}"));
}

#[test]
fn smoothing_group_revert_is_a_fixed_point() {
    // `s off` then `s 1` reverts the smoothing group with no face in
    // between; the two `s 1` face groups must coalesce into one primitive.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
s 1
f 1 2 3
s off
s 1
f 1 2 3
";
    let scene = obj::parse_obj(src).expect("parses");
    assert_eq!(
        scene.meshes[0].primitives.len(),
        1,
        "the reverted smoothing group must leave a single coalesced primitive"
    );
    assert_fixed_point("smoothing-revert", src);
}

#[test]
fn usemtl_revert_is_a_fixed_point() {
    // `usemtl m2` then `usemtl m0` leaves an empty `m2` primitive wedged
    // between two `m0` primitives; dropping the empty one and coalescing
    // the survivors keeps the round-trip stable.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
usemtl m0
f 1 2 3
usemtl m2
usemtl m0
f 1 2 3
";
    let scene = obj::parse_obj(src).expect("parses");
    assert_eq!(
        scene.meshes[0].primitives.len(),
        1,
        "the reverted material must leave a single coalesced primitive"
    );
    assert_fixed_point("usemtl-revert", src);
}

#[test]
fn bevel_revert_is_a_fixed_point() {
    assert_fixed_point(
        "bevel-revert",
        "\
v 0 0 0
v 1 0 0
v 0 1 0
bevel on
f 1 2 3
bevel off
bevel on
f 1 2 3
",
    );
}

#[test]
fn distinct_states_still_split() {
    // A real, geometry-bearing state change on both sides must NOT be
    // coalesced away.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
usemtl m0
f 1 2 3
usemtl m1
f 1 2 3
";
    let scene = obj::parse_obj(src).expect("parses");
    assert_eq!(
        scene.meshes[0].primitives.len(),
        2,
        "distinct materials with faces on both sides stay separate primitives"
    );
    assert_fixed_point("distinct-materials", src);
}

#[test]
fn non_adjacent_same_state_not_merged() {
    // m0, m1, m0 — the two m0 groups are not adjacent (m1 between), so
    // they must remain three primitives.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
usemtl m0
f 1 2 3
usemtl m1
f 1 2 3
usemtl m0
f 1 2 3
";
    let scene = obj::parse_obj(src).expect("parses");
    assert_eq!(scene.meshes[0].primitives.len(), 3);
    assert_fixed_point("m0-m1-m0", src);
}
