//! Wavefront OBJ ASCII parser + serialiser.
//!
//! Strictly the polygonal subset (vertex / face / line / grouping /
//! material directives). Free-form curves/surfaces and the `.mod`
//! binary form are intentionally not handled.
//!
//! The grammar is line-oriented; whitespace-separated; `#` introduces
//! a comment to end of line. Continuation lines (trailing `\\`) are
//! supported by gluing the next line on before tokenisation.

use std::collections::HashMap;

use oxideav_mesh3d::{Error, Indices, Mesh, Primitive, Result, Scene3D, Topology};

use crate::mtl::parse_mtl;

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Per-face-vertex index triple. `0` means "not present".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct FaceVert {
    /// 1-based geometric-vertex index (resolved from raw OBJ).
    v: u32,
    /// 1-based texture-coord index, or 0 if absent.
    vt: u32,
    /// 1-based normal index, or 0 if absent.
    vn: u32,
}

/// One face-or-line element captured during the first parse pass.
#[derive(Debug)]
enum Element {
    Face(Vec<FaceVert>),
    Line(Vec<FaceVert>),
}

/// One open primitive — accumulates face/line elements while a single
/// `usemtl` (or "no material") is active.
#[derive(Debug, Default)]
struct PrimAccum {
    elements: Vec<Element>,
    material: Option<String>,
    /// Last seen smoothing group token (`"off"` or an integer string).
    smoothing_group: Option<String>,
    /// All distinct group names seen during this primitive.
    groups: Vec<String>,
}

/// One open mesh — accumulates primitives while a single `o <name>`
/// (or default object) is active.
#[derive(Debug, Default)]
struct MeshAccum {
    name: Option<String>,
    primitives: Vec<PrimAccum>,
}

impl MeshAccum {
    fn current_or_new(&mut self) -> &mut PrimAccum {
        if self.primitives.is_empty() {
            self.primitives.push(PrimAccum::default());
        }
        self.primitives.last_mut().unwrap()
    }
}

/// The polygonal data parsed out of an OBJ document.
///
/// This intermediate form keeps positions / texcoords / normals in
/// their original 1-based numbering so the resolution of negative and
/// 1-based face indices into 0-based primitive-local indices happens
/// in one well-defined place ([`build_scene`]).
#[derive(Debug, Default)]
struct ObjDoc {
    positions: Vec<[f32; 3]>,
    texcoords: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    /// Material library file names referenced by `mtllib`.
    mtllibs: Vec<String>,
    /// All material definitions resolved from `mtllib` references
    /// supplied via [`ObjDoc::with_resolved_mtllibs`]. Round 1 ships
    /// no IO so we accept these via an external resolver hook on the
    /// caller.
    resolved_materials: HashMap<String, oxideav_mesh3d::Material>,
    meshes: Vec<MeshAccum>,
}

/// Glue line-continuation (`\\` + newline) before line splitting and
/// strip comments (`#…` to end of line). Returns owned strings since
/// continuation gluing rewrites the input.
fn preprocess_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut acc = String::new();
    for raw_line in text.split('\n') {
        // Strip a trailing CR so CRLF inputs land cleanly.
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        // Strip comments — `#` past the start of a token introduces
        // an end-of-line comment per the spec.
        let no_comment = match line.find('#') {
            Some(idx) => &line[..idx],
            None => line,
        };
        let trimmed = no_comment.trim_end();
        if let Some(stripped) = trimmed.strip_suffix('\\') {
            acc.push_str(stripped);
            acc.push(' ');
        } else {
            acc.push_str(trimmed);
            out.push(std::mem::take(&mut acc));
        }
    }
    if !acc.is_empty() {
        out.push(acc);
    }
    out
}

/// Parse a face-vertex token. Accepts `v`, `v/vt`, `v//vn`, `v/vt/vn`.
/// Each component is a non-zero integer (negative => relative-from-end).
/// Resolution to 1-based positive indices happens here; 0-based
/// primitive-local indexing happens in [`build_scene`].
fn parse_face_vertex(tok: &str, n_pos: i64, n_tex: i64, n_norm: i64) -> Result<FaceVert> {
    let mut parts = tok.split('/');
    let v = parts
        .next()
        .ok_or_else(|| Error::invalid(format!("face vertex missing position: {tok:?}")))?;
    let vt = parts.next().unwrap_or("");
    let vn = parts.next().unwrap_or("");

    let resolve = |s: &str, n: i64, kind: &str| -> Result<u32> {
        if s.is_empty() {
            return Ok(0);
        }
        let raw: i64 = s.parse().map_err(|_| {
            Error::invalid(format!(
                "invalid {kind} index in face vertex {tok:?}: {s:?}"
            ))
        })?;
        let resolved = if raw < 0 { n + 1 + raw } else { raw };
        if resolved <= 0 || resolved > n {
            return Err(Error::invalid(format!(
                "{kind} index out of range in face vertex {tok:?}: {raw} (have {n})"
            )));
        }
        Ok(resolved as u32)
    };

    Ok(FaceVert {
        v: resolve(v, n_pos, "position")?,
        vt: resolve(vt, n_tex, "texcoord")?,
        vn: resolve(vn, n_norm, "normal")?,
    })
}

