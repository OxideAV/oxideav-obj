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
            | "sp" | "end" | "bzp" | "bsp" | "bmat" | "step" => {
                // Free-form geometry directives. Captured verbatim as
                // a `(keyword, args)` sequence on the document so the
                // encoder can replay them after the polygonal section.
                // No semantic interpretation: the round-trip preserves
                // the operator's exact token sequence.
                //
                // Spec §"Free-form curve/surface attributes" /
                // §"Specifying free-form curves/surfaces" /
                // §"Free-form curve/surface body statements" /
                // §"Superseded statements (bzp / bsp)" /
                // §"bmat u/v matrix" + §"step stepu stepv".
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

    // Source-of-truth position pool — kept in 1-based parallel order
    // for free-form directives (`curv` / `surf`) that reference
    // vertices by index. Without this, an OBJ whose free-form section
    // is the *only* consumer of those positions would lose them on
    // re-encode (the encoder pools positions only from polygonal
    // primitives). The encoder re-emits any `obj:positions` entry not
    // already covered by polygonal primitives, in their original
    // 1-based order, so `curv 0 1 N M K` directives keep resolving
    // to the same coordinates after a decode → encode → decode cycle.
    //
    // Position colours / weights ride along on the same parallel
    // arrays so the `xyzrgb` / `xyzw` extension widths survive.
    if !doc.positions.is_empty()
        && (doc.freeform_directives.iter().any(|d| {
            matches!(
                d.first().map(String::as_str),
                Some("curv" | "curv2" | "surf" | "bzp" | "bsp")
            )
        }))
    {
        scene.extras.insert(
            "obj:positions".to_string(),
            serde_json::to_value(&doc.positions).unwrap(),
        );
        if doc.position_weights.iter().any(Option::is_some) {
            scene.extras.insert(
                "obj:position_weights".to_string(),
                serde_json::to_value(&doc.position_weights).unwrap(),
            );
        }
        if doc.position_colors.iter().any(Option::is_some) {
            scene.extras.insert(
                "obj:position_colors".to_string(),
                serde_json::to_value(&doc.position_colors).unwrap(),
            );
        }
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

/// Walk the captured free-form directive sequence in [`ObjDoc`] and
/// synthesise one [`Primitive`] (Topology::LineStrip, indexed) per
/// `curv` directive that sits under a supported `cstype` header.
///
/// Supported `cstype` values:
///   * `bmatrix` — round 10, evaluated via the user-supplied basis
///     matrix from `bmat u` and the step size from `step` (spec §"Basis
///     matrix"). Each polynomial segment is constructed by walking the
///     control-point list at the step size and computing
///     `P(t) = Σ_i Σ_j B[i][j] · t^j · p_i` per axis (`bmat u`
///     stores `B` in row-major order with column index `j` varying
///     fastest, per spec §"bmat u/v matrix").
///
///   * `bezier` / `rat bezier` — round 7, de Casteljau evaluation on the
///     `[0, 1]` basis domain.
///   * `bspline` / `rat bspline` — round 8, Cox-deBoor recursive basis
///     functions evaluated on `[t_min, t_max]` derived from the curve's
///     `u_min` / `u_max` clipped against the active knot vector parsed
///     from the most-recent `parm u` body statement.
///   * `cardinal` — round 9, cubic Catmull-Rom evaluation via the spec's
///     conversion to Bezier control points (`b1 = c1 + (c2 - c0) / 6`,
///     `b2 = c2 - (c3 - c1) / 6`, `b0 = c1`, `b3 = c2`). Sliding-window
///     piecewise: each segment i uses `c[i..i+4]`. Cardinal is cubic only
///     per spec §"Cardinal" — non-cubic `deg` is rejected.
///   * `taylor` — round 9, direct polynomial evaluation
///     `P(t) = Σ_{i=0..n} c_i · t^i` where each control point IS a
///     coefficient vector (spec §"Taylor": "control points are the
///     polynomial coefficients"). Sample range `[u_min, u_max]`.
///
/// Each curve is evaluated at `samples + 1` uniformly-spaced parameter
/// values across its evaluation interval. The resulting points become a
/// polyline.
///
/// `cstype` modifiers other than the listed kinds are ignored. This
/// function handles only 1D `curv` directives; 2-parameter `surf`
/// surfaces are evaluated separately by [`tessellate_surfaces`] (Bezier
/// tensor-product, round 11). NURBS surfaces remain captured-only.
///
/// Per-curve provenance lands on `Primitive::extras`:
///
///   * `obj:tessellated_curve` — `true` (sentinel for filters).
///   * `obj:curve_kind` — `"bezier"` / `"rat_bezier"` / `"bspline"` /
///     `"rat_bspline"` / `"cardinal"` / `"taylor"` / `"bmatrix"`.
///   * `obj:curve_degree` — basis polynomial degree.
///   * `obj:curve_u_range` — `[u_min, u_max]` from the `curv` directive.
///   * `obj:curve_samples` — sample count emitted.
///
/// Spec references: §"Curve and surface type" (cstype), §"Degree"
/// (deg), §"Curve" (curv), §"Parameter values and knot vectors"
/// (parm), §"B-spline" (Cox-deBoor recursion), §"Cardinal" (Catmull-Rom
/// conversion to Bezier), §"Taylor" (polynomial-coefficient basis),
/// §"Basis matrix" (general arbitrary-degree user-defined basis,
/// `bmat u/v` + `step` body statements),
/// §"Free-form curve/surface body statements" (rational weight semantics).
fn tessellate_curves(doc: &ObjDoc, samples: u32) -> Vec<Primitive> {
    // Spec §"Specifying free-form curves/surfaces": the curve / surface
    // header (`curv` / `surf`) lists control points, and the *body*
    // statements (`parm`, `trim`, `hole`, `scrv`, `sp`) follow before
    // the block-terminating `end`. That means a `curv` directive is
    // syntactically ahead of the `parm u …` knot vector it depends on
    // — we can't tessellate B-splines on a single linear walk.
    //
    // Strategy: scan into per-block records (`cstype` opens, `end`
    // closes), accumulate the relevant directives, then evaluate every
    // pending `curv` once the body is fully visible. The Bezier path
    // doesn't need the body but uses the same scaffolding for
    // simplicity.
    let mut out: Vec<Primitive> = Vec::new();

    // Pending state inside the current `cstype` … `end` block.
    let mut active_kind: Option<&'static str> = None;
    let mut active_degree: Option<u32> = None;
    let mut parm_u: Vec<f32> = Vec::new();
    // Basis-matrix block state (spec §"Basis matrix"): `bmat u <matrix>`
    // supplies the (n+1)×(n+1) basis stored row-major (column j varies
    // fastest per spec); `step <stepu>` supplies the integer stride
    // between successive segment windows of control points.
    let mut bmat_u: Vec<f32> = Vec::new();
    let mut step_u: Option<u32> = None;
    // `curv` directives queued for this block — evaluated on `end`.
    let mut pending_curves: Vec<&Vec<String>> = Vec::new();

    for entry in &doc.freeform_directives {
        if entry.is_empty() {
            continue;
        }
        match entry[0].as_str() {
            "cstype" => {
                // Flush the previous block (rare — OBJ usually ends
                // each block with `end`, but be defensive).
                flush_block(
                    &mut out,
                    doc,
                    active_kind,
                    active_degree,
                    &parm_u,
                    &bmat_u,
                    step_u,
                    &pending_curves,
                    samples,
                );
                pending_curves.clear();
                parm_u.clear();
                bmat_u.clear();
                step_u = None;
                active_degree = None;

                // Spec §"Curve and surface type": `cstype [rat] type`.
                let mut iter = entry.iter().skip(1);
                let first = iter.next().map(String::as_str);
                let second = iter.next().map(String::as_str);
                active_kind = match (first, second) {
                    (Some("bezier"), _) => Some("bezier"),
                    (Some("rat"), Some("bezier")) => Some("rat_bezier"),
                    (Some("bspline"), _) => Some("bspline"),
                    (Some("rat"), Some("bspline")) => Some("rat_bspline"),
                    // Spec §"Cardinal": cubic Catmull-Rom. The `rat`
                    // qualifier is permitted but the spec note says the
                    // unit-weight default is reasonable for Cardinal
                    // because its basis functions sum to 1; we don't
                    // currently differentiate rat_cardinal from cardinal
                    // because the per-vertex weight is rarely populated
                    // in real Cardinal data.
                    (Some("cardinal"), _) => Some("cardinal"),
                    (Some("rat"), Some("cardinal")) => Some("cardinal"),
                    // Spec §"Taylor": polynomial-coefficient basis. The
                    // spec note explicitly warns that the rational form
                    // "does not make sense for Taylor" so we accept the
                    // `rat` qualifier but route to the same evaluator.
                    (Some("taylor"), _) => Some("taylor"),
                    (Some("rat"), Some("taylor")) => Some("taylor"),
                    // Spec §"Basis matrix": `cstype bmatrix` — the
                    // user supplies the basis via `bmat u <matrix>` and
                    // the segment stride via `step <stepu>`. The spec
                    // note on rational forms says the unit-weight
                    // default "may or may not make sense for a
                    // representation given in basis-matrix form", so
                    // we accept `rat bmatrix` but don't apply weights
                    // (the user's basis is the source of truth).
                    (Some("bmatrix"), _) => Some("bmatrix"),
                    (Some("rat"), Some("bmatrix")) => Some("bmatrix"),
                    _ => None,
                };
            }
            "deg" => {
                // Spec §"Degree": `deg degu [degv]`. We only consume
                // `degu` for 1D `curv` tessellation; `degv` is captured
                // in the directive sequence but unused here.
                if let Some(d) = entry.get(1).and_then(|t| t.parse::<u32>().ok()) {
                    active_degree = Some(d);
                }
            }
            // Spec §"Parameter values and knot vectors":
            // `parm u p1 p2 p3 …` (or `parm v …`). For 1D curves we
            // only need the `u` knot vector / parameter vector.
            "parm" if entry.get(1).map(String::as_str) == Some("u") => {
                parm_u = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f32>().ok())
                    .collect();
            }
            // Spec §"bmat u/v matrix": `bmat u m_00 m_01 … m_nn` (row-
            // major with column index `j` varying fastest). Only the
            // u-direction matrix is consumed by 1D `curv` evaluation;
            // `bmat v` is captured in the directive sequence but only
            // matters for surface tessellation (deferred).
            "bmat" if entry.get(1).map(String::as_str) == Some("u") => {
                bmat_u = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f32>().ok())
                    .collect();
            }
            // Spec §"step stepu stepv": `step stepu [stepv]`. `stepu`
            // is the integer stride between successive segment windows
            // of control points (`stepv` is required only for
            // surfaces).
            "step" => {
                step_u = entry.get(1).and_then(|t| t.parse::<u32>().ok());
            }
            "curv" => {
                // Defer evaluation until `end` — the body statement
                // `parm u …` that supplies the B-spline knot vector
                // hasn't been seen yet at this point.
                pending_curves.push(entry);
            }
            "end" => {
                flush_block(
                    &mut out,
                    doc,
                    active_kind,
                    active_degree,
                    &parm_u,
                    &bmat_u,
                    step_u,
                    &pending_curves,
                    samples,
                );
                pending_curves.clear();
                parm_u.clear();
                bmat_u.clear();
                step_u = None;
                active_kind = None;
                active_degree = None;
            }
            // `surf`, `curv2`, `trim`, `hole`, `scrv`, `sp`, `bzp`,
            // `bsp` etc. are tracked through `freeform_directives` but
            // don't influence 1D-curve tessellation directly. `surf`
            // (a 2-parameter surface) is evaluated by the separate
            // `tessellate_surfaces` pass (round 11, Bezier tensor-
            // product).
            _ => {}
        }
    }
    // Tail flush — a malformed OBJ might omit the closing `end`. Spec
    // §"Free-form curve/surface body statements" requires it, but the
    // rest of the loader is lenient so we are too.
    flush_block(
        &mut out,
        doc,
        active_kind,
        active_degree,
        &parm_u,
        &bmat_u,
        step_u,
        &pending_curves,
        samples,
    );
    out
}

