//! Special-point (`sp`) typed accessor + synthetic-primitive pass —
//! spec §"Special point", §"sp vp1 vp …".
//!
//! `sp` is a free-form-geometry body statement that lists one or more
//! 1-based references into the `vp` parameter-vertex pool. The
//! enclosing element kind (`curv` / `curv2` / `surf`) decides how many
//! of each referenced `vp`'s components are meaningful — spec §"Special
//! point":
//!
//! - "For space curves and trimming curves, the parameter vertices
//!   must be 1D" → the `u` component of each `vp` is meaningful.
//! - "For surfaces, the parameter vertices must be 2D" → both `u` and
//!   `v` are meaningful.
//! - "A special point on a trimming curve is essentially the same as a
//!   special point on the surface it trims" → `curv2`-enclosed special
//!   points additionally surface the `v` component.
//!
//! Round 246 lands two parse-time-only surfaces on top of the existing
//! verbatim `obj:freeform_directives` channel:
//!
//! 1. `Scene3D::extras["obj:special_points"]` — a typed array of
//!    objects with the stable keys `element_kind`, `vp_index_1based`,
//!    `u`, `v`, in source order.
//!
//! 2. A synthetic `Topology::Points` primitive on a new
//!    `"obj:sps"` mesh, one per `sp` directive, with per-primitive
//!    provenance extras (`obj:special_point` marker, the shared
//!    `obj:tessellated_curve` encoder-filter sentinel, the resolved
//!    `obj:special_point_vp_refs` array, and the resolved
//!    `obj:special_point_element_kind` string).
//!
//! Verbatim round-trip through `obj:freeform_directives` is unchanged:
//! the encoder filters the synthetic mesh out via the shared
//! `obj:tessellated_curve` sentinel and replays the original `sp …`
//! line from the directive array.

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder};

/// Spec §"Special point" Example: rational Bezier 2D trimming curve
/// with two special points (`sp 2 3`) at vp 2 and vp 3.
const SP_TRIMMING_CURVE_EXAMPLE: &str = "\
vp -0.675  1.850  3.000
vp  0.915  1.930
vp  2.485  0.470  2.000
vp  2.485 -1.030
vp  1.605 -1.890 10.700
vp -0.745 -0.654  0.500
cstype rat bezier
curv2 -6 -5 -4 -3 -2 -1 -6
parm u 0.00 1.00 2.00
sp 2 3
end
";

/// Spec §"Example 9: Trimming with special points" — a space curve
/// (`curv`) with one special point (`sp 1`) plus a trimming curve
/// (`curv2`) with two (`sp 2 3`) and a surface (`surf`) with one
/// (`sp 4`). Three element kinds, three different `vp`-resolution
/// rules.
const SP_EXAMPLE_9: &str = "\
vp 0.500
vp 0.700
vp 1.100
vp 0.200 0.950
v  0.300 1.500 0.100
v  0.000  0.000  0.000
v  1.000  1.000  0.000
v  2.000  1.000  0.000
v  3.000  0.000  0.000
cstype bezier
deg 3
curv 0.2 0.9 -4 -3 -2 -1
sp 1
parm u 0.00 1.00
end
vp -0.675  1.850  3.000
vp  0.915  1.930
vp  2.485  0.470  2.000
vp  2.485 -1.030
vp  1.605 -1.890 10.700
vp -0.745 -0.654  0.500
cstype rat bezier
curv2 -6 -5 -4 -3 -2 -1 -6
parm u 0.00 1.00 2.00
sp 2 3
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
surf -1.0 2.5 -2.0 2.0 -9 -8 -7 -6 -5 -4 -3 -2 -1
parm u -1.00 -1.00 -1.00 2.50 2.50 2.50
parm v -2.00 -2.00 -2.00 2.00 2.00 2.00
trim 0.0 2.0 1
sp 4
end
";

fn typed_sp_array(scene: &oxideav_mesh3d::Scene3D) -> &Vec<serde_json::Value> {
    let v = scene
        .extras
        .get("obj:special_points")
        .expect("obj:special_points typed view must be present");
    match v {
        serde_json::Value::Array(arr) => arr,
        other => panic!("obj:special_points must be an Array, got {other:?}"),
    }
}

fn sp_mesh(scene: &oxideav_mesh3d::Scene3D) -> Option<&oxideav_mesh3d::Mesh> {
    scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:sps"))
}

fn decode(src: &str, samples: u32) -> oxideav_mesh3d::Scene3D {
    let mut dec = ObjDecoder::new().with_curve_tessellation(samples);
    dec.decode(src.as_bytes()).expect("decode must succeed")
}

