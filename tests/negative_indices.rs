//! `f -1 -2 -3` resolves the relative-from-end indices to the real
//! vertex pool.

use oxideav_obj::obj;

#[test]
fn face_with_all_negative_indices_resolves_to_last_three_vertices() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 0.5 0.5 0
f -1 -2 -3
";
    let scene = obj::parse_obj(text).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.triangle_count(), 1);
    // The face references vertices 5, 4, 3 → positions (0.5,0.5,0),
    // (0,1,0), (1,1,0). FaceVertex dedup keys on (v, vt, vn) so we
    // get exactly 3 positions in the primitive buffer.
    assert_eq!(prim.positions.len(), 3);
    let p0 = prim.positions[0];
    let p1 = prim.positions[1];
    let p2 = prim.positions[2];
    assert_eq!(p0, [0.5, 0.5, 0.0]);
    assert_eq!(p1, [0.0, 1.0, 0.0]);
    assert_eq!(p2, [1.0, 1.0, 0.0]);
}

#[test]
fn negative_index_out_of_range_errors_cleanly() {
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
f -1 -2 -99
";
    let err = obj::parse_obj(text).unwrap_err();
    let s = format!("{err}");
    assert!(s.contains("position"));
}

#[test]
fn mixed_positive_and_negative_indices_resolve_to_the_same_face() {
    let text_pos = "\
v 0 0 0
v 1 0 0
v 1 1 0
f 1 2 3
";
    let text_neg = "\
v 0 0 0
v 1 0 0
v 1 1 0
f -3 -2 -1
";
    let s_pos = obj::parse_obj(text_pos).unwrap();
    let s_neg = obj::parse_obj(text_neg).unwrap();
    assert_eq!(
        s_pos.meshes[0].primitives[0].positions,
        s_neg.meshes[0].primitives[0].positions
    );
}