/// Parse the geometry part of an OBJ document into the intermediate
/// [`ObjDoc`] form. No I/O — `mtllib` lines are recorded by name only;
/// the caller resolves them.
fn parse_obj_doc(text: &str) -> Result<ObjDoc> {
    let mut doc = ObjDoc::default();
    // One implicit mesh until an `o` directive opens a named one.
    doc.meshes.push(MeshAccum::default());

    let lines = preprocess_lines(text);
    for line in &lines {
        let mut tokens = line.split_whitespace();
        let Some(keyword) = tokens.next() else {
            continue;
        };
        match keyword {
            "v" => {
                let coords: Vec<f32> = tokens
                    .map(str::parse)
                    .collect::<std::result::Result<Vec<f32>, _>>()
                    .map_err(|e| Error::invalid(format!("v: bad float ({e})")))?;
                if coords.len() < 3 {
                    return Err(Error::invalid(format!(
                        "v: expected ≥3 coords, got {}",
                        coords.len()
                    )));
                }
                // The optional 4th `w` is silently dropped (rare in practice).
                doc.positions.push([coords[0], coords[1], coords[2]]);
            }
            "vt" => {
                let coords: Vec<f32> = tokens
                    .map(str::parse)
                    .collect::<std::result::Result<Vec<f32>, _>>()
                    .map_err(|e| Error::invalid(format!("vt: bad float ({e})")))?;
                if coords.is_empty() {
                    return Err(Error::invalid("vt: expected ≥1 coord"));
                }
                let u = coords[0];
                let v = coords.get(1).copied().unwrap_or(0.0);
                // Drop optional 3rd `w` — meaningless to glTF UV.
                doc.texcoords.push([u, v]);
            }
            "vn" => {
                let coords: Vec<f32> = tokens
                    .map(str::parse)
                    .collect::<std::result::Result<Vec<f32>, _>>()
                    .map_err(|e| Error::invalid(format!("vn: bad float ({e})")))?;
                if coords.len() != 3 {
                    return Err(Error::invalid(format!(
                        "vn: expected 3 coords, got {}",
                        coords.len()
                    )));
                }
                doc.normals.push([coords[0], coords[1], coords[2]]);
            }
            "vp" => {
                // Parameter-space vertex — silently skipped (free-form
                // surface support is out of scope for round 1).
            }
            "f" => {
                let n_pos = doc.positions.len() as i64;
                let n_tex = doc.texcoords.len() as i64;
                let n_norm = doc.normals.len() as i64;
                let verts: Vec<FaceVert> = tokens
                    .map(|t| parse_face_vertex(t, n_pos, n_tex, n_norm))
                    .collect::<Result<Vec<_>>>()?;
                if verts.len() < 3 {
                    return Err(Error::invalid(format!(
                        "f: face needs ≥3 vertices, got {}",
                        verts.len()
                    )));
                }
                let mesh = doc.meshes.last_mut().unwrap();
                mesh.current_or_new().elements.push(Element::Face(verts));
            }
            "l" => {
                let n_pos = doc.positions.len() as i64;
                let n_tex = doc.texcoords.len() as i64;
                let n_norm = doc.normals.len() as i64;
                let verts: Vec<FaceVert> = tokens
                    .map(|t| parse_face_vertex(t, n_pos, n_tex, n_norm))
                    .collect::<Result<Vec<_>>>()?;
                if verts.len() < 2 {
                    return Err(Error::invalid(format!(
                        "l: line needs ≥2 vertices, got {}",
                        verts.len()
                    )));
                }
                let mesh = doc.meshes.last_mut().unwrap();
                mesh.current_or_new().elements.push(Element::Line(verts));
            }
            "p" => {
                // Point element — not modelled (mesh3d Topology::Points
                // exists but OBJ point usage is vanishingly rare).
                // Silently skip.
            }
            "o" => {
                let name: String = tokens.collect::<Vec<_>>().join(" ");
                // Open a fresh mesh — but if the current mesh is still
                // empty (no primitives accumulated yet), reuse it so we
                // don't end up with a leading empty mesh.
                let last = doc.meshes.last_mut().unwrap();
                if last.name.is_none() && last.primitives.is_empty() {
                    last.name = if name.is_empty() { None } else { Some(name) };
                } else {
                    doc.meshes.push(MeshAccum {
                        name: if name.is_empty() { None } else { Some(name) },
                        primitives: Vec::new(),
                    });
                }
            }
            "g" => {
                // The spec (Wavefront *Advanced Visualizer* Appendix B,
                // §"Grouping") explicitly permits multiple group names
                // on one line: `g group_name1 group_name2 …`. Each
                // whitespace-separated token is its own group; the
                // following elements belong to ALL listed groups.
                let names: Vec<String> = tokens.map(|t| t.to_string()).collect();
                if names.is_empty() {
                    continue;
                }
                let mesh = doc.meshes.last_mut().unwrap();
                let prim = mesh.current_or_new();
                for name in names {
                    if !prim.groups.iter().any(|g| g == &name) {
                        prim.groups.push(name);
                    }
                }
            }
            "s" => {
                // `s 0` and `s off` both mean "no smoothing"; preserve
                // the operator's chosen spelling verbatim for round-trip.
                let v: String = tokens.collect::<Vec<_>>().join(" ");
                if v.is_empty() {
                    continue;
                }
                let mesh = doc.meshes.last_mut().unwrap();
                let last = mesh.current_or_new();
                if last.elements.is_empty() {
                    // No elements yet — overwrite the pending value.
                    last.smoothing_group = Some(v);
                } else if last.smoothing_group.as_deref() != Some(v.as_str()) {
                    // Smoothing changed mid-stream; spec says it's
                    // state-setting and applies to subsequent
                    // elements, so split into a new primitive that
                    // inherits the current material + groups.
                    let mat = last.material.clone();
                    let groups = last.groups.clone();
                    mesh.primitives.push(PrimAccum {
                        material: mat,
                        smoothing_group: Some(v),
                        groups,
                        elements: Vec::new(),
                    });
                }
            }
            "usemtl" => {
                let name: String = tokens.collect::<Vec<_>>().join(" ");
                let mesh = doc.meshes.last_mut().unwrap();
                let last = mesh.current_or_new();
                if last.elements.is_empty() && last.material.is_none() {
                    // First usemtl in this primitive — adopt directly.
                    last.material = if name.is_empty() { None } else { Some(name) };
                } else {
                    // Subsequent usemtl — start a new primitive.
                    mesh.primitives.push(PrimAccum {
                        material: if name.is_empty() { None } else { Some(name) },
                        ..PrimAccum::default()
                    });
                }
            }
            "mtllib" => {
                // Each `mtllib` line can list multiple .mtl files.
                for tok in tokens {
                    if !doc.mtllibs.iter().any(|m| m == tok) {
                        doc.mtllibs.push(tok.to_string());
                    }
                }
            }
            // Unhandled keywords (curves/surfaces/display attributes/etc.) are
            // silently skipped per spec lenient-loader convention.
            _ => {}
        }
    }

    Ok(doc)
}