#[test]
fn sp_trimming_curve_example_typed_view_present() {
    // The spec example has one `sp 2 3` line inside a `curv2` (rat bezier)
    // block referencing vp 2 and vp 3. The trimming-curve rule surfaces
    // both `u` and `v` (spec: "essentially the same as a special point
    // on the surface it trims").
    let scene = decode(SP_TRIMMING_CURVE_EXAMPLE, 0);
    let arr = typed_sp_array(&scene);
    assert_eq!(arr.len(), 2, "two resolved special points expected");

    // First special point: vp 2 → (0.915, 1.930, _) — w discarded.
    let p0 = &arr[0];
    assert_eq!(p0["element_kind"], serde_json::json!("curv2"));
    assert_eq!(p0["vp_index_1based"], serde_json::json!(2));
    let u0 = p0["u"].as_f64().unwrap();
    let v0 = p0["v"].as_f64().unwrap();
    assert!((u0 - 0.915).abs() < 1e-3, "u0 = {u0}");
    assert!((v0 - 1.930).abs() < 1e-3, "v0 = {v0}");

    // Second special point: vp 3 → (2.485, 0.470, _).
    let p1 = &arr[1];
    assert_eq!(p1["element_kind"], serde_json::json!("curv2"));
    assert_eq!(p1["vp_index_1based"], serde_json::json!(3));
    let u1 = p1["u"].as_f64().unwrap();
    let v1 = p1["v"].as_f64().unwrap();
    assert!((u1 - 2.485).abs() < 1e-3, "u1 = {u1}");
    assert!((v1 - 0.470).abs() < 1e-3, "v1 = {v1}");
}

#[test]
fn sp_curv_space_curve_v_is_null() {
    // Inside a `curv` block the parameter vertices are 1D per spec.
    // The typed view surfaces `u` only; `v` is JSON null.
    const SRC: &str = "\
vp 0.42
vp 0.84
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
cstype bezier
deg 2
curv 0.0 1.0 1 2 3
sp 1 2
parm u 0.0 1.0
end
";
    let scene = decode(SRC, 0);
    let arr = typed_sp_array(&scene);
    assert_eq!(arr.len(), 2);
    for entry in arr {
        assert_eq!(entry["element_kind"], serde_json::json!("curv"));
        assert!(
            entry["v"].is_null(),
            "v must be null for curv special points, got {:?}",
            entry["v"]
        );
    }
    let u0 = arr[0]["u"].as_f64().unwrap();
    let u1 = arr[1]["u"].as_f64().unwrap();
    assert!((u0 - 0.42).abs() < 1e-3, "u0 = {u0}");
    assert!((u1 - 0.84).abs() < 1e-3, "u1 = {u1}");
}

#[test]
fn sp_surf_uses_both_u_and_v() {
    // Inside a `surf` block the parameter vertices are 2D per spec; both
    // `u` and `v` are meaningful.
    const SRC: &str = "\
vp 0.250 0.750
vp 0.500 0.500
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
sp 1 2
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let scene = decode(SRC, 0);
    let arr = typed_sp_array(&scene);
    assert_eq!(arr.len(), 2);
    for entry in arr {
        assert_eq!(entry["element_kind"], serde_json::json!("surf"));
        assert!(entry["v"].is_f64());
    }
    let u0 = arr[0]["u"].as_f64().unwrap();
    let v0 = arr[0]["v"].as_f64().unwrap();
    let u1 = arr[1]["u"].as_f64().unwrap();
    let v1 = arr[1]["v"].as_f64().unwrap();
    assert!((u0 - 0.250).abs() < 1e-4, "u0 = {u0}");
    assert!((v0 - 0.750).abs() < 1e-4, "v0 = {v0}");
    assert!((u1 - 0.500).abs() < 1e-4, "u1 = {u1}");
    assert!((v1 - 0.500).abs() < 1e-4, "v1 = {v1}");
}

#[test]
fn sp_example_9_three_element_kinds_in_order() {
    // Example 9 covers all three element kinds in source order — `curv`
    // (sp 1), `curv2` (sp 2 3), then `surf` (sp 4). The typed view
    // walks freeform_directives in source order so the kinds appear in
    // that sequence.
    let scene = decode(SP_EXAMPLE_9, 0);
    let arr = typed_sp_array(&scene);
    assert_eq!(arr.len(), 4, "1 + 2 + 1 special points expected");

    assert_eq!(arr[0]["element_kind"], serde_json::json!("curv"));
    assert!(arr[0]["v"].is_null());

    assert_eq!(arr[1]["element_kind"], serde_json::json!("curv2"));
    assert!(arr[1]["v"].is_f64());
    assert_eq!(arr[2]["element_kind"], serde_json::json!("curv2"));
    assert!(arr[2]["v"].is_f64());

    assert_eq!(arr[3]["element_kind"], serde_json::json!("surf"));
    assert!(arr[3]["v"].is_f64());
}

