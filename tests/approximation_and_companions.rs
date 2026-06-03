//! Approximation-technique directives (`ctech` / `stech`) and the
//! shadow / ray-tracing companion-object directives (`shadow_obj` /
//! `trace_obj`) — round-trip preservation.
//!
//! Coverage:
//!   * Spec §"ctech technique resolution" — three forms `cparm res`,
//!     `cspace maxlength`, `curv maxdist maxangle`.
//!   * Spec §"stech technique resolution" — four forms `cparma ures
//!     vres`, `cparmb uvres`, `cspace maxlength`, `curv maxdist
//!     maxangle`.
//!   * Spec §"shadow_obj filename" — top-level last-wins shadow caster
//!     companion file ("Only one shadow object can be stored in a file.
//!     If more than one shadow object is specified, the last one
//!     specified will be used.").
//!   * Spec §"trace_obj filename" — top-level last-wins ray-tracing
//!     reflection-target companion file (same last-wins behaviour, per
//!     spec text "Only one trace object can be stored in a file. If
//!     more than one is specified, the last one is used.").
//!
//! Round-trip strategy: `ctech` / `stech` ride the same
//! `Scene3D::extras["obj:freeform_directives"]` verbatim-capture
//! channel as the other free-form directives, so the encoder replays
//! them after the polygonal section. `shadow_obj` / `trace_obj` surface
//! as plain strings on `Scene3D::extras["obj:shadow_obj"]` /
//! `["obj:trace_obj"]` and the encoder writes them out in the preamble
//! (right after `mtllib`, matching the worked examples in spec
//! §"Examples").

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder};
use oxideav_obj::{ObjDecoder, ObjEncoder, obj};

/// All three `ctech` forms inside one free-form block, taken from the
/// three spec sub-sections under §"ctech technique resolution":
///   * `ctech cparm 1.000000` (constant parametric subdivision)
///   * `ctech cspace 0.500000` (constant spatial subdivision)
///   * `ctech curv 0.100000 5.000000` (curvature-dependent subdivision)
const CTECH_OBJ: &str = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
cstype bezier
deg 3
curv 0.0 1.0 1 2 3 4
parm u 0.0 1.0
ctech cparm 1.000000
ctech cspace 0.500000
ctech curv 0.100000 5.000000
end
";

#[test]
fn ctech_round_trip_all_three_forms() {
    let scene = obj::parse_obj(CTECH_OBJ).unwrap();
    let directives = scene
        .extras
        .get("obj:freeform_directives")
        .expect("ctech captured as free-form directive");
    let arr = directives.as_array().unwrap();

    let ctechs: Vec<&serde_json::Value> = arr
        .iter()
        .filter(|d| {
            d.as_array()
                .and_then(|a| a.first())
                .and_then(|t| t.as_str())
                == Some("ctech")
        })
        .collect();
    assert_eq!(ctechs.len(), 3, "all three ctech lines captured");

    // First form: `cparm res`.
    let cparm = ctechs[0].as_array().unwrap();
    assert_eq!(cparm[1].as_str(), Some("cparm"));
    assert_eq!(cparm[2].as_str(), Some("1.000000"));
    assert_eq!(cparm.len(), 3);

    // Second form: `cspace maxlength`.
    let cspace = ctechs[1].as_array().unwrap();
    assert_eq!(cspace[1].as_str(), Some("cspace"));
    assert_eq!(cspace[2].as_str(), Some("0.500000"));

    // Third form: `curv maxdist maxangle`.
    let curv = ctechs[2].as_array().unwrap();
    assert_eq!(curv[1].as_str(), Some("curv"));
    assert_eq!(curv[2].as_str(), Some("0.100000"));
    assert_eq!(curv[3].as_str(), Some("5.000000"));

    // Encoder replays them verbatim.
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("ctech cparm 1.000000"));
    assert!(text.contains("ctech cspace 0.500000"));
    assert!(text.contains("ctech curv 0.100000 5.000000"));

    // Stability: re-decoding regenerates the same directive sequence.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let directives2 = scene2.extras.get("obj:freeform_directives").unwrap();
    assert_eq!(directives, directives2, "ctech round-trip not stable");
}

