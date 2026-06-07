//! Typed accessor for the `parm u …` / `parm v …` body statement.
//!
//! Coverage:
//!   * Spec §"parm u/v" + §"Free-form curve/surface body statements" —
//!     each `parm` line declares the **global parameters** (Bezier /
//!     Cardinal / Taylor / basis-matrix) or the **knot vector**
//!     (B-spline / NURBS) for one parametric direction of the
//!     surrounding free-form element. Surfaces can carry two `parm`
//!     lines per block (one `parm u`, one `parm v`); curves and
//!     trimming curves only ever write `parm u`.
//!
//! Parallel to the verbatim `obj:freeform_directives` channel, a
//! parse-time-only decomposition lands on `Scene3D::extras["obj:parms"]`
//! as an array of objects with the four stable keys
//! `direction` / `element_kind` / `cstype` / `values`. The encoder is
//! still driven by the verbatim channel; the typed view exists so
//! consumers don't have to walk the directive sequence pairing every
//! `parm` line with its enclosing `cstype` block + element kind.

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::{ObjDecoder, obj};

/// Spec §"B-spline surface" Examples §3 — cubic B-spline patch with
/// non-uniform knot vectors `parm u 0.0 0.0 0.0 1.0 1.0 1.0` and
/// `parm v 0.0 0.0 0.0 1.0 1.0 1.0`.
const BSPLINE_SURFACE_OBJ: &str = "\
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
cstype bspline
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 0.0 1.0 1.0
parm v 0.0 0.0 1.0 1.0
end
";

