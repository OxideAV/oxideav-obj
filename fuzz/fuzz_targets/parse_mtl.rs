#![no_main]

//! Panic-freedom fuzz target for the `oxideav-obj` MTL parser.
//!
//! MTL has a narrower grammar than OBJ but distinct hazards:
//!
//! * **`Tf` alternative-form dispatch** — the keyword can be followed
//!   by three mutually-exclusive shapes: `Tf r [g [b]]` (Phong),
//!   `Tf spectral file.rfl [factor]`, and `Tf xyz x [y [z]]`. The
//!   token-1 sniff (`spectral` / `xyz` / numeric) must reject a
//!   zero-token `Tf` line without panicking on the `.next()` of an
//!   empty iterator.
//! * **`map_*` option-flag walker** — `-blendu`, `-blendv`, `-cc`,
//!   `-clamp` (1-arg `on|off`), `-bm`, `-boost`, `-mm` (2-arg
//!   `base gain`), `-o`, `-s`, `-t` (3-arg `u v w`), `-texres`
//!   (1-arg int), `-imfchan` (1-arg single char), `-type` (1-arg
//!   sphere|cube_*) — every option that runs out of arguments mid-
//!   walk must surface as `Err` rather than panic on the
//!   `iter.next().unwrap()`.
//! * **`refl -type` set bundling** — six face variants
//!   (`cube_top|cube_bottom|cube_front|cube_back|cube_left|cube_right`)
//!   plus `sphere`; an unknown `-type` value lands in the legacy
//!   single-string slot rather than panicking on the switch.
//! * **`d -halo factor`** — the `d` line has two forms: `d <factor>`
//!   and `d -halo <factor>`. The parser sniffs the `-halo` token; a
//!   `d -halo` with no following factor must surface as `Err`.
//! * **PBR extension** — `Pr`, `Pm`, `Pc`, `Ps`, `map_Pr`, `map_Pm`
//!   share the keyword-with-one-numeric-arg shape; a zero-token PBR
//!   line must surface as `Err`.
//! * **`newmtl` material-block dispatch** — every directive between
//!   two `newmtl` lines binds to the active material; a directive
//!   before the first `newmtl` must surface as `Err` (no implicit
//!   default material), not silently coalesce into a never-named
//!   slot.

use libfuzzer_sys::fuzz_target;
use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::{decoder::MtlDecoder, mtl};

fuzz_target!(|data: &[u8]| {
    // 1. High-level standalone-MTL decoder via the trait surface.
    let mut dec = MtlDecoder::new();
    let _ = dec.decode(data);

    // 2. Free-function entries. Drives `parse_mtl` (returns
    //    `Vec<Material>`) AND `parse_mtl_with_scene` (returns the full
    //    `Scene3D` shape) so both wrappers get exercised.
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = mtl::parse_mtl(text);
        let _ = mtl::parse_mtl_with_scene(text);
    }

    // 3. Truncation. Drives the per-block dispatch into truncated
    //    inputs (`newmtl` with no following name, `Tf spectral` with
    //    no file, `map_Kd -clamp` with no `on|off`, etc.). Capped at
    //    8 prefixes.
    let trunc_cap = data.len().min(8);
    for take in 0..trunc_cap {
        let mut dec = MtlDecoder::new();
        let _ = dec.decode(&data[..take]);
    }
});
