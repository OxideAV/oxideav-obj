//! Wavefront free-form geometry directives — `vp`, `cstype`, `deg`,
//! `curv`, `surf`, `parm`, `trim`, `hole`, `scrv`, `sp`, `end`, `bzp`
//! (and the older `bsp` / `cdc` / `cdp` / `res` superseded forms).
//!
//! Coverage: spec §"vp u v w", §"Specifying free-form curves/surfaces"
//! (`cstype`, `deg`), §"Free-form curve/surface body statements"
//! (`parm`, `trim`, `hole`, `scrv`, `sp`, `end`), §"Elements" (`curv`,
//! `surf`), §"Superseded statements" (`bzp` / `bsp` / `cdc` / `cdp` /
//! `res`).
//!
//! Round-trip strategy: the parser captures every free-form directive
//! verbatim into `Scene3D::extras["obj:freeform_directives"]` as a
//! sequence of `[keyword, arg1, arg2, …]` arrays, and the parameter-
//! space vertex pool into `Scene3D::extras["obj:vp"]` as a list of
//! `[u, v, w]` triples (1-based numbering parallel to `v` / `vt` /
//! `vn`). The encoder replays both verbatim — `vp` lines after `v`
//! and the directive sequence after the polygonal section.

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder};
use oxideav_obj::{ObjDecoder, ObjEncoder, obj};

/// Bezier curve example lifted from the spec §"Free-form curve/surface
/// body statements", "Examples / Bezier curve" (truncated to 5 control
/// points so the test stays compact).
const BEZIER_CURVE_OBJ: &str = "\
v -2.300000 1.950000 0.000000
v -2.200000 0.790000 0.000000
v -2.340000 -1.510000 0.000000
v -1.530000 -1.490000 0.000000
v -0.720000 -1.470000 0.000000
cstype bezier
deg 3
curv 0.0 1.0 1 2 3 4 5
parm u 0.0 1.0
end
";

#[test]
fn cstype_deg_curv_parm_end_round_trip() {
    let scene = obj::parse_obj(BEZIER_CURVE_OBJ).unwrap();
    // Polygonal data: nothing — five `v` lines but no `f` / `l` / `p`.
    assert!(scene.meshes.is_empty(), "no polygonal elements expected");
    // Free-form directives captured verbatim.
    let directives = scene
        .extras
        .get("obj:freeform_directives")
        .expect("captured");
    let arr = directives.as_array().unwrap();
    assert_eq!(arr.len(), 5, "expected 5 directive lines, got {arr:?}");
    assert_eq!(arr[0][0], "cstype");
    assert_eq!(arr[0][1], "bezier");
    assert_eq!(arr[1][0], "deg");
    assert_eq!(arr[1][1], "3");
    assert_eq!(arr[2][0], "curv");
    assert_eq!(arr[2].as_array().unwrap().len(), 1 + 7); // keyword + u0 u1 + 5 indices
    assert_eq!(arr[3][0], "parm");
    assert_eq!(arr[3][1], "u");
    assert_eq!(arr[4][0], "end");

    // Encoder replays the directives.
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    for keyword in ["cstype bezier", "deg 3", "curv 0", "parm u", "end"] {
        assert!(
            text.lines().any(|l| l.starts_with(keyword)),
            "missing `{keyword}` line in:\n{text}"
        );
    }

    // Round-trip stability: re-decoding the encoder output yields the
    // same directive sequence.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let directives2 = scene2.extras.get("obj:freeform_directives").unwrap();
    assert_eq!(directives, directives2, "round-trip not stable");
}

/// Trimmed B-spline surface example from spec §"Free-form curve/
/// surface body statements", "knot vector / trimming loop" sample.
const TRIMMED_SURFACE_OBJ: &str = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 2 0 0
v 2 1 0
v 2 2 0
v 0 2 0
v 1 2 0
vp 0.0 0.0
vp 1.0 0.0
vp 1.0 1.0
vp 0.0 1.0
cstype rat bspline
deg 2 2
surf -1.0 2.5 -2.0 2.0 -9 -8 -7 -6 -5 -4 -3 -2 -1
parm u -1.00 -1.00 -1.00 2.50 2.50 2.50
parm v -2.00 -2.00 -2.00 -2.00 -2.00 -2.00
trim 0.0 2.0 1
end
";

