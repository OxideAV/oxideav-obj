//! PBR-extension scalar-field texture-map siblings round-trip through
//! the parser/encoder: `map_Ps` (sheen), `map_Pc` (clearcoat-thickness),
//! `map_Pcr` (clearcoat-roughness), and `map_aniso` / `map_anisor`
//! (anisotropy / anisotropy-rotation).
//!
//! These are the texture-map companions of the already-modelled `Ps` /
//! `Pc` / `Pcr` / `aniso` / `anisor` scalar PBR fields. glTF's
//! metallic-roughness material has no direct channel for any of them,
//! so the parser keeps the file reference (plus any `-flag value`
//! option chunks) verbatim in `Material::extras["mtl:<map>"]` and the
//! encoder's generic string-passthrough re-emits it. Before this they
//! were silently dropped, making a decode -> encode round-trip lossy
//! for the upper half of the PBR map family.

use oxideav_obj::mtl;

fn round_trip(src: &str) -> String {
    let mats = mtl::parse_mtl(src).unwrap();
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ids = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn map_ps_sheen_preserved_in_extras() {
    let mats = mtl::parse_mtl("newmtl Velvet\nKd 0.5 0.1 0.2\nPs 0.8\nmap_Ps sheen.png\n").unwrap();
    let m = &mats[0];
    // The Ps scalar still lands in extras.
    assert!((m.extras.get("mtl:Ps").and_then(|v| v.as_f64()).unwrap() - 0.8).abs() < 1e-6);
    // The map filename is preserved (previously dropped).
    assert_eq!(
        m.extras.get("mtl:map_Ps").and_then(|v| v.as_str()),
        Some("sheen.png"),
    );
}

#[test]
fn full_pbr_map_family_round_trips() {
    let src = "\
newmtl PbrShell
Kd 0.6 0.6 0.6
Pr 0.4
Pm 0.0
Pc 1.0
Pcr 0.2
Ps 0.5
aniso 0.3
anisor 0.1
map_Ps sheen.png
map_Pc clearcoat.png
map_Pcr ccrough.png
map_aniso aniso.png
map_anisor anisorot.png
";
    let text = round_trip(src);
    for line in [
        "map_Ps sheen.png",
        "map_Pc clearcoat.png",
        "map_Pcr ccrough.png",
        "map_aniso aniso.png",
        "map_anisor anisorot.png",
    ] {
        assert!(text.contains(line), "missing `{line}` in:\n{text}");
    }
}

#[test]
fn map_pc_with_options_splices_flags_back() {
    // The PBR map siblings share the standard `-flag value` option
    // parser, so option chunks are stripped from the filename and
    // re-spliced ahead of it on encode.
    let mats =
        mtl::parse_mtl("newmtl Lacquer\nKd 1 1 1\nmap_Pc -clamp on -bm 0.5 coat.png\n").unwrap();
    let m = &mats[0];
    // Filename arrives clean.
    assert_eq!(
        m.extras.get("mtl:map_Pc").and_then(|v| v.as_str()),
        Some("coat.png"),
    );
    // Option chunk preserved verbatim.
    let opts = m
        .extras
        .get("mtl:map_Pc:options")
        .and_then(|v| v.as_array())
        .unwrap();
    let strs: Vec<&str> = opts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["-clamp on", "-bm 0.5"]);

    // Encoder re-splices them ahead of the filename.
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("map_Pc -clamp on -bm 0.5 coat.png"),
        "missing spliced options in:\n{text}",
    );
}

#[test]
fn map_anisor_alias_round_trips() {
    let text = round_trip("newmtl Brushed\nKd 0.8 0.8 0.8\nmap_anisor rot.png\n");
    assert!(
        text.contains("map_anisor rot.png"),
        "missing map_anisor in:\n{text}"
    );
}
