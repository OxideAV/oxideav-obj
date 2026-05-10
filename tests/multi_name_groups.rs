//! Wavefront OBJ spec §"Grouping" allows multiple group names on
//! one `g` line — `g name1 name2 name3` belongs to all three groups.
//! The parser captures every name and the encoder re-emits them on
//! the same line.

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::{ObjDecoder, obj};

const MULTI_GROUP_OBJ: &str = "\
o Square
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
g panel left wall
f 1 2 3 4
";

#[test]
fn three_names_on_one_g_line_become_three_distinct_groups() {
    let scene = obj::parse_obj(MULTI_GROUP_OBJ).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    let groups = prim
        .extras
        .get("obj:groups")
        .and_then(|v| v.as_array())
        .unwrap();
    let names: Vec<&str> = groups.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names, vec!["panel", "left", "wall"]);
}

#[test]
fn multi_name_group_round_trips_through_encoder() {
    let scene1 = obj::parse_obj(MULTI_GROUP_OBJ).unwrap();
    let bytes = obj::serialize_obj(&scene1, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    // The encoder emits the names on a single `g ` line.
    let g_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("g ")).collect();
    assert_eq!(g_lines.len(), 1);
    let toks: Vec<&str> = g_lines[0].split_whitespace().collect();
    assert_eq!(toks, vec!["g", "panel", "left", "wall"]);

    // Re-decode and re-verify.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let prim2 = &scene2.meshes[0].primitives[0];
    let names2: Vec<&str> = prim2
        .extras
        .get("obj:groups")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(names2, vec!["panel", "left", "wall"]);
}
