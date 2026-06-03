//! Connectivity between free-form surfaces (`con`) and the general
//! `call` / `csh` statements — round-trip preservation.
//!
//! Coverage:
//!   * Spec §"Connectivity between free-form surfaces" / §"con
//!     surf_1 q0_1 q1_1 curv2d_1 surf_2 q0_2 q1_2 curv2d_2" — a
//!     top-level free-form geometry statement that ties two
//!     previously-declared `surf` blocks together along a shared
//!     trimming-curve segment for edge merging.
//!   * Spec §"General statement" — `call filename.ext arg1 arg2 …`
//!     (inline include of a sibling `.obj` / `.mod` file with
//!     positional argument substitution) and `csh command` /
//!     `csh -command` (shell-execute a UNIX command, leading `-`
//!     flagging "ignore error on non-zero exit").
//!
//! Round-trip strategy: `con` rides the existing
//! `Scene3D::extras["obj:freeform_directives"]` verbatim-capture
//! channel alongside the other free-form geometry directives so the
//! encoder replays it after the polygonal section.
//! `call` / `csh` surface on a separate
//! `Scene3D::extras["obj:general_directives"]` array; the encoder
//! writes them at the top of the preamble (right after `mtllib` and
//! the `shadow_obj` / `trace_obj` companion-file block). The OBJ
//! spec is silent on placement for `call` / `csh` ("The call
//! statement can be inserted into .obj files using a text editor"),
//! so source-line position relative to the polygonal section is not
//! preserved by design.
//!
//! `csh` is NOT executed (sandbox-escape trapdoor for any consumer
//! that round-trips untrusted OBJ input); `call` is NOT recursively
//! resolved (would require IO + nested-call depth tracking outside
//! the clean-room parser's scope). Both are captured-only.

use oxideav_mesh3d::{Mesh3DDecoder, Mesh3DEncoder};
use oxideav_obj::{ObjDecoder, ObjEncoder, obj};

/// Spec §"Connectivity between free-form surfaces" §"Example 1" —
/// connectivity between two Bezier patches sharing a `curv2` trimming
/// curve along the seam u = 1.0 of surface 1 == u = 0.0 of surface 2.
const CON_OBJ_EXAMPLE_1: &str = "\
cstype bezier
deg 1 1
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
vp 0 0
vp 1 0
vp 1 1
vp 0 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0
end
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
trim 0.0 4.0 1
end
v 1 0 0
v 2 0 0
v 1 1 0
v 2 1 0
surf 0.0 1.0 0.0 1.0 5 6 7 8
parm u 0.0 1.0
parm v 0.0 1.0
trim 0.0 4.0 1
end
con 1 2.0 2.0 1 2 4.0 3.0 1
";

#[test]
fn con_round_trip_captures_eight_arguments() {
    let scene = obj::parse_obj(CON_OBJ_EXAMPLE_1).unwrap();
    let directives = scene
        .extras
        .get("obj:freeform_directives")
        .expect("con captured as free-form directive");
    let arr = directives.as_array().unwrap();

    let cons: Vec<&serde_json::Value> = arr
        .iter()
        .filter(|d| {
            d.as_array()
                .and_then(|a| a.first())
                .and_then(|t| t.as_str())
                == Some("con")
        })
        .collect();
    assert_eq!(cons.len(), 1, "exactly one con line captured");

    // Spec §"con surf_1 q0_1 q1_1 curv2d_1 surf_2 q0_2 q1_2 curv2d_2":
    // the keyword + eight positional integer/float arguments.
    let entry = cons[0].as_array().unwrap();
    assert_eq!(entry.len(), 9, "con keyword + 8 args");
    assert_eq!(entry[0].as_str(), Some("con"));
    assert_eq!(entry[1].as_str(), Some("1")); // surf_1
    assert_eq!(entry[2].as_str(), Some("2.0")); // q0_1
    assert_eq!(entry[3].as_str(), Some("2.0")); // q1_1
    assert_eq!(entry[4].as_str(), Some("1")); // curv2d_1
    assert_eq!(entry[5].as_str(), Some("2")); // surf_2
    assert_eq!(entry[6].as_str(), Some("4.0")); // q0_2
    assert_eq!(entry[7].as_str(), Some("3.0")); // q1_2
    assert_eq!(entry[8].as_str(), Some("1")); // curv2d_2

    // Encoder replays the con line verbatim.
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("con 1 2.0 2.0 1 2 4.0 3.0 1"),
        "con line not re-emitted verbatim: {text}"
    );

    // Stability: re-decoding regenerates the same directive sequence.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let directives2 = scene2.extras.get("obj:freeform_directives").unwrap();
    assert_eq!(directives, directives2, "con round-trip not stable");
}

