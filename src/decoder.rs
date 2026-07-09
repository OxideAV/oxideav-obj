//! [`Mesh3DDecoder`] adaptors for OBJ + standalone MTL inputs.
//!
//! These are thin wrappers around [`crate::obj::parse_obj`] /
//! [`crate::mtl::parse_mtl_with_scene`] that conform to the
//! `oxideav-mesh3d` decoder trait. The OBJ decoder carries one
//! optional knob — free-form curve tessellation (Bezier + B-spline,
//! rational and non-rational), opt-in via
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
    /// Free-form curve tessellation sample count (default 0 —
    /// disabled). See [`Self::with_curve_tessellation`].
    curve_tessellation_samples: u32,
    /// Vertex-normal synthesis policy for `vn`-less polygonal faces.
    /// Default [`obj::NormalGeneration::Disabled`]. See
    /// [`Self::with_normal_generation`].
    generate_normals: obj::NormalGeneration,
}

impl ObjDecoder {
    /// Construct a fresh decoder. Equivalent to `ObjDecoder::default()`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable tessellation of `cstype` free-form curves *and* `surf`
    /// surfaces:
    ///
    ///   * `cstype bezier` / `cstype rat bezier` — de Casteljau
    ///     evaluation on the `[0, 1]` basis domain (1D `curv` curves
    ///     *and* 2D `surf` surfaces via tensor-product de Casteljau).
    ///   * `cstype bspline` / `cstype rat bspline` — Cox-deBoor
    ///     recursive basis on the knot vector supplied by the most-
    ///     recent `parm u …` directive (1D `curv` *and* 2D `surf`
    ///     surfaces via tensor-product Cox-deBoor).
    ///   * `cstype cardinal` / `cstype rat cardinal` — cubic
    ///     Catmull-Rom (1D `curv` *and* 2D `surf` surfaces via
    ///     tensor-product Cardinal evaluation).
    ///   * `cstype taylor` / `cstype rat taylor` — direct
    ///     polynomial evaluation (1D `curv` Horner-rule *and* 2D
    ///     `surf` surfaces via tensor-product bivariate Horner-rule
    ///     evaluation).
    ///   * `cstype bmatrix` — 1D `curv` only.
    ///
    /// `samples` is the number of *intervals*. A `curv` curve yields a
    /// `LineStrip` of `samples + 1` vertices; a Bezier `surf` surface
    /// yields a `Topology::Triangles` grid of `(samples + 1)²` vertices
    /// (so `samples == 32` produces a 33×33 lattice, smooth enough for
    /// diagnostic display without overwhelming the buffer).
    ///
    /// Curve primitives land on a synthetic mesh named `"obj:curves"`;
    /// Bezier surface primitives land on `"obj:surfaces"`. Each carries
    /// `extras["obj:tessellated_curve"] == true` (surfaces also carry
    /// `extras["obj:tessellated_surface"] == true`) so downstream
    /// consumers that don't want the derived geometry can filter it
    /// out. The free-form directive sequence remains preserved in
    /// `Scene3D::extras["obj:freeform_directives"]` so re-encoding
    /// regenerates the original `cstype` / `deg` / `curv` / `surf` /
    /// `parm` / `end` section.
    ///
    /// `samples == 0` (the default) disables tessellation, matching
    /// the round 1-6 behaviour. Bezier / B-spline / Cardinal / Taylor
    /// `surf` surfaces are tessellated; basis-matrix surfaces remain
    /// captured-only.
    ///
    /// The budget is a caller-supplied knob, so it is clamped to
    /// [`obj::MAX_TESSELLATION_SAMPLES`] before it reaches any
    /// evaluator: an oversized value can neither overflow the internal
    /// `samples + 1` sample-index arithmetic nor demand an unbounded
    /// `(samples + 1)²` surface lattice. Values at or below the ceiling
    /// pass through unchanged, so this never affects a realistic
    /// diagnostic-density request.
    pub fn with_curve_tessellation(mut self, samples: u32) -> Self {
        self.curve_tessellation_samples = samples;
        self
    }

    /// Synthesise vertex normals for polygonal face primitives that
    /// carry no `vn` references, honouring the OBJ smoothing-group
    /// state (spec §"Grouping": smoothing groups are "a quick way to
    /// specify vertex normals").
    ///
    /// Faces in an active smoothing group (`s 1`, `s 2`, …) receive
    /// smooth, area-weighted averaged normals; faces with smoothing off
    /// (`s off` / `s 0`, or no `s` directive) receive faceted per-face
    /// normals (the primitive's vertices are de-shared so the hard edge
    /// between adjacent faces survives). Primitives that already carry
    /// explicit `vn` data are left untouched — the spec states explicit
    /// normals "supersede smoothing groups".
    ///
    /// Synthesised normals are flagged `obj:generated_normals` on the
    /// primitive (value `"smooth"` / `"flat"`) so the encoder re-emits
    /// the original `vn`-free face syntax rather than fabricating `vn`
    /// lines on a round-trip.
    ///
    /// The default ([`obj::NormalGeneration::Disabled`]) leaves
    /// `vn`-less primitives with `normals == None`, matching the
    /// historical behaviour.
    pub fn with_normal_generation(mut self, mode: obj::NormalGeneration) -> Self {
        self.generate_normals = mode;
        self
    }
}

impl Mesh3DDecoder for ObjDecoder {
    fn decode(&mut self, bytes: &[u8]) -> Result<Scene3D> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| oxideav_mesh3d::Error::invalid("OBJ input contained non-UTF-8 bytes"))?;
        let options = obj::ParseOptions {
            curve_tessellation_samples: self.curve_tessellation_samples,
            generate_normals: self.generate_normals,
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
