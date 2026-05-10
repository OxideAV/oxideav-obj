//! Wavefront MTL `refl -type sphere` and `refl -type cube_*` reflection
//! map typed forms.
//!
//! Spec §"Reflection Map" lists three discriminated forms:
//!
//!   refl -type sphere [-options] filename
//!   refl -type cube_top|cube_bottom|cube_front|cube_back|cube_left|cube_right [-options] filename
//!   refl filename                              (legacy bare form)
//!
//! Round 4 tunnelled every `refl` line through the generic
//! `mtl:refl = filename` extras slot, which means six `cube_*` lines
//! collapsed onto each other (last-write-wins). Round 5 lifts each
//! typed variant into a structured extras slot:
//!
//!   mtl:refl:sphere = { file, options? }
//!   mtl:refl:cube   = { cube_top: { file, options? }, cube_bottom: { ... }, … }
//!
//! The encoder re-emits one line per face / sphere with options
//! spliced ahead of the filename in the same shape the parser saw.

use oxideav_obj::mtl;

#[test]
fn refl_sphere_with_options_round_trips() {
    let text = "newmtl Mirror\nKd 0.8 0.8 0.8\nrefl -type sphere -mm 0 1 chrome.mpc\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let m = &mats[0];
    let entry = m
        .extras
        .get("mtl:refl:sphere")
        .and_then(|v| v.as_object())
        .expect("sphere refl captured");
    assert_eq!(
        entry.get("file").and_then(|v| v.as_str()),
        Some("chrome.mpc")
    );
    let opts = entry
        .get("options")
        .and_then(|v| v.as_array())
        .expect("options captured");
    assert_eq!(opts.len(), 1);
    assert_eq!(opts[0].as_str(), Some("-mm 0 1"));
    // Should NOT also populate the legacy single-string slot.
    assert!(!m.extras.contains_key("mtl:refl"));

    // Re-encode and verify line shape.
    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let line = s
        .lines()
        .find(|l| l.starts_with("refl "))
        .expect("refl line emitted");
    assert_eq!(line, "refl -type sphere -mm 0 1 chrome.mpc");
}

#[test]
fn refl_six_cube_faces_bundle_into_one_cubemap() {
    // The six cube faces span six separate `refl` lines; the parser
    // must fold them into one `mtl:refl:cube` object so consumers see
    // a single cubemap rather than six unrelated textures.
    let text = "\
newmtl Sky
Kd 1 1 1
refl -type cube_top    sky_top.png
refl -type cube_bottom sky_bot.png
refl -type cube_front  sky_fnt.png
refl -type cube_back   sky_bck.png
refl -type cube_left   sky_lft.png
refl -type cube_right  sky_rgt.png
";
    let mats = mtl::parse_mtl(text).unwrap();
    let cube = mats[0]
        .extras
        .get("mtl:refl:cube")
        .and_then(|v| v.as_object())
        .expect("cubemap captured");
    assert_eq!(cube.len(), 6);
    for face in [
        "cube_top",
        "cube_bottom",
        "cube_front",
        "cube_back",
        "cube_left",
        "cube_right",
    ] {
        let entry = cube.get(face).and_then(|v| v.as_object()).unwrap();
        let file = entry.get("file").and_then(|v| v.as_str()).unwrap();
        assert!(
            file.starts_with("sky_"),
            "face {face} has unexpected file {file:?}"
        );
    }

    // Re-encode and verify one `refl -type cube_*` line per face,
    // emitted in deterministic order.
    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let refl_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("refl ")).collect();
    assert_eq!(refl_lines.len(), 6);
    assert_eq!(refl_lines[0], "refl -type cube_top sky_top.png");
    assert_eq!(refl_lines[1], "refl -type cube_bottom sky_bot.png");
    assert_eq!(refl_lines[2], "refl -type cube_front sky_fnt.png");
    assert_eq!(refl_lines[3], "refl -type cube_back sky_bck.png");
    assert_eq!(refl_lines[4], "refl -type cube_left sky_lft.png");
    assert_eq!(refl_lines[5], "refl -type cube_right sky_rgt.png");
}

#[test]
fn refl_cube_per_face_options_round_trip() {
    // Different option flags on different faces (clamp on top,
    // tex-resolution on bottom) must round-trip per-face.
    let text = "\
newmtl Box
Kd 1 1 1
refl -type cube_top -clamp on box_top.png
refl -type cube_bottom -texres 256 box_bot.png
";
    let mats = mtl::parse_mtl(text).unwrap();
    let cube = mats[0]
        .extras
        .get("mtl:refl:cube")
        .and_then(|v| v.as_object())
        .unwrap();
    let top = cube.get("cube_top").and_then(|v| v.as_object()).unwrap();
    let top_opts = top.get("options").and_then(|v| v.as_array()).unwrap();
    assert_eq!(top_opts[0].as_str(), Some("-clamp on"));
    let bot = cube.get("cube_bottom").and_then(|v| v.as_object()).unwrap();
    let bot_opts = bot.get("options").and_then(|v| v.as_array()).unwrap();
    assert_eq!(bot_opts[0].as_str(), Some("-texres 256"));

    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let refl_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("refl ")).collect();
    assert_eq!(refl_lines.len(), 2);
    assert_eq!(refl_lines[0], "refl -type cube_top -clamp on box_top.png");
    assert_eq!(
        refl_lines[1],
        "refl -type cube_bottom -texres 256 box_bot.png"
    );
}

#[test]
fn refl_legacy_bare_form_still_works() {
    // Backwards-compat: `refl filename` (no `-type`) keeps the legacy
    // `mtl:refl` string slot used in r3.
    let text = "newmtl C\nKd 1 1 1\nrefl bare.png\n";
    let mats = mtl::parse_mtl(text).unwrap();
    assert_eq!(
        mats[0].extras.get("mtl:refl").and_then(|v| v.as_str()),
        Some("bare.png")
    );
    assert!(!mats[0].extras.contains_key("mtl:refl:sphere"));
    assert!(!mats[0].extras.contains_key("mtl:refl:cube"));
}