// ---------------------------------------------------------------------------
// Scene assembly
// ---------------------------------------------------------------------------

/// Convert the intermediate [`ObjDoc`] into a [`Scene3D`].
///
/// Indices are de-duplicated per-primitive so the resulting vertex
/// buffer carries `unique_face_vertices` entries (matching glTF's
/// per-primitive interleaved-attribute model). Original face arities
/// are stored in `Mesh::extras["obj:original_face_arities"]` so the
/// encoder can reconstruct the n-gons.
fn build_scene(doc: ObjDoc) -> Result<Scene3D> {
    use oxideav_mesh3d::{Axis, Material, Unit};

    let mut scene = Scene3D::new();
    // OBJ has no unit metadata; the primer says "Metres is the safe
    // default" and "Y-up matches the glTF default".
    scene.up_axis = Axis::PosY;
    scene.unit = Unit::Metres;

    // Materials first so primitives can point at their MaterialId.
    // Insertion order is preserved (HashMap iteration order is
    // unspecified, so sort by name to keep round-trip deterministic).
    let mut material_ids: HashMap<String, oxideav_mesh3d::MaterialId> = HashMap::new();
    let mut material_names: Vec<String> = doc.resolved_materials.keys().cloned().collect();
    material_names.sort();
    for name in &material_names {
        let mut mat = doc
            .resolved_materials
            .get(name)
            .cloned()
            .unwrap_or_else(Material::new);
        if mat.name.is_none() {
            mat.name = Some(name.clone());
        }
        let id = scene.add_material(mat);
        material_ids.insert(name.clone(), id);
    }

    for mesh_acc in doc.meshes {
        // Drop genuinely empty meshes (no primitives that emit anything).
        let has_anything = mesh_acc.primitives.iter().any(|p| !p.elements.is_empty());
        if !has_anything {
            continue;
        }

        let mut mesh = Mesh::new(mesh_acc.name.clone());

        for prim_acc in mesh_acc.primitives {
            let (mut primitive, arities) = build_primitive(
                &prim_acc,
                &doc.positions,
                &doc.texcoords,
                &doc.normals,
                &material_ids,
            )?;
            // Skip primitives that never accumulated any element.
            if primitive.positions.is_empty() {
                continue;
            }
            // Stash original face arities per-primitive when the primitive
            // contained at least one non-triangle face. Mesh has no
            // `extras` field, so the round-trip annotation lives on the
            // primitive — symmetrical with the smoothing-group / groups /
            // usemtl extras already populated by `build_primitive`.
            if arities.iter().any(|&a| a != 3) {
                primitive.extras.insert(
                    "obj:original_face_arities".to_string(),
                    serde_json::to_value(&arities).unwrap(),
                );
            }
            mesh.primitives.push(primitive);
        }

        scene.add_mesh(mesh);
    }

    // Keep the mtllib references in scene extras so a re-encode that
    // wants to point back at a specific MTL file can find them.
    if !doc.mtllibs.is_empty() {
        scene.extras.insert(
            "obj:mtllibs".to_string(),
            serde_json::to_value(&doc.mtllibs).unwrap(),
        );
    }

    Ok(scene)
}