/// Evaluate every `curv` entry queued for the current `cstype … end`
/// block, appending tessellated primitives to `out`. A block whose
/// state is incomplete (missing `cstype`, missing knot vector for
/// B-spline, malformed control-point indices, …) is silently dropped —
/// the directive sequence already rides on `Scene3D::extras` for
/// downstream consumers.
#[allow(clippy::too_many_arguments)]
fn flush_block(
    out: &mut Vec<Primitive>,
    doc: &ObjDoc,
    active_kind: Option<&'static str>,
    active_degree: Option<u32>,
    parm_u: &[f32],
    bmat_u: &[f32],
    step_u: Option<u32>,
    pending_curves: &[&Vec<String>],
    samples: u32,
) {
    let Some(kind) = active_kind else {
        return;
    };
    for entry in pending_curves {
        // tokens past "curv" — first two are u_min / u_max,
        // remaining are 1-based / negative position indices.
        if entry.len() < 5 {
            // Minimum: keyword + u0 + u1 + at least 2 control points
            // (a line / degree-1 curve). Anything shorter is malformed;
            // skip rather than abort — the lenient-loader pattern
            // matches the rest of the codebase.
            continue;
        }
        let Ok(u_min) = entry[1].parse::<f32>() else {
            continue;
        };
        let Ok(u_max) = entry[2].parse::<f32>() else {
            continue;
        };
        let n_pos = doc.positions.len() as i64;
        let mut control_points: Vec<[f32; 3]> = Vec::new();
        let mut control_weights: Vec<f32> = Vec::new();
        let mut bad = false;
        for tok in &entry[3..] {
            let Ok(raw) = tok.parse::<i64>() else {
                bad = true;
                break;
            };
            let resolved = if raw < 0 { n_pos + 1 + raw } else { raw };
            if resolved <= 0 || resolved > n_pos {
                bad = true;
                break;
            }
            let pos = doc.positions[(resolved as usize) - 1];
            control_points.push(pos);
            // For rational forms, take the position's 4th-w weight from
            // the parallel `position_weights` pool (`v x y z w`).
            // Default 1.0 per spec when absent.
            let w = doc.position_weights[(resolved as usize) - 1].unwrap_or(1.0);
            control_weights.push(w);
        }
        if bad || control_points.len() < 2 {
            continue;
        }

        let curve_points = match kind {
            "bezier" | "rat_bezier" => sample_bezier(
                &control_points,
                &control_weights,
                kind,
                u_min,
                u_max,
                samples,
            ),
            "bspline" | "rat_bspline" => {
                // B-spline needs a knot vector and a degree. Spec
                // §"B-spline" condition 6: K = q - n - 1 ⇒ knot count
                // must equal control-point count + degree + 1. Skip
                // silently when missing — the source OBJ is incomplete
                // in spec terms but we don't want to abort the whole
                // decode.
                let Some(degree) = active_degree else {
                    continue;
                };
                if parm_u.len() != control_points.len() + degree as usize + 1 {
                    continue;
                }
                sample_bspline(
                    &control_points,
                    &control_weights,
                    kind,
                    degree,
                    parm_u,
                    u_min,
                    u_max,
                    samples,
                )
            }
            "cardinal" => {
                // Spec §"Cardinal": "Cardinal splines are only defined
                // for the cubic case." Reject non-cubic `deg`. The
                // `parm` count requirement (K - n + 2 values, ⇒ K - 2
                // segments) is informational here — we slide a window
                // of 4 control points and emit segments directly
                // without needing the global parameter vector for the
                // basis evaluation itself, since the Catmull-Rom
                // tangent definition is purely local (segment i uses
                // c[i..i+4]).
                if active_degree.is_some_and(|d| d != 3) {
                    continue;
                }
                // Need at least 4 control points for one segment.
                if control_points.len() < 4 {
                    continue;
                }
                sample_cardinal(&control_points, samples)
            }
            "taylor" => {
                // Spec §"Taylor": basis function is t^i; control points
                // are the polynomial coefficients. `deg n` ⇒ n + 1
                // coefficient vectors expected. Reject when the count
                // doesn't match (lenient: also accept missing `deg` and
                // infer n = K).
                let degree = match active_degree {
                    Some(d) => d as usize,
                    None => control_points.len().saturating_sub(1),
                };
                if control_points.len() != degree + 1 {
                    continue;
                }
                sample_taylor(&control_points, u_min, u_max, samples)
            }
            "bmatrix" => {
                // Spec §"Basis matrix": needs `deg n` + `bmat u <(n+1)²
                // floats>` + `step <stepu>` body statements. Without any
                // of those, the block is malformed in spec terms — skip
                // silently (lenient-loader pattern). The basis matrix is
                // (n + 1) × (n + 1) per spec §"Consistency conditions":
                // "the size of the basis matrix is (n + 1) x (n + 1)".
                let Some(degree) = active_degree else {
                    continue;
                };
                let Some(step) = step_u else {
                    continue;
                };
                let n_plus_1 = degree as usize + 1;
                if bmat_u.len() != n_plus_1 * n_plus_1 {
                    continue;
                }
                if step == 0 {
                    continue;
                }
                // Need at least n + 1 control points for one segment.
                if control_points.len() < n_plus_1 {
                    continue;
                }
                sample_bmatrix(&control_points, bmat_u, degree, step, samples)
            }
            _ => continue,
        };
        if curve_points.len() < 2 {
            continue;
        }

        let mut prim = Primitive::new(Topology::LineStrip);
        let n = curve_points.len() as u32;
        prim.positions = curve_points;
        // Implicit 0..N strip indices keep the buffer compact and
        // match how `LineStrip` consumers normally walk the vertex
        // array.
        if n > u16::MAX as u32 {
            prim.indices = Some(Indices::U32((0..n).collect()));
        } else {
            prim.indices = Some(Indices::U16((0..n).map(|i| i as u16).collect()));
        }

        prim.extras.insert(
            "obj:tessellated_curve".to_string(),
            serde_json::Value::Bool(true),
        );
        prim.extras.insert(
            "obj:curve_kind".to_string(),
            serde_json::Value::String(kind.to_string()),
        );
        // Reported degree: for Bezier the basis degree always equals
        // N − 1 (control-point count − 1). For B-spline the basis
        // degree is the `deg` value (independent of the control-point
        // count). We report whichever is semantically correct for the
        // basis.
        let reported_degree = match kind {
            "bezier" | "rat_bezier" => (control_points.len() - 1) as u64,
            "bspline" | "rat_bspline" => active_degree.unwrap_or(0) as u64,
            // Spec §"Cardinal": "Cardinal splines are only defined for
            // the cubic case." Always 3.
            "cardinal" => 3,
            // Spec §"Taylor": degree n ⇒ K + 1 = n + 1 coefficients.
            "taylor" => active_degree
                .map(u64::from)
                .unwrap_or_else(|| (control_points.len() - 1) as u64),
            // Spec §"Basis matrix": degree comes from `deg n`; the
            // basis matrix is (n + 1) × (n + 1).
            "bmatrix" => active_degree.map(u64::from).unwrap_or(0),
            _ => 0,
        };
        prim.extras.insert(
            "obj:curve_degree".to_string(),
            serde_json::Value::Number(serde_json::Number::from(reported_degree)),
        );
        let range_arr = serde_json::Value::Array(vec![
            serde_json::Value::from(u_min as f64),
            serde_json::Value::from(u_max as f64),
        ]);
        prim.extras
            .insert("obj:curve_u_range".to_string(), range_arr);
        prim.extras.insert(
            "obj:curve_samples".to_string(),
            serde_json::Value::Number(serde_json::Number::from(samples as u64)),
        );

        out.push(prim);
    }
}

