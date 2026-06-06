//! Round 243 — OBJ rendering-identifier pair `maplib` / `usemap`.
//!
//! Spec §"maplib filename1 filename2 ..." declares a texture-map
//! library at the top level (sibling to `mtllib`); spec
//! §"usemap map_name/off" binds the named texture-map to the
//! elements that follow (sibling to `usemtl`). Both pre-date the
//! glTF-style PBR pipeline but stay relevant for round-trip
//! preservation when an OBJ produced by a Wavefront-era authoring
//! tool flows through us.
//!
//! Coverage:
//!   1. `maplib lib.map` lands in `Scene3D::extras["obj:maplibs"]`
//!      and re-emits on encode.
//!   2. Multi-name `maplib a.map b.map c.map` preserves order and
//!      strips duplicates (matching `mtllib` policy).
//!   3. `usemap TexA` binds the map to the following primitive and
//!      surfaces on `Primitive::extras["obj:usemap"]`.
//!   4. `usemap off` round-trips verbatim as `usemap off`.
//!   5. Mid-stream `usemap` switch splits the primitive (parallel to
//!      how `usemtl` and `s` / `mg` / `bevel` etc. split).
//!   6. A `usemtl` switch INHERITS the active `usemap` binding (the
//!      two state-setters operate independently per spec).
//!   7. Round-trip: decoded scene → encoded bytes → decoded scene
//!      preserves both the maplib list and per-primitive usemap.

use oxideav_obj::obj::{SerializeOptions, parse_obj, serialize_obj_with_options};

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn scene_from(src: &str) -> oxideav_mesh3d::Scene3D {
    parse_obj(src).expect("parse_obj")
}

#[test]
fn maplib_single_file_round_trips() {
    let src = "\
maplib textures.map
v 0 0 0
v 1 0 0
v 0 1 0
usemap MyTex
f 1 2 3
";
    let scene = scene_from(src);

    let libs = scene
        .extras
        .get("obj:maplibs")
        .and_then(|v| v.as_array())
        .expect("maplibs");
    assert_eq!(libs.len(), 1);
    assert_eq!(libs[0].as_str(), Some("textures.map"));

    let mesh = &scene.meshes[0];
    let prim = &mesh.primitives[0];
    assert_eq!(
        prim.extras.get("obj:usemap").and_then(|v| v.as_str()),
        Some("MyTex"),
    );

    // Encode + re-decode preserves both.
    let bytes = serialize_obj_with_options(&scene, &SerializeOptions::default()).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("maplib textures.map"));
    assert!(text.contains("usemap MyTex"));

    let scene2 = scene_from(text);
    let libs2 = scene2
        .extras
        .get("obj:maplibs")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(libs2.len(), 1);
    let prim2 = &scene2.meshes[0].primitives[0];
    assert_eq!(
        prim2.extras.get("obj:usemap").and_then(|v| v.as_str()),
        Some("MyTex"),
    );
}

#[test]
fn maplib_multi_file_preserves_order() {
    let src = "\
maplib a.map b.map c.map
v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
";
    let scene = scene_from(src);
    let libs: Vec<&str> = scene
        .extras
        .get("obj:maplibs")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(libs, vec!["a.map", "b.map", "c.map"]);
}

#[test]
fn maplib_dedupes_repeated_entries() {
    // The dedup rule matches `mtllib`: a name that appears on a later
    // line (or twice on the same line) is suppressed so the encoder
    // emits one declaration per file.
    let src = "\
maplib a.map b.map
maplib a.map c.map
v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
";
    let scene = scene_from(src);
    let libs: Vec<&str> = scene
        .extras
        .get("obj:maplibs")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(libs, vec!["a.map", "b.map", "c.map"]);

    let bytes = serialize_obj_with_options(&scene, &SerializeOptions::default()).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    // One emit per unique name, on its own line.
    assert_eq!(count(text, "maplib a.map\n"), 1);
    assert_eq!(count(text, "maplib b.map\n"), 1);
    assert_eq!(count(text, "maplib c.map\n"), 1);
}

