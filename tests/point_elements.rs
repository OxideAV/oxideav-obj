//! `p v1 v2 v3 …` point elements per Wavefront OBJ spec §"Polygonal
//! geometry statement (p)".
//!
//! Point elements map to [`oxideav_mesh3d::Topology::Points`]. They are
//! state-incompatible with face/line elements and split into a fresh
//! primitive whenever they appear next to incompatible geometry under
//! the same `usemtl` (mirroring the multi-element handling already
//! used between `f` and `l`).

use oxideav_mesh3d::{Mesh3DDecoder, Topology};
use oxideav_obj::{ObjDecoder, obj};

const POINTS_OBJ: &str = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
p 1 2 3 4
";

#[test]
fn p_directive_creates_points_primitive() {
    let scene = obj::parse_obj(POINTS_OBJ).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.topology, Topology::Points);
    // Four point references → four positions interned + four indices.
    assert_eq!(prim.positions.len(), 4);
    assert_eq!(prim.indices.as_ref().unwrap().len(), 4);
}

#[test]
fn multi_point_line_packs_back_onto_one_p_line() {
    let scene1 = obj::parse_obj(POINTS_OBJ).unwrap();
    let bytes = obj::serialize_obj(&scene1, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let p_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("p ")).collect();
    assert_eq!(p_lines.len(), 1, "expected one packed `p` line in:\n{text}");
    let toks: Vec<&str> = p_lines[0].split_whitespace().collect();
    assert_eq!(toks, vec!["p", "1", "2", "3", "4"]);

    // Round-trip via the decoder.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let prim2 = &scene2.meshes[0].primitives[0];
    assert_eq!(prim2.topology, Topology::Points);
    assert_eq!(prim2.indices.as_ref().unwrap().len(), 4);
}

#[test]
fn faces_then_points_split_into_two_primitives() {
    // Mixing face and point elements under one usemtl forces a split
    // because they map to different `Topology` variants.
    let text = "\
o Mixed
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 2 0 0
f 1 2 3
p 4 5
";
    let scene = obj::parse_obj(text).unwrap();
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.primitives.len(), 2);
    assert_eq!(mesh.primitives[0].topology, Topology::Triangles);
    assert_eq!(mesh.primitives[1].topology, Topology::Points);
}
