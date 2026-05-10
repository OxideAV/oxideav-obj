//! `map_*` directive option flags per MTL spec §"Options for texture
//! map statements" (`-blendu`, `-bm`, `-mm`, `-clamp`, `-imfchan`,
//! `-o`, `-s`, `-t`, `-texres`).
//!
//! Leading `-flag value` pairs are stripped out of the filename and
//! surfaced via `Material::extras["mtl:<map_name>:options"]` as an
//! array of `"<flag> <args>"` strings. Round-trips through the
//! encoder splice the option chunks back ahead of the filename.

use oxideav_obj::mtl;

#[test]
fn map_kd_with_blendu_clamp_strips_flags_from_filename() {
    let mats =
        mtl::parse_mtl("newmtl Tex\nKd 1 1 1\nmap_Kd -blendu off -clamp on diffuse.png\n").unwrap();
    let m = &mats[0];

    // Filename arrives clean (no leading flags).
    let pending = m.extras.get("mtl:pending_textures").unwrap();
    assert_eq!(pending["base_color"].as_str(), Some("diffuse.png"));

    // Options preserved verbatim.
    let opts = m
        .extras
        .get("mtl:map_Kd:options")
        .and_then(|v| v.as_array())
        .unwrap();
    let strs: Vec<&str> = opts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["-blendu off", "-clamp on"]);
}

#[test]
fn bump_with_bm_multiplier_round_trips() {
    let mats =
        mtl::parse_mtl("newmtl Stone\nKd 0.5 0.5 0.5\nbump -bm 0.3 -clamp on rocks.png\n").unwrap();
    let m = &mats[0];
    let pending = m.extras.get("mtl:pending_textures").unwrap();
    assert_eq!(pending["normal"].as_str(), Some("rocks.png"));
    let opts = m
        .extras
        .get("mtl:bump:options")
        .and_then(|v| v.as_array())
        .unwrap();
    let strs: Vec<&str> = opts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["-bm 0.3", "-clamp on"]);

    // Encoder re-emits options ahead of the filename, even though the
    // canonical map keyword is `map_Bump`.
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("map_Bump -bm 0.3 -clamp on rocks.png"),
        "missing spliced options in:\n{text}",
    );
}

#[test]
fn map_kd_with_mm_two_args_consumes_both() {
    // `-mm base gain` takes two numeric arguments — neither should leak
    // into the filename.
    let mats = mtl::parse_mtl("newmtl Cloud\nKd 1 1 1\nmap_Kd -mm 0.1 0.95 sky.png\n").unwrap();
    let m = &mats[0];
    let pending = m.extras.get("mtl:pending_textures").unwrap();
    assert_eq!(pending["base_color"].as_str(), Some("sky.png"));
    let opts = m
        .extras
        .get("mtl:map_Kd:options")
        .and_then(|v| v.as_array())
        .unwrap();
    let strs: Vec<&str> = opts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["-mm 0.1 0.95"]);
}

#[test]
fn map_d_imfchan_and_decal_round_trip_keeps_options() {
    let mats = mtl::parse_mtl(
        "newmtl Glass\nKd 0.6 0.7 0.8\nmap_d -imfchan m -clamp on alpha.png\ndecal -texres 512 sticker.png\n",
    )
    .unwrap();
    let m = &mats[0];

    // map_d filename keeps its options out.
    assert_eq!(
        m.extras.get("mtl:map_d").and_then(|v| v.as_str()),
        Some("alpha.png"),
    );
    let map_d_opts = m
        .extras
        .get("mtl:map_d:options")
        .and_then(|v| v.as_array())
        .unwrap();
    let strs: Vec<&str> = map_d_opts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["-imfchan m", "-clamp on"]);

    // decal filename keeps its options out.
    assert_eq!(
        m.extras.get("mtl:decal").and_then(|v| v.as_str()),
        Some("sticker.png"),
    );
    let decal_opts = m
        .extras
        .get("mtl:decal:options")
        .and_then(|v| v.as_array())
        .unwrap();
    let strs: Vec<&str> = decal_opts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["-texres 512"]);

    // Encoder splices both option chunks back inline.
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("map_d -imfchan m -clamp on alpha.png"),
        "missing spliced map_d options in:\n{text}",
    );
    assert!(
        text.contains("decal -texres 512 sticker.png"),
        "missing spliced decal options in:\n{text}",
    );
}

#[test]
fn map_kd_with_o_and_s_consumes_three_floats_each() {
    // `-o u v w` and `-s u v w` each take three numeric args. The
    // filename comes after both option chunks have been consumed.
    let mats =
        mtl::parse_mtl("newmtl Tile\nKd 1 1 1\nmap_Kd -o 0.1 0.0 0.0 -s 2 2 1 tile.png\n").unwrap();
    let m = &mats[0];
    let pending = m.extras.get("mtl:pending_textures").unwrap();
    assert_eq!(pending["base_color"].as_str(), Some("tile.png"));
    let opts = m
        .extras
        .get("mtl:map_Kd:options")
        .and_then(|v| v.as_array())
        .unwrap();
    let strs: Vec<&str> = opts.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["-o 0.1 0.0 0.0", "-s 2 2 1"]);
}