#[test]
fn con_preserves_source_order_relative_to_surf_blocks() {
    let scene = obj::parse_obj(CON_OBJ_EXAMPLE_1).unwrap();
    let arr = scene
        .extras
        .get("obj:freeform_directives")
        .unwrap()
        .as_array()
        .unwrap();

    // The example places `con` AFTER the second surface's `end`. The
    // captured sequence should preserve that — `con` is the last
    // free-form directive entry.
    let last = arr.last().unwrap().as_array().unwrap();
    assert_eq!(last[0].as_str(), Some("con"));

    // Two surf headers must precede the con line.
    let surf_positions: Vec<usize> = arr
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if e.as_array()
                .and_then(|a| a.first())
                .and_then(|t| t.as_str())
                == Some("surf")
            {
                Some(i)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(surf_positions.len(), 2);
    assert!(surf_positions[0] < surf_positions[1]);
    assert!(surf_positions[1] < arr.len() - 1, "con sits after surfaces");
}

#[test]
fn con_with_negative_indices_round_trips() {
    // Spec §"con" doesn't explicitly call out relative-from-end
    // indices for the surf / curv2d slots, but the rest of the free-
    // form geometry vocabulary treats them as legal. Captured-verbatim
    // round-trip means whatever the operator wrote on the line lands
    // back in the output identically.
    let src = "\
v 0 0 0
v 1 0 0
con -2 0.0 1.0 -1 -1 0.0 1.0 -1
";
    let scene = obj::parse_obj(src).unwrap();
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("con -2 0.0 1.0 -1 -1 0.0 1.0 -1"));
}

// ---------------------------------------------------------------------------
// `call` general statement
// ---------------------------------------------------------------------------

#[test]
fn call_round_trip_basic_filename() {
    // Spec §"call filename.ext arg1 arg2 …" — bare include of a
    // sibling .obj file with no positional arguments.
    let src = "\
call sibling.obj
v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
";
    let scene = obj::parse_obj(src).unwrap();
    let directives = scene
        .extras
        .get("obj:general_directives")
        .expect("call captured as general directive");
    let arr = directives.as_array().unwrap();
    assert_eq!(arr.len(), 1, "one call line captured");

    let entry = arr[0].as_array().unwrap();
    assert_eq!(entry[0].as_str(), Some("call"));
    assert_eq!(entry[1].as_str(), Some("sibling.obj"));
    assert_eq!(entry.len(), 2);

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("call sibling.obj"));

    // Stability: re-decode preserves the directive.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();
    let arr2 = scene2
        .extras
        .get("obj:general_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(arr2.len(), 1);
}

#[test]
fn call_with_positional_arguments_round_trips() {
    // Spec §"call" worked example: `call filename.obj $1` — the `$1`
    // positional arg is captured as a plain token (no substitution
    // performed; consumers re-interpret).
    let src = "\
call animated.obj $1
call lookup.obj 1 2 3
v 0 0 0
";
    let scene = obj::parse_obj(src).unwrap();
    let arr = scene
        .extras
        .get("obj:general_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(arr.len(), 2, "two call lines captured");

    let first = arr[0].as_array().unwrap();
    assert_eq!(first[0].as_str(), Some("call"));
    assert_eq!(first[1].as_str(), Some("animated.obj"));
    assert_eq!(first[2].as_str(), Some("$1"));

    let second = arr[1].as_array().unwrap();
    assert_eq!(second[0].as_str(), Some("call"));
    assert_eq!(second[1].as_str(), Some("lookup.obj"));
    assert_eq!(second[2].as_str(), Some("1"));
    assert_eq!(second[3].as_str(), Some("2"));
    assert_eq!(second[4].as_str(), Some("3"));

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("call animated.obj $1"));
    assert!(text.contains("call lookup.obj 1 2 3"));
}

#[test]
fn call_does_not_perform_file_inclusion() {
    // Captured-only: the referenced `sibling.obj` is NOT pulled into
    // the scene. Only the locally-written vertices land on
    // `Scene3D::meshes`, not anything that a sibling file might have
    // contributed. (Validates the documented clean-room behaviour:
    // recursive resolution is left to the caller.)
    let src = "\
call sibling.obj
v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
";
    let scene = obj::parse_obj(src).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.primitives.len(), 1);
    assert_eq!(mesh.primitives[0].positions.len(), 3);
}

// ---------------------------------------------------------------------------
// `csh` general statement
// ---------------------------------------------------------------------------

#[test]
fn csh_round_trip_basic_command() {
    // Spec §"csh command": a UNIX command invocation. We capture the
    // whole command text verbatim but do NOT execute it.
    let src = "\
csh date
v 0 0 0
";
    let scene = obj::parse_obj(src).unwrap();
    let arr = scene
        .extras
        .get("obj:general_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(arr.len(), 1);

    let entry = arr[0].as_array().unwrap();
    assert_eq!(entry[0].as_str(), Some("csh"));
    assert_eq!(entry[1].as_str(), Some("date"));

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("csh date"));
}

