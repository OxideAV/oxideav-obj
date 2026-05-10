//! `s 1` / `s off` / `s 0` smoothing-group state-setting per
//! Wavefront OBJ spec §"Grouping".
//!
//! Smoothing is state-setting: a change mid-stream splits the
//! primitive so each [`oxideav_mesh3d::Primitive`] carries one
//! consistent smoothing-group assignment in its
//! `extras["obj:smoothing_group"]`.

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::{ObjDecoder, obj};

#[test]
fn smoothing_token_is_preserved_verbatim() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
s 1
f 1 2 3
";
    let scene = obj::parse_obj(text).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.extras
            .get("obj:smoothing_group")
            .and_then(|v| v.as_str()),
        Some("1"),
    );
}

#[test]
fn s_off_round_trips_as_off() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
s off
f 1 2 3
";
    let scene1 = obj::parse_obj(text).unwrap();
    let bytes = obj::serialize_obj(&scene1, None).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    // Spec allows `0` and `off` interchangeably; we preserve the
    // operator's chosen spelling.
    assert!(s.contains("\ns off\n"), "expected 's off' line in:\n{s}");

    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let prim = &scene2.meshes[0].primitives[0];
    assert_eq!(
        prim.extras
            .get("obj:smoothing_group")
            .and_then(|v| v.as_str()),
        Some("off"),
    );
}

#[test]
fn smoothing_change_mid_object_splits_into_two_primitives() {
    let text = "\
o Cube
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 0 0 1
v 1 0 1
v 1 1 1
v 0 1 1
s 1
f 1 2 3
f 1 3 4
s 2
f 5 6 7
f 5 7 8
";
    let scene = obj::parse_obj(text).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    let mesh = &scene.meshes[0];
    // Two distinct smoothing-group regions ⇒ two primitives.
    assert_eq!(mesh.primitives.len(), 2);
    assert_eq!(
        mesh.primitives[0]
            .extras
            .get("obj:smoothing_group")
            .and_then(|v| v.as_str()),
        Some("1"),
    );
    assert_eq!(
        mesh.primitives[1]
            .extras
            .get("obj:smoothing_group")
            .and_then(|v| v.as_str()),
        Some("2"),
    );
}
