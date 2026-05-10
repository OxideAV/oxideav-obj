//! Display-attribute state per Wavefront OBJ spec §"Polygonal display
//! attributes": `bevel on/off`, `c_interp on/off`, `d_interp on/off`,
//! and `lod <level>`.
//!
//! Each attribute is captured per-primitive in
//! `Primitive::extras["obj:<keyword>"]`; mid-stream changes split the
//! primitive so each one carries one consistent value (mirrors the
//! `s` smoothing-group / `mg` merging-group state-setting).

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::{ObjDecoder, obj};

const DISPLAY_OBJ: &str = "\
o Box
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
bevel on
c_interp on
d_interp off
lod 5
f 1 2 3
f 1 3 4
";

#[test]
fn all_four_display_attrs_land_in_extras() {
    let scene = obj::parse_obj(DISPLAY_OBJ).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.extras.get("obj:bevel").and_then(|v| v.as_str()),
        Some("on"),
    );
    assert_eq!(
        prim.extras.get("obj:c_interp").and_then(|v| v.as_str()),
        Some("on"),
    );
    assert_eq!(
        prim.extras.get("obj:d_interp").and_then(|v| v.as_str()),
        Some("off"),
    );
    assert_eq!(
        prim.extras.get("obj:lod").and_then(|v| v.as_str()),
        Some("5"),
    );
}

#[test]
fn display_attrs_round_trip_through_encoder() {
    let scene1 = obj::parse_obj(DISPLAY_OBJ).unwrap();
    let bytes = obj::serialize_obj(&scene1, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("\nbevel on\n"), "missing bevel in:\n{text}");
    assert!(
        text.contains("\nc_interp on\n"),
        "missing c_interp in:\n{text}",
    );
    assert!(
        text.contains("\nd_interp off\n"),
        "missing d_interp in:\n{text}",
    );
    assert!(text.contains("\nlod 5\n"), "missing lod in:\n{text}");

    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let prim = &scene2.meshes[0].primitives[0];
    assert_eq!(
        prim.extras.get("obj:bevel").and_then(|v| v.as_str()),
        Some("on"),
    );
    assert_eq!(
        prim.extras.get("obj:lod").and_then(|v| v.as_str()),
        Some("5"),
    );
}

#[test]
fn lod_change_mid_object_splits_primitives_and_keeps_other_state() {
    // The previous primitive's smoothing-group token must propagate
    // into the post-split primitive; only the changed display attr
    // gets the new value.
    let text = "\
o Detail
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 2 0 0
v 2 1 0
s 1
bevel on
lod 1
f 1 2 3
lod 2
f 1 3 4
f 4 5 6
";
    let scene = obj::parse_obj(text).unwrap();
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.primitives.len(), 2);
    // Smoothing + bevel propagate to the second primitive too.
    for prim in &mesh.primitives {
        assert_eq!(
            prim.extras
                .get("obj:smoothing_group")
                .and_then(|v| v.as_str()),
            Some("1"),
        );
        assert_eq!(
            prim.extras.get("obj:bevel").and_then(|v| v.as_str()),
            Some("on"),
        );
    }
    assert_eq!(
        mesh.primitives[0]
            .extras
            .get("obj:lod")
            .and_then(|v| v.as_str()),
        Some("1"),
    );
    assert_eq!(
        mesh.primitives[1]
            .extras
            .get("obj:lod")
            .and_then(|v| v.as_str()),
        Some("2"),
    );
}