#[test]
fn csh_dash_form_round_trips_with_leading_dash_preserved() {
    // Spec §"csh -command": a leading dash flags "ignore error on
    // non-zero exit". The dash MUST survive the round-trip so the
    // semantic distinction (failure-tolerant vs. failure-fatal)
    // doesn't get silently inverted.
    let src = "\
csh -ls /nonexistent/path
v 0 0 0
";
    let scene = obj::parse_obj(src).unwrap();
    let arr = scene
        .extras
        .get("obj:general_directives")
        .unwrap()
        .as_array()
        .unwrap();
    let entry = arr[0].as_array().unwrap();
    assert_eq!(entry[0].as_str(), Some("csh"));
    assert_eq!(entry[1].as_str(), Some("-ls"));
    assert_eq!(entry[2].as_str(), Some("/nonexistent/path"));

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("csh -ls /nonexistent/path"),
        "leading-dash form not preserved: {text}"
    );
}

#[test]
fn csh_does_not_execute_the_command() {
    // Captured-only: the parser does NOT exec the captured command.
    // Validated by writing a payload that would have observable
    // side effects if executed (touching a file under a fixed tmp
    // path) — the file should not exist after parse.
    let probe_path = std::env::temp_dir().join("oxideav_obj_r229_csh_probe.txt");
    if probe_path.exists() {
        std::fs::remove_file(&probe_path).unwrap();
    }
    let src = format!(
        "csh touch {}\nv 0 0 0\n",
        probe_path.to_str().unwrap().replace(' ', "_")
    );
    let _ = obj::parse_obj(&src).unwrap();
    assert!(
        !probe_path.exists(),
        "csh command was actually executed by the parser"
    );
}

// ---------------------------------------------------------------------------
// Mixed coverage
// ---------------------------------------------------------------------------

#[test]
fn con_call_csh_combined_round_trip_through_decoder_trait() {
    let src = "\
mtllib materials.mtl
shadow_obj caster.obj
trace_obj raytraced.obj
call init.obj
csh -echo hello
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
end
surf 0.0 1.0 0.0 1.0 1 2 3 4
end
con 1 0.0 1.0 1 2 0.0 1.0 1
";
    let scene = ObjDecoder::new().decode(src.as_bytes()).unwrap();

    // All three side-channels are populated.
    assert!(scene.extras.contains_key("obj:freeform_directives"));
    assert!(scene.extras.contains_key("obj:general_directives"));
    assert_eq!(
        scene.extras.get("obj:shadow_obj").and_then(|v| v.as_str()),
        Some("caster.obj")
    );
    assert_eq!(
        scene.extras.get("obj:trace_obj").and_then(|v| v.as_str()),
        Some("raytraced.obj")
    );

    // Encoder replays everything.
    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("shadow_obj caster.obj"));
    assert!(text.contains("trace_obj raytraced.obj"));
    assert!(text.contains("call init.obj"));
    assert!(text.contains("csh -echo hello"));
    assert!(text.contains("con 1 0.0 1.0 1 2 0.0 1.0 1"));

    // The encoder ordering convention (documented on
    // `ObjDoc::general_directives`): `call` / `csh` ride the preamble
    // right after the companion-file block. Walk line-by-line to
    // verify the relative ordering.
    let mut saw_trace = false;
    let mut saw_call = false;
    let mut saw_vertex = false;
    for line in text.lines() {
        if line.starts_with("trace_obj ") {
            saw_trace = true;
        } else if line.starts_with("call ") {
            assert!(saw_trace, "call line must come after trace_obj");
            saw_call = true;
        } else if line.starts_with("v ") {
            assert!(saw_call, "vertex pool must come after call/csh block");
            saw_vertex = true;
        }
    }
    assert!(saw_vertex);
}

#[test]
fn missing_general_directives_omits_the_extra_key() {
    // Spec lenient-loader convention: absent directives leave the
    // scene extras unchanged. No `general_directives` key should
    // surface when the source has no `call` / `csh` lines.
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
";
    let scene = obj::parse_obj(src).unwrap();
    assert!(!scene.extras.contains_key("obj:general_directives"));
}

#[test]
fn empty_csh_line_with_no_command_text_is_dropped() {
    // A bare `csh` line with no argument tokens is ill-formed per
    // spec (the §"csh command" syntax requires `command`). The
    // captured `[keyword]` entry length-1 still rides on the side
    // channel, but the encoder's `parts.is_empty()` guard never trips
    // — the keyword alone is one token. We exercise the realistic
    // case where the line is `csh\n` and only check that the parser
    // accepts it without panic and the encoder emits the lone
    // keyword on its own line.
    let src = "csh\nv 0 0 0\n";
    let scene = obj::parse_obj(src).unwrap();
    let arr = scene
        .extras
        .get("obj:general_directives")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(arr.len(), 1);
    let entry = arr[0].as_array().unwrap();
    assert_eq!(entry.len(), 1, "bare csh: just the keyword");
    assert_eq!(entry[0].as_str(), Some("csh"));

    let bytes = obj::serialize_obj(&scene, None).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.lines().any(|l| l == "csh"));
}
