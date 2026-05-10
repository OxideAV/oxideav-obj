//! Two-material OBJ → one Mesh containing two Primitives, each with
//! its own MaterialId.

use oxideav_obj::obj;

const TWO_MAT_OBJ: &str = "\
mtllib teapot.mtl
o Teapot
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 0 0 1
v 1 0 1
v 1 1 1
v 0 1 1
usemtl Body
f 1 2 3
f 1 3 4
usemtl Spout
f 5 6 7
f 5 7 8
";

const TEAPOT_MTL: &str = "\
newmtl Body
Kd 0.8 0.4 0.2

newmtl Spout
Kd 0.2 0.4 0.8
";

#[test]
fn two_usemtl_switches_yield_two_primitives_each_pointing_at_their_own_material() {
    let scene =
        obj::parse_obj_with_resolver(TWO_MAT_OBJ, |_| Ok(TEAPOT_MTL.as_bytes().to_vec())).unwrap();

    assert_eq!(scene.meshes.len(), 1);
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("Teapot"));
    assert_eq!(mesh.primitives.len(), 2);
    assert_eq!(scene.materials.len(), 2);

    // Both primitives have a material binding (which one binds to which
    // depends on the alphabetical ordering used in build_scene; just
    // assert the two are distinct).
    let m0 = mesh.primitives[0].material.expect("p0 has a material");
    let m1 = mesh.primitives[1].material.expect("p1 has a material");
    assert_ne!(m0, m1);

    // Both `obj:usemtl` extras point at distinct names too.
    let n0 = mesh.primitives[0]
        .extras
        .get("obj:usemtl")
        .and_then(|v| v.as_str())
        .unwrap();
    let n1 = mesh.primitives[1]
        .extras
        .get("obj:usemtl")
        .and_then(|v| v.as_str())
        .unwrap();
    let mut names = [n0, n1];
    names.sort();
    assert_eq!(names, ["Body", "Spout"]);
}