#[test]
fn surf_with_parm_trim_end_and_vp_pool_round_trip() {
    let scene = obj::parse_obj(TRIMMED_SURFACE_OBJ).unwrap();

    // Parameter-space vertex pool lands on the scene.
    let vp = scene.extras.get("obj:vp").unwrap().as_array().unwrap();
    assert_eq!(vp.len(), 4);
    // Encoder skips the trailing-zero `w` so `vp 0.0 0.0` round-trips
    // as a 2D point, not the spec's 3-coordinate "rational trimming
    // curve" form (which would force the operator to type a non-zero
    // `w`).
    let first = vp[0].as_array().unwrap();
    assert_eq!(first.len(), 3, "stored as 3-tuple internally");

    // Free-form directive sequence captured.
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(dirs.len(), 7, "cstype + deg + surf + 2x parm + trim + end");
    assert_eq!(dirs[2][0], "surf");
    assert_eq!(dirs[3][0], "parm");
    assert_eq!(dirs[4][0], "parm");
    assert_eq!(dirs[5][0], "trim");

    // Encoder emits `vp` after the `v` block.
    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let v_lines: Vec<usize> = text
        .lines()
        .enumerate()
        .filter_map(|(i, l)| if l.starts_with("v ") { Some(i) } else { None })
        .collect();
    let vp_lines: Vec<usize> = text
        .lines()
        .enumerate()
        .filter_map(|(i, l)| if l.starts_with("vp ") { Some(i) } else { None })
        .collect();
    assert_eq!(vp_lines.len(), 4, "all 4 vp lines emitted");
    assert!(
        v_lines.iter().max() < vp_lines.iter().min(),
        "vp must follow v in:\n{text}"
    );
    // surf / parm / trim / end appear after the vp block.
    assert!(text.contains("\nsurf "), "surf line missing");
    assert!(text.contains("\nparm u "), "parm u line missing");
    assert!(text.contains("\nparm v "), "parm v line missing");
    assert!(text.contains("\ntrim 0"), "trim line missing");
    assert!(text.contains("\nend"), "end line missing");
}

#[test]
fn vp_lines_truncate_trailing_zero_components() {
    // 1D / 2D / 3D `vp` lines per spec §"vp u v w".
    let text = "\
vp 0.5
vp 0.5 0.25
vp 0.5 0.25 1.0
";
    let scene = obj::parse_obj(text).unwrap();
    let vp = scene.extras.get("obj:vp").unwrap().as_array().unwrap();
    assert_eq!(vp.len(), 3);
    assert_eq!(vp[0][0], 0.5);
    assert_eq!(vp[0][1], 0.0);
    assert_eq!(vp[0][2], 0.0);
    assert_eq!(vp[1][1], 0.25);
    assert_eq!(vp[1][2], 0.0);
    assert_eq!(vp[2][2], 1.0);

    // Encoder emits exactly the right number of components per line.
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let txt = std::str::from_utf8(&bytes).unwrap();
    let vp_lines: Vec<&str> = txt.lines().filter(|l| l.starts_with("vp ")).collect();
    assert_eq!(vp_lines.len(), 3);
    assert_eq!(vp_lines[0].split_whitespace().count(), 2, "1D vp");
    assert_eq!(vp_lines[1].split_whitespace().count(), 3, "2D vp");
    assert_eq!(vp_lines[2].split_whitespace().count(), 4, "3D vp");
}

