//! `v x y z r g b` per-vertex colour extension (MeshLab / libigl /
//! Meshroom / OpenCV de-facto). Not in the original Wavefront spec but
//! flagged in `docs/3d/obj/README.md` as the canonical "widely used but
//! never standardised" `v`-line extension. This crate's decoder accepts
//! 3, 4, 6, or 7 floats on `v` (xyz, xyzw, xyzrgb, xyzwrgb) and the
//! encoder mirrors the original width on re-emit.

use oxideav_obj::obj;

const TRI_COLORED: &str = "\
# triangle with one red, one green, one blue vertex
v 0.0 0.0 0.0 1.0 0.0 0.0
v 1.0 0.0 0.0 0.0 1.0 0.0
v 0.0 1.0 0.0 0.0 0.0 1.0
f 1 2 3
";

#[test]
fn six_float_v_line_decodes_to_primitive_colors() {
    let scene = obj::parse_obj(TRI_COLORED).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.positions.len(), 3);
    assert_eq!(prim.colors.len(), 1, "one colour channel populated");
    assert_eq!(prim.colors[0].len(), 3);
    assert_eq!(prim.colors[0][0], [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(prim.colors[0][1], [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(prim.colors[0][2], [0.0, 0.0, 1.0, 1.0]);
    // The bitmap surfaces in extras so the encoder picks the same
    // 6-token form on re-emit.
    let present = prim
        .extras
        .get("obj:vertex_color_present")
        .expect("vertex_color_present recorded");
    let arr = present.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    for entry in arr {
        assert_eq!(entry.as_bool(), Some(true));
    }
}

#[test]
fn six_float_v_line_round_trips_with_rgb_preserved() {
    let scene = obj::parse_obj(TRI_COLORED).unwrap();
    let out = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&out).unwrap();
    // Each `v` line should carry the original 6-token form.
    let v_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("v ")).collect();
    assert_eq!(v_lines.len(), 3);
    for line in &v_lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 7, "expected `v x y z r g b`: {line:?}");
    }
    // Second-pass parse: structural fixed point.
    let scene2 = obj::parse_obj(text).unwrap();
    let prim2 = &scene2.meshes[0].primitives[0];
    assert_eq!(
        prim2.colors[0],
        [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        ]
    );
}

const TRI_WEIGHTED: &str = "\
v 0.0 0.0 0.0 1.0
v 1.0 0.0 0.0 0.5
v 0.0 1.0 0.0 2.0
f 1 2 3
";

#[test]
fn four_float_v_line_preserves_w_weight_through_round_trip() {
    let scene = obj::parse_obj(TRI_WEIGHTED).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    // No colour channel — only rational-weight metadata.
    assert!(prim.colors.is_empty());
    let weights = prim
        .extras
        .get("obj:vertex_weight")
        .expect("vertex_weight recorded")
        .as_array()
        .unwrap();
    assert_eq!(weights.len(), 3);
    assert_eq!(weights[0].as_f64(), Some(1.0));
    assert_eq!(weights[1].as_f64(), Some(0.5));
    assert_eq!(weights[2].as_f64(), Some(2.0));

    let out = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&out).unwrap();
    for line in text.lines().filter(|l| l.starts_with("v ")) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 5, "expected `v x y z w`: {line:?}");
    }
}

const TRI_WEIGHTED_AND_COLORED: &str = "\
v 0.0 0.0 0.0 1.0 1.0 0.0 0.0
v 1.0 0.0 0.0 0.5 0.0 1.0 0.0
v 0.0 1.0 0.0 2.0 0.0 0.0 1.0
f 1 2 3
";

#[test]
fn seven_float_v_line_carries_both_w_and_rgb() {
    let scene = obj::parse_obj(TRI_WEIGHTED_AND_COLORED).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.colors[0].len(), 3);
    assert_eq!(prim.colors[0][1], [0.0, 1.0, 0.0, 1.0]);
    let weights = prim.extras["obj:vertex_weight"].as_array().unwrap();
    assert_eq!(weights[1].as_f64(), Some(0.5));

    let out = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&out).unwrap();
    for line in text.lines().filter(|l| l.starts_with("v ")) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 8, "expected `v x y z w r g b`: {line:?}");
    }
}

#[test]
fn standard_three_token_v_line_stays_unchanged_on_round_trip() {
    let obj = "v 0.0 0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 2 3\n";
    let scene = obj::parse_obj(obj).unwrap();
    let prim = &scene.meshes[0].primitives[0];
    assert!(prim.colors.is_empty());
    assert!(!prim.extras.contains_key("obj:vertex_color_present"));
    assert!(!prim.extras.contains_key("obj:vertex_weight"));

    let out = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&out).unwrap();
    for line in text.lines().filter(|l| l.starts_with("v ")) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 4, "expected `v x y z`: {line:?}");
    }
}

#[test]
fn mixed_colored_and_uncolored_vertices_preserve_partition_on_re_emit() {
    // The first vertex carries colour, the second + third don't. We
    // expect the encoder to emit a 6-token `v` for the first vertex
    // and 3-token `v` for the others — i.e. no synthetic white
    // injected for vertices that didn't originally spell out colour.
    let obj = "\
v 0.0 0.0 0.0 1.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
    let scene = obj::parse_obj(obj).unwrap();
    let out = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&out).unwrap();
    let v_widths: Vec<usize> = text
        .lines()
        .filter(|l| l.starts_with("v "))
        .map(|l| l.split_whitespace().count())
        .collect();
    assert_eq!(v_widths, vec![7, 4, 4]);
}

#[test]
fn rejects_v_lines_with_five_tokens() {
    // Five floats is genuinely ambiguous (`xyzwR`? `xyzr` plus
    // typo?) so the loader refuses rather than silently mis-parses.
    let obj = "v 0.0 0.0 0.0 1.0 0.5\nf 1 1 1\n";
    let err = obj::parse_obj(obj).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("v:"),
        "want a `v:`-prefixed error, got {msg:?}"
    );
}