/// Tessellate every `surf` element that sits under a `cstype bezier` /
/// `cstype rat bezier` header into a triangulated [`Topology::Triangles`]
/// primitive (round 11). Mirrors [`tessellate_curves`] but evaluates the
/// bivariate Bezier tensor product (spec §"Rational and non-rational
/// curves and surfaces", §"Bezier", §"Surface vertex data — control
/// points").
///
/// Only the Bezier basis is handled here; B-spline / Cardinal / Taylor /
/// basis-matrix surfaces are captured-only (the directive sequence still
/// round-trips through `Scene3D::extras["obj:freeform_directives"]`).
///
/// `surf` token layout (spec §"surf s0 s1 t0 t1 v1/vt1/vn1 …"):
/// `surf s0 s1 t0 t1` followed by `v/vt/vn` control-vertex references.
/// Only the leading position index of each `v/vt/vn` token is consumed;
/// texture / normal references are interpolation extras the renderer
/// would blend with the same basis (spec §"Texture vertices …",
/// §"Vertex normals …") but they don't change the surface shape, so the
/// position-only evaluation is sufficient for the polyline/triangle
/// approximation.
///
/// Control-point ordering (spec §"Surface vertex data — control
/// points"): "listed in the order i = 0 to K1 for j = 0, followed by
/// i = 0 to K1 for j = 1, and so on until j = K2." That is row-major
/// with the u index (`i`) varying fastest. For a single Bezier patch
/// `K1 = degu` and `K2 = degv`, so the control grid is
/// `(degu + 1) × (degv + 1)`.
///
/// Per-surface provenance lands on `Primitive::extras`:
///   * `obj:tessellated_curve` — `true` (shared sentinel so the encoder's
///     existing filter skips this synthetic geometry).
///   * `obj:tessellated_surface` — `true` (surface-specific sentinel).
///   * `obj:surface_kind` — `"bezier"` / `"rat_bezier"`.
///   * `obj:surface_degree` — `[degu, degv]`.
///   * `obj:surface_u_range` / `obj:surface_v_range` — `[s0, s1]` /
///     `[t0, t1]` from the `surf` directive.
///   * `obj:surface_samples` — sample count per parametric direction.
fn tessellate_surfaces(doc: &ObjDoc, samples: u32) -> Vec<Primitive> {
    let mut out: Vec<Primitive> = Vec::new();
    if samples == 0 {
        return out;
    }

    // Block state, accumulated between `cstype` … `end`.
    let mut active_kind: Option<&'static str> = None;
    let mut deg_u: Option<u32> = None;
    let mut deg_v: Option<u32> = None;
    let mut pending_surfs: Vec<&Vec<String>> = Vec::new();

    let flush = |out: &mut Vec<Primitive>,
                 kind: Option<&'static str>,
                 deg_u: Option<u32>,
                 deg_v: Option<u32>,
                 surfs: &[&Vec<String>]| {
        let Some(kind) = kind else {
            return;
        };
        for entry in surfs {
            if let Some(prim) = flush_surface(doc, kind, deg_u, deg_v, entry, samples) {
                out.push(prim);
            }
        }
    };

    for entry in &doc.freeform_directives {
        if entry.is_empty() {
            continue;
        }
        match entry[0].as_str() {
            "cstype" => {
                flush(&mut out, active_kind, deg_u, deg_v, &pending_surfs);
                pending_surfs.clear();
                deg_u = None;
                deg_v = None;
                // Spec §"Curve and surface type": `cstype [rat] type`.
                let mut iter = entry.iter().skip(1);
                let first = iter.next().map(String::as_str);
                let second = iter.next().map(String::as_str);
                active_kind = match (first, second) {
                    (Some("bezier"), _) => Some("bezier"),
                    (Some("rat"), Some("bezier")) => Some("rat_bezier"),
                    // Other surface bases stay captured-only for now.
                    _ => None,
                };
            }
            "deg" => {
                // Spec §"Degree": `deg degu [degv]`. For surfaces both
                // are required; `degv` defaults to `degu` only if a
                // single value was given (matches the spec note that
                // `degv` is "required only for surfaces").
                deg_u = entry.get(1).and_then(|t| t.parse::<u32>().ok());
                deg_v = entry.get(2).and_then(|t| t.parse::<u32>().ok()).or(deg_u);
            }
            "surf" => pending_surfs.push(entry),
            "end" => {
                flush(&mut out, active_kind, deg_u, deg_v, &pending_surfs);
                pending_surfs.clear();
                active_kind = None;
                deg_u = None;
                deg_v = None;
            }
            _ => {}
        }
    }
    // Tail flush — defensive against a missing closing `end`.
    flush(&mut out, active_kind, deg_u, deg_v, &pending_surfs);
    out
}