#[test]
fn freeform_with_polygonal_data_round_trips_both() {
    // Mixing a free-form curve with a polygonal triangle — both
    // sections survive the round trip and the polygonal section
    // doesn't lose its `f` line just because free-form geometry is
    // present.
    let text = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
f 1 2 3
cstype bezier
deg 3
curv 0.0 1.0 1 2 3 4
parm u 0.0 1.0
end
";
    let scene = obj::parse_obj(text).unwrap();
    // Polygonal: one mesh with one Triangles primitive containing one
    // 3-vertex face (after fan triangulation).
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.meshes[0].primitives.len(), 1);
    let prim = &scene.meshes[0].primitives[0];
    assert_eq!(prim.indices.as_ref().unwrap().len(), 3);
    // Free-form directives present.
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(dirs.len(), 5);

    // Encoder layout: `v` lines first, then `f`, then the free-form
    // section. Matters because the spec requires references in `curv`
    // to resolve against the active vertex pool at point of reading,
    // not after.
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let txt = std::str::from_utf8(&bytes).unwrap();
    let f_pos = txt.find("\nf ").expect("face line");
    let curv_pos = txt.find("\ncurv ").expect("curv line");
    assert!(f_pos < curv_pos, "polygonal must precede free-form\n{txt}");

    // Round-trip stability.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let dirs2 = scene2
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(dirs.len(), dirs2.len());
    for (a, b) in dirs.iter().zip(dirs2.iter()) {
        assert_eq!(a, b);
    }
}

/// Spec §"Free-form curve/surface body statements", §"trim" / §"hole"
/// — a surface with one outer trim loop and one hole.
const HOLE_SURFACE_OBJ: &str = "\
v 0 0 0
v 2 0 0
v 2 2 0
v 0 2 0
vp 0.0 0.0
vp 1.0 0.0
vp 0.5 0.5
cstype bezier
deg 1 1
surf 0.0 2.0 0.0 2.0 1 2 3 4
parm u 0.00 2.00
parm v 0.00 2.00
trim 0.0 4.0 1
hole 0.0 4.0 2
end
";

#[test]
fn trim_and_hole_round_trip() {
    let scene = obj::parse_obj(HOLE_SURFACE_OBJ).unwrap();
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    let kinds: Vec<&str> = dirs.iter().map(|e| e[0].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        vec![
            "cstype", "deg", "surf", "parm", "parm", "trim", "hole", "end"
        ]
    );

    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let txt = std::str::from_utf8(&bytes).unwrap();
    assert!(txt.contains("\ntrim 0"), "trim missing");
    assert!(txt.contains("\nhole 0"), "hole missing");
}

#[test]
fn scrv_and_sp_body_statements_captured() {
    // Special-curve and special-point body statements per spec
    // §"Free-form curve/surface body statements / scrv" / "sp".
    // Vertex/parameter-vertex pools are kept minimal.
    let text = "\
v 0 0 0
v 1 0 0
vp 0.5 0.5
vp 0.25 0.75
cstype rat bezier
curv2 -2 -1 -2
parm u 0.00 1.00 2.00
sp 1 2
scrv 0.0 1.0 1
end
";
    let scene = obj::parse_obj(text).unwrap();
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    let kinds: Vec<&str> = dirs.iter().map(|e| e[0].as_str().unwrap()).collect();
    assert!(kinds.contains(&"curv2"));
    assert!(kinds.contains(&"sp"));
    assert!(kinds.contains(&"scrv"));
    assert!(kinds.contains(&"end"));

    // Encode + re-decode = same directive list.
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let scene2 = obj::parse_obj(std::str::from_utf8(&bytes).unwrap()).unwrap();
    assert_eq!(
        scene.extras.get("obj:freeform_directives"),
        scene2.extras.get("obj:freeform_directives")
    );
}

#[test]
fn bzp_and_bsp_superseded_keywords_captured() {
    // Spec §"Superseded statements" — `bzp` / `bsp` Bezier / B-spline
    // patches with sixteen control points each. We accept them in
    // input (the spec calls these "this release is the last release
    // that will read these") and round-trip them verbatim.
    let mut text = String::from(
        "v 0 0 0\nv 1 0 0\nv 2 0 0\nv 3 0 0\n\
v 0 1 0\nv 1 1 0\nv 2 1 0\nv 3 1 0\n\
v 0 2 0\nv 1 2 0\nv 2 2 0\nv 3 2 0\n\
v 0 3 0\nv 1 3 0\nv 2 3 0\nv 3 3 0\n",
    );
    text.push_str("bzp 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16\n");
    text.push_str("bsp 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16\n");

    let scene = obj::parse_obj(&text).unwrap();
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(dirs.len(), 2);
    assert_eq!(dirs[0][0], "bzp");
    assert_eq!(dirs[0].as_array().unwrap().len(), 1 + 16);
    assert_eq!(dirs[1][0], "bsp");
    assert_eq!(dirs[1].as_array().unwrap().len(), 1 + 16);

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let txt = std::str::from_utf8(&bytes).unwrap();
    assert!(txt.contains("\nbzp 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16"));
    assert!(txt.contains("\nbsp 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16"));
}

