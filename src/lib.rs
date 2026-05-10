//! Pure-Rust Wavefront OBJ + MTL 3D mesh codec.
//!
//! Implements [`oxideav_mesh3d::Mesh3DDecoder`] and
//! [`oxideav_mesh3d::Mesh3DEncoder`] for the polygonal subset of the
//! OBJ format published by Wavefront Technologies in the early 1990s
//! (Appendix B of the *Advanced Visualizer* manual). Companion MTL
//! material files are parsed/serialised by the same crate so a
//! decoded [`Scene3D`](oxideav_mesh3d::Scene3D) carries its full
//! material set.
//!
//! # Modules
//!
//! * [`obj`] — line-oriented parser and serialiser for the geometry
//!   format itself (`v` / `vt` / `vn` / `f` / `l` / `o` / `g` / `s` /
//!   `usemtl` / `mtllib`).
//! * [`mtl`] — line-oriented parser and serialiser for the material
//!   library (`newmtl` / `Ka` / `Kd` / `Ks` / `Ns` / `Ni` / `d` /
//!   `Tr` / `illum` / `map_*` and the Wavefront-PBR extension
//!   `Pr` / `Pm` / `Pc` / `Ps` / `map_Pr` / `map_Pm`).
//! * [`decoder`] — [`ObjDecoder`] type wired to the
//!   [`Mesh3DDecoder`](oxideav_mesh3d::Mesh3DDecoder) trait.
//! * [`encoder`] — [`ObjEncoder`] type wired to the
//!   [`Mesh3DEncoder`](oxideav_mesh3d::Mesh3DEncoder) trait.
//!
//! # Standalone build
//!
//! Drop the `registry` feature for a free-standing build:
//!
//! ```toml
//! oxideav-obj = { version = "0.0", default-features = false }
//! ```
//!
//! This compiles out the [`register`] helper and the `oxideav-core`
//! dependency; the [`ObjDecoder`] and [`ObjEncoder`] types remain
//! usable directly through the standalone
//! [`Mesh3DDecoder`](oxideav_mesh3d::Mesh3DDecoder) /
//! [`Mesh3DEncoder`](oxideav_mesh3d::Mesh3DEncoder) traits.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod decoder;
pub mod encoder;
pub mod mtl;
pub mod obj;

pub use decoder::ObjDecoder;
pub use encoder::ObjEncoder;

/// Register OBJ + MTL decoders and encoders with a
/// [`Mesh3DRegistry`](oxideav_mesh3d::Mesh3DRegistry).
///
/// Two format ids land:
///
/// * `"obj"` — extension `.obj`. Decoder produces a fully-populated
///   `Scene3D`; encoder emits a single OBJ document and accepts a
///   companion MTL via [`ObjEncoder::with_mtl_basename`].
/// * `"mtl"` — extension `.mtl`. Decoder produces a `Scene3D` with
///   only the materials populated (no meshes / nodes); encoder emits
///   the MTL document for the scene's materials.
///
/// The OBJ encoder does **not** automatically write the companion MTL
/// — that's the caller's job (since they own the file-system context).
/// See [`mtl::serialize_mtl`] / [`obj::serialize_obj`] if you need to
/// drive both halves manually.
#[cfg(feature = "registry")]
pub fn register(registry: &mut oxideav_mesh3d::Mesh3DRegistry) {
    registry.register_decoder("obj", &["obj"], Box::new(|| Box::new(ObjDecoder::new())));
    registry.register_encoder("obj", &["obj"], Box::new(|| Box::new(ObjEncoder::new())));
    registry.register_decoder(
        "mtl",
        &["mtl"],
        Box::new(|| Box::new(decoder::MtlDecoder::new())),
    );
    registry.register_encoder(
        "mtl",
        &["mtl"],
        Box::new(|| Box::new(encoder::MtlEncoder::new())),
    );
}