/// Evaluate one `surf` element against an active Bezier `cstype` and
/// return the triangulated primitive, or `None` when the directive is
/// incomplete / malformed (lenient-loader pattern — the directive still
/// round-trips through `obj:freeform_directives`).
fn flush_surface(
    doc: &ObjDoc,
    kind: &'static str,
    deg_u: Option<u32>,
    deg_v: Option<u32>,
    entry: &[String],
    samples: u32,
) -> Option<Primitive> {
    // `surf s0 s1 t0 t1 v1/vt1/vn1 …` — minimum is the keyword + 4
    // range scalars + at least one control vertex.
    if entry.len() < 6 {
        return None;
    }
    let s0 = entry[1].parse::<f32>().ok()?;
    let s1 = entry[2].parse::<f32>().ok()?;
    let t0 = entry[3].parse::<f32>().ok()?;
    let t1 = entry[4].parse::<f32>().ok()?;

    // Spec §"surf": both degu and degv are required for a surface.
    let du = deg_u? as usize;
    let dv = deg_v? as usize;
    // Single-patch Bezier needs exactly (degu + 1) × (degv + 1) control
    // points. Multi-patch surfaces (where the control-point count is a
    // larger grid stitched together) are not split here; they would need
    // the `step` stride which the Bezier basis doesn't carry, so we only
    // tessellate the single-patch case and leave bigger grids
    // captured-only.
    let cols = du + 1; // u-direction count (K1 + 1)
    let rows = dv + 1; // v-direction count (K2 + 1)
    let expected = cols * rows;

    let n_pos = doc.positions.len() as i64;
    let mut grid: Vec<[f32; 3]> = Vec::with_capacity(expected);
    let mut weights: Vec<f32> = Vec::with_capacity(expected);
    for tok in &entry[5..] {
        // Each control vertex is a `v/vt/vn` reference; we only need the
        // leading position index.
        let first_field = tok.split('/').next().unwrap_or(tok);
        let raw = first_field.parse::<i64>().ok()?;
        let resolved = if raw < 0 { n_pos + 1 + raw } else { raw };
        if resolved <= 0 || resolved > n_pos {
            return None;
        }
        grid.push(doc.positions[(resolved as usize) - 1]);
        let w = doc.position_weights[(resolved as usize) - 1].unwrap_or(1.0);
        weights.push(w);
    }
    if grid.len() != expected {
        // Not a single patch of the declared degree — leave it captured-
        // only rather than guessing the patch decomposition.
        return None;
    }

    let positions = sample_bezier_surface(&grid, &weights, kind, cols, rows, samples);
    if positions.is_empty() {
        return None;
    }

    // Build a triangle grid over the (samples + 1) × (samples + 1)
    // sample lattice. Vertex (su, sv) lives at index sv * stride + su.
    let stride = samples as usize + 1;
    let mut indices: Vec<u32> = Vec::with_capacity((samples as usize) * (samples as usize) * 6);
    for sv in 0..samples as usize {
        for su in 0..samples as usize {
            let i00 = (sv * stride + su) as u32;
            let i10 = (sv * stride + su + 1) as u32;
            let i01 = ((sv + 1) * stride + su) as u32;
            let i11 = ((sv + 1) * stride + su + 1) as u32;
            // Two CCW triangles per cell (spec §"surf" note: the front
            // of the surface is the side where u increases to the right
            // and v increases upward).
            indices.push(i00);
            indices.push(i10);
            indices.push(i11);
            indices.push(i00);
            indices.push(i11);
            indices.push(i01);
        }
    }

    let mut prim = Primitive::new(Topology::Triangles);
    let n_verts = positions.len() as u32;
    prim.positions = positions;
    prim.indices = if n_verts > u16::MAX as u32 {
        Some(Indices::U32(indices))
    } else {
        Some(Indices::U16(indices.iter().map(|&i| i as u16).collect()))
    };

    prim.extras.insert(
        "obj:tessellated_curve".to_string(),
        serde_json::Value::Bool(true),
    );
    prim.extras.insert(
        "obj:tessellated_surface".to_string(),
        serde_json::Value::Bool(true),
    );
    prim.extras.insert(
        "obj:surface_kind".to_string(),
        serde_json::Value::String(kind.to_string()),
    );
    prim.extras.insert(
        "obj:surface_degree".to_string(),
        serde_json::Value::Array(vec![
            serde_json::Value::from(du as u64),
            serde_json::Value::from(dv as u64),
        ]),
    );
    prim.extras.insert(
        "obj:surface_u_range".to_string(),
        serde_json::Value::Array(vec![
            serde_json::Value::from(s0 as f64),
            serde_json::Value::from(s1 as f64),
        ]),
    );
    prim.extras.insert(
        "obj:surface_v_range".to_string(),
        serde_json::Value::Array(vec![
            serde_json::Value::from(t0 as f64),
            serde_json::Value::from(t1 as f64),
        ]),
    );
    prim.extras.insert(
        "obj:surface_samples".to_string(),
        serde_json::Value::Number(serde_json::Number::from(samples as u64)),
    );

    Some(prim)
}