#[test]
fn parm_typed_view_pairs_two_directions_per_surface_block() {
    let scene = obj::parse_obj(BSPLINE_SURFACE_OBJ).unwrap();
    let parms = scene
        .extras
        .get("obj:parms")
        .expect("typed parm view present for a surface with parm u + parm v");
    let arr = parms.as_array().unwrap();
    assert_eq!(arr.len(), 2, "exactly two parm lines (u + v)");

    let u = arr[0].as_object().unwrap();
    assert_eq!(u["direction"].as_str(), Some("u"));
    assert_eq!(u["element_kind"].as_str(), Some("surf"));
    assert_eq!(u["cstype"].as_str(), Some("bspline"));
    let u_vals: Vec<f64> = u["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(u_vals, vec![0.0, 0.0, 1.0, 1.0]);

    let v = arr[1].as_object().unwrap();
    assert_eq!(v["direction"].as_str(), Some("v"));
    assert_eq!(v["element_kind"].as_str(), Some("surf"));
    assert_eq!(v["cstype"].as_str(), Some("bspline"));
    let v_vals: Vec<f64> = v["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(v_vals, vec![0.0, 0.0, 1.0, 1.0]);

    // All four keys present, nothing extra.
    for key in ["direction", "element_kind", "cstype", "values"] {
        assert!(u.contains_key(key), "missing typed key on u: {key}");
        assert!(v.contains_key(key), "missing typed key on v: {key}");
    }
    assert_eq!(u.len(), 4);
    assert_eq!(v.len(), 4);
}

#[test]
fn parm_typed_view_carries_bezier_global_parameters() {
    // Spec §"Bezier" Examples §2 — non-rational Bezier curve with the
    // `parm u 0.0 1.0 2.0 3.0 4.0` global-parameter break points.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
cstype bezier
deg 1
curv 0.0 4.0 1 2 3 4
parm u 0.0 1.0 2.0 3.0 4.0
end
";
    let scene = obj::parse_obj(src).unwrap();
    let arr = scene.extras.get("obj:parms").unwrap().as_array().unwrap();
    assert_eq!(arr.len(), 1, "Bezier curves only carry parm u");
    let entry = arr[0].as_object().unwrap();
    assert_eq!(entry["direction"].as_str(), Some("u"));
    assert_eq!(entry["element_kind"].as_str(), Some("curv"));
    assert_eq!(entry["cstype"].as_str(), Some("bezier"));
    let values: Vec<f64> = entry["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(values, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn parm_typed_view_classifies_curv2_inside_bezier_block() {
    // Spec §"Special point" Examples — `curv2` inside a `cstype rat
    // bezier` block; the typed view pairs the `parm u` with
    // element_kind = "curv2" and cstype = "rat_bezier".
    let src = "\
v 0 0 0
vp 0.0 0.0
vp 1.0 0.0
vp 0.5 0.5
cstype rat bezier
curv2 1 2 3
parm u 0.0 1.0 2.0
end
";
    let scene = obj::parse_obj(src).unwrap();
    let arr = scene.extras.get("obj:parms").unwrap().as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let entry = arr[0].as_object().unwrap();
    assert_eq!(entry["element_kind"].as_str(), Some("curv2"));
    assert_eq!(entry["cstype"].as_str(), Some("rat_bezier"));
}

#[test]
fn parm_typed_view_survives_round_trip_unchanged() {
    let scene = obj::parse_obj(BSPLINE_SURFACE_OBJ).unwrap();
    let typed_before = scene.extras.get("obj:parms").cloned().unwrap();

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let typed_after = scene2.extras.get("obj:parms").cloned().unwrap();

    assert_eq!(
        typed_before, typed_after,
        "typed parm view differs across decode → encode → decode cycle"
    );
}

#[test]
fn parm_typed_view_handles_multiple_blocks_in_source_order() {
    // Two surface blocks back-to-back; the typed array preserves
    // source order: block 1 (u, v), block 2 (u, v) — four entries
    // total. The element_kind / cstype slots track the most recent
    // header inside each block (verifying that `end` resets state and
    // a fresh `cstype` re-seeds it).
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
cstype bezier
deg 1 1
surf 0 1 0 1 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
cstype bspline
deg 1 1
surf 0 1 0 1 1 2 3 4
parm u 0.0 0.0 1.0 1.0
parm v 0.0 0.0 1.0 1.0
end
";
    let scene = obj::parse_obj(src).unwrap();
    let arr = scene.extras.get("obj:parms").unwrap().as_array().unwrap();
    assert_eq!(arr.len(), 4);
    // Block 1 — Bezier
    assert_eq!(
        arr[0].as_object().unwrap()["cstype"].as_str(),
        Some("bezier")
    );
    assert_eq!(arr[0].as_object().unwrap()["direction"].as_str(), Some("u"));
    assert_eq!(
        arr[1].as_object().unwrap()["cstype"].as_str(),
        Some("bezier")
    );
    assert_eq!(arr[1].as_object().unwrap()["direction"].as_str(), Some("v"));
    // Block 2 — B-spline
    assert_eq!(
        arr[2].as_object().unwrap()["cstype"].as_str(),
        Some("bspline")
    );
    assert_eq!(arr[2].as_object().unwrap()["direction"].as_str(), Some("u"));
    assert_eq!(
        arr[3].as_object().unwrap()["cstype"].as_str(),
        Some("bspline")
    );
    assert_eq!(arr[3].as_object().unwrap()["direction"].as_str(), Some("v"));
}

#[test]
fn parm_typed_view_drops_unknown_direction_token() {
    // Spec §"parm u/v" defines exactly two direction tokens (`u` /
    // `v`). A `parm` line with anything else (`parm w …`, `parm 0 …`)
    // drops from the typed view but still rides the verbatim channel
    // for byte-faithful round-trip.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
cstype bezier
deg 1 1
surf 0 1 0 1 1 2 3 4
parm u 0.0 1.0
parm w 0.0 1.0
end
";
    let scene = obj::parse_obj(src).unwrap();
    let typed = scene.extras.get("obj:parms").unwrap().as_array().unwrap();
    assert_eq!(
        typed.len(),
        1,
        "only the parm u line survives the typed view"
    );
    assert_eq!(
        typed[0].as_object().unwrap()["direction"].as_str(),
        Some("u")
    );
}

#[test]
fn parm_typed_view_drops_lines_outside_an_element() {
    // A `parm` line that sits inside a `cstype` block but BEFORE any
    // `curv` / `curv2` / `surf` directive has no element to anchor to.
    // The typed view drops it; the verbatim channel still replays it.
    let src = "\
cstype bezier
deg 1
parm u 0.0 1.0
v 0 0 0
v 1 0 0
curv 0.0 1.0 1 2
parm u 0.0 1.0
end
";
    let scene = obj::parse_obj(src).unwrap();
    let typed = scene.extras.get("obj:parms").unwrap().as_array().unwrap();
    assert_eq!(typed.len(), 1, "only the parm AFTER curv survives");
    assert_eq!(
        typed[0].as_object().unwrap()["element_kind"].as_str(),
        Some("curv")
    );
}

#[test]
fn parm_typed_view_absent_when_no_parm_lines() {
    // No free-form section at all → no `obj:parms` key on the scene.
    let src = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let scene = obj::parse_obj(src).unwrap();
    assert!(!scene.extras.contains_key("obj:parms"));
}

#[test]
fn parm_typed_view_drops_non_numeric_tokens_within_a_line() {
    // Spec §"parm" says values are floats; a non-numeric token inside a
    // `parm` line is dropped from the typed `values` array (mirrors the
    // lenient-on-malformed policy of the existing sp / con typed views).
    // The verbatim channel still replays the original token sequence.
    let src = "\
v 0 0 0
v 1 0 0
cstype bezier
deg 1
curv 0.0 1.0 1 2
parm u 0.0 garbage 1.0
end
";
    let scene = obj::parse_obj(src).unwrap();
    let typed = scene.extras.get("obj:parms").unwrap().as_array().unwrap();
    assert_eq!(typed.len(), 1);
    let values: Vec<f64> = typed[0].as_object().unwrap()["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(values, vec![0.0, 1.0], "non-numeric token dropped");

    // Verbatim channel preserved the original line.
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("parm u 0.0 garbage 1.0"));
}

#[test]
fn parm_typed_view_marks_unknown_cstype_when_block_type_unrecognised() {
    // A `cstype` whose type slug isn't one of the recognised values
    // (`bezier` / `bspline` / `cardinal` / `taylor` / `bmatrix`) leaves
    // the typed view's `cstype` slot as `"unknown"`. The line still
    // surfaces (the consumer might still want the parsed values).
    let src = "\
v 0 0 0
v 1 0 0
cstype mystery
deg 1
curv 0.0 1.0 1 2
parm u 0.0 1.0
end
";
    let scene = obj::parse_obj(src).unwrap();
    let typed = scene.extras.get("obj:parms").unwrap().as_array().unwrap();
    assert_eq!(typed.len(), 1);
    assert_eq!(
        typed[0].as_object().unwrap()["cstype"].as_str(),
        Some("unknown")
    );
}
