//! `ObjEncoder::with_negative_indices(true)` emits face/line vertex
//! indices in the relative-from-end form. The output round-trips to
//! the same Scene3D as positive-index emission.

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder};
use oxideav_obj::{ObjDecoder, ObjEncoder, obj};

const TRIANGLE_OBJ: &str = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 2 0 0
v 2 1 0
v 3 1 0
f 1 2 3
f 4 5 6
";

#[test]
fn negative_index_encoder_emits_relative_indices() {
    let scene = obj::parse_obj(TRIANGLE_OBJ).unwrap();
    let bytes = ObjEncoder::new()
        .with_negative_indices(true)
        .encode(&scene)
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let face_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("f ")).collect();
    assert_eq!(face_lines.len(), 2);
    // Every face token (other than `f`) must be a negative integer.
    for line in &face_lines {
        for tok in line.split_whitespace().skip(1) {
            // Strip /vt/vn for triple form (this fixture has only `v`).
            let v_tok = tok.split('/').next().unwrap();
            let n: i64 = v_tok.parse().unwrap_or(0);
            assert!(
                n < 0,
                "expected negative index in line {line:?}, got {tok:?}"
            );
        }
    }
}

#[test]
fn negative_and_positive_outputs_decode_to_the_same_scene() {
    let scene_in = obj::parse_obj(TRIANGLE_OBJ).unwrap();

    let pos_bytes = ObjEncoder::new().encode(&scene_in).unwrap();
    let neg_bytes = ObjEncoder::new()
        .with_negative_indices(true)
        .encode(&scene_in)
        .unwrap();

    let scene_pos = ObjDecoder::new().decode(&pos_bytes).unwrap();
    let scene_neg = ObjDecoder::new().decode(&neg_bytes).unwrap();

    // Same number of meshes, primitives, positions, and triangles.
    assert_eq!(scene_pos.meshes.len(), scene_neg.meshes.len());
    let p_pos = &scene_pos.meshes[0].primitives[0];
    let p_neg = &scene_neg.meshes[0].primitives[0];
    assert_eq!(p_pos.positions, p_neg.positions);
    assert_eq!(p_pos.triangle_count(), p_neg.triangle_count());
}

#[test]
fn negative_index_with_textures_and_normals_is_well_formed() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
vt 0 0
vt 1 0
vt 1 1
vn 0 0 1
f 1/1/1 2/2/1 3/3/1
";
    let scene = obj::parse_obj(text).unwrap();
    let bytes = ObjEncoder::new()
        .with_negative_indices(true)
        .encode(&scene)
        .unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let face_line = s
        .lines()
        .find(|l| l.starts_with("f "))
        .expect("face line in output");
    // Each `v/vt/vn` triple should have all-negative components.
    for tok in face_line.split_whitespace().skip(1) {
        let parts: Vec<&str> = tok.split('/').collect();
        assert_eq!(parts.len(), 3, "expected v/vt/vn triple in {tok:?}");
        for p in &parts {
            let n: i64 = p.parse().unwrap();
            assert!(n < 0, "expected negative index in {tok:?}");
        }
    }
    // And the round-trip back through the decoder yields the same scene shape.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let p1 = &scene.meshes[0].primitives[0];
    let p2 = &scene2.meshes[0].primitives[0];
    assert_eq!(p1.positions, p2.positions);
    assert_eq!(p1.uvs[0], p2.uvs[0]);
    assert_eq!(p1.normals, p2.normals);
}
