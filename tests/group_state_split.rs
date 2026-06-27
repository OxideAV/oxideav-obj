//! Regression: a `g` directive is state-setting (spec §"Grouping") — a
//! mid-stream group change splits the primitive so the new membership
//! applies only to *subsequent* elements, not the ones already
//! accumulated.
//!
//! The decoder previously appended the new group names onto the current
//! primitive even when it already held faces, retroactively re-tagging
//! those faces. On round-trip the encoder then emitted the `g` line one
//! element too early, so decode → encode wasn't a fixed point.

use oxideav_obj::obj;

#[test]
fn trailing_group_only_tags_following_elements() {
    // The `g grpA grpB` sits *after* the first face — it must apply only
    // to `f 2 3 4`, not to `f 1 2 3`.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
f 1 2 3
g grpA grpB
f 2 3 4
";
    let scene = obj::parse_obj(src).unwrap();
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.primitives.len(), 2, "the `g` change splits the mesh");

    // First primitive carries no group; second carries [grpA, grpB].
    assert!(
        !mesh.primitives[0].extras.contains_key("obj:groups"),
        "the first face predates the `g` line — no group"
    );
    let groups = mesh.primitives[1]
        .extras
        .get("obj:groups")
        .and_then(|v| v.as_array())
        .unwrap();
    let names: Vec<&str> = groups.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(names, vec!["grpA", "grpB"]);

    // And decode → encode is a fixed point.
    let gen1 = String::from_utf8(obj::serialize_obj(&scene, None).unwrap()).unwrap();
    let scene2 = obj::parse_obj(&gen1).unwrap();
    let gen2 = String::from_utf8(obj::serialize_obj(&scene2, None).unwrap()).unwrap();
    assert_eq!(gen1, gen2, "g state-set is a fixed point:\n{gen1}");

    // The `g` line must sit between the two faces, not before the first.
    let f1_pos = gen1.find("f 1 2 3").unwrap();
    let g_pos = gen1.find("g grpA grpB").unwrap();
    assert!(
        g_pos > f1_pos,
        "the `g` line must follow the first face, not precede it:\n{gen1}"
    );
}

#[test]
fn group_set_before_any_element_tags_the_first_primitive() {
    // A `g` that precedes the first element applies to it (no split).
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
g grpA
f 1 2 3
";
    let scene = obj::parse_obj(src).unwrap();
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.primitives.len(), 1);
    let groups = mesh.primitives[0]
        .extras
        .get("obj:groups")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(groups[0].as_str().unwrap(), "grpA");
}

#[test]
fn changing_group_membership_mid_stream_splits_each_time() {
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
g a
f 1 2 3
g b
f 2 3 4
g a b
f 1 3 4
";
    let scene = obj::parse_obj(src).unwrap();
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.primitives.len(), 3, "each `g` change is its own prim");

    let membership = |i: usize| -> Vec<String> {
        mesh.primitives[i]
            .extras
            .get("obj:groups")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    };
    assert_eq!(membership(0), vec!["a"]);
    assert_eq!(membership(1), vec!["b"]);
    assert_eq!(membership(2), vec!["a", "b"]);
}
