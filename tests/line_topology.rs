//! `Topology::LineStrip` / `Topology::LineLoop` promotion for single-`l`
//! polylines.
//!
//! Round 4 lowered every `l` element to `Topology::Lines` and decomposed
//! the polyline into pairwise segments, leaning on the encoder to rejoin
//! contiguous chains on the way out. Round 5 promotes a single-`l`
//! primitive to the more specific topology so consumers that hand the
//! scene to a renderer get the natural strip / loop semantics:
//!
//!   l v1 v2 v3 v4         →  Topology::LineStrip  ([v1,v2,v3,v4])
//!   l v1 v2 v3 v4 v1      →  Topology::LineLoop   ([v1,v2,v3,v4])
//!   l v1 v2               →  Topology::Lines      ([v1,v2])  (plain segment)
//!   l v1 v2 v3
//!   l v4 v5 v6            →  Topology::Lines      (multiple `l` elements)

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder, obj};

#[test]
fn single_l_element_promotes_to_line_strip() {
    let text = "\
v 0 0 0
v 1 0 0
v 2 0 0
v 3 0 0
l 1 2 3 4
";
    let scene = obj::parse_obj(text).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.topology, Topology::LineStrip);
    assert_eq!(prim.indices.as_ref().unwrap().len(), 4);

    // Encoder re-emits a single `l` line carrying the full strip.
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let l_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("l ")).collect();
    assert_eq!(l_lines.len(), 1);
    assert_eq!(
        l_lines[0].split_whitespace().collect::<Vec<_>>(),
        vec!["l", "1", "2", "3", "4"]
    );
}

#[test]
fn closed_polyline_promotes_to_line_loop() {
    // First and last vertex coincide → LineLoop, with the redundant
    // closing index dropped from the buffer.
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
l 1 2 3 4 1
";
    let scene = obj::parse_obj(text).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.topology, Topology::LineLoop);
    assert_eq!(prim.indices.as_ref().unwrap().len(), 4); // closing vertex stripped

    // Encoder emits the closing edge so the round-trip parser sees
    // first==last and re-detects the loop.
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let l_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("l ")).collect();
    assert_eq!(l_lines.len(), 1);
    assert_eq!(
        l_lines[0].split_whitespace().collect::<Vec<_>>(),
        vec!["l", "1", "2", "3", "4", "1"]
    );

    // Re-decode and confirm topology survives.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let prim2 = &scene2.meshes[0].primitives[0];
    assert_eq!(prim2.topology, Topology::LineLoop);
}

#[test]
fn two_vertex_l_stays_as_lines() {
    // A 2-vertex `l` is a plain segment — nothing to promote.
    let text = "\
v 0 0 0
v 1 0 0
l 1 2
";
    let scene = obj::parse_obj(text).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.topology, Topology::Lines);
    assert_eq!(prim.indices.as_ref().unwrap().len(), 2);
}

#[test]
fn multiple_l_lines_stay_as_lines() {
    // Two separate `l` elements aren't representable as a single strip
    // — fall back to `Lines` so the encoder rejoins each contiguous
    // chain independently.
    let text = "\
v 0 0 0
v 1 0 0
v 2 0 0
v 5 0 0
v 6 0 0
l 1 2 3
l 4 5
";
    let scene = obj::parse_obj(text).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.topology, Topology::Lines);
    // 2 segments from polyline 1 + 1 segment from polyline 2 = 3 pairs.
    assert_eq!(prim.indices.as_ref().unwrap().len(), 6);
}

#[test]
fn line_strip_round_trip_via_encoder_api() {
    // Build a Scene3D directly with a `LineStrip` primitive and ensure
    // it serialises to a single `l` line covering all 4 vertices.
    use oxideav_mesh3d::{Indices, Mesh, Primitive, Scene3D};
    let mut scene = Scene3D::new();
    let mut prim = Primitive::new(Topology::LineStrip);
    prim.positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    prim.indices = Some(Indices::U16(vec![0, 1, 2, 3]));
    let mut mesh = Mesh::new(None);
    mesh.primitives.push(prim);
    scene.add_mesh(mesh);

    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let l_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("l ")).collect();
    assert_eq!(l_lines.len(), 1);
    assert_eq!(
        l_lines[0].split_whitespace().collect::<Vec<_>>(),
        vec!["l", "1", "2", "3", "4"]
    );
}

#[test]
fn line_loop_round_trip_via_encoder_api() {
    // Symmetric: a `LineLoop` primitive emits an `l` line that closes
    // back to the first vertex.
    use oxideav_mesh3d::{Indices, Mesh, Primitive, Scene3D};
    let mut scene = Scene3D::new();
    let mut prim = Primitive::new(Topology::LineLoop);
    prim.positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]];
    prim.indices = Some(Indices::U16(vec![0, 1, 2]));
    let mut mesh = Mesh::new(None);
    mesh.primitives.push(prim);
    scene.add_mesh(mesh);

    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let l_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("l ")).collect();
    assert_eq!(l_lines.len(), 1);
    assert_eq!(
        l_lines[0].split_whitespace().collect::<Vec<_>>(),
        vec!["l", "1", "2", "3", "1"]
    );
}
