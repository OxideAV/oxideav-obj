#![no_main]

//! Panic-freedom fuzz target for the `oxideav-obj` OBJ parser.
//!
//! Feeds arbitrary attacker-controlled bytes through every public
//! decoder entry point and asserts that none of them panic, abort,
//! debug-overflow, or index out of bounds. The return values are
//! intentionally discarded — the contract under test is *the call
//! returns*, not what it returns.
//!
//! Classic OBJ parser danger spots this target drives:
//!
//! * **Negative-index resolution** — `f -3 -2 -1` resolves against the
//!   current vertex-pool length via `len - n`. An attacker can declare
//!   a face that references a position with `|n| > len` before any
//!   `v` line has been seen, which must surface as `Err`, not wrap
//!   around to a giant `usize` and panic on the subsequent slice.
//!   Same hazard on `l` and `p` and the `surf` / `curv`
//!   control-vertex references that also accept negative indices.
//! * **Per-face arity** — n-gon fan triangulation produces `n - 2`
//!   triangles; a `f` line with one or two indices is degenerate and
//!   must surface as `Err` rather than underflow the triangulation
//!   loop. The parser also caches the original arity in
//!   `Mesh::extras["obj:original_face_arities"]` so the encoder can
//!   round-trip — the cache vector length must always match the
//!   triangle count.
//! * **UTF-8 boundaries in comments and option-flag walkers** — every
//!   line is split on ASCII whitespace, then individual tokens are
//!   walked for `-flag value` chunks on the MTL side. Multi-byte
//!   UTF-8 characters mid-token must not panic the slice operations
//!   (the source is `&str` so the boundary checks are
//!   `char_indices`-based, but the harness still drives raw bytes
//!   so any future regression that retypes a `&str` slice as `&[u8]`
//!   gets caught).
//! * **Free-form directive sequence** — `cstype` / `parm` / `deg` /
//!   `bmat` / `step` are state-setters whose interaction with
//!   `curv` / `surf` is order-dependent. A `surf` with no preceding
//!   `cstype` (or a `parm u` with no matching `cstype bspline` header)
//!   must be captured-only without panicking on the optional-`cstype`
//!   lookup. The tessellator additionally walks `parm u` knot vectors
//!   whose length must satisfy the spec condition
//!   `len == K + degree + 2` — a malformed length must yield
//!   "captured-only", never an out-of-bounds index.
//! * **`curv` / `surf` u/v-range parsing** — every `curv` line starts
//!   with `u_min u_max ctl_indices…`; the parser splits on whitespace
//!   and parses the first two tokens as f32. A missing or non-numeric
//!   token must surface as `Err` rather than `unwrap()` on the parse.
//!   `surf` adds `t0 t1` so the same hazard sits on the second pair.
//! * **`v` token-width dispatch** — `v x y z`, `v x y z w`, `v x y z r g b`,
//!   `v x y z w r g b` (3 / 4 / 6 / 7 tokens) all share the keyword;
//!   5-token and 8+-token lines are spec-ambiguous and must surface
//!   as `Err`, not panic in the token-count switch.
//! * **`mtllib` resolver re-entrancy** — the default `ObjDecoder` is
//!   wired with a no-op resolver that returns `Ok(Vec::new())` for
//!   every library; this still exercises the `parse_obj_with_options`
//!   internal vec-clone of `doc.mtllibs` and the per-lib UTF-8 guard
//!   on the resolved bytes.
//! * **Curve / surface tessellation** — the high-level
//!   `ObjDecoder::with_curve_tessellation(8)` path evaluates Bezier /
//!   B-spline / Cardinal / Taylor curves and Bezier / B-spline /
//!   Cardinal surfaces; each evaluator has its own
//!   knot-vector / control-point-window / per-axis bounds that must
//!   reject malformed inputs without panicking. The harness uses a
//!   small sample count (8) to keep iteration cost bounded.
//!
//! The target does NOT exercise the encoder against attacker bytes:
//! there's no attacker-controlled `Scene3D` to feed it (the only way
//! to construct one is to go through the decoder first, which is
//! what's under test). A separate roundtrip target would re-validate
//! `serialize_obj`'s output against `parse_obj`'s acceptance — useful,
//! but a distinct contract. This target keeps a tight focus on the
//! decoder.

use libfuzzer_sys::fuzz_target;
use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::{obj, ObjDecoder};

fuzz_target!(|data: &[u8]| {
    // 1. High-level decoder via the trait surface. This is the path a
    //    `Mesh3DRegistry` consumer actually hits — every parser bug
    //    eventually surfaces here.
    let mut dec = ObjDecoder::new();
    let _ = dec.decode(data);

    // 2. Curve-tessellation variant. Drives the free-form evaluator
    //    (Bezier / B-spline / Cardinal / Taylor / basis-matrix curves,
    //    Bezier / B-spline / Cardinal surfaces). Sample count of 8
    //    keeps per-iteration cost bounded while still hitting the
    //    per-axis sampling loops.
    let mut dec_tess = ObjDecoder::new().with_curve_tessellation(8);
    let _ = dec_tess.decode(data);

    // 3. UTF-8 decode + free-function parser. The trait surface above
    //    runs through `obj::parse_obj_with_options` after a UTF-8
    //    check; this path skips the trait wrapper and exercises
    //    `parse_obj` directly for any future regression that splits
    //    the two paths.
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = obj::parse_obj(text);

        // 4. Explicit-options entry — wires the explicit `ParseOptions`
        //    knob and a no-op `mtllib` resolver (returns empty bytes
        //    rather than feeding the same input back as MTL, which
        //    would let a `mtllib`-heavy attacker input amplify
        //    iteration cost linearly with the number of `mtllib`
        //    lines). The MTL parse path gets its own coverage from
        //    the separate `parse_mtl` fuzz target.
        let opts = obj::ParseOptions {
            curve_tessellation_samples: 4,
        };
        let _ = obj::parse_obj_with_options(text, &opts, |_libname| Ok(Vec::new()));
    }

    // 5. Truncation. A common attacker move is to declare a long file
    //    in the header but ship a short one — re-run the high-level
    //    decoder on a handful of prefix lengths so the per-line state
    //    machine hits its boundary cases (unterminated `cstype` block,
    //    `usemtl` with no following elements, `g` with no following
    //    `f`, etc.). Capped at 4 prefixes to bound iteration cost.
    let trunc_cap = data.len().min(4);
    for take in 0..trunc_cap {
        let mut dec = ObjDecoder::new();
        let _ = dec.decode(&data[..take]);
    }
});