/// All four `stech` forms inside one free-form block (taken from the
/// four spec sub-sections under §"stech technique resolution").
const STECH_OBJ: &str = "\
v 0 0 0
v 1 0 0
v 0 1 0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 1
parm u 0.0 1.0
parm v 0.0 1.0
stech cparma 1.000000 1.000000
stech cparmb 2.000000
stech cspace 0.500000
stech curv 0.100000 5.000000
end
";

#[test]
fn stech_round_trip_all_four_forms() {
    let scene = obj::parse_obj(STECH_OBJ).unwrap();
    let directives = scene
        .extras
        .get("obj:freeform_directives")
        .expect("stech captured as free-form directive");
    let arr = directives.as_array().unwrap();

    let stechs: Vec<&serde_json::Value> = arr
        .iter()
        .filter(|d| {
            d.as_array()
                .and_then(|a| a.first())
                .and_then(|t| t.as_str())
                == Some("stech")
        })
        .collect();
    assert_eq!(stechs.len(), 4, "all four stech lines captured");

    // First form: `cparma ures vres`.
    let cparma = stechs[0].as_array().unwrap();
    assert_eq!(cparma[1].as_str(), Some("cparma"));
    assert_eq!(cparma[2].as_str(), Some("1.000000"));
    assert_eq!(cparma[3].as_str(), Some("1.000000"));

    // Second form: `cparmb uvres`.
    let cparmb = stechs[1].as_array().unwrap();
    assert_eq!(cparmb[1].as_str(), Some("cparmb"));
    assert_eq!(cparmb[2].as_str(), Some("2.000000"));

    // Third form: `cspace maxlength`.
    let cspace = stechs[2].as_array().unwrap();
    assert_eq!(cspace[1].as_str(), Some("cspace"));
    assert_eq!(cspace[2].as_str(), Some("0.500000"));

    // Fourth form: `curv maxdist maxangle`.
    let curv = stechs[3].as_array().unwrap();
    assert_eq!(curv[1].as_str(), Some("curv"));
    assert_eq!(curv[2].as_str(), Some("0.100000"));
    assert_eq!(curv[3].as_str(), Some("5.000000"));

    // Encoder replays them verbatim.
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("stech cparma 1.000000 1.000000"));
    assert!(text.contains("stech cparmb 2.000000"));
    assert!(text.contains("stech cspace 0.500000"));
    assert!(text.contains("stech curv 0.100000 5.000000"));

    // Stability.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let directives2 = scene2.extras.get("obj:freeform_directives").unwrap();
    assert_eq!(directives, directives2);
}

/// Spec §"Examples", case 2 ("Cube casting a shadow"): a `shadow_obj`
/// referencing the geometry file as its own caster.
const SHADOW_OBJ: &str = "\
mtllib master.mtl
shadow_obj cube.obj
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
f 1 2 3 4
";

#[test]
fn shadow_obj_round_trip() {
    let scene = obj::parse_obj(SHADOW_OBJ).unwrap();
    let stored = scene
        .extras
        .get("obj:shadow_obj")
        .expect("shadow_obj captured as scene extra");
    assert_eq!(stored.as_str(), Some("cube.obj"));

    // Encoder writes it in the preamble (before vertex data).
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("shadow_obj cube.obj"),
        "missing shadow_obj in emitted text:\n{text}"
    );
    // Placement check: the directive sits before the first `v ` line.
    let shadow_pos = text.find("shadow_obj cube.obj").unwrap();
    let first_v_pos = text.find("\nv ").unwrap();
    assert!(
        shadow_pos < first_v_pos,
        "shadow_obj should precede vertex data"
    );

    // Round-trip stability.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    assert_eq!(
        scene2.extras.get("obj:shadow_obj").and_then(|v| v.as_str()),
        Some("cube.obj")
    );
}