/// Evaluate a Bezier (or rational-Bezier) surface patch at a
/// `(samples + 1) × (samples + 1)` lattice via the tensor-product de
/// Casteljau algorithm.
///
/// `grid` is the control mesh in row-major order with the u index
/// varying fastest (spec §"Surface vertex data — control points"):
/// `cols` control points per v-row, `rows` v-rows. For each `(u, v)`
/// sample the surface is `S(u, v) = Σ_i Σ_j B_i(u) · B_j(v) · d_{i,j}`.
/// We collapse the inner u sum first by running de Casteljau on each
/// v-row, then a second de Casteljau on the resulting `rows` points in
/// the v direction.
///
/// For `kind == "rat_bezier"` each control point is lifted to its
/// homogeneous `(w·x, w·y, w·z, w)` form, both de Casteljau passes run
/// in 4D, and the result is projected back via `x / w` (spec
/// §"Rational and non-rational curves and surfaces").
///
/// Output vertices are ordered row-major in the sample lattice: sample
/// `(su, sv)` lands at index `sv * (samples + 1) + su`.
fn sample_bezier_surface(
    grid: &[[f32; 3]],
    weights: &[f32],
    kind: &str,
    cols: usize,
    rows: usize,
    samples: u32,
) -> Vec<[f32; 3]> {
    if samples == 0 || cols == 0 || rows == 0 || grid.len() != cols * rows {
        return Vec::new();
    }
    let rational = kind == "rat_bezier";
    // Lift to homogeneous 4D so a single de Casteljau loop handles both
    // forms (non-rational uses w == 1).
    let homo: Vec<[f32; 4]> = grid
        .iter()
        .zip(weights.iter())
        .map(|(p, w)| {
            let weight = if rational { *w } else { 1.0 };
            [p[0] * weight, p[1] * weight, p[2] * weight, weight]
        })
        .collect();

    let n = samples as usize + 1;
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n * n);
    for sv in 0..n {
        let v = if n == 1 {
            0.0
        } else {
            sv as f32 / (n - 1) as f32
        };
        for su in 0..n {
            let u = if n == 1 {
                0.0
            } else {
                su as f32 / (n - 1) as f32
            };
            // Inner pass: de Casteljau across each v-row in u, leaving
            // one homogeneous point per row.
            let mut col_pts: Vec<[f32; 4]> = Vec::with_capacity(rows);
            for r in 0..rows {
                let row = &homo[r * cols..r * cols + cols];
                col_pts.push(de_casteljau_4d(row, u));
            }
            // Outer pass: de Casteljau in v over the collapsed points.
            let pt = de_casteljau_4d(&col_pts, v);
            let [x, y, z, w] = pt;
            if rational && w.abs() > f32::EPSILON {
                out.push([x / w, y / w, z / w]);
            } else {
                out.push([x, y, z]);
            }
        }
    }
    out
}

/// de Casteljau evaluation of a homogeneous 4D Bezier control polygon at
/// parameter `t ∈ [0, 1]`. Shared by the row and column passes of
/// [`sample_bezier_surface`].
fn de_casteljau_4d(points: &[[f32; 4]], t: f32) -> [f32; 4] {
    if points.is_empty() {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let mut buf: Vec<[f32; 4]> = points.to_vec();
    let n = buf.len();
    for level in 1..n {
        for j in 0..(n - level) {
            buf[j] = [
                (1.0 - t) * buf[j][0] + t * buf[j + 1][0],
                (1.0 - t) * buf[j][1] + t * buf[j + 1][1],
                (1.0 - t) * buf[j][2] + t * buf[j + 1][2],
                (1.0 - t) * buf[j][3] + t * buf[j + 1][3],
            ];
        }
    }
    buf[0]
}

/// Evaluate a Bezier (or rational-Bezier) curve at `samples + 1`
/// uniformly-spaced parameter values from `u_min` to `u_max` via the
/// numerically-stable de Casteljau algorithm.
///
/// For `kind == "bezier"` weights are ignored and the result is the
/// straight 3D control-point combination.
///
/// For `kind == "rat_bezier"` each control point is treated as a
/// homogeneous `(w·x, w·y, w·z, w)` 4-tuple, de Casteljau runs on the
/// 4D form, and the final point is projected back to 3D by `x/w`.
/// This matches the spec §"Curve" rational form.
fn sample_bezier(
    control_points: &[[f32; 3]],
    control_weights: &[f32],
    kind: &str,
    _u_min: f32,
    _u_max: f32,
    samples: u32,
) -> Vec<[f32; 3]> {
    if control_points.is_empty() || samples == 0 {
        return Vec::new();
    }
    let rational = kind == "rat_bezier";
    // Build the working buffer in 4D so the same de Casteljau loop
    // covers both rational and non-rational cases (non-rational uses
    // w == 1).
    let homogeneous: Vec<[f32; 4]> = control_points
        .iter()
        .zip(control_weights.iter())
        .map(|(p, w)| {
            let weight = if rational { *w } else { 1.0 };
            [p[0] * weight, p[1] * weight, p[2] * weight, weight]
        })
        .collect();

    let n_samples = samples + 1;
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n_samples as usize);
    for i in 0..n_samples {
        // Normalise sample index into the curve's parameter range so
        // `u_min` and `u_max` aren't mandatorily [0, 1].
        let t01 = if n_samples == 1 {
            0.0
        } else {
            i as f32 / (n_samples - 1) as f32
        };
        // The `u_min` / `u_max` arguments on `curv` are spec-defined
        // clip bounds for trimming the basis evaluation, not a
        // re-parameterisation of the basis. For a single un-trimmed
        // Bezier segment they have no effect on shape — the curve
        // domain is `[0, 1]` in basis space. We sample uniformly on
        // `t01 ∈ [0, 1]` (so a non-trivial `u_min, u_max` doesn't
        // distort the polyline), which is what every other OBJ
        // tessellator does.
        let t = t01;
        let mut buf: Vec<[f32; 4]> = homogeneous.clone();
        let n = buf.len();
        for level in 1..n {
            for j in 0..(n - level) {
                buf[j] = [
                    (1.0 - t) * buf[j][0] + t * buf[j + 1][0],
                    (1.0 - t) * buf[j][1] + t * buf[j + 1][1],
                    (1.0 - t) * buf[j][2] + t * buf[j + 1][2],
                    (1.0 - t) * buf[j][3] + t * buf[j + 1][3],
                ];
            }
        }
        let [x, y, z, w] = buf[0];
        if rational && w.abs() > f32::EPSILON {
            out.push([x / w, y / w, z / w]);
        } else {
            out.push([x, y, z]);
        }
    }
    out
}

