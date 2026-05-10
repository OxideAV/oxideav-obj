//! `mg <group_number> [res]` merging-group state per Wavefront OBJ
//! spec §"mg group_number res".
//!
//! Like `s`, `mg` is state-setting; the operator's spelling is
//! preserved verbatim in `Primitive::extras["obj:merging_group"]` and
//! a change mid-stream splits the primitive so each one carries a
//! single consistent assignment.

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::{ObjDecoder, obj};

#[test]
fn mg_token_preserved_verbatim_with_res() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
mg 1 0.5
f 1 2 3
";
    let scene = obj::parse_obj(text).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.extras
            .get("obj:merging_group")
            .and_then(|v| v.as_str()),
        Some("1 0.5"),
    );
}

#[test]
fn mg_off_round_trips_as_off() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
mg off
f 1 2 3
";
    let scene1 = obj::parse_obj(text).unwrap();
    let bytes = obj::serialize_obj(&scene1, None).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(s.contains("\nmg off\n"), "expected 'mg off' line in:\n{s}");

    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let prim = &scene2.meshes[0].primitives[0];
    assert_eq!(
        prim.extras
            .get("obj:merging_group")
            .and_then(|v| v.as_str()),
        Some("off"),
    );
}

#[test]
fn mg_change_mid_object_splits_into_two_primitives() {
    let text = "\
o Surf
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 2 0 0
v 2 1 0
mg 1 0.1
f 1 2 3
f 1 3 4
mg 2 0.05
f 3 5 6
";
    let scene = obj::parse_obj(text).unwrap();
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.primitives.len(), 2);
    assert_eq!(
        mesh.primitives[0]
            .extras
            .get("obj:merging_group")
            .and_then(|v| v.as_str()),
        Some("1 0.1"),
    );
    assert_eq!(
        mesh.primitives[1]
            .extras
            .get("obj:merging_group")
            .and_then(|v| v.as_str()),
        Some("2 0.05"),
    );
}