#[test]
fn sp_synthetic_points_mesh_emitted_when_tessellated() {
    // The synthetic `"obj:sps"` mesh is gated on `with_curve_tessellation`
    // (matches the curv / surf / scrv pattern). With `samples == 0` the
    // typed view still lands but the synthetic primitive is suppressed.
    let scene_no_tess = decode(SP_EXAMPLE_9, 0);
    assert!(
        sp_mesh(&scene_no_tess).is_none(),
        "obj:sps must not be present when tessellation samples == 0"
    );
    assert!(
        !typed_sp_array(&scene_no_tess).is_empty(),
        "typed view must land regardless of samples"
    );

    // With `samples > 0` the synthetic mesh appears: 3 `sp` lines →
    // 3 `Topology::Points` primitives.
    let scene_tess = decode(SP_EXAMPLE_9, 16);
    let mesh = sp_mesh(&scene_tess).expect("obj:sps mesh must exist when tessellated");
    assert_eq!(mesh.primitives.len(), 3, "one primitive per `sp` line");
    for prim in &mesh.primitives {
        assert_eq!(prim.topology, Topology::Points);
        assert_eq!(
            prim.extras
                .get("obj:tessellated_curve")
                .and_then(|v| v.as_bool()),
            Some(true),
            "shared encoder-filter sentinel must be set"
        );
        assert_eq!(
            prim.extras
                .get("obj:special_point")
                .and_then(|v| v.as_bool()),
            Some(true),
            "sp marker must be set"
        );
        assert!(prim.extras.contains_key("obj:special_point_element_kind"));
        assert!(prim.extras.contains_key("obj:special_point_vp_refs"));
    }

    // The first sp directive carries one vp ref (sp 1 inside curv).
    let first = &mesh.primitives[0];
    assert_eq!(first.positions.len(), 1);
    assert_eq!(
        first.extras["obj:special_point_element_kind"],
        serde_json::json!("curv")
    );
    let refs = first.extras["obj:special_point_vp_refs"]
        .as_array()
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].as_i64(), Some(1));

    // The second carries two (sp 2 3 inside curv2).
    let second = &mesh.primitives[1];
    assert_eq!(second.positions.len(), 2);
    assert_eq!(
        second.extras["obj:special_point_element_kind"],
        serde_json::json!("curv2")
    );

    // The third carries one (sp 4 inside surf).
    let third = &mesh.primitives[2];
    assert_eq!(third.positions.len(), 1);
    assert_eq!(
        third.extras["obj:special_point_element_kind"],
        serde_json::json!("surf")
    );
}

#[test]
fn sp_synthetic_primitive_positions_lift_correctly() {
    // For curv: 1D vp → lifted as `[u, 0, 0]` (spec: "the parameter
    // vertices must be 1D").
    const CURV_SRC: &str = "\
vp 0.42
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
cstype bezier
deg 2
curv 0.0 1.0 1 2 3
sp 1
parm u 0.0 1.0
end
";
    let scene = decode(CURV_SRC, 8);
    let mesh = sp_mesh(&scene).expect("obj:sps mesh must exist");
    assert_eq!(mesh.primitives.len(), 1);
    let pos = &mesh.primitives[0].positions[0];
    assert!((pos[0] - 0.42).abs() < 1e-4);
    assert!((pos[1] - 0.0).abs() < 1e-6);
    assert!((pos[2] - 0.0).abs() < 1e-6);

    // For surf: 2D vp → lifted as `[u, v, 0]`.
    const SURF_SRC: &str = "\
vp 0.30 0.70
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
sp 1
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let scene = decode(SURF_SRC, 8);
    let mesh = sp_mesh(&scene).expect("obj:sps mesh must exist");
    let pos = &mesh.primitives[0].positions[0];
    assert!((pos[0] - 0.30).abs() < 1e-4);
    assert!((pos[1] - 0.70).abs() < 1e-4);
    assert!((pos[2] - 0.0).abs() < 1e-6);
}