/// Build one [`Primitive`] from an accumulated [`PrimAccum`].
///
/// Returns the primitive plus a per-element arity vector — one entry
/// per face (3 for a triangle, 4 for a quad, ≥5 for an n-gon). Lines
/// don't contribute arity entries (the encoder switches on topology
/// instead).
fn build_primitive(
    prim_acc: &PrimAccum,
    positions: &[[f32; 3]],
    texcoords: &[[f32; 2]],
    normals: &[[f32; 3]],
    material_ids: &HashMap<String, oxideav_mesh3d::MaterialId>,
) -> Result<(Primitive, Vec<u32>)> {
    // Decide topology + attribute presence by looking at the first
    // element. Mixed-element primitives (lines + faces under one
    // `usemtl`) aren't representable in mesh3d so we error cleanly.
    let first = prim_acc.elements.first();
    let topology = match first {
        Some(Element::Face(_)) => Topology::Triangles,
        Some(Element::Line(_)) => Topology::Lines,
        None => Topology::Triangles,
    };
    for elt in &prim_acc.elements {
        let ok = matches!(
            (&topology, elt),
            (Topology::Triangles, Element::Face(_)) | (Topology::Lines, Element::Line(_))
        );
        if !ok {
            return Err(Error::unsupported(
                "OBJ primitive mixes face and line elements under one usemtl",
            ));
        }
    }

    let has_uv = prim_acc.elements.iter().any(|elt| match elt {
        Element::Face(verts) | Element::Line(verts) => verts.iter().any(|fv| fv.vt != 0),
    });
    let has_normal = prim_acc.elements.iter().any(|elt| match elt {
        Element::Face(verts) | Element::Line(verts) => verts.iter().any(|fv| fv.vn != 0),
    });

    let mut prim = Primitive::new(topology);
    if has_uv {
        prim.uvs.push(Vec::new());
    }
    if has_normal {
        prim.normals = Some(Vec::new());
    }

    // De-duplicate face-vertices into a single interleaved buffer.
    let mut indexer: HashMap<FaceVert, u32> = HashMap::new();
    let mut arities: Vec<u32> = Vec::new();
    let mut local_indices: Vec<u32> = Vec::new();

    let intern =
        |fv: FaceVert, prim: &mut Primitive, indexer: &mut HashMap<FaceVert, u32>| -> Result<u32> {
            if let Some(&idx) = indexer.get(&fv) {
                return Ok(idx);
            }
            let pos = positions.get((fv.v - 1) as usize).ok_or_else(|| {
                Error::invalid(format!("face references missing position {}", fv.v))
            })?;
            prim.positions.push(*pos);
            if has_uv {
                let uv = if fv.vt == 0 {
                    [0.0, 0.0]
                } else {
                    *texcoords.get((fv.vt - 1) as usize).ok_or_else(|| {
                        Error::invalid(format!("face references missing texcoord {}", fv.vt))
                    })?
                };
                prim.uvs[0].push(uv);
            }
            if has_normal {
                let n = if fv.vn == 0 {
                    [0.0, 0.0, 0.0]
                } else {
                    *normals.get((fv.vn - 1) as usize).ok_or_else(|| {
                        Error::invalid(format!("face references missing normal {}", fv.vn))
                    })?
                };
                prim.normals.as_mut().unwrap().push(n);
            }
            let new_idx = (prim.positions.len() - 1) as u32;
            indexer.insert(fv, new_idx);
            Ok(new_idx)
        };

    for elt in &prim_acc.elements {
        match elt {
            Element::Face(verts) => {
                let arity = verts.len() as u32;
                arities.push(arity);
                let resolved: Vec<u32> = verts
                    .iter()
                    .map(|&fv| intern(fv, &mut prim, &mut indexer))
                    .collect::<Result<Vec<_>>>()?;
                // Fan triangulate: (v0, v1, v2), (v0, v2, v3), …
                for i in 1..(resolved.len() - 1) {
                    local_indices.push(resolved[0]);
                    local_indices.push(resolved[i]);
                    local_indices.push(resolved[i + 1]);
                }
            }
            Element::Line(verts) => {
                let resolved: Vec<u32> = verts
                    .iter()
                    .map(|&fv| intern(fv, &mut prim, &mut indexer))
                    .collect::<Result<Vec<_>>>()?;
                // Decompose polyline into Lines (pairs).
                for w in resolved.windows(2) {
                    local_indices.push(w[0]);
                    local_indices.push(w[1]);
                }
            }
        }
    }

    // Promote to U32 if any index >= 65536; U16 otherwise.
    if local_indices.iter().any(|&i| i >= u16::MAX as u32) {
        prim.indices = Some(Indices::U32(local_indices));
    } else {
        prim.indices = Some(Indices::U16(
            local_indices.into_iter().map(|i| i as u16).collect(),
        ));
    }

    if let Some(name) = &prim_acc.material {
        if let Some(id) = material_ids.get(name) {
            prim.material = Some(*id);
        }
        prim.extras.insert(
            "obj:usemtl".to_string(),
            serde_json::Value::String(name.clone()),
        );
    }
    if let Some(s) = &prim_acc.smoothing_group {
        prim.extras.insert(
            "obj:smoothing_group".to_string(),
            serde_json::Value::String(s.clone()),
        );
    }
    if !prim_acc.groups.is_empty() {
        prim.extras.insert(
            "obj:groups".to_string(),
            serde_json::to_value(&prim_acc.groups).unwrap(),
        );
    }

    Ok((prim, arities))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse an OBJ document (no MTL resolution).
///
/// `usemtl` directives still create one `Primitive` per switch and the
/// material name lands in `Primitive::extras["obj:usemtl"]` even with
/// no actual `Material` constructed. Use [`parse_obj_with_resolver`]
/// when companion MTL data is available.
pub fn parse_obj(text: &str) -> Result<Scene3D> {
    parse_obj_with_resolver(text, |_path| Ok(Vec::new()))
}

/// Parse an OBJ document at `path`, resolving `mtllib` references
/// against the OBJ file's parent directory.
///
/// Convenience wrapper around [`parse_obj_with_resolver`] for the
/// overwhelmingly common case of "I have a path, please load it and
/// follow the MTL references". Each `mtllib foo.mtl` directive becomes
/// a sibling-file read; missing libraries surface the underlying
/// [`std::io::Error`] (wrapped in [`Error::invalid`]) rather than
/// silently dropping. If you want lenient missing-MTL handling, use
/// [`parse_obj_with_resolver`] directly.
pub fn parse_obj_from_path<P: AsRef<std::path::Path>>(path: P) -> Result<Scene3D> {
    let path = path.as_ref();
    let bytes =
        std::fs::read(path).map_err(|e| Error::invalid(format!("OBJ read {path:?}: {e}")))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Error::invalid(format!("OBJ {path:?} contained non-UTF-8 bytes")))?;
    let parent = path.parent().map(std::path::Path::to_path_buf);
    parse_obj_with_resolver(text, |libname| {
        // Empty / absolute / parent-relative library names are honoured
        // verbatim; bare names are resolved against the OBJ's parent
        // directory.
        let lib_path = match &parent {
            Some(dir) => dir.join(libname),
            None => std::path::PathBuf::from(libname),
        };
        std::fs::read(&lib_path)
            .map_err(|e| Error::invalid(format!("mtllib read {lib_path:?}: {e}")))
    })
}

