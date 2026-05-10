//! Encoder reconstructs contiguous polyline strips from
//! `Topology::Lines` segment pairs.
//!
//! The decoder splits a polyline `l v1 v2 v3 v4` into three
//! consecutive segments stored back-to-back in the index buffer
//! `[v1,v2, v2,v3, v3,v4]`. Round 3 teaches the encoder to detect
//! that contiguity and re-emit a single `l v1 v2 v3 v4` line rather
//! than three pairwise `l` lines.

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::{ObjDecoder, obj};

#[test]
fn four_vertex_polyline_round_trips_as_one_l_line() {
    let text = "\
v 0 0 0
v 1 0 0
v 2 0 0
v 3 0 0
l 1 2 3 4
";
    let scene1 = obj::parse_obj(text).unwrap();
    let bytes = obj::serialize_obj(&scene1, None).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let l_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("l ")).collect();
    assert_eq!(
        l_lines.len(),
        1,
        "expected one joined polyline, got {l_lines:?} in:\n{s}"
    );
    let toks: Vec<&str> = l_lines[0].split_whitespace().collect();
    assert_eq!(toks, vec!["l", "1", "2", "3", "4"]);

    // Re-decode and verify the topology promoted to `LineStrip`
    // (round-5 behaviour) — a single `l` element with 4 distinct
    // vertices is the prototypical strip case. Index buffer is the
    // raw vertex sequence (no segment-pair decomposition).
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let prim2 = &scene2.meshes[0].primitives[0];
    assert_eq!(prim2.topology, oxideav_mesh3d::Topology::LineStrip);
    assert_eq!(prim2.indices.as_ref().unwrap().len(), 4);
}

#[test]
fn two_disjoint_polylines_emit_as_two_l_lines() {
    let text = "\
v 0 0 0
v 1 0 0
v 2 0 0
v 5 0 0
v 6 0 0
l 1 2 3
l 4 5
";
    let scene1 = obj::parse_obj(text).unwrap();
    let bytes = obj::serialize_obj(&scene1, None).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let l_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("l ")).collect();
    // Polyline 1 has three vertices ⇒ two segments [(1,2),(2,3)] joined.
    // Polyline 2 has two vertices ⇒ one segment.
    // Encoder must emit them as two separate `l` lines because they
    // share no endpoint.
    assert_eq!(l_lines.len(), 2, "expected two `l` lines in:\n{s}");
}

#[test]
fn three_segment_polyline_with_a_branch_keeps_them_separate() {
    // Indices laid out as: [a,b, b,c, d,e] — first two share an
    // endpoint and join into `a b c`; the third stands alone.
    let text = "\
v 0 0 0
v 1 0 0
v 2 0 0
v 5 5 0
v 6 5 0
l 1 2
l 2 3
l 4 5
";
    let scene1 = obj::parse_obj(text).unwrap();
    let bytes = obj::serialize_obj(&scene1, None).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let l_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("l ")).collect();
    assert_eq!(l_lines.len(), 2, "expected two `l` lines in:\n{s}");
    let toks0: Vec<&str> = l_lines[0].split_whitespace().collect();
    assert_eq!(toks0, vec!["l", "1", "2", "3"]);
    let toks1: Vec<&str> = l_lines[1].split_whitespace().collect();
    assert_eq!(toks1, vec!["l", "4", "5"]);
}
