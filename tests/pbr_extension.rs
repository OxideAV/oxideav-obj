//! Wavefront-PBR extension fields (`Pr`, `Pm`, `Pc`, `Ps`,
//! `map_Pr`, `map_Pm`) round-trip through Material's PBR slots and
//! the encoder.

use oxideav_obj::mtl;

const PBR_MTL: &str = "\
newmtl Steel
Kd 0.7 0.7 0.7
Pr 0.25
Pm 0.95
Pc 0.5
Ps 0.1
map_Pr roughness.png
";

#[test]
fn pr_pm_pc_ps_land_in_pbr_slots_and_re_emit_intact() {
    let mats = mtl::parse_mtl(PBR_MTL).unwrap();
    assert_eq!(mats.len(), 1);
    let m = &mats[0];
    assert!((m.roughness - 0.25).abs() < 1e-6);
    assert!((m.metallic - 0.95).abs() < 1e-6);
    assert!((m.extras.get("mtl:Pc").and_then(|v| v.as_f64()).unwrap() as f32 - 0.5).abs() < 1e-6);

    // `map_Pr` should be queued as the metallic_roughness texture.
    let pending = m.extras.get("mtl:pending_textures").unwrap();
    assert_eq!(
        pending["metallic_roughness"].as_str(),
        Some("roughness.png")
    );

    // Hoist into a Scene3D so the texture binding becomes real.
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ids = mtl::merge_materials_into_scene(&mut scene, mats);
    assert_eq!(scene.materials.len(), 1);
    assert_eq!(scene.textures.len(), 1);
    let mat = &scene.materials[0];
    assert!(mat.metallic_roughness_texture.is_some());
    assert!((mat.metallic - 0.95).abs() < 1e-6);
    assert!((mat.roughness - 0.25).abs() < 1e-6);

    // Re-encode the MTL.
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("Pr 0.25"));
    assert!(text.contains("Pm 0.95"));
    assert!(text.contains("Pc 0.5"));
    assert!(text.contains("Ps 0.1"));
    assert!(text.contains("map_Pr roughness.png"));
}