#[test]
fn cdc_cdp_res_superseded_keywords_captured() {
    // Spec §"Superseded statements" — `cdc` (Cardinal curve, ≥4 control
    // points), `cdp` (Cardinal patch, 16 control points), and `res`
    // (reference/display segment-count statement). Like `bzp` / `bsp`,
    // the spec marks these read-only ("This release is the last release
    // that will read these statements"), so we accept them in input and
    // round-trip them verbatim rather than silently dropping them.
    //
    // The `cdc 1 2 3 4 5 6` line is the spec §"Comparison of 2.11 and
    // 3.0 syntax", "Cardinal curve" worked example; `res useg vseg`
    // carries the two segment counts.
    let mut text = String::from(
        "v 2.570000 1.280000 0.000000\n\
v 0.940000 1.340000 0.000000\n\
v -0.670000 0.820000 0.000000\n\
v -0.770000 -0.940000 0.000000\n\
v 1.030000 -1.350000 0.000000\n\
v 3.070000 -1.310000 0.000000\n",
    );
    // 16 more positions so the `cdp` indices resolve to real vertices.
    for i in 0..16 {
        text.push_str(&format!("v {i}.0 0.0 0.0\n"));
    }
    text.push_str("res 4 4\n");
    text.push_str("cdc 1 2 3 4 5 6\n");
    text.push_str("cdp 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22\n");

    let scene = obj::parse_obj(&text).unwrap();
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(dirs.len(), 3, "expected res + cdc + cdp captured");
    assert_eq!(dirs[0][0], "res");
    assert_eq!(dirs[0].as_array().unwrap().len(), 1 + 2); // res useg vseg
    assert_eq!(dirs[1][0], "cdc");
    assert_eq!(dirs[1].as_array().unwrap().len(), 1 + 6); // cdc + 6 indices
    assert_eq!(dirs[2][0], "cdp");
    assert_eq!(dirs[2].as_array().unwrap().len(), 1 + 16); // cdp + 16 indices

    // `cdc` / `cdp` reference vertex positions by index, so the position
    // pool must survive the round-trip even though no polygonal element
    // consumes it.
    assert!(
        scene.extras.contains_key("obj:positions"),
        "cdc/cdp position pool must round-trip"
    );

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let txt = std::str::from_utf8(&bytes).unwrap();
    assert!(txt.contains("\nres 4 4"));
    assert!(txt.contains("\ncdc 1 2 3 4 5 6"));
    assert!(txt.contains("\ncdp 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22"));

    // Round-trip stability: re-decoding yields the same directive set.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let dirs2 = scene2.extras.get("obj:freeform_directives").unwrap();
    assert_eq!(
        scene.extras.get("obj:freeform_directives").unwrap(),
        dirs2,
        "cdc/cdp/res round-trip not stable"
    );
}

#[test]
fn no_freeform_means_no_freeform_extras() {
    let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let scene = obj::parse_obj(text).unwrap();
    assert!(!scene.extras.contains_key("obj:vp"));
    assert!(!scene.extras.contains_key("obj:freeform_directives"));
}

#[test]
fn cstype_rat_modifier_preserved() {
    // `cstype rat <type>` — the optional `rat` token before the type
    // selects the rational form; round-trip must keep both tokens.
    let text = "\
cstype rat bspline
deg 2 2
end
";
    let scene = obj::parse_obj(text).unwrap();
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(dirs[0][0], "cstype");
    assert_eq!(dirs[0][1], "rat");
    assert_eq!(dirs[0][2], "bspline");
    assert_eq!(dirs[1][0], "deg");
    assert_eq!(dirs[1].as_array().unwrap().len(), 3); // deg degu degv

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let txt = std::str::from_utf8(&bytes).unwrap();
    assert!(txt.contains("\ncstype rat bspline"));
    assert!(txt.contains("\ndeg 2 2"));
}
