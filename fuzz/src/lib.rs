//! Shared helpers for the `oxideav-obj-fuzz` targets.
//!
//! Currently empty — the `parse_obj` and `parse_mtl` targets are each
//! self-contained because the OBJ / MTL fuzz surface is decode-only
//! (no encoder bootstrap, no oracle cross-decode). The library exists
//! so future targets that need a shared corpus generator (e.g. a
//! structured Arbitrary-driven free-form directive synthesiser feeding
//! the encoder + decoder roundtrip) can grow here without re-wiring
//! Cargo.