/// Parse an OBJ document, calling `resolve` once per `mtllib` entry to
/// fetch the bytes of the named material library. Each library is
/// parsed via [`parse_mtl`] and its materials merged into the resulting
/// scene; references in `usemtl` directives bind to those materials by
/// name.
///
/// The resolver returns `Ok(Vec::new())` to signal "this library
/// couldn't be located but skip silently"; any other `Err` aborts the
/// parse.
pub fn parse_obj_with_resolver<R>(text: &str, mut resolve: R) -> Result<Scene3D>
where
    R: FnMut(&str) -> Result<Vec<u8>>,
{
    let mut doc = parse_obj_doc(text)?;

    // Resolve material libraries, if any.
    for lib in doc.mtllibs.clone() {
        let bytes = resolve(&lib)?;
        if bytes.is_empty() {
            continue;
        }
        let lib_text = std::str::from_utf8(&bytes)
            .map_err(|_| Error::invalid(format!("mtllib {lib:?} contained non-UTF-8 bytes")))?;
        let materials = parse_mtl(lib_text)?;
        for mat in materials {
            if let Some(name) = mat.name.clone() {
                doc.resolved_materials.insert(name, mat);
            }
        }
    }

    build_scene(doc)
}

/// Serialiser configuration. Keeps the public free-function signature
/// stable while letting the [`crate::ObjEncoder`] thread richer options
/// through.
#[derive(Clone, Debug, Default)]
pub struct SerializeOptions<'a> {
    /// Reference an external MTL file via an `mtllib <basename>.mtl`
    /// header line. Equivalent to the `mtl_basename` parameter on
    /// [`serialize_obj`].
    pub mtl_basename: Option<&'a str>,
    /// When `true`, emit face/line vertex indices in the relative
    /// negative-index form (`f -1 -2 -3`) instead of absolute 1-based.
    /// Round-trips verbatim back through the parser; useful when the
    /// caller wants their re-encoded OBJ to mirror an input that used
    /// negative indices throughout.
    pub negative_indices: bool,
}