/// Evaluate a B-spline (or rational B-spline / NURBS) curve at
/// `samples + 1` uniformly-spaced parameter values from `t_min` to
/// `t_max`, where the interval is clipped against the spec-required
/// `[x_n, x_{K+1}]` evaluation range of the knot vector (spec §"B-spline"
/// condition 5: `x_n ≤ t_min < t_max ≤ x_{K+1}`).
///
/// Mathematics — Cox-deBoor recursion (spec §"B-spline"):
///
///   N_{i,0}(t) = 1 if x_i ≤ t < x_{i+1} else 0
///   N_{i,k}(t) = (t - x_i) / (x_{i+k} - x_i)         · N_{i,k-1}(t)
///              + (x_{i+k+1} - t) / (x_{i+k+1} - x_{i+1}) · N_{i+1,k-1}(t)
///
/// by convention `0/0 = 0`. The curve at parameter t is
///
///   C(t) = Σ_{i=0..K} N_{i,n}(t) · d_i
///
/// For the rational form, the weighted homogeneous sum is computed and
/// projected back to 3D via `x/w`:
///
///   C(t) = Σ N_{i,n}(t) · w_i · d_i / Σ N_{i,n}(t) · w_i
///
/// `kind` selects `"bspline"` (weights ignored, w = 1) or
/// `"rat_bspline"` (per-vertex `w` from `v x y z w`).
#[allow(clippy::too_many_arguments)]
fn sample_bspline(
    control_points: &[[f32; 3]],
    control_weights: &[f32],
    kind: &str,
    degree: u32,
    knots: &[f32],
    u_min: f32,
    u_max: f32,
    samples: u32,
) -> Vec<[f32; 3]> {
    if control_points.is_empty() || samples == 0 {
        return Vec::new();
    }
    let n = degree as usize;
    let k_plus_1 = control_points.len(); // = K + 1 control points.
    // Spec §"B-spline" condition 6: K = q - n - 1 ⇒ knots.len() must
    // equal control_points.len() + degree + 1. The caller already
    // checks this; double-check defensively.
    if knots.len() != k_plus_1 + n + 1 {
        return Vec::new();
    }
    // Spec condition 5: evaluation parameter t must satisfy
    //   x_n ≤ t_min < t_max ≤ x_{K+1}
    // Clip the caller-supplied u_min / u_max against that window so the
    // basis functions evaluate to defined values (any t outside the
    // window gives N = 0 across the support and a degenerate sample).
    let t_lo_bound = knots[n];
    let t_hi_bound = knots[k_plus_1]; // x_{K+1} index = K+1 = k_plus_1.
    let t_min = u_min.max(t_lo_bound);
    let t_max = u_max.min(t_hi_bound);
    if t_min > t_max {
        return Vec::new();
    }

    let rational = kind == "rat_bspline";
    let n_samples = samples + 1;
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n_samples as usize);

    for i in 0..n_samples {
        let t01 = if n_samples == 1 {
            0.0
        } else {
            i as f32 / (n_samples - 1) as f32
        };
        let mut t = t_min + t01 * (t_max - t_min);
        // Numerical guard — when t == t_hi_bound, the half-open interval
        // convention `x_i ≤ t < x_{i+1}` makes N_{i,0} zero everywhere.
        // Nudge the last sample fractionally below the upper bound so
        // it lies inside the last non-empty knot span (a standard NURBS-
        // evaluator pattern; the resulting blend converges to the curve
        // endpoint as the bias shrinks).
        if t >= t_hi_bound {
            t = t_hi_bound - (t_hi_bound - t_lo_bound).abs() * 1e-7 - f32::EPSILON;
            if t < t_lo_bound {
                t = t_lo_bound;
            }
        }
        let basis = bspline_basis(t, knots, n);
        // Σ N_{i,n}(t) · w_i · d_i  (3D positions blended).
        // For non-rational, w_i = 1 ⇒ standard polynomial blend.
        let mut acc = [0.0f32; 3];
        let mut wsum = 0.0f32;
        for j in 0..k_plus_1 {
            let bj = basis[j];
            if bj == 0.0 {
                continue;
            }
            let w = if rational { control_weights[j] } else { 1.0 };
            let bw = bj * w;
            wsum += bw;
            acc[0] += bw * control_points[j][0];
            acc[1] += bw * control_points[j][1];
            acc[2] += bw * control_points[j][2];
        }
        if rational && wsum.abs() > f32::EPSILON {
            out.push([acc[0] / wsum, acc[1] / wsum, acc[2] / wsum]);
        } else if !rational && wsum.abs() > f32::EPSILON {
            // Non-rational basis functions sum to 1 inside the valid
            // window by partition-of-unity (spec note: "basis functions
            // sum to 1.0, such as Bezier, Cardinal, and NURB"); no
            // division needed in theory, but we still emit `acc` as-is.
            out.push(acc);
        } else {
            // Sample fell outside the support of every basis function —
            // emit the running accumulator (which is zero) so the
            // polyline length still matches `samples + 1`. In practice
            // the clip + nudge above prevents this branch except for
            // pathological knot vectors.
            out.push(acc);
        }
    }
    out
}

/// Cox-deBoor recursive basis-function evaluation at parameter `t`
/// against the given knot vector. Returns one weight per control point
/// (control-point count = knots.len() − degree − 1).
///
/// Uses the iterative bottom-up formulation: build degree-0 step
/// functions, then accumulate higher-degree polynomials in place. This
/// is `O(k_plus_1 · (degree + 1))` work per evaluation, which suffices
/// for the modest curve sizes typical of OBJ files. The standard
/// `0/0 = 0` convention is applied via explicit denominator guards
/// (spec §"B-spline" inline note).
fn bspline_basis(t: f32, knots: &[f32], degree: usize) -> Vec<f32> {
    let m = knots.len();
    if m <= degree + 1 {
        return Vec::new();
    }
    let k_plus_1 = m - degree - 1;
    // Allocate one row of `m - 1` degree-0 weights (one per knot span);
    // we'll fold this down to k_plus_1 weights at the end.
    let mut basis: Vec<f32> = Vec::with_capacity(m - 1);
    for i in 0..(m - 1) {
        // Degree-0: indicator function on the half-open knot span. Use
        // the closed-on-the-right convention for the final span so that
        // a t exactly at the upper bound still falls inside the last
        // non-empty interval (NURBS-evaluator convention).
        let inside = if i + 1 == m - 1 {
            knots[i] <= t && t <= knots[i + 1]
        } else {
            knots[i] <= t && t < knots[i + 1]
        };
        basis.push(if inside { 1.0 } else { 0.0 });
    }
    // Recursive degree promotion.
    for k in 1..=degree {
        // After this loop iteration we want length (m - 1 - k); we
        // overwrite in place, indexing j and j+1.
        let new_len = m - 1 - k;
        for j in 0..new_len {
            let denom_left = knots[j + k] - knots[j];
            let denom_right = knots[j + k + 1] - knots[j + 1];
            let left = if denom_left.abs() < f32::EPSILON {
                0.0
            } else {
                (t - knots[j]) / denom_left * basis[j]
            };
            let right = if denom_right.abs() < f32::EPSILON {
                0.0
            } else {
                (knots[j + k + 1] - t) / denom_right * basis[j + 1]
            };
            basis[j] = left + right;
        }
        basis.truncate(new_len);
    }
    debug_assert_eq!(basis.len(), k_plus_1);
    basis
}

/// Evaluate a cubic Cardinal (Catmull-Rom) curve at `samples + 1`
/// uniformly-spaced parameter values from `t = 0` (start of first
/// segment) to `t = K - 2` (end of last segment) where `K = control_points.len()`.
///
/// Spec §"Cardinal": Cardinal splines are cubic only and interpolate all
/// but the first and last control points. The conversion to Bezier
/// control points for one segment over `c0, c1, c2, c3` is:
///
///   b0 = c1
///   b1 = c1 + (c2 - c0) / 6
///   b2 = c2 - (c3 - c1) / 6
///   b3 = c2
///
/// The full curve is the concatenation of `K - 3` such Bezier segments
/// produced by sliding a 4-point window across the control polygon —
/// segment `i` consumes `c[i..i+4]` and traces from the interpolated
/// midpoint `c[i+1]` to `c[i+2]`. This yields a C¹-continuous piecewise
/// curve that passes through every interior control point exactly.
///
/// The result is emitted as one polyline carrying `samples + 1` total
/// vertices distributed across all segments in proportion to their share
/// of the parameter range. To keep the implementation simple and the
/// polyline density uniform along the curve, we evaluate `samples` total
/// intervals (`samples + 1` points) globally, mapping each global sample
/// to a segment index plus a local `t ∈ [0, 1]` within that segment.
///
/// Weights / rationality: the spec note says the unit-weight default is
/// reasonable for Cardinal because its basis functions sum to 1, so we
/// don't differentiate `rat cardinal` from `cardinal` — the per-vertex
/// 4th `w` weight is read from `position_weights` but treated as 1 in
/// the Bezier-conversion form (where it would otherwise alter the shape
/// in a way the spec doesn't explicitly define).
fn sample_cardinal(control_points: &[[f32; 3]], samples: u32) -> Vec<[f32; 3]> {
    if control_points.len() < 4 || samples == 0 {
        return Vec::new();
    }
    let n_segments = control_points.len() - 3;
    let n_samples = samples + 1;
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n_samples as usize);

    for i in 0..n_samples {
        // Global `s ∈ [0, n_segments]`; integer part picks the segment,
        // fractional part is the local `t ∈ [0, 1]`. Pin the last sample
        // to the very end of the last segment so the polyline closes
        // exactly on `c[K-2]`.
        let s = if i == n_samples - 1 {
            n_segments as f32
        } else {
            i as f32 * n_segments as f32 / (n_samples - 1) as f32
        };
        let mut seg = s.floor() as usize;
        let mut t = s - seg as f32;
        if seg >= n_segments {
            seg = n_segments - 1;
            t = 1.0;
        }
        // 4 Cardinal control points for this segment.
        let c0 = control_points[seg];
        let c1 = control_points[seg + 1];
        let c2 = control_points[seg + 2];
        let c3 = control_points[seg + 3];
        // Spec §"Cardinal" Bezier conversion (component-wise per axis):
        //   b0 = c1
        //   b1 = c1 + (c2 - c0) / 6
        //   b2 = c2 - (c3 - c1) / 6
        //   b3 = c2
        let mut b: [[f32; 3]; 4] = [[0.0; 3]; 4];
        for a in 0..3 {
            b[0][a] = c1[a];
            b[1][a] = c1[a] + (c2[a] - c0[a]) / 6.0;
            b[2][a] = c2[a] - (c3[a] - c1[a]) / 6.0;
            b[3][a] = c2[a];
        }
        // Cubic Bezier evaluation (Bernstein form, expanded for n = 3
        // since the spec only defines Cardinal for the cubic case):
        //   B(t) = (1-t)^3 b0 + 3(1-t)^2 t b1 + 3(1-t) t^2 b2 + t^3 b3
        let u = 1.0 - t;
        let w0 = u * u * u;
        let w1 = 3.0 * u * u * t;
        let w2 = 3.0 * u * t * t;
        let w3 = t * t * t;
        let p = [
            w0 * b[0][0] + w1 * b[1][0] + w2 * b[2][0] + w3 * b[3][0],
            w0 * b[0][1] + w1 * b[1][1] + w2 * b[2][1] + w3 * b[3][1],
            w0 * b[0][2] + w1 * b[1][2] + w2 * b[2][2] + w3 * b[3][2],
        ];
        out.push(p);
    }
    out
}