/// Spec §"Examples", case 3 ("Cube casting a reflection"): a
/// `trace_obj` referencing the geometry file as its own reflection
/// target.
const TRACE_OBJ: &str = "\
mtllib master.mtl
trace_obj cube.obj
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
f 1 2 3 4
";

#[test]
fn trace_obj_round_trip() {
    let scene = obj::parse_obj(TRACE_OBJ).unwrap();
    let stored = scene
        .extras
        .get("obj:trace_obj")
        .expect("trace_obj captured as scene extra");
    assert_eq!(stored.as_str(), Some("cube.obj"));

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("trace_obj cube.obj"));

    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    assert_eq!(
        scene2.extras.get("obj:trace_obj").and_then(|v| v.as_str()),
        Some("cube.obj")
    );
}

/// Both companion directives in one file plus per-spec last-wins
/// behaviour ("Only one shadow object can be stored in a file. If more
/// than one shadow object is specified, the last one specified will be
/// used.").
const COMPANION_LAST_WINS_OBJ: &str = "\
mtllib m.mtl
shadow_obj first.obj
trace_obj first_trace.obj
shadow_obj final.obj
trace_obj final_trace.obj
v 0 0 0
v 1 0 0
v 1 1 0
f 1 2 3
";

#[test]
fn shadow_and_trace_last_wins() {
    let scene = obj::parse_obj(COMPANION_LAST_WINS_OBJ).unwrap();
    assert_eq!(
        scene.extras.get("obj:shadow_obj").and_then(|v| v.as_str()),
        Some("final.obj"),
        "spec: last shadow_obj wins"
    );
    assert_eq!(
        scene.extras.get("obj:trace_obj").and_then(|v| v.as_str()),
        Some("final_trace.obj"),
        "spec: last trace_obj wins"
    );

    // Encoder emits exactly one of each (the survivor).
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert_eq!(
        text.matches("shadow_obj ").count(),
        1,
        "exactly one shadow_obj line should survive"
    );
    assert_eq!(
        text.matches("trace_obj ").count(),
        1,
        "exactly one trace_obj line should survive"
    );
    assert!(text.contains("shadow_obj final.obj"));
    assert!(text.contains("trace_obj final_trace.obj"));
}

/// Empty `shadow_obj` / `trace_obj` (just the keyword with no filename)
/// is treated as absent, mirroring the lenient-loader pattern used for
/// `mg` / `s` / `g`.
#[test]
fn shadow_and_trace_empty_filename_dropped() {
    let src = "shadow_obj\ntrace_obj\nv 0 0 0\nv 1 0 0\nv 1 1 0\nf 1 2 3\n";
    let scene = obj::parse_obj(src).unwrap();
    assert!(!scene.extras.contains_key("obj:shadow_obj"));
    assert!(!scene.extras.contains_key("obj:trace_obj"));
}

/// `ctech` / `stech` placement is anywhere a free-form body statement
/// can appear (spec lists them under §"Free-form geometry statement").
/// A file with only `ctech` / `stech` and no `cstype` block still has
/// them captured as free-form directives; the encoder replays them in
/// source order even when no enclosing block exists.
#[test]
fn ctech_stech_outside_cstype_block_still_round_trips() {
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
ctech cparm 1.000000
stech cparma 1.000000 1.000000
f 1 2 3
";
    let scene = obj::parse_obj(src).unwrap();
    let directives = scene
        .extras
        .get("obj:freeform_directives")
        .expect("captured even outside cstype block");
    let arr = directives.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0][0].as_str(), Some("ctech"));
    assert_eq!(arr[1][0].as_str(), Some("stech"));

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("ctech cparm 1.000000"));
    assert!(text.contains("stech cparma 1.000000 1.000000"));
}

/// `ObjDecoder` / `ObjEncoder` trait surface (the public Mesh3D entry
/// points) carries the same round-trip behaviour as the free functions.
#[test]
fn decoder_encoder_trait_surface_round_trip() {
    let scene = ObjDecoder::new().decode(SHADOW_OBJ.as_bytes()).unwrap();
    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("shadow_obj cube.obj"));
}
