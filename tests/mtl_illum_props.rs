//! MTL `illum` integer is decomposed into the spec's per-model
//! property breakdown alongside the raw value, so consumers can
//! introspect a material's shading intent without re-deriving the
//! table from the integer.
//!
//! Round 212. Reference: Wavefront MTL spec §"illum illum_#"
//! summary table (Advanced Visualizer manual p.5-30), mirrored in
//! `docs/3d/obj/wavefront-mtl-spec.html`.

use oxideav_obj::mtl;

/// Pull `mtl:illum_props` from a material's extras and assert the
/// per-flag truth values match the spec table for the given model.
fn assert_props(text: &str, expected: &[(&str, bool)]) {
    let mats = mtl::parse_mtl(text).unwrap();
    assert_eq!(mats.len(), 1, "expected one material");
    let m = &mats[0];
    let props = m
        .extras
        .get("mtl:illum_props")
        .and_then(|v| v.as_object())
        .unwrap_or_else(|| panic!("missing mtl:illum_props on:\n{text}"));
    for &(key, want) in expected {
        let got = props
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| panic!("flag {key} missing in props={props:?}"));
        assert_eq!(
            got, want,
            "model props mismatch on key {key}: got {got}, want {want}\n{text}"
        );
    }
}

#[test]
fn illum_0_color_on_ambient_off() {
    // Spec row 0: "Color on and Ambient off". No shading terms beyond
    // the flat colour.
    assert_props(
        "newmtl Flat\nKd 0.4 0.5 0.6\nillum 0\n",
        &[
            ("color", true),
            ("ambient", false),
            ("highlight", false),
            ("reflection", false),
            ("ray_trace", false),
            ("transparency_glass", false),
            ("transparency_refraction", false),
            ("fresnel", false),
            ("casts_shadow_on_invisible", false),
        ],
    );
}

#[test]
fn illum_1_lambertian_with_ambient() {
    // Spec row 1: "Color on and Ambient on" — Lambertian diffuse.
    assert_props(
        "newmtl Lambert\nKd 0.4 0.5 0.6\nillum 1\n",
        &[
            ("color", true),
            ("ambient", true),
            ("highlight", false),
            ("reflection", false),
            ("ray_trace", false),
            ("fresnel", false),
        ],
    );
}

#[test]
fn illum_2_blinn_phong_highlight() {
    // Spec row 2: "Highlight on" — diffuse + Blinn-Phong specular.
    assert_props(
        "newmtl Plastic\nKd 0.4 0.5 0.6\nKs 0.9 0.9 0.9\nNs 50\nillum 2\n",
        &[
            ("color", true),
            ("ambient", true),
            ("highlight", true),
            ("reflection", false),
            ("ray_trace", false),
            ("fresnel", false),
        ],
    );
}

#[test]
fn illum_3_reflection_and_ray_trace() {
    // Spec row 3: "Reflection on and Ray trace on".
    assert_props(
        "newmtl Chrome\nKd 0.5 0.5 0.5\nillum 3\n",
        &[
            ("color", true),
            ("ambient", true),
            ("highlight", true),
            ("reflection", true),
            ("ray_trace", true),
            ("fresnel", false),
            ("transparency_glass", false),
            ("transparency_refraction", false),
        ],
    );
}

#[test]
fn illum_4_glass_transparency_ray_trace() {
    // Spec row 4: "Transparency: Glass on; Reflection: Ray trace on".
    assert_props(
        "newmtl Glass\nKd 0.9 0.9 0.9\nillum 4\n",
        &[
            ("color", true),
            ("ambient", true),
            ("highlight", true),
            ("reflection", true),
            ("ray_trace", true),
            ("transparency_glass", true),
            ("transparency_refraction", false),
            ("fresnel", false),
        ],
    );
}

#[test]
fn illum_5_fresnel_ray_trace() {
    // Spec row 5: "Reflection: Fresnel on and Ray trace on".
    assert_props(
        "newmtl Fresnel\nKd 0.6 0.6 0.6\nillum 5\n",
        &[
            ("color", true),
            ("ambient", true),
            ("highlight", true),
            ("reflection", true),
            ("ray_trace", true),
            ("fresnel", true),
            ("transparency_glass", false),
            ("transparency_refraction", false),
        ],
    );
}

#[test]
fn illum_6_refraction_fresnel_off_ray_trace() {
    // Spec row 6: "Transparency: Refraction on; Reflection: Fresnel
    // off, Ray trace on" — the Fresnel-off form distinguishes this
    // row from row 7.
    assert_props(
        "newmtl Water\nKd 0.8 0.85 1.0\nNi 1.33\nillum 6\n",
        &[
            ("color", true),
            ("ambient", true),
            ("highlight", true),
            ("reflection", true),
            ("ray_trace", true),
            ("transparency_refraction", true),
            ("fresnel", false),
            ("transparency_glass", false),
        ],
    );
}

#[test]
fn illum_7_refraction_fresnel_ray_trace() {
    // Spec row 7: "Transparency: Refraction on; Reflection: Fresnel on,
    // Ray trace on".
    assert_props(
        "newmtl FresnelGlass\nKd 0.8 0.85 1.0\nNi 1.5\nillum 7\n",
        &[
            ("color", true),
            ("ambient", true),
            ("highlight", true),
            ("reflection", true),
            ("ray_trace", true),
            ("transparency_refraction", true),
            ("fresnel", true),
            ("transparency_glass", false),
        ],
    );
}

