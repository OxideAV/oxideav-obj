//! [`Mesh3DDecoder`] adaptors for OBJ + standalone MTL inputs.
//!
//! These are thin wrappers around [`crate::obj::parse_obj`] /
//! [`crate::mtl::parse_mtl_with_scene`] that conform to the
//! `oxideav-mesh3d` decoder trait. The OBJ decoder carries one
//! optional knob — Bezier curve tessellation, opt-in via
//! [`ObjDecoder::with_curve_tessellation`]; the wrapper exists so
//! registry-based lookup gets a uniform `Box<dyn Mesh3DDecoder>`
//! factory.

use oxideav_mesh3d::{Mesh3DDecoder, Result, Scene3D};

use crate::{mtl, obj};

/// Wavefront OBJ decoder.
///
/// Decodes a self-contained OBJ document into a [`Scene3D`]. Companion
/// MTL files referenced by `mtllib` directives are not auto-resolved
/// (the decoder has no file-system context); the material names land
/// in `Primitive::extras["obj:usemtl"]` and the library file names
/// land in `Scene3D::extras["obj:mtllibs"]` so the caller can fetch
/// them and merge via [`crate::mtl::merge_materials_into_scene`].
///
/// Use [`obj::parse_obj_with_resolver`] directly when MTL bytes are
/// already available.
#[derive(Debug, Default)]
pub struct ObjDecoder {
    /// Bezier curve tessellation sample count (default 0 — disabled).
    /// See [`Self::with_curve_tessellation`].
    curve_tessellation_samples: u32,
}

impl ObjDecoder {
    /// Construct a fresh decoder. Equivalent to `ObjDecoder::default()`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable de Casteljau tessellation of `cstype bezier` /
    /// `cstype rat bezier` free-form curves. `samples` is the number
    /// of *intervals* — the resulting `LineStrip` carries
    /// `samples + 1` vertices (so `samples == 32` produces a 33-point
    /// polyline per curve, smooth enough for diagnostic display
    /// without overwhelming the buffer).
    ///
    /// The synthesised primitives land on a synthetic mesh named
    /// `"obj:curves"`; each primitive carries
    /// `extras["obj:tessellated_curve"] == true` so downstream
    /// consumers that don't want the curve geometry can filter it
    /// out. The free-form directive sequence remains preserved in
    /// `Scene3D::extras["obj:freeform_directives"]` so re-encoding
    /// regenerates the original `cstype` / `deg` / `curv` / `end`
    /// section.
    ///
    /// `samples == 0` (the default) disables tessellation, matching
    /// the round 1-6 behaviour.
    pub fn with_curve_tessellation(mut self, samples: u32) -> Self {
        self.curve_tessellation_samples = samples;
        self
    }
}

impl Mesh3DDecoder for ObjDecoder {
    fn decode(&mut self, bytes: &[u8]) -> Result<Scene3D> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| oxideav_mesh3d::Error::invalid("OBJ input contained non-UTF-8 bytes"))?;
        let options = obj::ParseOptions {
            curve_tessellation_samples: self.curve_tessellation_samples,
        };
        obj::parse_obj_with_options(text, &options, |_path| Ok(Vec::new()))
    }
}

/// Standalone MTL decoder. Returns a [`Scene3D`] populated with the
/// MTL's materials and any external textures they reference; no
/// meshes / nodes / cameras / lights / animations are added.
#[derive(Debug, Default)]
pub struct MtlDecoder {
    _private: (),
}

impl MtlDecoder {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Mesh3DDecoder for MtlDecoder {
    fn decode(&mut self, bytes: &[u8]) -> Result<Scene3D> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| oxideav_mesh3d::Error::invalid("MTL input contained non-UTF-8 bytes"))?;
        mtl::parse_mtl_with_scene(text)
    }
}
