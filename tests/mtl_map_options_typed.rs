//! Typed decomposition of `map_*` option flags per MTL spec
//! §"Options for texture map statements".
//!
//! Parallel to the raw `mtl:<map>:options` array, the parser surfaces
//! a typed object on `mtl:<map>:options_typed` whose stable keys map
//! each recognised flag to a primitive value (`bool` for `on`/`off`
//! flags, `f64` for single-numeric flags, `[f64; 2]` for `-mm`,
//! `[f64; 3]` for `-o` / `-s` / `-t`, `String` for `-imfchan` /
//! `-type`). The raw array still drives encoder round-trip, so an
//! input → output cycle stays byte-stable; the typed view is parse-
//! time-only.

use oxideav_obj::mtl;

fn typed_obj<'a>(
    mat: &'a oxideav_mesh3d::Material,
    keyword: &str,
) -> &'a serde_json::Map<String, serde_json::Value> {
    let key = format!("mtl:{keyword}:options_typed");
    mat.extras
        .get(&key)
        .and_then(|v| v.as_object())
        .unwrap_or_else(|| panic!("missing or non-object {key}; extras = {:?}", mat.extras))
}

#[test]
fn map_kd_blendu_clamp_decomposes_to_bools() {
    let mats =
        mtl::parse_mtl("newmtl Tex\nKd 1 1 1\nmap_Kd -blendu off -clamp on diffuse.png\n").unwrap();
    let m = &mats[0];
    let typed = typed_obj(m, "map_Kd");

    assert_eq!(typed.get("blendu").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(typed.get("clamp").and_then(|v| v.as_bool()), Some(true));
    assert!(
        typed.get("blendv").is_none(),
        "blendv shouldn't appear without an explicit flag"
    );
}

#[test]
fn bump_bm_multiplier_decomposes_to_f64() {
    let mats =
        mtl::parse_mtl("newmtl Stone\nKd 0.5 0.5 0.5\nbump -bm 0.3 -clamp on rocks.png\n").unwrap();
    let m = &mats[0];
    let typed = typed_obj(m, "bump");

    let bm = typed.get("bm").and_then(|v| v.as_f64()).unwrap();
    assert!((bm - 0.3).abs() < 1e-9, "expected bm=0.3, got {bm}");
    assert_eq!(typed.get("clamp").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn map_kd_mm_pair_decomposes_to_two_floats() {
    let mats = mtl::parse_mtl("newmtl Cloud\nKd 1 1 1\nmap_Kd -mm 0.1 0.95 sky.png\n").unwrap();
    let m = &mats[0];
    let typed = typed_obj(m, "map_Kd");

    let mm = typed
        .get("mm")
        .and_then(|v| v.as_array())
        .expect("mm should be an array");
    assert_eq!(mm.len(), 2);
    let base = mm[0].as_f64().unwrap();
    let gain = mm[1].as_f64().unwrap();
    assert!((base - 0.1).abs() < 1e-9);
    assert!((gain - 0.95).abs() < 1e-9);
}

#[test]
fn map_kd_o_and_s_decompose_to_three_floats_each() {
    let mats =
        mtl::parse_mtl("newmtl Tile\nKd 1 1 1\nmap_Kd -o 0.1 0.2 0.3 -s 2 1.5 0.5 tile.png\n")
            .unwrap();
    let m = &mats[0];
    let typed = typed_obj(m, "map_Kd");

    let o = typed
        .get("o")
        .and_then(|v| v.as_array())
        .expect("o should be an array");
    assert_eq!(o.len(), 3);
    let oxs: Vec<f64> = o.iter().filter_map(|v| v.as_f64()).collect();
    assert!((oxs[0] - 0.1).abs() < 1e-9);
    assert!((oxs[1] - 0.2).abs() < 1e-9);
    assert!((oxs[2] - 0.3).abs() < 1e-9);

    let s = typed
        .get("s")
        .and_then(|v| v.as_array())
        .expect("s should be an array");
    let sxs: Vec<f64> = s.iter().filter_map(|v| v.as_f64()).collect();
    assert!((sxs[0] - 2.0).abs() < 1e-9);
    assert!((sxs[1] - 1.5).abs() < 1e-9);
    assert!((sxs[2] - 0.5).abs() < 1e-9);
}

#[test]
fn map_d_imfchan_single_letter_round_trips_to_string() {
    let mats =
        mtl::parse_mtl("newmtl Glass\nKd 0.6 0.7 0.8\nmap_d -imfchan m alpha.png\n").unwrap();
    let m = &mats[0];
    let typed = typed_obj(m, "map_d");
    assert_eq!(typed.get("imfchan").and_then(|v| v.as_str()), Some("m"));
}

#[test]
fn imfchan_outside_spec_alphabet_is_dropped_from_typed() {
    // The raw `:options` array still preserves the verbatim source.
    let mats = mtl::parse_mtl("newmtl Bad\nKd 1 1 1\nmap_d -imfchan q garbled.png\n").unwrap();
    let m = &mats[0];

    let raw = m
        .extras
        .get("mtl:map_d:options")
        .and_then(|v| v.as_array())
        .unwrap();
    let strs: Vec<&str> = raw.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["-imfchan q"]);

    // Typed view should NOT contain a stray imfchan (q is not in the
    // spec's r|g|b|m|l|z alphabet). The key may be absent entirely if
    // no other recognised flags appeared.
    let typed = m
        .extras
        .get("mtl:map_d:options_typed")
        .and_then(|v| v.as_object());
    assert!(
        typed.is_none() || typed.unwrap().get("imfchan").is_none(),
        "imfchan q should be dropped from the typed view"
    );
}

#[test]
fn decal_texres_single_int_decomposes_to_f64() {
    let mats = mtl::parse_mtl("newmtl Card\nKd 1 1 1\ndecal -texres 512 sticker.png\n").unwrap();
    let m = &mats[0];
    let typed = typed_obj(m, "decal");
    let texres = typed.get("texres").and_then(|v| v.as_f64()).unwrap();
    assert!((texres - 512.0).abs() < 1e-9);
}

#[test]
fn typed_view_does_not_round_trip_through_encoder() {
    // The typed key is parse-time-only — the encoder must keep using
    // the raw `:options` array so source-order tokens stay byte-stable
    // and the `options_typed` key never appears on disk.
    let mats =
        mtl::parse_mtl("newmtl Tex\nKd 1 1 1\nmap_Kd -blendu off -clamp on diffuse.png\n").unwrap();

    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();

    assert!(
        text.contains("map_Kd -blendu off -clamp on diffuse.png"),
        "raw options should drive the emit:\n{text}",
    );
    assert!(
        !text.contains("options_typed"),
        "typed key should never be serialised:\n{text}",
    );
}

#[test]
fn raw_options_still_present_alongside_typed() {
    // Both keys land on the same parsed material so consumers can pick
    // whichever shape they prefer.
    let mats =
        mtl::parse_mtl("newmtl Tile\nKd 1 1 1\nmap_Kd -blendu on -mm 0 1 -s 2 2 1 tile.png\n")
            .unwrap();
    let m = &mats[0];

    let raw = m
        .extras
        .get("mtl:map_Kd:options")
        .and_then(|v| v.as_array())
        .expect("raw options array must persist");
    let strs: Vec<&str> = raw.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["-blendu on", "-mm 0 1", "-s 2 2 1"]);

    let typed = typed_obj(m, "map_Kd");
    assert_eq!(typed.get("blendu").and_then(|v| v.as_bool()), Some(true));
    let mm = typed.get("mm").and_then(|v| v.as_array()).unwrap();
    assert_eq!(mm.len(), 2);
    let s = typed.get("s").and_then(|v| v.as_array()).unwrap();
    assert_eq!(s.len(), 3);
}

#[test]
fn refl_sphere_with_options_nests_typed_view() {
    // The `refl -type sphere -mm 0 1 clouds.mpc` example from spec
    // §"Examples" lands the nested options chunk inside the
    // `mtl:refl:sphere` entry; the typed decomposition rides alongside
    // the raw array under `options_typed`.
    let mats =
        mtl::parse_mtl("newmtl Sky\nKd 1 1 1\nrefl -type sphere -mm 0 1 -clamp on clouds.mpc\n")
            .unwrap();
    let m = &mats[0];
    let entry = m
        .extras
        .get("mtl:refl:sphere")
        .and_then(|v| v.as_object())
        .expect("sphere entry");
    let typed = entry
        .get("options_typed")
        .and_then(|v| v.as_object())
        .expect("nested options_typed on sphere entry");
    let mm = typed.get("mm").and_then(|v| v.as_array()).unwrap();
    assert_eq!(mm.len(), 2);
    assert!((mm[0].as_f64().unwrap() - 0.0).abs() < 1e-9);
    assert!((mm[1].as_f64().unwrap() - 1.0).abs() < 1e-9);
    assert_eq!(typed.get("clamp").and_then(|v| v.as_bool()), Some(true));

    // File still resolves to the bare path.
    assert_eq!(
        entry.get("file").and_then(|v| v.as_str()),
        Some("clouds.mpc")
    );
}

#[test]
fn refl_cube_face_with_options_nests_typed_view() {
    let mats = mtl::parse_mtl(
        "newmtl Room\nKd 1 1 1\nrefl -type cube_top -blendu off ceiling.png\nrefl -type cube_bottom -clamp on floor.png\n",
    )
    .unwrap();
    let m = &mats[0];
    let cube = m
        .extras
        .get("mtl:refl:cube")
        .and_then(|v| v.as_object())
        .expect("cube bundle");

    let top = cube
        .get("cube_top")
        .and_then(|v| v.as_object())
        .expect("cube_top entry");
    let top_typed = top
        .get("options_typed")
        .and_then(|v| v.as_object())
        .expect("cube_top nested typed view");
    assert_eq!(
        top_typed.get("blendu").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        top.get("file").and_then(|v| v.as_str()),
        Some("ceiling.png")
    );

    let bot = cube
        .get("cube_bottom")
        .and_then(|v| v.as_object())
        .expect("cube_bottom entry");
    let bot_typed = bot
        .get("options_typed")
        .and_then(|v| v.as_object())
        .expect("cube_bottom nested typed view");
    assert_eq!(bot_typed.get("clamp").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn unknown_flag_does_not_create_options_typed() {
    // `-glow` isn't in the spec's option list and isn't recognised by
    // the chunker either (no arg-count entry). The raw `:options`
    // array still picks it up verbatim, but the typed view should
    // never appear (no recognised flags means no typed object).
    let mats = mtl::parse_mtl("newmtl Tex\nKd 1 1 1\nmap_Kd -glow on weird.png\n").unwrap();
    let m = &mats[0];
    assert!(
        !m.extras.contains_key("mtl:map_Kd:options_typed"),
        "no recognised flags ⇒ no typed key"
    );
}

#[test]
fn boolean_bad_argument_drops_flag_from_typed_but_keeps_raw() {
    // `-clamp maybe` is malformed — the typed view should not invent
    // a bool. The raw array still carries it verbatim so the encoder
    // round-trips the operator's exact source.
    let mats = mtl::parse_mtl("newmtl Tex\nKd 1 1 1\nmap_Kd -clamp maybe weird.png\n").unwrap();
    let m = &mats[0];
    let raw = m
        .extras
        .get("mtl:map_Kd:options")
        .and_then(|v| v.as_array())
        .unwrap();
    let strs: Vec<&str> = raw.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["-clamp maybe"]);

    assert!(
        !m.extras.contains_key("mtl:map_Kd:options_typed"),
        "malformed clamp argument should drop typed view"
    );
}

#[test]
fn t_turbulence_decomposes_to_three_floats() {
    // Spec §"-t u v w" describes turbulence; the typed view exposes
    // the three components as an [u, v, w] array under the `t` key.
    let mats =
        mtl::parse_mtl("newmtl Marble\nKd 1 1 1\nmap_Kd -t 0.1 0.2 0.3 marble.png\n").unwrap();
    let m = &mats[0];
    let typed = typed_obj(m, "map_Kd");
    let t = typed.get("t").and_then(|v| v.as_array()).expect("t array");
    assert_eq!(t.len(), 3);
    let vs: Vec<f64> = t.iter().filter_map(|v| v.as_f64()).collect();
    assert!((vs[0] - 0.1).abs() < 1e-9);
    assert!((vs[1] - 0.2).abs() < 1e-9);
    assert!((vs[2] - 0.3).abs() < 1e-9);
}

#[test]
fn boost_and_cc_decompose_alongside_each_other() {
    // Two unrelated single-arg flags on the same line should both
    // surface — `boost` as f64, `cc` as bool.
    let mats = mtl::parse_mtl("newmtl Tex\nKd 1 1 1\nmap_Kd -boost 1.5 -cc on hot.png\n").unwrap();
    let m = &mats[0];
    let typed = typed_obj(m, "map_Kd");
    let boost = typed.get("boost").and_then(|v| v.as_f64()).unwrap();
    assert!((boost - 1.5).abs() < 1e-9);
    assert_eq!(typed.get("cc").and_then(|v| v.as_bool()), Some(true));
}