/// Serialise a [`Scene3D`] to OBJ format.
///
/// `mtl_basename`, when supplied, emits an `mtllib <basename>.mtl`
/// directive at the top so a sibling MTL file (written separately via
/// [`crate::mtl::serialize_mtl`]) is referenced.
pub fn serialize_obj(scene: &Scene3D, mtl_basename: Option<&str>) -> Result<Vec<u8>> {
    serialize_obj_with_options(
        scene,
        &SerializeOptions {
            mtl_basename,
            ..SerializeOptions::default()
        },
    )
}

/// Serialise a [`Scene3D`] to OBJ format with explicit options.
///
/// See [`SerializeOptions`] for the supported knobs.
pub fn serialize_obj_with_options(
    scene: &Scene3D,
    options: &SerializeOptions<'_>,
) -> Result<Vec<u8>> {
    let mtl_basename = options.mtl_basename;
    let negative = options.negative_indices;
    use std::fmt::Write;
    let mut out = String::new();
    writeln!(out, "# OBJ generated by oxideav-obj").unwrap();
    if let Some(base) = mtl_basename {
        writeln!(out, "mtllib {base}.mtl").unwrap();
    }
    // Replay any mtllib refs preserved on the scene itself when no
    // explicit basename was supplied.
    if mtl_basename.is_none() {
        if let Some(serde_json::Value::Array(list)) = scene.extras.get("obj:mtllibs") {
            for entry in list {
                if let Some(s) = entry.as_str() {
                    writeln!(out, "mtllib {s}").unwrap();
                }
            }
        }
    }

    // Deduplicated global vertex / texcoord / normal pools so emitted
    // index references match the canonical 1-based numbering.
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut texcoords: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut pos_map: HashMap<KeyVec3, u32> = HashMap::new();
    let mut tex_map: HashMap<KeyVec2, u32> = HashMap::new();
    let mut nor_map: HashMap<KeyVec3, u32> = HashMap::new();

    let intern_pos =
        |p: [f32; 3], positions: &mut Vec<[f32; 3]>, map: &mut HashMap<KeyVec3, u32>| -> u32 {
            let key = KeyVec3::from(p);
            if let Some(&i) = map.get(&key) {
                return i;
            }
            positions.push(p);
            let idx = positions.len() as u32;
            map.insert(key, idx);
            idx
        };
    let intern_tex =
        |p: [f32; 2], texcoords: &mut Vec<[f32; 2]>, map: &mut HashMap<KeyVec2, u32>| -> u32 {
            let key = KeyVec2::from(p);
            if let Some(&i) = map.get(&key) {
                return i;
            }
            texcoords.push(p);
            let idx = texcoords.len() as u32;
            map.insert(key, idx);
            idx
        };
    let intern_nor =
        |p: [f32; 3], normals: &mut Vec<[f32; 3]>, map: &mut HashMap<KeyVec3, u32>| -> u32 {
            let key = KeyVec3::from(p);
            if let Some(&i) = map.get(&key) {
                return i;
            }
            normals.push(p);
            let idx = normals.len() as u32;
            map.insert(key, idx);
            idx
        };

    // First pass: emit `v` / `vt` / `vn` lists and remember the global
    // indices for each (mesh, primitive, vertex) triple.
    type GlobalTriple = (u32, u32, u32); // (v_idx, vt_idx_or_0, vn_idx_or_0)
    let mut global_indices: Vec<Vec<Vec<GlobalTriple>>> = Vec::new();
    for mesh in &scene.meshes {
        let mut mesh_globals: Vec<Vec<GlobalTriple>> = Vec::new();
        for prim in &mesh.primitives {
            let has_uv = !prim.uvs.is_empty();
            let has_normal = prim.normals.is_some();
            let mut prim_globals: Vec<GlobalTriple> = Vec::with_capacity(prim.positions.len());
            for vi in 0..prim.positions.len() {
                let v_idx = intern_pos(prim.positions[vi], &mut positions, &mut pos_map);
                let vt_idx = if has_uv {
                    intern_tex(prim.uvs[0][vi], &mut texcoords, &mut tex_map)
                } else {
                    0
                };
                let vn_idx = if has_normal {
                    intern_nor(
                        prim.normals.as_ref().unwrap()[vi],
                        &mut normals,
                        &mut nor_map,
                    )
                } else {
                    0
                };
                prim_globals.push((v_idx, vt_idx, vn_idx));
            }
            mesh_globals.push(prim_globals);
        }
        global_indices.push(mesh_globals);
    }

    for p in &positions {
        writeln!(
            out,
            "v {} {} {}",
            fmt_float(p[0]),
            fmt_float(p[1]),
            fmt_float(p[2])
        )
        .unwrap();
    }
    for t in &texcoords {
        writeln!(out, "vt {} {}", fmt_float(t[0]), fmt_float(t[1])).unwrap();
    }
    for n in &normals {
        writeln!(
            out,
            "vn {} {} {}",
            fmt_float(n[0]),
            fmt_float(n[1]),
            fmt_float(n[2])
        )
        .unwrap();
    }

    // Second pass: per-mesh `o` directive, per-primitive `usemtl` +
    // groups + smoothing-group, then face/line elements.
    for (mi, mesh) in scene.meshes.iter().enumerate() {
        if let Some(name) = &mesh.name {
            writeln!(out, "o {name}").unwrap();
        }

        for (pi, prim) in mesh.primitives.iter().enumerate() {
            // Per-primitive arity vector for n-gon re-emission, if any.
            let arities: Option<Vec<u32>> = prim
                .extras
                .get("obj:original_face_arities")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            // Groups + smoothing first (spec convention: state tokens
            // precede the elements they apply to).
            if let Some(serde_json::Value::Array(gs)) = prim.extras.get("obj:groups") {
                let names: Vec<&str> = gs.iter().filter_map(|v| v.as_str()).collect();
                if !names.is_empty() {
                    writeln!(out, "g {}", names.join(" ")).unwrap();
                }
            }
            if let Some(s) = prim
                .extras
                .get("obj:smoothing_group")
                .and_then(|v| v.as_str())
            {
                writeln!(out, "s {s}").unwrap();
            }

            // usemtl: prefer extras["obj:usemtl"] (loss-tolerant
            // round-trip name), fall back to the bound material's name.
            let mtl_name: Option<String> = prim
                .extras
                .get("obj:usemtl")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    prim.material.and_then(|id| {
                        scene
                            .materials
                            .get(id.0 as usize)
                            .and_then(|m| m.name.clone())
                    })
                });
            if let Some(name) = &mtl_name {
                writeln!(out, "usemtl {name}").unwrap();
            }

            let prim_globals = &global_indices[mi][pi];
            let has_uv = !prim.uvs.is_empty();
            let has_normal = prim.normals.is_some();

            // Build the per-element index iterator. For Triangles topology
            // re-shape into n-gons via `arities` if present; otherwise emit
            // one triangle per 3 indices. For Lines topology emit `l`
            // per pair (we don't reverse strips back into polylines —
            // that's lossy and the round-trip test doesn't need it).
            match prim.topology {
                Topology::Triangles => {
                    let face_indices: Vec<u32> = match &prim.indices {
                        Some(Indices::U16(v)) => v.iter().map(|&x| x as u32).collect(),
                        Some(Indices::U32(v)) => v.clone(),
                        None => {
                            // Implicit indices: 0, 1, 2, …
                            (0..prim.positions.len() as u32).collect()
                        }
                    };
                    if let Some(per_prim_arities) = arities.as_ref() {
                        // Reconstruct n-gons from triangle fans. Each
                        // n-gon contributed (n - 2) triangles.
                        let mut tri_pos: usize = 0;
                        for &arity in per_prim_arities {
                            let mut verts: Vec<u32> = Vec::with_capacity(arity as usize);
                            // The fan was: (v0, v1, v2), (v0, v2, v3), (v0, v3, v4), …
                            let n_tris = (arity as usize).saturating_sub(2);
                            // First triangle gives v0, v1, v2.
                            verts.push(face_indices[tri_pos * 3]);
                            verts.push(face_indices[tri_pos * 3 + 1]);
                            verts.push(face_indices[tri_pos * 3 + 2]);
                            // Each subsequent triangle adds one new vertex (the third index).
                            for k in 1..n_tris {
                                verts.push(face_indices[(tri_pos + k) * 3 + 2]);
                            }
                            tri_pos += n_tris;

                            write_face(
                                &mut out,
                                &verts,
                                prim_globals,
                                has_uv,
                                has_normal,
                                negative,
                                positions.len() as u32,
                                texcoords.len() as u32,
                                normals.len() as u32,
                            );
                        }
                        // Any leftover triangles after the recorded arities
                        // (e.g. a primitive grew after the arity vector was
                        // captured) are emitted as plain triangles.
                        let consumed = per_prim_arities
                            .iter()
                            .map(|&a| (a as usize).saturating_sub(2))
                            .sum::<usize>();
                        for tri in consumed..(face_indices.len() / 3) {
                            let verts = [
                                face_indices[tri * 3],
                                face_indices[tri * 3 + 1],
                                face_indices[tri * 3 + 2],
                            ];
                            write_face(
                                &mut out,
                                &verts,
                                prim_globals,
                                has_uv,
                                has_normal,
                                negative,
                                positions.len() as u32,
                                texcoords.len() as u32,
                                normals.len() as u32,
                            );
                        }
                    } else {
                        for tri in 0..(face_indices.len() / 3) {
                            let verts = [
                                face_indices[tri * 3],
                                face_indices[tri * 3 + 1],
                                face_indices[tri * 3 + 2],
                            ];
                            write_face(
                                &mut out,
                                &verts,
                                prim_globals,
                                has_uv,
                                has_normal,
                                negative,
                                positions.len() as u32,
                                texcoords.len() as u32,
                                normals.len() as u32,
                            );
                        }
                    }
                }
                Topology::Lines => {
                    let line_indices: Vec<u32> = match &prim.indices {
                        Some(Indices::U16(v)) => v.iter().map(|&x| x as u32).collect(),
                        Some(Indices::U32(v)) => v.clone(),
                        None => (0..prim.positions.len() as u32).collect(),
                    };
                    let total_v = positions.len() as u32;
                    for w in line_indices.chunks_exact(2) {
                        let a = prim_globals[w[0] as usize];
                        let b = prim_globals[w[1] as usize];
                        let av = fmt_index(a.0, total_v, negative);
                        let bv = fmt_index(b.0, total_v, negative);
                        writeln!(out, "l {av} {bv}").unwrap();
                    }
                }
                other => {
                    return Err(Error::unsupported(format!(
                        "OBJ encoder: topology {other:?} not representable"
                    )));
                }
            }
        }
    }

    Ok(out.into_bytes())
}

