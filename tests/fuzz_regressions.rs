//! Regression tests for crashes discovered by the `parse_obj` /
//! `parse_mtl` cargo-fuzz harnesses (see `crates/oxideav-obj/fuzz/`).
//!
//! Each test wraps a minimised attacker input that previously panicked
//! a parser entry point and asserts the call now returns `Result`
//! cleanly (success or `Err`, never panic / abort / overflow). The
//! tests do not pin the specific `Ok` / `Err` variant — the contract
//! under test is panic-freedom, and a parser that later starts
//! accepting an input we previously rejected (or vice-versa) is fine
//! as long as the call still returns.
//!
//! Inputs are spelled out as raw byte literals so the regression is
//! self-documenting; the original fuzz artefact lives under
//! `fuzz/artifacts/parse_obj/` for the libFuzzer-driven reproduction.

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::ObjDecoder;

/// Round 171, `parse_obj` fuzz target, artefact
/// `crash-2e23c4a295f47c61eda9d2b6405947c3ecd9e812`.
///
/// Minimised input: ` 1\np /13\0\0\xfd\0\0`.
///
/// Cause: `parse_face_vertex` allowed the leading `v` slot (the
/// position index, before the first `/`) to be empty — the
/// `resolve()` closure returned `Ok(0)` for an empty string regardless
/// of which component it was parsing. Downstream consumers in
/// `build_scene` then computed `(fv.v - 1) as usize`, which underflows
/// `u32` when `fv.v == 0` and panics in debug. The fix rejects an
/// empty position slot at parse time so the `fv.v >= 1` invariant
/// holds end-to-end.
///
/// The minimised input is `p /13` after a single `v 1` line: a point
/// element referencing position index "" (empty before the slash) with
/// texture-coord index `13`. The trailing NUL bytes were noise from
/// the fuzzer's mutation; they're not load-bearing for the crash but
/// they exercise the per-line preprocessor's tolerance of embedded
/// NULs which is also worth pinning.
#[test]
fn parse_obj_point_with_empty_position_slot_does_not_panic() {
    let input: &[u8] = b" 1\np /13\0\0\xfd\0\0";
    let mut dec = ObjDecoder::new();
    let _ = dec.decode(input);
}

/// Same hazard reachable via the `f` directive: `f /1/2 /3/4 /5/6`
/// would compute `fv.v == 0` for every face vertex without the
/// parse-time guard. The triangulation loop then crashes in
/// `build_scene` on the first `(fv.v - 1) as usize`. The fix above
/// rejects the input at parse time so this returns `Err` cleanly.
#[test]
fn parse_obj_face_with_empty_position_slots_does_not_panic() {
    let input: &[u8] = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf /1 /2 /3\n";
    let mut dec = ObjDecoder::new();
    let _ = dec.decode(input);
}

/// Same hazard on `l` (line) elements — share the `parse_face_vertex`
/// path.
#[test]
fn parse_obj_line_with_empty_position_slots_does_not_panic() {
    let input: &[u8] = b"v 0 0 0\nv 1 0 0\nvt 0 0\nvt 1 0\nl /1 /2\n";
    let mut dec = ObjDecoder::new();
    let _ = dec.decode(input);
}

/// Round 171, `parse_obj` fuzz target, artefact
/// `crash-ae89b7f6b9f355d365b3bf8d8bd52d61694916ad` (minimised to
/// `cstype bezier\nsurf 0 1 0 0 /\ndeg 111111`).
///
/// Cause: `tessellate_surfaces`' Bezier branch computed the expected
/// control-grid size as `(degu + 1, degv + 1)` and then allocated
/// `Vec::with_capacity(cols * rows)` for the grid pool. With an
/// attacker-supplied `deg 111111`, `111112 * 111112` is ~12 GiB of
/// reservation — libfuzzer-driven AddressSanitizer flagged this as
/// `allocation-size-too-big` and aborted. The fix gates the allocation
/// behind `checked_add` / `checked_mul` and on a quick "expected ==
/// declared-control-vertex count" check so any mismatch bails before
/// the allocation request.
///
/// Reachable only with the curve-tessellation knob enabled (the
/// default `samples == 0` path skips `tessellate_surfaces` entirely),
/// but the trait-surface harness drives both paths so the regression
/// must hold under either.
#[test]
fn parse_obj_bezier_surf_with_huge_deg_does_not_blow_allocation() {
    let input: &[u8] = b"v 0 0 0\ncstype bezier\nsurf 0 1 0 0 /\ndeg 111111\n";
    let mut dec = ObjDecoder::new().with_curve_tessellation(4);
    let _ = dec.decode(input);
}