/// Evaluate a Taylor polynomial curve at `samples + 1` uniformly-spaced
/// parameter values from `u_min` to `u_max`.
///
/// Spec §"Taylor": "The basis function is simply t^i" with the note
/// that the control points are the polynomial coefficients (and have no
/// geometric significance). So for `K + 1` control points c_0..c_K
/// supplied via `curv`, the curve is:
///
///   P(t) = c_0 + c_1 · t + c_2 · t^2 + … + c_K · t^K
///
/// applied component-wise per axis. This is Horner's-rule territory —
/// we use the straightforward bottom-up evaluation:
///
///   P(t) = ((c_K · t + c_{K-1}) · t + c_{K-2}) · t + … + c_0
///
/// which is numerically well-behaved for the modest degrees typical of
/// real Taylor curves (the spec example is degree 4).
///
/// The `u_min` / `u_max` arguments on the `curv` directive are the
/// global parameter clip bounds; Taylor curves evaluate against `t`
/// directly (not a normalised `[0, 1]` re-parameterisation) so we
/// sample at `t_i = u_min + i / samples · (u_max - u_min)`.
fn sample_taylor(
    control_points: &[[f32; 3]],
    u_min: f32,
    u_max: f32,
    samples: u32,
) -> Vec<[f32; 3]> {
    if control_points.is_empty() || samples == 0 {
        return Vec::new();
    }
    let n_samples = samples + 1;
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n_samples as usize);
    let k = control_points.len();
    for i in 0..n_samples {
        let frac = if n_samples == 1 {
            0.0
        } else {
            i as f32 / (n_samples - 1) as f32
        };
        let t = u_min + frac * (u_max - u_min);
        // Horner's rule on the coefficient vector. Walk from the
        // highest-order coefficient down to c_0.
        let mut acc = control_points[k - 1];
        for j in (0..(k - 1)).rev() {
            acc[0] = acc[0] * t + control_points[j][0];
            acc[1] = acc[1] * t + control_points[j][1];
            acc[2] = acc[2] * t + control_points[j][2];
        }
        out.push(acc);
    }
    out
}

/// Evaluate a basis-matrix curve at `samples + 1` total points.
///
/// Spec §"Basis matrix": general arbitrary-degree curves whose basis is
/// expressed through a user-supplied `(n + 1) × (n + 1)` matrix `B`
/// (passed via `bmat u`) and segment stride `s` (passed via `step`).
/// Each polynomial segment `i` consumes the control-point window
/// `c[i·s .. i·s + n]` (0-based) and evaluates per spec §"Basis matrix":
///
/// ```text
///   P(t) = Σ_{i=0..n} Σ_{j=0..n} B[i][j] · t^j · p_i
/// ```
///
/// where `B[i][j]` is the row-major element of `bmat u` with column
/// index `j` varying fastest (per spec §"bmat u/v matrix": "matrix
/// lists the contents of the basis matrix with column subscript j
/// varying the fastest"). For the spec's cubic-Bezier-as-bmatrix
/// example, this produces the standard Bernstein basis.
///
/// Number of segments per spec §"step": with `K` control points,
/// degree `n`, and step `s`, segment `i` uses indices
/// `c_{i·s + 1} .. c_{i·s + n + 1}` (1-based) ⇒ the segment count is
/// `floor((K - n - 1) / s) + 1` when `K ≥ n + 1`. Samples are
/// distributed proportionally across all segments so the polyline
/// density is uniform along the global parameter.
///
/// Rationality: the spec note in §"Free-form curve/surface body
/// statements" explicitly says the unit-weight default "may or may
/// not make sense for a representation given in basis-matrix form",
/// so we don't apply per-vertex weights here — the user's `bmat u`
/// is the authoritative basis.
fn sample_bmatrix(
    control_points: &[[f32; 3]],
    bmat_u: &[f32],
    degree: u32,
    step: u32,
    samples: u32,
) -> Vec<[f32; 3]> {
    let n_plus_1 = degree as usize + 1;
    if control_points.len() < n_plus_1
        || bmat_u.len() != n_plus_1 * n_plus_1
        || step == 0
        || samples == 0
    {
        return Vec::new();
    }
    // Spec §"step stepu stepv": segment `i` uses control points
    // `c_{i·s + 1} .. c_{i·s + n + 1}` (1-based). Solve for the largest
    // i with `i·s + n + 1 ≤ K` ⇒ `i ≤ (K - n - 1) / s`.
    let s = step as usize;
    let n_segments = (control_points.len() - n_plus_1) / s + 1;
    let n_samples = samples + 1;
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n_samples as usize);

    for i in 0..n_samples {
        // Global `g ∈ [0, n_segments]` with integer part = segment and
        // fractional part = local `t ∈ [0, 1]` within that segment. Pin
        // the last sample exactly to the end of the final segment so
        // the polyline closes on the spec-defined endpoint.
        let g = if i == n_samples - 1 {
            n_segments as f32
        } else {
            i as f32 * n_segments as f32 / (n_samples - 1) as f32
        };
        let mut seg = g.floor() as usize;
        let mut t = g - seg as f32;
        if seg >= n_segments {
            seg = n_segments - 1;
            t = 1.0;
        }
        let base = seg * s;

        // Compute t^0 .. t^n once.
        let mut t_pow: Vec<f32> = Vec::with_capacity(n_plus_1);
        let mut p = 1.0_f32;
        for _ in 0..n_plus_1 {
            t_pow.push(p);
            p *= t;
        }

        // P(t) = Σ_i p_i · (Σ_j B[i][j] · t^j) summed component-wise.
        let mut accum = [0.0_f32; 3];
        for ii in 0..n_plus_1 {
            // Row `ii` of B, dotted against `[t^0, t^1, …, t^n]`.
            let mut coef = 0.0_f32;
            for jj in 0..n_plus_1 {
                coef += bmat_u[ii * n_plus_1 + jj] * t_pow[jj];
            }
            let cp = control_points[base + ii];
            accum[0] += coef * cp[0];
            accum[1] += coef * cp[1];
            accum[2] += coef * cp[2];
        }
        out.push(accum);
    }
    out
}