#[allow(clippy::too_many_arguments)]
fn write_face(
    out: &mut String,
    verts: &[u32],
    prim_globals: &[(u32, u32, u32)],
    has_uv: bool,
    has_normal: bool,
    negative: bool,
    total_v: u32,
    total_vt: u32,
    total_vn: u32,
) {
    use std::fmt::Write;
    out.push('f');
    for &local in verts {
        let (v, vt, vn) = prim_globals[local as usize];
        let v_s = fmt_index(v, total_v, negative);
        let vt_s = fmt_index(vt, total_vt, negative);
        let vn_s = fmt_index(vn, total_vn, negative);
        match (has_uv, has_normal) {
            (true, true) => write!(out, " {v_s}/{vt_s}/{vn_s}").unwrap(),
            (true, false) => write!(out, " {v_s}/{vt_s}").unwrap(),
            (false, true) => write!(out, " {v_s}//{vn_s}").unwrap(),
            (false, false) => write!(out, " {v_s}").unwrap(),
        }
    }
    out.push('\n');
}

/// Render a 1-based positive index as either its absolute form
/// (`5`) or a negative-from-end form (`-3`, when `total = 7`).
/// `idx == 0` means "no index" — we always emit `0` regardless of
/// the negative flag so the parser still treats it as absent.
fn fmt_index(idx: u32, total: u32, negative: bool) -> String {
    if idx == 0 || !negative {
        idx.to_string()
    } else {
        // total = 7, idx = 5  ⇒  -3  (i.e. "third from the end").
        // Parser computes: resolved = total + 1 + raw  ⇒  raw = idx - total - 1.
        let raw = (idx as i64) - (total as i64) - 1;
        raw.to_string()
    }
}

