//! `map_aat on` per-material texture anti-aliasing toggle per MTL
//! spec §"map_aat on".
//!
//! The spec documents `map_aat on` as a per-material switch that
//! enables texture anti-aliasing for that material without affecting
//! the rest of the scene. We surface it as a boolean
//! `Material::extras["mtl:map_aat"]` and round-trip the exact
//! `on` / `off` token.

use oxideav_obj::mtl;

fn aat(m: &oxideav_mesh3d::Material) -> Option<bool> {
    m.extras.get("mtl:map_aat").and_then(|v| v.as_bool())
}

#[test]
fn map_aat_on_records_true_extra() {
    let mats = mtl::parse_mtl("newmtl Chrome\nKd 0.6 0.6 0.6\nmap_aat on\n").unwrap();
    assert_eq!(aat(&mats[0]), Some(true));
}

#[test]
fn map_aat_off_records_false_extra() {
    let mats = mtl::parse_mtl("newmtl Matte\nKd 0.6 0.6 0.6\nmap_aat off\n").unwrap();
    assert_eq!(aat(&mats[0]), Some(false));
}

#[test]
fn absent_map_aat_leaves_no_extra() {
    let mats = mtl::parse_mtl("newmtl Plain\nKd 1 1 1\n").unwrap();
    assert_eq!(aat(&mats[0]), None);
}

#[test]
fn malformed_map_aat_dropped_without_failing_parse() {
    // Neither `on` nor `off` (and the no-argument form) drop the line
    // silently rather than failing the parse.
    let mats = mtl::parse_mtl("newmtl A\nKd 1 1 1\nmap_aat maybe\nmap_aat\n").unwrap();
    assert_eq!(aat(&mats[0]), None);
}

#[test]
fn map_aat_on_round_trips_through_serializer() {
    let mats = mtl::parse_mtl("newmtl Chrome\nKd 1 1 1\nmap_aat on\n").unwrap();
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("map_aat on\n"),
        "expected `map_aat on` line in:\n{text}",
    );

    // Re-decode and re-verify the flag survives.
    let mats2 = mtl::parse_mtl(text).unwrap();
    assert_eq!(aat(&mats2[0]), Some(true));
}

#[test]
fn map_aat_off_round_trips_through_serializer() {
    let mats = mtl::parse_mtl("newmtl Matte\nKd 1 1 1\nmap_aat off\n").unwrap();
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("map_aat off\n"),
        "expected `map_aat off` line in:\n{text}",
    );
    let mats2 = mtl::parse_mtl(text).unwrap();
    assert_eq!(aat(&mats2[0]), Some(false));
}
