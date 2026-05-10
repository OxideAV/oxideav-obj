//! `d -halo factor` orientation-dependent dissolve per MTL spec
//! §"d -halo factor".
//!
//! The optional `-halo` flag makes the dissolve dependent on the
//! surface orientation; the factor argument is the same scalar that
//! ordinary `d <value>` consumes. We surface the halo flag via
//! `Material::extras["mtl:d_halo_factor"]` so the encoder can re-emit
//! the same `d -halo <value>` form.

use oxideav_obj::mtl;

#[test]
fn d_halo_sets_alpha_and_records_halo_extra() {
    let mats = mtl::parse_mtl("newmtl Sphere\nKd 0.6 0.6 0.6\nd -halo 0.66\n").unwrap();
    let m = &mats[0];
    assert!((m.base_color[3] - 0.66).abs() < 1e-6);
    assert!(matches!(m.alpha_mode, oxideav_mesh3d::AlphaMode::Blend));
    let halo = m
        .extras
        .get("mtl:d_halo_factor")
        .and_then(|v| v.as_f64())
        .unwrap();
    assert!((halo as f32 - 0.66).abs() < 1e-6);
}

#[test]
fn d_halo_round_trips_through_serializer() {
    let mats = mtl::parse_mtl("newmtl Edge\nKd 1 1 1\nd -halo 0.0\n").unwrap();
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("d -halo 0\n"),
        "expected `d -halo 0` line in:\n{text}",
    );

    // Re-decode and re-verify.
    let mats2 = mtl::parse_mtl(text).unwrap();
    assert!(mats2[0].extras.contains_key("mtl:d_halo_factor"));
}

#[test]
fn plain_d_without_halo_does_not_set_halo_extra() {
    let mats = mtl::parse_mtl("newmtl Plain\nKd 1 1 1\nd 0.5\n").unwrap();
    assert!(!mats[0].extras.contains_key("mtl:d_halo_factor"));
}