/// Format a float without scientific notation; trims trailing zeros
/// while keeping at least one digit after the decimal point. Keeps the
/// emitted file human-diffable.
fn fmt_float(x: f32) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    let s = format!("{x:.6}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed
    }
}

// ---------------------------------------------------------------------------
// Float keys for the dedup HashMap (f32 isn't Hash).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct KeyVec2 {
    a: u32,
    b: u32,
}
impl From<[f32; 2]> for KeyVec2 {
    fn from(v: [f32; 2]) -> Self {
        Self {
            a: v[0].to_bits(),
            b: v[1].to_bits(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct KeyVec3 {
    a: u32,
    b: u32,
    c: u32,
}
impl From<[f32; 3]> for KeyVec3 {
    fn from(v: [f32; 3]) -> Self {
        Self {
            a: v[0].to_bits(),
            b: v[1].to_bits(),
            c: v[2].to_bits(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (unit-level — integration tests live under `tests/`).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_strips_comments_and_glues_continuations() {
        let lines =
            preprocess_lines("v 1.0 2.0 \\\n3.0 # comment\nv 4 5 6\n# pure comment\nf 1 2 3");
        assert_eq!(lines[0].trim(), "v 1.0 2.0  3.0");
        assert_eq!(lines[1].trim(), "v 4 5 6");
        // The pure-comment line collapses to an empty preprocessed line.
        assert_eq!(lines[2].trim(), "");
        assert_eq!(lines[3].trim(), "f 1 2 3");
    }

    #[test]
    fn fmt_float_is_diff_friendly() {
        assert_eq!(fmt_float(1.0), "1");
        assert_eq!(fmt_float(0.0), "0");
        assert_eq!(fmt_float(-0.5), "-0.5");
        assert_eq!(fmt_float(1.0 / 3.0), "0.333333");
    }
}
