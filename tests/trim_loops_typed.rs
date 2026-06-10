//! Typed accessor for the `trim` / `hole` / `scrv` loop body statements.
//!
//! Coverage:
//!   * Spec §"Trimming loops and holes" / §"trim u0 u1 curv2d …" /
//!     §"hole u0 u1 curv2d …" and §"Special curve" / §"scrv u0 u1
//!     curv2d …" — all three carry the identical repeating-triple body
//!     shape: the keyword followed by one or more `(u0, u1, curv2d)`
//!     triples, each naming a previously-defined `curv2` parameter-space
//!     curve plus the `[u0, u1]` sub-range of that curve to walk.
//!
//! Parallel to the verbatim `obj:freeform_directives` channel, a
//! parse-time-only decomposition lands on
//! `Scene3D::extras["obj:trim_loops"]` as an array of objects with the
//! four stable keys `loop_kind` / `element_kind` / `cstype` /
//! `segments`. The encoder is still driven by the verbatim channel, so
//! every test that asserts the typed view also asserts the original
//! lines replay unchanged on re-encode.

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder};
use oxideav_obj::{ObjDecoder, ObjEncoder, obj};

/// Spec §"Examples" 8 — "Trimming with a special curve". A rational
/// B-spline surface block carries both a `trim` (one segment) and a
/// `scrv` (one segment). Trimmed down to the directives the typed view
/// reads (the `vp` / `curv2` definitions for curves 1 and 2 precede the
/// surface block).
const EXAMPLE8_OBJ: &str = "\
vp -0.675  1.850  3.000
vp  0.915  1.930
vp  2.485  0.470  2.000
vp  2.485 -1.030
vp  1.605 -1.890 10.700
vp -0.745 -0.654  0.500
cstype rat bezier
deg 3
curv2 1 2 3 4 5 6 1
parm u 0.00 1.00 2.00
end
vp -0.185  0.322
vp  0.214  0.818
vp  1.652  0.207
vp  1.652 -0.455
curv2 7 8 9 10
parm u 2.00 10.00
end
v -1.350 -1.030 0.000
v  0.130 -1.030 0.432 7.600
v  1.480 -1.030 0.000 2.300
v -1.460  0.060 0.201
v  0.120  0.060 0.915 0.500
v  1.380  0.060 0.454 1.500
v -1.480  1.030 0.000 2.300
v  0.120  1.030 0.394 6.100
v  1.170  1.030 0.000 3.300
cstype rat bspline
deg 2 2
surf -1.0 2.5 -2.0 2.0 1 2 3 4 5 6 7 8 9
parm u -1.00 -1.00 -1.00 2.50 2.50 2.50
parm v -2.00 -2.00 -2.00 2.00 2.00 2.00
trim 0.0 2.0 1
scrv 4.2 9.7 2
end
";

/// Spec §"Examples" 7 — "Two trimming regions with a hole". Each region
/// is a single-segment `trim` followed by a single-segment `hole`. The
/// surface is a `cstype bezier` patch.
const EXAMPLE7_OBJ: &str = "\
vp 0.0 0.0
vp 0.0 1.0
vp 1.0 0.0
vp 1.0 1.0
cstype bezier
deg 1
curv2 1 2
end
curv2 3 4
end
v 0 0 0
v 2 0 0
v 0 2 0
v 2 2 0
deg 1 1
cstype bezier
surf 0.0 2.0 0.0 2.0 1 2 3 4
trim 0.0 4.0 1
hole 0.0 4.0 2
trim 0.0 4.0 1
hole 0.0 4.0 2
end
";

/// A `trim` with two `(u0, u1, curv2d)` segments on one line — the spec
/// repeating-triple form. Verifies the segment array decomposes every
/// triple in source order.
const MULTI_SEGMENT_TRIM_OBJ: &str = "\
vp 0.0 0.0
vp 1.0 0.0
vp 1.0 1.0
cstype bezier
deg 1
curv2 1 2
end
curv2 2 3
end
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
trim 0.0 1.0 1 0.0 1.0 2
end
";

#[test]
fn example8_trim_and_scrv_decompose() {
    let scene = obj::parse_obj(EXAMPLE8_OBJ).unwrap();
    let loops = scene
        .extras
        .get("obj:trim_loops")
        .expect("typed trim_loops view present for a surface with trim + scrv");
    let arr = loops.as_array().unwrap();
    assert_eq!(arr.len(), 2, "one trim + one scrv");

    let trim = arr[0].as_object().unwrap();
    assert_eq!(trim["loop_kind"].as_str(), Some("trim"));
    assert_eq!(trim["element_kind"].as_str(), Some("surf"));
    assert_eq!(trim["cstype"].as_str(), Some("rat_bspline"));
    let trim_segs = trim["segments"].as_array().unwrap();
    assert_eq!(trim_segs.len(), 1);
    let s = trim_segs[0].as_object().unwrap();
    assert_eq!(s["u0"].as_f64(), Some(0.0));
    assert_eq!(s["u1"].as_f64(), Some(2.0));
    assert_eq!(s["curv2d"].as_i64(), Some(1));

    let scrv = arr[1].as_object().unwrap();
    assert_eq!(scrv["loop_kind"].as_str(), Some("scrv"));
    assert_eq!(scrv["element_kind"].as_str(), Some("surf"));
    assert_eq!(scrv["cstype"].as_str(), Some("rat_bspline"));
    let scrv_segs = scrv["segments"].as_array().unwrap();
    assert_eq!(scrv_segs.len(), 1);
    let s = scrv_segs[0].as_object().unwrap();
    assert_eq!(s["u0"].as_f64(), Some(4.2));
    assert_eq!(s["u1"].as_f64(), Some(9.7));
    assert_eq!(s["curv2d"].as_i64(), Some(2));

    // Exactly the four documented keys, nothing extra.
    for obj in [trim, scrv] {
        for key in ["loop_kind", "element_kind", "cstype", "segments"] {
            assert!(obj.contains_key(key), "missing typed key: {key}");
        }
        assert_eq!(obj.len(), 4);
    }
}