/// Defence-in-depth: the same attacker control reachable via the
/// basis-matrix curve path — `sample_bmatrix` validates
/// `bmat_u.len() == (n + 1) * (n + 1)` where `n` is the parsed `deg`
/// value. A huge `deg` would overflow the multiplication in debug
/// (before the fix added `checked_mul`).
#[test]
fn parse_obj_bmatrix_with_huge_deg_does_not_overflow_basis_size() {
    let input: &[u8] =
        b"v 0 0 0\nv 1 0 0\ncstype bmatrix\ndeg 4294967295\nbmat u 1\nstep 1\ncurv 0 1 1 2\nend\n";
    let mut dec = ObjDecoder::new().with_curve_tessellation(4);
    let _ = dec.decode(input);
}

/// Round 402 hardening: the tessellation *budget* itself is a
/// caller-supplied knob, not a file-derived count. A caller passing a
/// near-`u32::MAX` sample count drove the curve evaluators'
/// `let n_samples = samples + 1;` bound past `u32::MAX`, panicking with
/// "attempt to add with overflow" in debug builds (and requesting a
/// multi-gigabyte lattice allocation in release). The parser now clamps
/// the effective count to `MAX_TESSELLATION_SAMPLES` before any
/// evaluator runs, so even the maximum budget resolves cleanly.
///
/// `curv` (1D Bezier evaluator, the direct `samples + 1` overflow site).
#[test]
fn parse_obj_curve_with_max_budget_does_not_overflow() {
    let input: &[u8] = b"v 0 0 0\nv 1 0 0\ncstype bezier\ncurv 0 1 1 2\nend\n";
    let mut dec = ObjDecoder::new().with_curve_tessellation(u32::MAX);
    let scene = dec.decode(input).expect("max-budget curve decodes cleanly");
    // The clamp keeps the synthesised polyline bounded rather than
    // attempting a `u32::MAX + 1`-vertex buffer.
    let curve_verts: usize = scene
        .meshes
        .iter()
        .filter(|m| m.name.as_deref() == Some("obj:curves"))
        .flat_map(|m| m.primitives.iter())
        .map(|p| p.positions.len())
        .sum();
    assert!(
        curve_verts > 0 && curve_verts <= 4097,
        "curve lattice clamped to the ceiling, got {curve_verts} vertices"
    );
}

/// Companion of the above via the surface evaluator: a `(samples + 1)²`
/// tensor-product lattice at `samples == u32::MAX` is both an
/// arithmetic overflow and an unsatisfiable allocation. The clamp keeps
/// the decode a clean `Result`.
#[test]
fn parse_obj_surface_with_max_budget_does_not_overflow() {
    let input: &[u8] =
        b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\ncstype bezier\nsurf 0 1 0 1 1 2 3 4\nend\n";
    let mut dec = ObjDecoder::new().with_curve_tessellation(u32::MAX);
    let _ = dec
        .decode(input)
        .expect("max-budget surface decodes cleanly");
}

/// The `ctech cparm` / `stech cparma` resolution overrides multiply the
/// budget by the curve/surface degree and cap the result at `budget ×
/// 64`; with a huge budget that cap alone still overflows downstream, so
/// the effective count is additionally clamped to the absolute ceiling.
#[test]
fn parse_obj_ctech_override_with_max_budget_does_not_overflow() {
    let input: &[u8] =
        b"v 0 0 0\nv 1 0 0\ncstype bezier\nctech cparm 1000000\ncurv 0 1 1 2 3\nend\n";
    let mut dec = ObjDecoder::new().with_curve_tessellation(u32::MAX);
    let _ = dec
        .decode(input)
        .expect("max-budget ctech override decodes cleanly");
}

/// The clamp is idempotent at the ceiling: any budget at or above
/// `MAX_TESSELLATION_SAMPLES` produces byte-for-byte the same synthesised
/// geometry as the ceiling itself, so callers can pass an arbitrarily
/// large value without changing the output shape (only the resource
/// bound is enforced).
#[test]
fn tessellation_budget_saturates_at_the_ceiling() {
    let input: &[u8] =
        b"v 0 0 0\nv 1 0 0\nv 2 1 0\nv 3 0 0\ncstype bezier\ncurv 0 1 1 2 3 4\nend\n";

    let curve_len = |budget: u32| -> usize {
        let mut dec = ObjDecoder::new().with_curve_tessellation(budget);
        let scene = dec.decode(input).expect("decodes cleanly");
        scene
            .meshes
            .iter()
            .filter(|m| m.name.as_deref() == Some("obj:curves"))
            .flat_map(|m| m.primitives.iter())
            .map(|p| p.positions.len())
            .sum()
    };

    let at_ceiling = curve_len(oxideav_obj::obj::MAX_TESSELLATION_SAMPLES);
    assert_eq!(
        at_ceiling,
        oxideav_obj::obj::MAX_TESSELLATION_SAMPLES as usize + 1
    );
    // Above the ceiling collapses onto the same result.
    assert_eq!(
        curve_len(oxideav_obj::obj::MAX_TESSELLATION_SAMPLES + 1),
        at_ceiling
    );
    assert_eq!(curve_len(u32::MAX), at_ceiling);
}
