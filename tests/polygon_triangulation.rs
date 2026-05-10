//! N-gon faces fan-triangulate on read; original arities are preserved
//! in `Primitive::extras["obj:original_face_arities"]` and the
//! encoder rebuilds the n-gons on output.

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::{ObjDecoder, obj};

const PENTAGON_OBJ: &str = "\
o Pent
v 0 0 0
v 1 0 0
v 1 1 0
v 0.5 1.5 0
v 0 1 0
f 1 2 3 4 5
";

#[test]
fn pentagon_fans_into_three_triangles_with_arity_5_recorded() {
    let scene = obj::parse_obj(PENTAGON_OBJ).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.triangle_count(), 3);
    let arities = prim
        .extras
        .get("obj:original_face_arities")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(arities.len(), 1);
    assert_eq!(arities[0].as_u64(), Some(5));
}

#[test]
fn arity_round_trip_re_emits_the_pentagon_as_one_face() {
    let scene1 = obj::parse_obj(PENTAGON_OBJ).unwrap();
    let bytes = obj::serialize_obj(&scene1, None).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    // Exactly one `f ` line should land in the output, with five vertex tokens.
    let face_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("f ")).collect();
    assert_eq!(face_lines.len(), 1);
    let tokens: Vec<&str> = face_lines[0].split_whitespace().collect();
    // "f" + 5 vertex tokens.
    assert_eq!(tokens.len(), 6);

    // Re-decode and assert the round-tripped scene also reports a 5-arity face.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let prim2 = &scene2.meshes[0].primitives[0];
    assert_eq!(prim2.triangle_count(), 3);
    let arities2 = prim2
        .extras
        .get("obj:original_face_arities")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(arities2[0].as_u64(), Some(5));
}

#[test]
fn quad_fans_into_two_triangles() {
    let obj_text = "\
o Q
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
f 1 2 3 4
";
    let scene = obj::parse_obj(obj_text).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.triangle_count(), 2);
}