#[test]
fn usemap_off_round_trips_verbatim() {
    // The spec §"usemap map_name/off" calls out `off` as a keyword
    // that turns texture mapping off for the following elements. The
    // literal `off` round-trips through the same `obj:usemap`
    // extras slot rather than collapsing to "no binding".
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
usemap off
f 1 2 3
";
    let scene = scene_from(src);
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(
        prim.extras.get("obj:usemap").and_then(|v| v.as_str()),
        Some("off"),
    );

    let bytes = serialize_obj_with_options(&scene, &SerializeOptions::default()).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("usemap off"));
}

#[test]
fn usemap_mid_stream_switch_splits_primitive() {
    // First `usemap TexA` binds the following face; then `usemap TexB`
    // binds the second face. The decoder splits into two primitives,
    // each carrying its own `obj:usemap` extras value.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
usemap TexA
f 1 2 3
usemap TexB
f 2 4 3
";
    let scene = scene_from(src);
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.primitives.len(), 2);
    assert_eq!(
        mesh.primitives[0]
            .extras
            .get("obj:usemap")
            .and_then(|v| v.as_str()),
        Some("TexA"),
    );
    assert_eq!(
        mesh.primitives[1]
            .extras
            .get("obj:usemap")
            .and_then(|v| v.as_str()),
        Some("TexB"),
    );
}

#[test]
fn usemap_inherits_across_usemtl_switch() {
    // The spec separates rendering identifiers (`usemap`) from
    // material identifiers (`usemtl`). A `usemtl` switch must NOT
    // reset the active `usemap` binding — each one operates
    // independently. After the round-trip:
    //   - prim[0]: usemtl=MatA, usemap=TexA
    //   - prim[1]: usemtl=MatB, usemap=TexA  (inherited)
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
usemap TexA
usemtl MatA
f 1 2 3
usemtl MatB
f 2 4 3
";
    let scene = scene_from(src);
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.primitives.len(), 2);
    assert_eq!(
        mesh.primitives[0]
            .extras
            .get("obj:usemtl")
            .and_then(|v| v.as_str()),
        Some("MatA"),
    );
    assert_eq!(
        mesh.primitives[1]
            .extras
            .get("obj:usemtl")
            .and_then(|v| v.as_str()),
        Some("MatB"),
    );
    // The headline check: TexA inherits across the MatA→MatB switch.
    assert_eq!(
        mesh.primitives[0]
            .extras
            .get("obj:usemap")
            .and_then(|v| v.as_str()),
        Some("TexA"),
    );
    assert_eq!(
        mesh.primitives[1]
            .extras
            .get("obj:usemap")
            .and_then(|v| v.as_str()),
        Some("TexA"),
    );
}

#[test]
fn full_round_trip_preserves_both() {
    // End-to-end: a doc that exercises maplib + multiple usemap
    // bindings re-decodes byte-stable on the second pass.
    let src = "\
maplib lib1.map lib2.map
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
usemap Tex1
f 1 2 3
usemap Tex2
f 2 4 3
usemap off
f 1 3 4
";
    let scene1 = scene_from(src);
    let bytes = serialize_obj_with_options(&scene1, &SerializeOptions::default()).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let scene2 = scene_from(text);

    // maplibs preserved.
    let libs: Vec<&str> = scene2
        .extras
        .get("obj:maplibs")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(libs, vec!["lib1.map", "lib2.map"]);

    // Three primitives, each with the right usemap binding.
    let mesh = &scene2.meshes[0];
    assert_eq!(mesh.primitives.len(), 3);
    let usemaps: Vec<&str> = mesh
        .primitives
        .iter()
        .filter_map(|p| p.extras.get("obj:usemap").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(usemaps, vec!["Tex1", "Tex2", "off"]);
}

#[test]
fn no_usemap_directive_means_no_extras_key() {
    // A document that never names a texture map MUST NOT fabricate
    // an `obj:usemap` entry — the spec's default is "off" but the
    // round-trip contract is "preserve only what the operator wrote".
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
";
    let scene = scene_from(src);
    assert!(!scene.extras.contains_key("obj:maplibs"));
    let prim = &scene.meshes[0].primitives[0];
    assert!(!prim.extras.contains_key("obj:usemap"));

    // And the encoder MUST NOT emit either keyword.
    let bytes = serialize_obj_with_options(&scene, &SerializeOptions::default()).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(!text.contains("maplib"));
    assert!(!text.contains("usemap"));
}