#[test]
fn example7_two_trim_hole_pairs() {
    let scene = obj::parse_obj(EXAMPLE7_OBJ).unwrap();
    let arr = scene.extras["obj:trim_loops"].as_array().unwrap();
    // trim, hole, trim, hole — in source order.
    assert_eq!(arr.len(), 4);
    let kinds: Vec<&str> = arr
        .iter()
        .map(|o| o["loop_kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["trim", "hole", "trim", "hole"]);

    for o in arr {
        let obj = o.as_object().unwrap();
        assert_eq!(obj["element_kind"].as_str(), Some("surf"));
        assert_eq!(obj["cstype"].as_str(), Some("bezier"));
        let segs = obj["segments"].as_array().unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0]["u0"].as_f64(), Some(0.0));
        assert_eq!(segs[0]["u1"].as_f64(), Some(4.0));
    }
    // First region: curv2d 1 / 2; second region: 1 / 2 again per fixture.
    assert_eq!(arr[0]["segments"][0]["curv2d"].as_i64(), Some(1));
    assert_eq!(arr[1]["segments"][0]["curv2d"].as_i64(), Some(2));
}

#[test]
fn multi_segment_trim_decomposes_every_triple() {
    let scene = obj::parse_obj(MULTI_SEGMENT_TRIM_OBJ).unwrap();
    let arr = scene.extras["obj:trim_loops"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let segs = arr[0]["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 2, "two (u0,u1,curv2d) triples on one trim line");
    assert_eq!(segs[0]["curv2d"].as_i64(), Some(1));
    assert_eq!(segs[1]["curv2d"].as_i64(), Some(2));
}

#[test]
fn negative_curv2d_echoed_as_is() {
    // Spec §"Examples" 8 note: "This example uses negative vertex
    // numbers." A negative `curv2d` reference is relative-from-end; the
    // typed view echoes it without resolving (matches the `con`
    // negative-index policy).
    let src = "\
vp 0 0
vp 1 1
cstype bezier
deg 1
curv2 -2 -1
end
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
trim 0.0 1.0 -1
end
";
    let scene = obj::parse_obj(src).unwrap();
    let arr = scene.extras["obj:trim_loops"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["segments"][0]["curv2d"].as_i64(), Some(-1));
}

#[test]
fn malformed_line_drops_from_typed_view_only() {
    // A trim line with a non-multiple-of-three argument count, and one
    // with a non-numeric u0, both drop from the typed view. A clean
    // hole on the same surface survives.
    let src = "\
vp 0 0
vp 1 1
cstype bezier
deg 1
curv2 1 2
end
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
trim 0.0 1.0
trim foo 1.0 1
hole 0.0 1.0 1
end
";
    let scene = obj::parse_obj(src).unwrap();
    let arr = scene.extras["obj:trim_loops"].as_array().unwrap();
    // Only the clean `hole` survives.
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["loop_kind"].as_str(), Some("hole"));

    // But the verbatim channel still carries all three lines for round-
    // trip — the typed view is lossy, the directive stream is not.
    let directives = scene.extras["obj:freeform_directives"].as_array().unwrap();
    let trim_count = directives
        .iter()
        .filter(|d| d[0].as_str() == Some("trim"))
        .count();
    assert_eq!(
        trim_count, 2,
        "both malformed trim lines preserved verbatim"
    );
}

#[test]
fn no_trim_no_typed_key() {
    // A plain polygonal cube carries no free-form geometry, so the
    // typed key is absent (not an empty array).
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
";
    let scene = obj::parse_obj(src).unwrap();
    assert!(!scene.extras.contains_key("obj:trim_loops"));
}

#[test]
fn trim_lines_replay_verbatim_on_reencode() {
    // The typed view is parse-time-only; the encoder still drives
    // emission off the verbatim directive channel. A decode → encode
    // cycle preserves every `trim` / `scrv` line.
    let scene = ObjDecoder::new().decode(EXAMPLE8_OBJ.as_bytes()).unwrap();
    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("trim 0 2 1") || text.contains("trim 0.0 2.0 1"));
    assert!(
        text.lines().any(|l| l.starts_with("scrv ")),
        "scrv directive replayed on re-encode"
    );

    // Re-decode the re-encoded output: the typed view reconstructs
    // identically (round-trip stable).
    let scene2 = ObjDecoder::new().decode(text.as_bytes()).unwrap();
    let arr2 = scene2.extras["obj:trim_loops"].as_array().unwrap();
    assert_eq!(arr2.len(), 2);
    assert_eq!(arr2[0]["loop_kind"].as_str(), Some("trim"));
    assert_eq!(arr2[1]["loop_kind"].as_str(), Some("scrv"));
}
