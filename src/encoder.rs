//! [`Mesh3DEncoder`] adaptors for OBJ + standalone MTL outputs.
//!
//! These wrap the [`crate::obj::serialize_obj`] /
//! [`crate::mtl::serialize_mtl`] free functions in the
//! `oxideav-mesh3d` encoder trait shape, with optional configuration
//! captured on the wrapper itself.

use oxideav_mesh3d::{Mesh3DEncoder, Result, Scene3D};

use crate::{mtl, obj};

/// Wavefront OBJ encoder.
///
/// By default the encoder emits a standalone OBJ (no `mtllib`
/// directive). Call [`ObjEncoder::with_mtl_basename`] to embed an
/// `mtllib <basename>.mtl` line at the top, in which case the caller
/// is responsible for writing the companion MTL file alongside (use
/// [`crate::mtl::serialize_mtl`] for that).
///
/// The encoder is loss-tolerant — it preserves the OBJ-specific
/// extras (`obj:groups`, `obj:smoothing_group`, `obj:original_face_arities`,
/// `obj:usemtl`, `obj:mtllibs`) populated by the decoder, so a
/// decode → encode → decode round-trip is structurally stable.
#[derive(Debug, Default)]
pub struct ObjEncoder {
    mtl_basename: Option<String>,
}

impl ObjEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Emit `mtllib <basename>.mtl` at the top of the output. The
    /// basename is used verbatim (no `.mtl` extension is appended
    /// twice; if you supply `"foo"` the directive becomes `mtllib foo.mtl`).
    pub fn with_mtl_basename(mut self, basename: impl Into<String>) -> Self {
        self.mtl_basename = Some(basename.into());
        self
    }
}

impl Mesh3DEncoder for ObjEncoder {
    fn encode(&mut self, scene: &Scene3D) -> Result<Vec<u8>> {
        obj::serialize_obj(scene, self.mtl_basename.as_deref())
    }
}

/// Standalone MTL encoder — serialises only the [`Scene3D::materials`]
/// vector (and any [`oxideav_mesh3d::Texture`]s referenced by them).
#[derive(Debug, Default)]
pub struct MtlEncoder {
    _private: (),
}

impl MtlEncoder {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Mesh3DEncoder for MtlEncoder {
    fn encode(&mut self, scene: &Scene3D) -> Result<Vec<u8>> {
        mtl::serialize_mtl(&scene.materials, &scene.textures)
    }
}
