//! Extra MTL parameters round-trip through the parser/encoder:
//! `Tf` transmission filter, `sharpness`, `bump`/`map_bump` aliases,
//! and `disp` displacement map.

use oxideav_obj::mtl;

const EXTRAS_MTL: &str = "\
newmtl FancyGlass
Kd 0.8 0.9 1.0
Tf 0.9 0.95 1.0
sharpness 75
Ns 200
Ni 1.45
illum 4
bump bumpmap.png
disp dispmap.png
";

#[test]
fn tf_sharpness_bump_disp_round_trip() {
    let mats = mtl::parse_mtl(EXTRAS_MTL).unwrap();
    assert_eq!(mats.len(), 1);
    let m = &mats[0];

    // Tf preserved as a 3-tuple in extras.
    let tf = m.extras.get("mtl:Tf").and_then(|v| v.as_array()).unwrap();
    assert_eq!(tf.len(), 3);
    assert!((tf[0].as_f64().unwrap() as f32 - 0.9).abs() < 1e-6);
    assert!((tf[2].as_f64().unwrap() as f32 - 1.0).abs() < 1e-6);

    // sharpness preserved as a scalar in extras.
    let sh = m
        .extras
        .get("mtl:sharpness")
        .and_then(|v| v.as_f64())
        .unwrap();
    assert!((sh as f32 - 75.0).abs() < 1e-6);

    // `bump` (no map_ prefix) lands in the normal-texture pending slot.
    let pending = m.extras.get("mtl:pending_textures").unwrap();
    assert_eq!(pending["normal"].as_str(), Some("bumpmap.png"));

    // `disp` lands in extras as a string (no PBR slot for displacement).
    assert_eq!(
        m.extras.get("mtl:disp").and_then(|v| v.as_str()),
        Some("dispmap.png"),
    );

    // Re-encode and verify the lines come back.
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ids = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("Tf 0.9 0.95 1"), "missing Tf in:\n{text}");
    assert!(
        text.contains("sharpness 75"),
        "missing sharpness in:\n{text}"
    );
    assert!(
        text.contains("disp dispmap.png"),
        "missing disp in:\n{text}"
    );
}

#[test]
fn tf_with_one_value_expands_to_grayscale_triple() {
    // Per MTL spec §"Tf r g b": "If only r is specified, then g and b
    // are assumed to be equal to r."
    let mats = mtl::parse_mtl("newmtl Smoke\nKd 1 1 1\nTf 0.4\n").unwrap();
    let tf = mats[0]
        .extras
        .get("mtl:Tf")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!((tf[0].as_f64().unwrap() - 0.4).abs() < 1e-6);
    assert!((tf[1].as_f64().unwrap() - 0.4).abs() < 1e-6);
    assert!((tf[2].as_f64().unwrap() - 0.4).abs() < 1e-6);
}

#[test]
fn map_bump_alias_emits_via_normal_texture_round_trip() {
    let mats = mtl::parse_mtl("newmtl Stone\nKd 0.5 0.5 0.5\nmap_bump rocks.png\n").unwrap();
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ids = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    // Encoder canonicalises to `map_Bump` (the documented spelling).
    assert!(
        text.contains("map_Bump rocks.png"),
        "expected canonical map_Bump in:\n{text}",
    );
}

#[test]
fn map_disp_alias_round_trips() {
    let mats = mtl::parse_mtl("newmtl Bricks\nKd 0.7 0.4 0.3\nmap_disp height.png\n").unwrap();
    assert_eq!(
        mats[0].extras.get("mtl:map_disp").and_then(|v| v.as_str()),
        Some("height.png"),
    );
}