#[test]
fn illum_8_reflection_no_ray_trace() {
    // Spec row 8: "Reflection on and Ray trace off". Mirrors model 3
    // semantics without ray tracing.
    assert_props(
        "newmtl Mirror\nKd 0.5 0.5 0.5\nillum 8\n",
        &[
            ("color", true),
            ("ambient", true),
            ("highlight", true),
            ("reflection", true),
            ("ray_trace", false),
            ("transparency_glass", false),
            ("fresnel", false),
        ],
    );
}

#[test]
fn illum_9_glass_no_ray_trace() {
    // Spec row 9: "Transparency: Glass on; Reflection: Ray trace off".
    assert_props(
        "newmtl GlassNoRT\nKd 0.9 0.9 0.9\nillum 9\n",
        &[
            ("color", true),
            ("ambient", true),
            ("highlight", true),
            ("reflection", true),
            ("ray_trace", false),
            ("transparency_glass", true),
            ("fresnel", false),
        ],
    );
}

#[test]
fn illum_10_shadowmatte() {
    // Spec row 10: "Casts shadows onto invisible surfaces". The pixel
    // colour of a shadowmatte is always black per the spec equation
    // section, so the `color` / `ambient` flags are off.
    assert_props(
        "newmtl ShadowCatcher\nillum 10\n",
        &[
            ("color", false),
            ("ambient", false),
            ("highlight", false),
            ("reflection", false),
            ("ray_trace", false),
            ("transparency_glass", false),
            ("transparency_refraction", false),
            ("fresnel", false),
            ("casts_shadow_on_invisible", true),
        ],
    );
}

#[test]
fn raw_illum_integer_still_lands() {
    // The decomposition is *additive* — the raw integer is still
    // surfaced unchanged so the round-trip emits the same line.
    let mats = mtl::parse_mtl("newmtl X\nKd 1 1 1\nillum 4\n").unwrap();
    let m = &mats[0];
    assert_eq!(m.extras.get("mtl:illum").and_then(|v| v.as_i64()), Some(4));
    assert!(m.extras.contains_key("mtl:illum_props"));
}

#[test]
fn out_of_range_illum_omits_props() {
    // Values outside 0..=10 are out-of-spec; we still capture the raw
    // integer so the round-trip is lossless, but the property
    // decomposition is omitted (no spec row to mirror).
    let mats = mtl::parse_mtl("newmtl Weird\nKd 1 1 1\nillum 42\n").unwrap();
    let m = &mats[0];
    assert_eq!(
        m.extras.get("mtl:illum").and_then(|v| v.as_i64()),
        Some(42),
        "raw integer must still land"
    );
    assert!(
        !m.extras.contains_key("mtl:illum_props"),
        "props must be absent for out-of-spec models"
    );
}

#[test]
fn negative_illum_omits_props() {
    // Same shape as out-of-range positive: raw int kept, props
    // omitted.
    let mats = mtl::parse_mtl("newmtl Weird\nKd 1 1 1\nillum -1\n").unwrap();
    let m = &mats[0];
    assert_eq!(m.extras.get("mtl:illum").and_then(|v| v.as_i64()), Some(-1),);
    assert!(!m.extras.contains_key("mtl:illum_props"));
}

#[test]
fn props_do_not_emit_extra_mtl_line() {
    // The encoder must not emit a synthetic `illum_props ...` line —
    // the decomposition is parse-time metadata, not a serialised
    // keyword. Re-encoding the material yields the same single
    // `illum N` line.
    let mats = mtl::parse_mtl("newmtl X\nKd 0.5 0.5 0.5\nillum 4\n").unwrap();
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ids = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let illum_lines: Vec<&str> = text
        .lines()
        .filter(|l| l.trim_start().starts_with("illum"))
        .collect();
    assert_eq!(
        illum_lines.len(),
        1,
        "expected exactly one illum line, got:\n{text}"
    );
    assert_eq!(illum_lines[0].trim(), "illum 4");
    assert!(
        !text.contains("illum_props"),
        "encoder must not leak the props key into MTL output:\n{text}"
    );
}

#[test]
fn round_trip_preserves_illum_through_scene_merge() {
    // Parsing → scene merge → re-parse the serialised output yields
    // the same illum integer and the same property decomposition.
    let mats = mtl::parse_mtl("newmtl X\nKd 0.5 0.5 0.5\nillum 7\n").unwrap();
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let _ids = mtl::merge_materials_into_scene(&mut scene, mats);
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).unwrap();
    let mats2 = mtl::parse_mtl(std::str::from_utf8(&bytes).unwrap()).unwrap();
    let m2 = &mats2[0];
    assert_eq!(m2.extras.get("mtl:illum").and_then(|v| v.as_i64()), Some(7));
    let props = m2
        .extras
        .get("mtl:illum_props")
        .and_then(|v| v.as_object())
        .unwrap();
    assert_eq!(props.get("fresnel").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        props
            .get("transparency_refraction")
            .and_then(|v| v.as_bool()),
        Some(true),
    );
    assert_eq!(props.get("ray_trace").and_then(|v| v.as_bool()), Some(true),);
}
