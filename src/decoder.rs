//! [`Mesh3DDecoder`] adaptors for OBJ + standalone MTL inputs.
//!
//! These are thin wrappers around [`crate::obj::parse_obj`] /
//! [`crate::mtl::parse_mtl_with_scene`] that conform to the
//! `oxideav-mesh3d` decoder trait. Held state is empty for now (the
//! parsers don't carry options); the wrapper exists so registry-based
//! lookup gets a uniform `Box<dyn Mesh3DDecoder>` factory.

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
    _private: (),
}

impl ObjDecoder {
    /// Construct a fresh decoder. Equivalent to `ObjDecoder::default()`.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Mesh3DDecoder for ObjDecoder {
    fn decode(&mut self, bytes: &[u8]) -> Result<Scene3D> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| oxideav_mesh3d::Error::invalid("OBJ input contained non-UTF-8 bytes"))?;
        obj::parse_obj(text)
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
