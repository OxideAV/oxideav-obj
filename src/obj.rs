//! Wavefront OBJ ASCII parser + serialiser.
//!
//! Polygonal subset (vertex / face / line / point / grouping / material
//! directives) is fully decoded into the typed [`Scene3D`] model. The
//! free-form curve/surface directives — `vp`, `cstype`, `deg`, `curv`,
//! `curv2`, `surf`, `parm`, `trim`, `hole`, `scrv`, `sp`, `end`, plus
//! the superseded `bzp` / `bsp` patches — are captured verbatim into
//! `Scene3D::extras["obj:vp"]` and
//! `Scene3D::extras["obj:freeform_directives"]` so a decode → encode
//! round-trip preserves the directive sequence and arguments without
//! semantic interpretation. The `.mod` binary form remains out of
//! scope.
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

/// One face / line / point element captured during the first parse pass.
///
/// Different element kinds map to different [`Topology`] variants and
/// can't share a single [`Primitive`]; the accumulator splits into
/// fresh primitives whenever the kind changes.
#[derive(Debug)]
enum Element {
    Face(Vec<FaceVert>),
    Line(Vec<FaceVert>),
    Point(Vec<FaceVert>),
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
    /// Last seen merging-group token (`"off"` / `"0"` or `"<n> <res>"`).
    /// Captured as a single state value rather than per-element since
    /// `mg` is state-setting per spec §"mg group_number res".
    merging_group: Option<String>,
    /// Display-attribute state — bevel-interpolation flag (`"on"` /
    /// `"off"`). Spec §"bevel on/off" — state-setting; default off.
    bevel: Option<String>,
    /// Color-interpolation flag (`"on"` / `"off"`). Spec
    /// §"c_interp on/off" — state-setting; default off.
    c_interp: Option<String>,
    /// Dissolve-interpolation flag (`"on"` / `"off"`). Spec
    /// §"d_interp on/off" — state-setting; default off.
    d_interp: Option<String>,
    /// Level-of-detail integer (1..100, or 0 / absent for "all").
    /// Spec §"lod level" — state-setting.
    lod: Option<String>,
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
    /// Per-position rational weight from the optional 4th `w` component
    /// of `v x y z w`. `None` means "no weight given" (the spec default
    /// is `1.0`); `Some(w)` is preserved verbatim so a round-trip emits
    /// the original 4-token form rather than collapsing to 3 tokens.
    /// Parallel to `positions` (1-based / 0-based index parity).
    /// Spec §"v x y z w" — w defaults to 1.0 for non-rational geometry.
    position_weights: Vec<Option<f32>>,
    /// Per-position vertex colour from the widely-deployed
    /// `v x y z r g b` extension (MeshLab, libigl, Meshroom, OpenCV).
    /// `None` for vertices written in the standard 3-token form.
    /// `Some([r, g, b, 1.0])` carries the linear-space RGB triplet
    /// (alpha pinned to opaque since the extension only spells out
    /// three colour channels). Parallel to `positions`.
    /// Not in the original spec — flagged in `docs/3d/obj/README.md`
    /// as the canonical "widely used but never standardised" extension.
    position_colors: Vec<Option<[f32; 4]>>,
    texcoords: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    /// Parameter-space vertices (`vp u v [w]`) from the free-form
    /// geometry portion of the spec — 1-based numbering, parallel to
    /// `positions` / `texcoords` / `normals`. Stored as a 3-tuple
    /// where missing components default to `0.0` (this matches what
    /// the spec calls out: `v` defaults to 0 for 1D points, `w`
    /// defaults to 1.0 for rational trimming curves but we leave the
    /// raw "what the file said" in extras and let the consumer
    /// interpret).
    vp: Vec<[f32; 3]>,
    /// Material library file names referenced by `mtllib`.
    mtllibs: Vec<String>,
    /// All material definitions resolved from `mtllib` references
    /// supplied via [`ObjDoc::with_resolved_mtllibs`]. Round 1 ships
    /// no IO so we accept these via an external resolver hook on the
    /// caller.
    resolved_materials: HashMap<String, oxideav_mesh3d::Material>,
    meshes: Vec<MeshAccum>,
    /// Verbatim sequence of free-form-geometry directives (`cstype`,
    /// `deg`, `curv`, `surf`, `parm`, `trim`, `hole`, `scrv`, `sp`,
    /// `end`, `bzp`, plus the older `bsp`). Each entry is the keyword
    /// followed by its whitespace-separated arguments. Round-trip
    /// preservation: the encoder replays the sequence verbatim after
    /// the polygonal section so consumers can carry free-form data
    /// through us without semantic loss. Body statements (`parm`,
    /// `trim`, `hole`, `scrv`, `sp`, `end`) are accepted in document
    /// order; the spec mandates they appear between an element start
    /// (`curv` / `surf`) and `end`, but we don't enforce that — a
    /// lenient loader pattern matches what tools in the wild emit.
    freeform_directives: Vec<Vec<String>>,
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
                // Spec §"v x y z w" defines 3 or 4 components (the 4th
                // is the rational weight, default 1.0). The
                // widely-deployed MeshLab / libigl / Meshroom extension
                // adds a per-vertex RGB triplet making 6 (`x y z r g b`)
                // or 7 (`x y z w r g b`) the supported widths in the
                // wild. We accept all four shapes and surface the extra
                // information through parallel `position_weights` /
                // `position_colors` arrays so the encoder can re-emit
                // the original token width on round-trip.
                let (w, rgb) = match coords.len() {
                    3 => (None, None),
                    4 => (Some(coords[3]), None),
                    6 => (None, Some([coords[3], coords[4], coords[5], 1.0])),
                    7 => (
                        Some(coords[3]),
                        Some([coords[4], coords[5], coords[6], 1.0]),
                    ),
                    n => {
                        return Err(Error::invalid(format!(
                            "v: expected 3, 4, 6, or 7 floats (xyz, xyzw, xyzrgb, or \
                             xyzwrgb per spec + MeshLab vertex-colour extension), got {n}"
                        )));
                    }
                };
                doc.positions.push([coords[0], coords[1], coords[2]]);
                doc.position_weights.push(w);
                doc.position_colors.push(rgb);
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
                // Parameter-space vertex (`vp u v [w]`) — used as the
                // control-point pool for free-form 2D trimming curves
                // (`curv2`, referenced by `trim`/`hole`/`scrv`) and
                // for special points (`sp`). Spec §"vp u v w".
                //
                // The number of meaningful coordinates depends on the
                // usage (1D for 1D special points, 2D for trimming
                // curves, 3D for rational trimming curves with a
                // weight). We always store a 3-tuple, padding with
                // `0.0` so the encoder can emit a faithful
                // `vp <u> <v> <w>` line for the rational case and a
                // shorter `vp <u> <v>` / `vp <u>` for the others.
                let coords: Vec<f32> = tokens
                    .map(str::parse)
                    .collect::<std::result::Result<Vec<f32>, _>>()
                    .map_err(|e| Error::invalid(format!("vp: bad float ({e})")))?;
                if coords.is_empty() {
                    return Err(Error::invalid("vp: expected ≥1 coord"));
                }
                let u = coords[0];
                let v = coords.get(1).copied().unwrap_or(0.0);
                let w = coords.get(2).copied().unwrap_or(0.0);
                doc.vp.push([u, v, w]);
            }
            "cstype" | "deg" | "curv" | "curv2" | "surf" | "parm" | "trim" | "hole" | "scrv"
            | "sp" | "end" | "bzp" | "bsp" => {
                // Free-form geometry directives. Captured verbatim as
                // a `(keyword, args)` sequence on the document so the
                // encoder can replay them after the polygonal section.
                // No semantic interpretation: the round-trip preserves
                // the operator's exact token sequence.
                //
                // Spec §"Free-form curve/surface attributes" /
                // §"Specifying free-form curves/surfaces" /
                // §"Free-form curve/surface body statements" /
                // §"Superseded statements (bzp / bsp)".
                let mut entry: Vec<String> = Vec::new();
                entry.push(keyword.to_string());
                for tok in tokens {
                    entry.push(tok.to_string());
                }
                doc.freeform_directives.push(entry);
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
                // Point elements are state-incompatible with face/line
                // primitives (different `Topology`); mirror the `usemtl`
                // pattern and split into a fresh primitive whenever the
                // current one already holds incompatible elements.
                let n_pos = doc.positions.len() as i64;
                let n_tex = doc.texcoords.len() as i64;
                let n_norm = doc.normals.len() as i64;
                // `p` only takes vertex references (no `/vt` or `//vn`),
                // but parse_face_vertex degrades gracefully when the
                // separators are absent.
                let verts: Vec<FaceVert> = tokens
                    .map(|t| parse_face_vertex(t, n_pos, n_tex, n_norm))
                    .collect::<Result<Vec<_>>>()?;
                if verts.is_empty() {
                    return Err(Error::invalid("p: needs ≥1 vertex"));
                }
                let mesh = doc.meshes.last_mut().unwrap();
                let prim = mesh.current_or_new();
                if prim
                    .elements
                    .iter()
                    .any(|e| !matches!(e, Element::Point(_)))
                {
                    // Mixed-kind elements aren't representable; open a
                    // fresh primitive that inherits material + groups +
                    // smoothing/merging/display-attr state.
                    let mat = prim.material.clone();
                    let groups = prim.groups.clone();
                    let smoothing = prim.smoothing_group.clone();
                    let merging = prim.merging_group.clone();
                    let bevel = prim.bevel.clone();
                    let c_interp = prim.c_interp.clone();
                    let d_interp = prim.d_interp.clone();
                    let lod = prim.lod.clone();
                    mesh.primitives.push(PrimAccum {
                        material: mat,
                        groups,
                        smoothing_group: smoothing,
                        merging_group: merging,
                        bevel,
                        c_interp,
                        d_interp,
                        lod,
                        elements: vec![Element::Point(verts)],
                    });
                } else {
                    prim.elements.push(Element::Point(verts));
                }
            }
            "bevel" | "c_interp" | "d_interp" | "lod" => {
                // Display-attribute state-setting — `bevel on/off`,
                // `c_interp on/off`, `d_interp on/off`, `lod <level>`.
                // Captured per-primitive; a mid-stream change splits
                // the primitive so each one carries one consistent
                // value (mirrors `s`/`mg`).
                let v: String = tokens.collect::<Vec<_>>().join(" ");
                if v.is_empty() {
                    continue;
                }
                let mesh = doc.meshes.last_mut().unwrap();
                let last = mesh.current_or_new();
                let current: Option<&str> = match keyword {
                    "bevel" => last.bevel.as_deref(),
                    "c_interp" => last.c_interp.as_deref(),
                    "d_interp" => last.d_interp.as_deref(),
                    "lod" => last.lod.as_deref(),
                    _ => unreachable!(),
                };
                if last.elements.is_empty() {
                    // Overwrite the pending value.
                    match keyword {
                        "bevel" => last.bevel = Some(v),
                        "c_interp" => last.c_interp = Some(v),
                        "d_interp" => last.d_interp = Some(v),
                        "lod" => last.lod = Some(v),
                        _ => unreachable!(),
                    }
                } else if current != Some(v.as_str()) {
                    let mat = last.material.clone();
                    let groups = last.groups.clone();
                    let smoothing = last.smoothing_group.clone();
                    let merging = last.merging_group.clone();
                    let mut bevel = last.bevel.clone();
                    let mut c_interp = last.c_interp.clone();
                    let mut d_interp = last.d_interp.clone();
                    let mut lod = last.lod.clone();
                    match keyword {
                        "bevel" => bevel = Some(v),
                        "c_interp" => c_interp = Some(v),
                        "d_interp" => d_interp = Some(v),
                        "lod" => lod = Some(v),
                        _ => unreachable!(),
                    }
                    mesh.primitives.push(PrimAccum {
                        material: mat,
                        smoothing_group: smoothing,
                        merging_group: merging,
                        groups,
                        bevel,
                        c_interp,
                        d_interp,
                        lod,
                        elements: Vec::new(),
                    });
                }
            }
            "mg" => {
                // Merging group — `mg <group_number> [res]` or `mg off`
                // / `mg 0`. Like `s`, it's state-setting; preserve the
                // operator's spelling verbatim. The semantic value
                // (smoothing across surface joins for free-form
                // surfaces) is meaningless without the free-form
                // surface support, but the round-trip preservation
                // matters for tools that round-trip mesh data through
                // us.
                let v: String = tokens.collect::<Vec<_>>().join(" ");
                if v.is_empty() {
                    continue;
                }
                let mesh = doc.meshes.last_mut().unwrap();
                let last = mesh.current_or_new();
                if last.elements.is_empty() {
                    // No elements yet — overwrite the pending value.
                    last.merging_group = Some(v);
                } else if last.merging_group.as_deref() != Some(v.as_str()) {
                    // Merging-group changed mid-stream; split into a
                    // fresh primitive so each one carries one
                    // consistent assignment (mirrors smoothing-group
                    // behaviour).
                    let mat = last.material.clone();
                    let groups = last.groups.clone();
                    let smoothing = last.smoothing_group.clone();
                    let bevel = last.bevel.clone();
                    let c_interp = last.c_interp.clone();
                    let d_interp = last.d_interp.clone();
                    let lod = last.lod.clone();
                    mesh.primitives.push(PrimAccum {
                        material: mat,
                        smoothing_group: smoothing,
                        groups,
                        merging_group: Some(v),
                        bevel,
                        c_interp,
                        d_interp,
                        lod,
                        elements: Vec::new(),
                    });
                }
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
                    // inherits the current material + groups +
                    // merging-group + display attributes.
                    let mat = last.material.clone();
                    let groups = last.groups.clone();
                    let merging = last.merging_group.clone();
                    let bevel = last.bevel.clone();
                    let c_interp = last.c_interp.clone();
                    let d_interp = last.d_interp.clone();
                    let lod = last.lod.clone();
                    mesh.primitives.push(PrimAccum {
                        material: mat,
                        smoothing_group: Some(v),
                        groups,
                        merging_group: merging,
                        bevel,
                        c_interp,
                        d_interp,
                        lod,
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
                &doc.position_weights,
                &doc.position_colors,
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

    // Free-form geometry side-channel: the parameter-space vertex pool
    // (`vp`) and the verbatim sequence of `cstype` / `deg` / `curv` /
    // `surf` / `parm` / `trim` / `hole` / `scrv` / `sp` / `end` / `bzp`
    // / `bsp` directives. The encoder replays these after the
    // polygonal section so consumers that don't care about free-form
    // geometry simply ignore the keys, while consumers that do can
    // walk the directive sequence themselves.
    if !doc.vp.is_empty() {
        scene
            .extras
            .insert("obj:vp".to_string(), serde_json::to_value(&doc.vp).unwrap());
    }
    if !doc.freeform_directives.is_empty() {
        scene.extras.insert(
            "obj:freeform_directives".to_string(),
            serde_json::to_value(&doc.freeform_directives).unwrap(),
        );
    }

    Ok(scene)
}

/// Promote a single-`l`-element primitive to `LineStrip` / `LineLoop`
/// when applicable; fall back to `Lines` for multi-element or 2-vertex
/// segments. See [`build_primitive`] for the surrounding context.
fn single_line_topology(elements: &[Element]) -> Topology {
    if elements.len() != 1 {
        return Topology::Lines;
    }
    let Element::Line(verts) = &elements[0] else {
        return Topology::Lines;
    };
    if verts.len() < 2 {
        return Topology::Lines;
    }
    // A 2-vertex `l` is a plain segment — keep it on `Lines` so the
    // round-trip stays minimal (one `l v1 v2` line either way).
    if verts.len() == 2 {
        return Topology::Lines;
    }
    // Closed polyline: first / last vertex coincide on the position
    // index. We don't need to compare uv/normal — `l` references only
    // ever populate the position component for the loop-detection
    // semantics specified by the spec §"Line elements".
    let same_start_end = verts.first().map(|fv| fv.v) == verts.last().map(|fv| fv.v);
    if same_start_end {
        Topology::LineLoop
    } else {
        Topology::LineStrip
    }
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
    position_weights: &[Option<f32>],
    position_colors: &[Option<[f32; 4]>],
    texcoords: &[[f32; 2]],
    normals: &[[f32; 3]],
    material_ids: &HashMap<String, oxideav_mesh3d::MaterialId>,
) -> Result<(Primitive, Vec<u32>)> {
    // Decide topology + attribute presence by looking at the first
    // element. Mixed-element primitives (lines + faces under one
    // `usemtl`) aren't representable in mesh3d so we error cleanly.
    //
    // For a single `l` element we promote to the more specific
    // `LineStrip` / `LineLoop` topology so consumers don't have to
    // reconstruct the polyline shape from disjoint segment pairs:
    //
    //   * exactly one `l` element with N ≥ 2 vertices whose last
    //     vertex equals its first → `LineLoop` (the redundant
    //     closing vertex is dropped from the index buffer).
    //   * exactly one `l` element with N ≥ 2 distinct end vertices →
    //     `LineStrip`.
    //   * multiple `l` elements (or a single 2-vertex `l` that is a
    //     plain segment) fall back to `Lines` for the existing
    //     contiguous-chain re-emit path on the encoder side.
    let first = prim_acc.elements.first();
    let topology = match first {
        Some(Element::Face(_)) => Topology::Triangles,
        Some(Element::Line(_)) => single_line_topology(&prim_acc.elements),
        Some(Element::Point(_)) => Topology::Points,
        None => Topology::Triangles,
    };
    for elt in &prim_acc.elements {
        let ok = matches!(
            (&topology, elt),
            (Topology::Triangles, Element::Face(_))
                | (Topology::Lines, Element::Line(_))
                | (Topology::LineStrip, Element::Line(_))
                | (Topology::LineLoop, Element::Line(_))
                | (Topology::Points, Element::Point(_))
        );
        if !ok {
            return Err(Error::unsupported(
                "OBJ primitive mixes face / line / point elements under one usemtl",
            ));
        }
    }

    let has_uv = prim_acc.elements.iter().any(|elt| match elt {
        Element::Face(verts) | Element::Line(verts) | Element::Point(verts) => {
            verts.iter().any(|fv| fv.vt != 0)
        }
    });
    let has_normal = prim_acc.elements.iter().any(|elt| match elt {
        Element::Face(verts) | Element::Line(verts) | Element::Point(verts) => {
            verts.iter().any(|fv| fv.vn != 0)
        }
    });
    // Per-vertex colour applies to a primitive whenever any of its
    // referenced positions carries the `v x y z r g b` extension. We
    // promote to a single-channel `colors[0]` set; vertices that
    // don't carry RGB fall back to white (the obvious "no colour
    // information" sentinel — preserves the standard glTF expectation
    // that a colour buffer is fully populated when present). The
    // round-trip-aware `obj:vertex_color_present` per-position
    // bitmap below guards the encoder against re-emitting a
    // synthetic white that the original file didn't spell out.
    let has_color = prim_acc.elements.iter().any(|elt| match elt {
        Element::Face(verts) | Element::Line(verts) | Element::Point(verts) => {
            verts.iter().any(|fv| {
                position_colors
                    .get((fv.v - 1) as usize)
                    .is_some_and(Option::is_some)
            })
        }
    });

    let mut prim = Primitive::new(topology);
    if has_uv {
        prim.uvs.push(Vec::new());
    }
    if has_normal {
        prim.normals = Some(Vec::new());
    }
    if has_color {
        prim.colors.push(Vec::new());
    }
    // Track per-interned-vertex "did this position carry RGB / a
    // weight in the source file?" so the encoder doesn't fabricate
    // colours / weights that the user never wrote. Both vectors are
    // parallel to `prim.positions` after interning completes.
    let mut color_present: Vec<bool> = Vec::new();
    let mut weights_seen: Vec<Option<f32>> = Vec::new();

    // De-duplicate face-vertices into a single interleaved buffer.
    let mut indexer: HashMap<FaceVert, u32> = HashMap::new();
    let mut arities: Vec<u32> = Vec::new();
    let mut local_indices: Vec<u32> = Vec::new();

    let intern = |fv: FaceVert,
                  prim: &mut Primitive,
                  indexer: &mut HashMap<FaceVert, u32>,
                  color_present: &mut Vec<bool>,
                  weights_seen: &mut Vec<Option<f32>>|
     -> Result<u32> {
        if let Some(&idx) = indexer.get(&fv) {
            return Ok(idx);
        }
        let pos = positions
            .get((fv.v - 1) as usize)
            .ok_or_else(|| Error::invalid(format!("face references missing position {}", fv.v)))?;
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
        if has_color {
            // Either the source file carried RGB for this vertex, or
            // we synthesise opaque white so the colour buffer stays
            // length-parallel with positions (mesh3d invariant).
            let rgba = position_colors
                .get((fv.v - 1) as usize)
                .copied()
                .flatten()
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            prim.colors[0].push(rgba);
            color_present.push(
                position_colors
                    .get((fv.v - 1) as usize)
                    .is_some_and(Option::is_some),
            );
        }
        weights_seen.push(position_weights.get((fv.v - 1) as usize).copied().flatten());
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
                    .map(|&fv| {
                        intern(
                            fv,
                            &mut prim,
                            &mut indexer,
                            &mut color_present,
                            &mut weights_seen,
                        )
                    })
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
                    .map(|&fv| {
                        intern(
                            fv,
                            &mut prim,
                            &mut indexer,
                            &mut color_present,
                            &mut weights_seen,
                        )
                    })
                    .collect::<Result<Vec<_>>>()?;
                match topology {
                    Topology::LineStrip => {
                        // Emit the polyline as a contiguous index list.
                        local_indices.extend_from_slice(&resolved);
                    }
                    Topology::LineLoop => {
                        // Drop the redundant closing vertex; consumers
                        // treat the strip as closed at draw time.
                        let n = resolved.len().saturating_sub(1);
                        local_indices.extend_from_slice(&resolved[..n]);
                    }
                    _ => {
                        // Plain `Lines` — decompose polyline into
                        // disjoint segment pairs (encoder rejoins
                        // contiguous chains on the way out).
                        for w in resolved.windows(2) {
                            local_indices.push(w[0]);
                            local_indices.push(w[1]);
                        }
                    }
                }
            }
            Element::Point(verts) => {
                // Each `p` line can carry multiple vertex references;
                // every reference becomes one element index for
                // `Topology::Points`. Original arities aren't tracked
                // since a re-emit can pack them on one line freely.
                for &fv in verts {
                    let idx = intern(
                        fv,
                        &mut prim,
                        &mut indexer,
                        &mut color_present,
                        &mut weights_seen,
                    )?;
                    local_indices.push(idx);
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

    // Per-vertex extension state — surfaced through `Primitive::extras`
    // so the encoder knows which `v` lines to expand to the 4-token
    // `xyzw`, 6-token `xyzrgb`, or 7-token `xyzwrgb` form. We only stash
    // the bitmaps when at least one vertex used the extension; the
    // common no-extension case stays free of decode-time noise.
    if has_color && color_present.iter().any(|&b| b) {
        prim.extras.insert(
            "obj:vertex_color_present".to_string(),
            serde_json::to_value(&color_present).unwrap(),
        );
    }
    if weights_seen.iter().any(Option::is_some) {
        prim.extras.insert(
            "obj:vertex_weight".to_string(),
            serde_json::to_value(&weights_seen).unwrap(),
        );
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
    if let Some(s) = &prim_acc.merging_group {
        prim.extras.insert(
            "obj:merging_group".to_string(),
            serde_json::Value::String(s.clone()),
        );
    }
    if let Some(s) = &prim_acc.bevel {
        prim.extras.insert(
            "obj:bevel".to_string(),
            serde_json::Value::String(s.clone()),
        );
    }
    if let Some(s) = &prim_acc.c_interp {
        prim.extras.insert(
            "obj:c_interp".to_string(),
            serde_json::Value::String(s.clone()),
        );
    }
    if let Some(s) = &prim_acc.d_interp {
        prim.extras.insert(
            "obj:d_interp".to_string(),
            serde_json::Value::String(s.clone()),
        );
    }
    if let Some(s) = &prim_acc.lod {
        prim.extras
            .insert("obj:lod".to_string(), serde_json::Value::String(s.clone()));
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
    // Parallel to `positions` — `Some(rgb)` when the source flagged
    // this vertex through the `obj:vertex_color_present` extras
    // bitmap, `None` otherwise. We *don't* emit synthetic white for a
    // `None` entry: the round-trip rule is "only re-emit RGB for
    // vertices that originally had it". When at least one position
    // carries colour the encoder also sets a flag so the entire
    // colour set isn't dropped on a partial-colouring file (mixed
    // colored / uncolored vertices in one primitive — re-emit
    // standard `v x y z` for the uncolored).
    let mut position_colors: Vec<Option<[f32; 4]>> = Vec::new();
    // Parallel to `positions` — preserved `v` 4th `w` weight whenever
    // the source carried it. `None` re-emits the standard 3-token form.
    let mut position_weights: Vec<Option<f32>> = Vec::new();
    let mut texcoords: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut pos_map: HashMap<KeyVec3, u32> = HashMap::new();
    let mut tex_map: HashMap<KeyVec2, u32> = HashMap::new();
    let mut nor_map: HashMap<KeyVec3, u32> = HashMap::new();

    // Intern a position into the shared global pool, attaching the
    // (optional) per-vertex colour + weight derived from the
    // `obj:vertex_color_present` / `obj:vertex_weight` extras. When the
    // same position appears across primitives, the *first* non-`None`
    // colour / weight wins — silently ignoring later overrides keeps
    // round-trip determinism without forcing a partition of duplicate
    // positions on differing colour metadata (which would force the
    // encoder to emit redundant `v` lines and bloat the output).
    let intern_pos = |p: [f32; 3],
                      colour: Option<[f32; 4]>,
                      weight: Option<f32>,
                      positions: &mut Vec<[f32; 3]>,
                      colours: &mut Vec<Option<[f32; 4]>>,
                      weights: &mut Vec<Option<f32>>,
                      map: &mut HashMap<KeyVec3, u32>|
     -> u32 {
        let key = KeyVec3::from(p);
        if let Some(&i) = map.get(&key) {
            // First-write-wins on extension metadata.
            let slot = (i - 1) as usize;
            if colours[slot].is_none() {
                colours[slot] = colour;
            }
            if weights[slot].is_none() {
                weights[slot] = weight;
            }
            return i;
        }
        positions.push(p);
        colours.push(colour);
        weights.push(weight);
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
            let has_color = !prim.colors.is_empty();
            // Per-vertex bitmap saying "did the source spell out RGB on
            // this vertex?". Missing extras / no-colors-set means every
            // vertex stays in the standard 3-token form.
            let color_present: Vec<bool> = prim
                .extras
                .get("obj:vertex_color_present")
                .and_then(serde_json::Value::as_array)
                .map(|arr| arr.iter().map(|v| v.as_bool().unwrap_or(false)).collect())
                .unwrap_or_else(|| vec![has_color; prim.positions.len()]);
            // Per-vertex weight overrides — preserved through extras.
            let weight_overrides: Vec<Option<f32>> = prim
                .extras
                .get("obj:vertex_weight")
                .and_then(serde_json::Value::as_array)
                .map(|arr| arr.iter().map(|v| v.as_f64().map(|f| f as f32)).collect())
                .unwrap_or_default();
            let mut prim_globals: Vec<GlobalTriple> = Vec::with_capacity(prim.positions.len());
            for vi in 0..prim.positions.len() {
                let colour = if has_color && color_present.get(vi).copied().unwrap_or(false) {
                    Some(prim.colors[0][vi])
                } else {
                    None
                };
                let weight = weight_overrides.get(vi).copied().flatten();
                let v_idx = intern_pos(
                    prim.positions[vi],
                    colour,
                    weight,
                    &mut positions,
                    &mut position_colors,
                    &mut position_weights,
                    &mut pos_map,
                );
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

    for (i, p) in positions.iter().enumerate() {
        // Pick the most-compact `v` form that still carries the
        // extension data: `xyz`, `xyzw` (rational weight), `xyzrgb`
        // (MeshLab vertex colour), or `xyzwrgb` (both). Each
        // extension is silently dropped if it would just spell out
        // the spec default (`w == 1.0`, no colour).
        let weight = position_weights[i];
        let colour = position_colors[i];
        let mut s = String::with_capacity(40);
        s.push_str("v ");
        s.push_str(&fmt_float(p[0]));
        s.push(' ');
        s.push_str(&fmt_float(p[1]));
        s.push(' ');
        s.push_str(&fmt_float(p[2]));
        if let Some(w) = weight {
            s.push(' ');
            s.push_str(&fmt_float(w));
        }
        if let Some(rgb) = colour {
            s.push(' ');
            s.push_str(&fmt_float(rgb[0]));
            s.push(' ');
            s.push_str(&fmt_float(rgb[1]));
            s.push(' ');
            s.push_str(&fmt_float(rgb[2]));
        }
        writeln!(out, "{s}").unwrap();
    }
    // Parameter-space vertices for the free-form geometry section. We
    // emit these after `v` and before `vt` to mirror the typical layout
    // produced by Wavefront-era authoring tools (the spec doesn't
    // mandate an ordering, but co-locating `vp` with the other vertex
    // pools keeps human diffs tidy).
    if let Some(serde_json::Value::Array(vps)) = scene.extras.get("obj:vp") {
        for entry in vps {
            if let serde_json::Value::Array(coords) = entry {
                let parts: Vec<f32> = coords
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect();
                if parts.is_empty() {
                    continue;
                }
                // Emit only as many coordinates as carry meaningful
                // information. The decoder padded with `0.0`, so a
                // trailing `0` is a strong signal "the operator
                // didn't supply this component". 1D / 2D / 3D `vp`
                // statements are all valid per spec §"vp u v w".
                let trim = if parts.len() >= 3 && parts[2] != 0.0 {
                    3
                } else if parts.len() >= 2 && parts[1] != 0.0 {
                    2
                } else {
                    1
                };
                let mut s = String::from("vp");
                for coord in parts.iter().take(trim) {
                    s.push(' ');
                    s.push_str(&fmt_float(*coord));
                }
                writeln!(out, "{s}").unwrap();
            }
        }
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
            if let Some(s) = prim
                .extras
                .get("obj:merging_group")
                .and_then(|v| v.as_str())
            {
                writeln!(out, "mg {s}").unwrap();
            }
            // Display-attribute state-setters — emitted ahead of the
            // elements they apply to. Order is fixed to keep round-trip
            // diffs deterministic.
            for keyword in ["bevel", "c_interp", "d_interp", "lod"] {
                let key = format!("obj:{keyword}");
                if let Some(s) = prim.extras.get(&key).and_then(|v| v.as_str()) {
                    writeln!(out, "{keyword} {s}").unwrap();
                }
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
                    // Walk segment pairs and join contiguous chains
                    // (segment N's end == segment N+1's start) into
                    // one polyline before emit. Saves bytes on the
                    // common case of a long polyline that round-tripped
                    // through `Topology::Lines` decomposition.
                    let mut chain: Vec<u32> = Vec::new();
                    let flush = |chain: &mut Vec<u32>, out: &mut String| {
                        if chain.len() < 2 {
                            chain.clear();
                            return;
                        }
                        let parts: Vec<String> = chain
                            .iter()
                            .map(|&local| {
                                fmt_index(prim_globals[local as usize].0, total_v, negative)
                            })
                            .collect();
                        writeln!(out, "l {}", parts.join(" ")).unwrap();
                        chain.clear();
                    };
                    for w in line_indices.chunks_exact(2) {
                        let (a, b) = (w[0], w[1]);
                        if chain.is_empty() {
                            chain.push(a);
                            chain.push(b);
                        } else if *chain.last().unwrap() == a {
                            chain.push(b);
                        } else {
                            flush(&mut chain, &mut out);
                            chain.push(a);
                            chain.push(b);
                        }
                    }
                    flush(&mut chain, &mut out);
                }
                Topology::LineStrip | Topology::LineLoop => {
                    // Reconstruct the strip's index list from whichever
                    // backing storage the primitive carries; bare
                    // positions imply implicit `0..N` indices. For
                    // `LineLoop` we re-append the first index so the
                    // emitted `l` line spells out the closing edge —
                    // the parser then detects start == end and round-
                    // trips back to `LineLoop`.
                    let mut strip_indices: Vec<u32> = match &prim.indices {
                        Some(Indices::U16(v)) => v.iter().map(|&x| x as u32).collect(),
                        Some(Indices::U32(v)) => v.clone(),
                        None => (0..prim.positions.len() as u32).collect(),
                    };
                    if matches!(prim.topology, Topology::LineLoop)
                        && let Some(&first) = strip_indices.first()
                    {
                        strip_indices.push(first);
                    }
                    if strip_indices.len() >= 2 {
                        let total_v = positions.len() as u32;
                        let parts: Vec<String> = strip_indices
                            .iter()
                            .map(|&local| {
                                fmt_index(prim_globals[local as usize].0, total_v, negative)
                            })
                            .collect();
                        writeln!(out, "l {}", parts.join(" ")).unwrap();
                    }
                }
                Topology::Points => {
                    let pt_indices: Vec<u32> = match &prim.indices {
                        Some(Indices::U16(v)) => v.iter().map(|&x| x as u32).collect(),
                        Some(Indices::U32(v)) => v.clone(),
                        None => (0..prim.positions.len() as u32).collect(),
                    };
                    let total_v = positions.len() as u32;
                    if !pt_indices.is_empty() {
                        // Pack every reference onto a single `p` line —
                        // the spec explicitly permits the multi-vertex
                        // form (`p v1 v2 v3 …`) and it's what most
                        // tools emit.
                        let parts: Vec<String> = pt_indices
                            .iter()
                            .map(|&local| {
                                fmt_index(prim_globals[local as usize].0, total_v, negative)
                            })
                            .collect();
                        writeln!(out, "p {}", parts.join(" ")).unwrap();
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

    // Free-form geometry section: replay the captured directive
    // sequence verbatim. The decoder records every `cstype` / `deg` /
    // `curv` / `surf` / `parm` / `trim` / `hole` / `scrv` / `sp` /
    // `end` / `bzp` / `bsp` line as `[keyword, arg1, arg2, …]` so the
    // encoder is purely textual — no semantic interpretation, which
    // means the round-trip is bit-exact for the directive args even
    // when the polygonal section sits between `vp` and the free-form
    // body.
    if let Some(serde_json::Value::Array(directives)) = scene.extras.get("obj:freeform_directives")
    {
        for entry in directives {
            if let serde_json::Value::Array(toks) = entry {
                let parts: Vec<&str> = toks.iter().filter_map(|v| v.as_str()).collect();
                if parts.is_empty() {
                    continue;
                }
                writeln!(out, "{}", parts.join(" ")).unwrap();
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