/// `true` when the primitive was synthesised by the curve tessellator
/// (see [`tessellate_curves`]). Encoder + serialiser branches use this
/// to skip emitting derived geometry as `v` lines — the original
/// `cstype` / `curv` / `end` directives carry the source-of-truth
/// shape.
fn is_tessellated_curve(prim: &Primitive) -> bool {
    prim.extras
        .get("obj:tessellated_curve")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
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

/// Parser configuration knobs.
///
/// The default leaves free-form geometry as captured-only extras
/// (back-compatible with rounds 1-6). Set
/// [`ParseOptions::curve_tessellation_samples`] to a non-zero value
/// to enable evaluation of `cstype bezier` / `cstype bspline`
/// (rational + non-rational) curves into real `LineStrip` primitives
/// (see [`crate::ObjDecoder::with_curve_tessellation`]).
#[derive(Clone, Debug, Default)]
pub struct ParseOptions {
    /// When > 0, every `curv` directive under an active `cstype bezier`
    /// / `cstype rat bezier` / `cstype bspline` / `cstype rat bspline`
    /// header is evaluated at `curve_tessellation_samples + 1`
    /// uniformly-spaced parameter values. The resulting polyline lands
    /// on a synthetic mesh named `"obj:curves"` whose primitives carry
    /// `Topology::LineStrip`. The directive itself is still preserved
    /// in `Scene3D::extras["obj:freeform_directives"]` so a round-trip
    /// re-emit produces the same free-form section — downstream
    /// consumers can opt out of the synthetic mesh by filtering on
    /// `Primitive::extras["obj:tessellated_curve"] == true`.
    ///
    /// B-spline curves additionally require a valid `parm u` knot
    /// vector (length must equal control-point count + degree + 1 per
    /// spec §"B-spline" condition 6); curves with an incomplete knot
    /// vector are skipped silently.
    ///
    /// `0` disables tessellation (the default; back-compat with r1-r6).
    pub curve_tessellation_samples: u32,
}

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
pub fn parse_obj_with_resolver<R>(text: &str, resolve: R) -> Result<Scene3D>
where
    R: FnMut(&str) -> Result<Vec<u8>>,
{
    parse_obj_with_options(text, &ParseOptions::default(), resolve)
}

/// Parse an OBJ document with explicit [`ParseOptions`] and a
/// caller-supplied `mtllib` resolver. Lifts the option struct out of
/// the otherwise-identical [`parse_obj_with_resolver`] signature.
pub fn parse_obj_with_options<R>(
    text: &str,
    options: &ParseOptions,
    mut resolve: R,
) -> Result<Scene3D>
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

    // Curve tessellation pass — captures the curve directives still in
    // `doc.freeform_directives` and synthesises `LineStrip` primitives
    // on a dedicated mesh. Skipped when samples == 0 (the default).
    // Supports `cstype bezier` / `rat bezier` (round 7) and
    // `cstype bspline` / `rat bspline` (round 8).
    let tessellated = if options.curve_tessellation_samples > 0 {
        tessellate_curves(&doc, options.curve_tessellation_samples)
    } else {
        Vec::new()
    };

    // Surface tessellation pass — the same sample knob drives Bezier
    // `surf` tensor-product evaluation (round 11). Synthesises a
    // `Topology::Triangles` mesh; the directives still ride on
    // `Scene3D::extras["obj:freeform_directives"]` for round-trip.
    let tessellated_surfaces = if options.curve_tessellation_samples > 0 {
        tessellate_surfaces(&doc, options.curve_tessellation_samples)
    } else {
        Vec::new()
    };

    let mut scene = build_scene(doc)?;

    if !tessellated.is_empty() {
        let mut mesh = Mesh::new(Some("obj:curves".to_string()));
        for prim in tessellated {
            mesh.primitives.push(prim);
        }
        scene.add_mesh(mesh);
    }

    if !tessellated_surfaces.is_empty() {
        let mut mesh = Mesh::new(Some("obj:surfaces".to_string()));
        for prim in tessellated_surfaces {
            mesh.primitives.push(prim);
        }
        scene.add_mesh(mesh);
    }

    Ok(scene)
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

    // Seed the position pool with `obj:positions` if present — these
    // are the source 1-based vertex coordinates captured on decode so
    // free-form directives (`curv`, `surf`, etc.) that reference
    // positions by absolute index keep resolving correctly across a
    // decode → encode → decode round-trip. Without this, the encoder
    // would only pool positions referenced by polygonal primitives and
    // the free-form directive numbering would silently drift.
    if let Some(serde_json::Value::Array(src_positions)) = scene.extras.get("obj:positions") {
        let src_weights: Vec<Option<f32>> = scene
            .extras
            .get("obj:position_weights")
            .and_then(serde_json::Value::as_array)
            .map(|arr| arr.iter().map(|v| v.as_f64().map(|f| f as f32)).collect())
            .unwrap_or_default();
        let src_colors: Vec<Option<[f32; 4]>> = scene
            .extras
            .get("obj:position_colors")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .map(|v| {
                        v.as_array().map(|c| {
                            let mut rgba = [1.0; 4];
                            for (i, x) in c.iter().enumerate().take(4) {
                                rgba[i] = x.as_f64().map(|f| f as f32).unwrap_or(0.0);
                            }
                            rgba
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        for (i, pv) in src_positions.iter().enumerate() {
            let serde_json::Value::Array(coords) = pv else {
                continue;
            };
            let mut p = [0.0_f32; 3];
            for (j, c) in coords.iter().enumerate().take(3) {
                p[j] = c.as_f64().map(|f| f as f32).unwrap_or(0.0);
            }
            let weight = src_weights.get(i).copied().flatten();
            let colour = src_colors.get(i).copied().flatten();
            intern_pos(
                p,
                colour,
                weight,
                &mut positions,
                &mut position_colors,
                &mut position_weights,
                &mut pos_map,
            );
        }
    }

    // First pass: emit `v` / `vt` / `vn` lists and remember the global
    // indices for each (mesh, primitive, vertex) triple.
    //
    // Primitives flagged `obj:tessellated_curve = true` are synthetic
    // (they came out of the Bezier evaluator, not source `v` lines).
    // We skip them here so their points don't pollute the `v` pool and
    // skip them again in the element-emit pass below — the original
    // `cstype` / `curv` / `end` directives still get replayed verbatim
    // from `Scene3D::extras["obj:freeform_directives"]`, so the
    // round-trip stays bit-stable for the directive section.
    type GlobalTriple = (u32, u32, u32); // (v_idx, vt_idx_or_0, vn_idx_or_0)
    let mut global_indices: Vec<Vec<Vec<GlobalTriple>>> = Vec::new();
    for mesh in &scene.meshes {
        let mut mesh_globals: Vec<Vec<GlobalTriple>> = Vec::new();
        for prim in &mesh.primitives {
            if is_tessellated_curve(prim) {
                // Push an empty slot so global_indices[mi][pi] still
                // lines up with mesh.primitives[mi][pi] in the second
                // pass — we'll just skip the empty slot there.
                mesh_globals.push(Vec::new());
                continue;
            }
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
        // Synthesised curve mesh — its primitives carry
        // `obj:tessellated_curve = true` and were produced by the
        // decoder's de-Casteljau pass. Skip the whole `o` block; the
        // original `cstype`/`curv`/`end` directives still get replayed
        // from `Scene3D::extras["obj:freeform_directives"]`.
        if mesh.primitives.iter().all(is_tessellated_curve) && !mesh.primitives.is_empty() {
            continue;
        }
        if let Some(name) = &mesh.name {
            writeln!(out, "o {name}").unwrap();
        }

        for (pi, prim) in mesh.primitives.iter().enumerate() {
            if is_tessellated_curve(prim) {
                continue;
            }
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