#[test]
fn sp_negative_vp_indices_resolve_from_end() {
    // Spec §"Example 9" mentions special points may reference negative
    // vp indices the same way `curv` and `curv2` lines do — the spec
    // doesn't prohibit it for `sp` and consistency with the surrounding
    // vp resolution rule is the natural interpretation. With 4 vp
    // entries, `sp -1 -4` resolves to indices 4 and 1.
    const SRC: &str = "\
vp 0.10
vp 0.20
vp 0.30
vp 0.40
v 0.0 0.0 0.0
v 1.0 0.0 0.0
cstype bezier
deg 1
curv 0.0 1.0 1 2
sp -1 -4
parm u 0.0 1.0
end
";
    let scene = decode(SRC, 0);
    let arr = typed_sp_array(&scene);
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["vp_index_1based"], serde_json::json!(4));
    assert!((arr[0]["u"].as_f64().unwrap() - 0.40).abs() < 1e-4);
    assert_eq!(arr[1]["vp_index_1based"], serde_json::json!(1));
    assert!((arr[1]["u"].as_f64().unwrap() - 0.10).abs() < 1e-4);
}

#[test]
fn sp_out_of_range_indices_silently_dropped() {
    // References outside the live vp pool are dropped from both the
    // typed view and the synthetic primitive; the encoder still replays
    // the original `sp` line verbatim.
    const SRC: &str = "\
vp 0.50
v 0.0 0.0 0.0
v 1.0 0.0 0.0
cstype bezier
deg 1
curv 0.0 1.0 1 2
sp 1 99 -99 0
parm u 0.0 1.0
end
";
    let scene = decode(SRC, 4);
    let arr = typed_sp_array(&scene);
    assert_eq!(arr.len(), 1, "only the in-range vp 1 survives");
    assert_eq!(arr[0]["vp_index_1based"], serde_json::json!(1));
    let mesh = sp_mesh(&scene).expect("obj:sps mesh must exist");
    assert_eq!(mesh.primitives.len(), 1);
    assert_eq!(mesh.primitives[0].positions.len(), 1);
}

#[test]
fn sp_roundtrip_replays_original_lines_verbatim() {
    // The encoder filters synthetic `obj:sps` primitives out via the
    // shared `obj:tessellated_curve` sentinel and drives `sp` emission
    // from `Scene3D::extras["obj:freeform_directives"]`. A
    // decode → encode cycle must preserve every original `sp` line.
    let scene = decode(SP_EXAMPLE_9, 16);
    let mut enc = ObjEncoder::new();
    let bytes = enc.encode(&scene).expect("encode must succeed");
    let reemitted = std::str::from_utf8(&bytes).expect("output is UTF-8");

    let sp_lines: Vec<&str> = reemitted
        .lines()
        .filter(|l| l.trim_start().starts_with("sp "))
        .collect();
    assert_eq!(
        sp_lines.len(),
        3,
        "three original `sp` lines must survive re-encoding, got {sp_lines:?}"
    );

    // A second decode reproduces the same typed view (idempotence under
    // round-trip).
    let scene2 = decode(reemitted, 0);
    let arr2 = typed_sp_array(&scene2);
    assert_eq!(arr2.len(), 4, "1 + 2 + 1 special points after round-trip");
    assert_eq!(arr2[0]["element_kind"], serde_json::json!("curv"));
    assert_eq!(arr2[1]["element_kind"], serde_json::json!("curv2"));
    assert_eq!(arr2[2]["element_kind"], serde_json::json!("curv2"));
    assert_eq!(arr2[3]["element_kind"], serde_json::json!("surf"));
}

#[test]
fn sp_outside_any_block_is_ignored() {
    // An `sp` line that doesn't sit inside a `cstype` … `end` block has
    // no enclosing element kind to resolve against. The typed view
    // omits it; the verbatim directive sequence still replays it.
    const SRC: &str = "\
vp 0.5
v 0.0 0.0 0.0
v 1.0 0.0 0.0
sp 1
cstype bezier
deg 1
curv 0.0 1.0 1 2
sp 1
end
";
    let scene = decode(SRC, 0);
    let arr = typed_sp_array(&scene);
    // Only the `sp` inside the curv block resolves.
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["element_kind"], serde_json::json!("curv"));
}

#[test]
fn sp_no_extras_key_without_sp_lines() {
    // OBJ files without any `sp` directive must not emit the typed key
    // (avoids littering scene.extras with empty arrays).
    const SRC: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
    let scene = decode(SRC, 0);
    assert!(!scene.extras.contains_key("obj:special_points"));
    assert!(sp_mesh(&scene).is_none());
}
