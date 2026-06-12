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
    /// Active texture-map name from `usemap <name>` or `usemap off`.
    /// Spec §"usemap map_name/off" — rendering identifier that names
    /// the texture map for the following elements. `None` means "no
    /// `usemap` directive has been seen yet" (the spec default is
    /// `off`); `Some("off")` is the explicit-off form; `Some(name)`
    /// is an active map binding. State-setting in the same shape as
    /// `usemtl`: a change mid-stream splits the primitive so each one
    /// carries one consistent binding.
    usemap: Option<String>,
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
    /// Texture-map library file names referenced by `maplib`. Spec
    /// §"maplib filename1 filename2 ..." — sibling to `mtllib` but for
    /// texture-map definitions consumed by `usemap`. Captured verbatim
    /// in document order, with later duplicates suppressed (matching
    /// the `mtllib` de-duplication policy). Surfaced through
    /// `Scene3D::extras["obj:maplibs"]` and replayed by the encoder.
    /// No external IO is performed — the spec mandates that `maplib`
    /// references resolve at render time, not at decode time, so we
    /// treat the listed filenames as opaque round-trip metadata.
    maplibs: Vec<String>,
    /// All material definitions resolved from `mtllib` references
    /// supplied via [`ObjDoc::with_resolved_mtllibs`]. Round 1 ships
    /// no IO so we accept these via an external resolver hook on the
    /// caller.
    resolved_materials: HashMap<String, oxideav_mesh3d::Material>,
    meshes: Vec<MeshAccum>,
    /// Verbatim sequence of free-form-geometry directives (`cstype`,
    /// `deg`, `curv`, `surf`, `parm`, `trim`, `hole`, `scrv`, `sp`,
    /// `end`, `bzp`, the older `bsp`, plus the curve / surface
    /// approximation-technique directives `ctech` / `stech`). Each entry
    /// is the keyword followed by its whitespace-separated arguments.
    /// Round-trip preservation: the encoder replays the sequence verbatim
    /// after the polygonal section so consumers can carry free-form data
    /// through us without semantic loss. Body statements (`parm`,
    /// `trim`, `hole`, `scrv`, `sp`, `end`) are accepted in document
    /// order; the spec mandates they appear between an element start
    /// (`curv` / `surf`) and `end`, but we don't enforce that — a
    /// lenient loader pattern matches what tools in the wild emit.
    freeform_directives: Vec<Vec<String>>,
    /// Shadow-casting object filename from a `shadow_obj filename`
    /// directive (spec §"shadow_obj filename"). Top-level state: the
    /// spec states "Only one shadow object can be stored in a file. If
    /// more than one shadow object is specified, the last one specified
    /// will be used." `None` if no directive appeared. Round-trip path:
    /// surfaced through `Scene3D::extras["obj:shadow_obj"]` and re-emitted
    /// before the polygonal section.
    shadow_obj: Option<String>,
    /// Ray-tracing reflection object filename from a `trace_obj filename`
    /// directive (spec §"trace_obj filename"). Mirrors `shadow_obj` —
    /// last-wins semantics per spec, surfaced through
    /// `Scene3D::extras["obj:trace_obj"]`.
    trace_obj: Option<String>,
    /// Verbatim sequence of "general statement" directives — spec
    /// §"General statement" lists `call filename.ext arg1 arg2 …`
    /// (inline file inclusion of a sibling `.obj` / `.mod` file) and
    /// `csh command` / `csh -command` (shell-execute, with the leading
    /// `-` flagging "ignore error on non-zero exit"). Both are
    /// captured verbatim for round-trip but NOT semantically
    /// interpreted — `call` does not pull the referenced file into the
    /// scene (would require IO and conflict with the clean-room
    /// boundary; consumers can re-resolve manually), and `csh` does
    /// not execute the requested command (would be a sandbox-escape
    /// trapdoor in any consumer that round-trips untrusted OBJ inputs).
    /// Surfaces through `Scene3D::extras["obj:general_directives"]` as
    /// an array of `[keyword, arg1, arg2, …]` arrays in document order.
    /// The encoder replays them in the preamble (right after `mtllib`
    /// and the `shadow_obj` / `trace_obj` companion-file block) since
    /// the spec is silent on placement ("The call statement can be
    /// inserted into .obj files using a text editor"); source-line
    /// position relative to the polygonal section is NOT preserved by
    /// design.
    general_directives: Vec<Vec<String>>,
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
///
/// The position component (the part before the first `/`) is mandatory
/// per spec ("v is the index of the geometric vertex, … required for
/// every reference"); an empty or missing `v` slot surfaces as
/// `Err(Error::invalid)` rather than coalescing to `0` and tripping the
/// downstream `(fv.v - 1) as usize` underflow.
fn parse_face_vertex(tok: &str, n_pos: i64, n_tex: i64, n_norm: i64) -> Result<FaceVert> {
    let mut parts = tok.split('/');
    let v = parts
        .next()
        .ok_or_else(|| Error::invalid(format!("face vertex missing position: {tok:?}")))?;
    if v.is_empty() {
        return Err(Error::invalid(format!(
            "face vertex missing position index: {tok:?}"
        )));
    }
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
            | "sp" | "end" | "bzp" | "bsp" | "bmat" | "step" | "ctech" | "stech" | "con" => {
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
                // §"bmat u/v matrix" + §"step stepu stepv" /
                // §"ctech technique resolution" (cparm / cspace / curv
                // forms) + §"stech technique resolution" (cparma /
                // cparmb / cspace / curv forms) — both classified as
                // free-form geometry statements by the spec and
                // captured verbatim in source order alongside the
                // structural directives so the round-trip preserves
                // the per-block approximation hints.
                //
                // `con surf_1 q0_1 q1_1 curv2d_1 surf_2 q0_2 q1_2
                // curv2d_2` (spec §"Connectivity between free-form
                // surfaces", §"con surf_1 q0_1 q1_1 curv2d_1 surf_2
                // q0_2 q1_2 curv2d_2") is a top-level free-form
                // geometry statement that ties two previously-declared
                // `surf` blocks together along a shared trimming-curve
                // segment for edge merging. It sits OUTSIDE any
                // `cstype … end` block (the worked example in spec
                // §"Connectivity between free-form surfaces"
                // §"Example 1" places it after the last surface's
                // `end`), so capturing it into the same verbatim
                // sequence keeps source order intact across the
                // polygonal section / free-form section boundary.
                // No semantic merging is performed — consumers that
                // care about connectivity walk the captured directive
                // sequence themselves; the round-trip is byte-faithful
                // for the args.
                let mut entry: Vec<String> = Vec::new();
                entry.push(keyword.to_string());
                for tok in tokens {
                    entry.push(tok.to_string());
                }
                doc.freeform_directives.push(entry);
            }
            "shadow_obj" => {
                // Spec §"shadow_obj filename": top-level last-wins
                // shadow-caster filename. The spec note ("If more than
                // one shadow object is specified, the last one
                // specified will be used.") makes the multi-line
                // collapse behaviour mandatory; we honour it directly
                // rather than carrying the discarded earlier entries.
                let v: String = tokens.collect::<Vec<_>>().join(" ");
                if !v.is_empty() {
                    doc.shadow_obj = Some(v);
                }
            }
            "trace_obj" => {
                // Spec §"trace_obj filename": top-level last-wins ray-
                // tracing reflection-target filename. Same last-wins
                // semantics as `shadow_obj`; reuses the same join +
                // last-write-wins pattern so quoted spaces (if any) in
                // the filename survive the tokenisation.
                let v: String = tokens.collect::<Vec<_>>().join(" ");
                if !v.is_empty() {
                    doc.trace_obj = Some(v);
                }
            }
            "call" | "csh" => {
                // Spec §"General statement" — `call filename.ext arg1
                // arg2 …` (inline include of a sibling `.obj` / `.mod`
                // file with positional argument substitution) and
                // `csh command` / `csh -command` (shell-execute a UNIX
                // command, with a leading `-` flagging "ignore error
                // on non-zero exit"). Both are spec-defined but
                // semantically expensive / unsafe to interpret here:
                //
                //   * `call` would require IO + recursive parser
                //     re-entry + nested-call depth tracking; consumers
                //     can re-resolve the included files themselves
                //     against the captured filename.
                //   * `csh` is a sandbox-escape trapdoor for any
                //     consumer that round-trips untrusted OBJ input,
                //     so the spec-mandated "executes the requested
                //     UNIX command" behaviour is deliberately NOT
                //     implemented — consumers can inspect the captured
                //     command text and decide for themselves.
                //
                // Capture verbatim into `general_directives`. Empty
                // arg lists land as `[keyword]` only (a bare `csh`
                // line with no command, while ill-formed, still
                // survives the round-trip rather than getting dropped
                // — mirrors the lenient-loader pattern used elsewhere).
                let mut entry: Vec<String> = Vec::new();
                entry.push(keyword.to_string());
                for tok in tokens {
                    entry.push(tok.to_string());
                }
                doc.general_directives.push(entry);
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
                    let usemap = prim.usemap.clone();
                    mesh.primitives.push(PrimAccum {
                        material: mat,
                        groups,
                        smoothing_group: smoothing,
                        merging_group: merging,
                        bevel,
                        c_interp,
                        d_interp,
                        lod,
                        usemap,
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
                    let usemap = last.usemap.clone();
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
                        usemap,
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
                    let usemap = last.usemap.clone();
                    mesh.primitives.push(PrimAccum {
                        material: mat,
                        smoothing_group: smoothing,
                        groups,
                        merging_group: Some(v),
                        bevel,
                        c_interp,
                        d_interp,
                        lod,
                        usemap,
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
                    let usemap = last.usemap.clone();
                    mesh.primitives.push(PrimAccum {
                        material: mat,
                        smoothing_group: Some(v),
                        groups,
                        merging_group: merging,
                        bevel,
                        c_interp,
                        d_interp,
                        lod,
                        usemap,
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
                    // Subsequent usemtl — start a new primitive that
                    // inherits the sibling state-setters (groups,
                    // smoothing/merging/display attrs, plus the active
                    // `usemap` binding which is independent of
                    // `usemtl` per spec §"usemap map_name/off").
                    let groups = last.groups.clone();
                    let smoothing = last.smoothing_group.clone();
                    let merging = last.merging_group.clone();
                    let bevel = last.bevel.clone();
                    let c_interp = last.c_interp.clone();
                    let d_interp = last.d_interp.clone();
                    let lod = last.lod.clone();
                    let usemap = last.usemap.clone();
                    mesh.primitives.push(PrimAccum {
                        material: if name.is_empty() { None } else { Some(name) },
                        groups,
                        smoothing_group: smoothing,
                        merging_group: merging,
                        bevel,
                        c_interp,
                        d_interp,
                        lod,
                        usemap,
                        elements: Vec::new(),
                    });
                }
            }
            "usemap" => {
                // Rendering identifier — `usemap <name>` or `usemap off`.
                // Spec §"usemap map_name/off": state-setting; applies to
                // the elements that follow until the next `usemap`. The
                // bind operates independently of `usemtl` (one chooses a
                // material, the other a texture-map definition), so a
                // change splits the primitive into a fresh one that
                // inherits everything but the map binding. An empty
                // line is treated as no-op rather than an explicit
                // turn-off (the spec spells out `off` as the keyword,
                // never an empty token list).
                let v: String = tokens.collect::<Vec<_>>().join(" ");
                if v.is_empty() {
                    continue;
                }
                let mesh = doc.meshes.last_mut().unwrap();
                let last = mesh.current_or_new();
                if last.elements.is_empty() {
                    // No elements bound to this primitive yet — overwrite.
                    last.usemap = Some(v);
                } else if last.usemap.as_deref() != Some(v.as_str()) {
                    let mat = last.material.clone();
                    let groups = last.groups.clone();
                    let smoothing = last.smoothing_group.clone();
                    let merging = last.merging_group.clone();
                    let bevel = last.bevel.clone();
                    let c_interp = last.c_interp.clone();
                    let d_interp = last.d_interp.clone();
                    let lod = last.lod.clone();
                    mesh.primitives.push(PrimAccum {
                        material: mat,
                        smoothing_group: smoothing,
                        merging_group: merging,
                        groups,
                        bevel,
                        c_interp,
                        d_interp,
                        lod,
                        usemap: Some(v),
                        elements: Vec::new(),
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
            "maplib" => {
                // Rendering identifier — `maplib filename1 filename2 ...`.
                // Spec §"maplib filename1 filename2 ...": parallel to
                // `mtllib` but for the texture-map library that
                // `usemap` references. Each line can list several
                // files; later duplicates are suppressed (same policy
                // as `mtllib`).
                for tok in tokens {
                    if !doc.maplibs.iter().any(|m| m == tok) {
                        doc.maplibs.push(tok.to_string());
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

    // Spec §"Special point", §"sp vp1 vp …" typed view: precomputed
    // here so the doc move into the `doc.meshes` for-loop below doesn't
    // strand the borrow. Parse-time-only — the encoder still drives
    // `sp` line emission off `obj:freeform_directives`.
    let sp_typed = if !doc.freeform_directives.is_empty() {
        let (typed, _) = collect_special_points(&doc);
        typed
    } else {
        Vec::new()
    };

    // Spec §"Connectivity between free-form surfaces", §"con surf_1 q0_1
    // q1_1 curv2d_1 surf_2 q0_2 q1_2 curv2d_2" typed view: precomputed
    // for the same borrow-stranding reason as `sp_typed` above. Parse-
    // time-only — the encoder still drives `con` line emission off
    // `obj:freeform_directives`.
    let con_typed = if !doc.freeform_directives.is_empty() {
        collect_connectivity(&doc)
    } else {
        Vec::new()
    };

    // Spec §"parm u/v" typed view: one object per `parm u …` / `parm v …`
    // body statement, paired with the enclosing element kind
    // (`curv` / `curv2` / `surf`) and `cstype` slug. Parse-time-only —
    // the encoder still drives `parm` line emission off
    // `obj:freeform_directives`.
    let parms_typed = if !doc.freeform_directives.is_empty() {
        collect_parms(&doc)
    } else {
        Vec::new()
    };

    // Spec §"ctech technique resolution" / §"stech technique resolution"
    // typed view: one object per `ctech` / `stech` body statement, paired
    // with the enclosing element kind (`"curve"` / `"surface"`), the
    // technique slug (`cparm` / `cspace` / `curv` for curves;
    // `cparma` / `cparmb` / `cspace` / `curv` for surfaces), the parsed
    // f64 resolution parameter array, and the `cstype` slug. Parse-time-
    // only — the encoder still drives `ctech` / `stech` emission off
    // `obj:freeform_directives`.
    let approximations_typed = if !doc.freeform_directives.is_empty() {
        collect_approximation_techniques(&doc)
    } else {
        Vec::new()
    };

    // Spec §"Trimming loops and holes" / §"Special curve" typed view:
    // one object per `trim` / `hole` / `scrv` body statement, paired
    // with the enclosing element kind + `cstype` slug and decomposed
    // into its `(u0, u1, curv2d)` segment triples. Parse-time-only —
    // the encoder still drives `trim` / `hole` / `scrv` emission off
    // `obj:freeform_directives`.
    let trim_loops_typed = if !doc.freeform_directives.is_empty() {
        collect_trim_loops(&doc)
    } else {
        Vec::new()
    };

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

    // Spec §"maplib filename1 filename2 ..." — sibling to `mtllib` but
    // for texture-map definitions consumed by `usemap`. Surfaced on
    // the scene so a re-encode replays the original library list.
    if !doc.maplibs.is_empty() {
        scene.extras.insert(
            "obj:maplibs".to_string(),
            serde_json::to_value(&doc.maplibs).unwrap(),
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
    if !sp_typed.is_empty() {
        // Spec §"Special point", §"sp vp1 vp …" typed view from the
        // precomputed pass above. Skipped when no `sp` resolves cleanly
        // (empty pool, all references out of range, or none of the
        // directives carry an `sp`).
        scene.extras.insert(
            "obj:special_points".to_string(),
            serde_json::Value::Array(sp_typed),
        );
    }
    if !con_typed.is_empty() {
        // Spec §"Connectivity between free-form surfaces" / §"con
        // surf_1 q0_1 q1_1 curv2d_1 surf_2 q0_2 q1_2 curv2d_2" typed
        // view from the precomputed pass above. Skipped when no `con`
        // line parsed cleanly (missing keyword, wrong argument count,
        // or any one of the eight slots failed to parse as the
        // appropriate i64/f64). The encoder still drives `con`
        // emission off `obj:freeform_directives`.
        scene.extras.insert(
            "obj:connectivity".to_string(),
            serde_json::Value::Array(con_typed),
        );
    }
    if !parms_typed.is_empty() {
        // Spec §"parm u/v" typed view from the precomputed pass above.
        // Skipped when no `parm` line resolved cleanly (no enclosing
        // element kind, unrecognised direction token, or every line
        // failed to surface any parseable values). The encoder still
        // drives `parm` emission off `obj:freeform_directives`.
        scene.extras.insert(
            "obj:parms".to_string(),
            serde_json::Value::Array(parms_typed),
        );
    }
    if !approximations_typed.is_empty() {
        // Spec §"ctech technique resolution" / §"stech technique
        // resolution" typed view from the precomputed pass above.
        // Skipped when no `ctech` / `stech` line resolved cleanly
        // (unrecognised technique slug, wrong argument count, or any
        // resolution parameter failed to parse as `f64`). The encoder
        // still drives line emission off `obj:freeform_directives`.
        scene.extras.insert(
            "obj:approximations".to_string(),
            serde_json::Value::Array(approximations_typed),
        );
    }
    if !trim_loops_typed.is_empty() {
        // Spec §"Trimming loops and holes" / §"Special curve" typed
        // view from the precomputed pass above. Skipped when no
        // `trim` / `hole` / `scrv` line resolved cleanly (argument
        // count not a positive multiple of three, or any segment's
        // `u0` / `u1` / `curv2d` token failed to parse). The encoder
        // still drives line emission off `obj:freeform_directives`.
        scene.extras.insert(
            "obj:trim_loops".to_string(),
            serde_json::Value::Array(trim_loops_typed),
        );
    }

    // Spec §"shadow_obj filename" / §"trace_obj filename": top-level
    // last-wins state. Surfaced as plain strings so the encoder can
    // replay them in the preamble (before the polygonal section).
    if let Some(name) = &doc.shadow_obj {
        scene.extras.insert(
            "obj:shadow_obj".to_string(),
            serde_json::Value::String(name.clone()),
        );
    }
    if let Some(name) = &doc.trace_obj {
        scene.extras.insert(
            "obj:trace_obj".to_string(),
            serde_json::Value::String(name.clone()),
        );
    }

    // Spec §"General statement" — `call` and `csh` directives are
    // captured verbatim into a separate side-channel keyed by
    // `obj:general_directives`. The encoder replays them in the
    // preamble right after the companion-file block. Source position
    // relative to the polygonal section is NOT preserved by design
    // (see the docstring on `ObjDoc::general_directives`).
    if !doc.general_directives.is_empty() {
        scene.extras.insert(
            "obj:general_directives".to_string(),
            serde_json::to_value(&doc.general_directives).unwrap(),
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
                // `checked_add` / `checked_mul` here guard against
                // attacker-supplied huge `deg` values whose squared
                // basis-matrix size would overflow `usize`; fall through
                // to captured-only on overflow.
                let Some(n_plus_1) = (degree as usize).checked_add(1) else {
                    continue;
                };
                let Some(expected_bmat) = n_plus_1.checked_mul(n_plus_1) else {
                    continue;
                };
                if bmat_u.len() != expected_bmat {
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

/// Tessellate every `curv2` 2D trimming / special / connectivity curve
/// (spec §"curv2") that sits under a supported `cstype` header into a
/// parameter-space polyline ([`Topology::LineStrip`]).
///
/// Where [`tessellate_curves`] evaluates 3D space curves whose control
/// points are geometric `v` vertices, a `curv2` references **parameter
/// vertices** (`vp u v [w]`, spec §"vp u v w") and lies in the 2D
/// parameter space of the surface it trims. The curve maths is identical
/// — same Bezier / B-spline / Cardinal / Taylor / basis-matrix basis as
/// the active `cstype` — so we reuse the 1D samplers component-wise by
/// lifting each `vp (u, v)` into a `[u, v, 0.0]` control point. The
/// sampled `x`/`y` are the parameter-space `(u, v)` coordinates; `z`
/// stays `0.0` (the curve is flat in parameter space).
///
/// Differences from the 3D `curv` path (spec §"curv2"):
///   * A `curv2` line carries **no** leading `u0 u1` range — it is just
///     `curv2 vp1 vp2 …`. The evaluation range for the B-spline window
///     comes from the block's `parm u` knot vector
///     (`[parm_u[0], parm_u[last]]`); Bezier / Taylor / Cardinal sample
///     uniformly on `[0, 1]` exactly as the 3D path does.
///   * Control points are 2D (non-rational) or 2D/3D (rational, the
///     optional 3rd `vp` coordinate is the weight, default 1.0). Since
///     `vp` storage pads a missing 3rd coordinate with `0.0` and a
///     zero rational weight is degenerate, a stored weight of exactly
///     `0.0` is read back as the spec default `1.0` for rational
///     evaluation.
///
/// Output primitives carry the same `obj:tessellated_curve` sentinel as
/// the 3D path (so the encoder filters them out and replays the original
/// `cstype` / `curv2` / `end` block verbatim from
/// `Scene3D::extras["obj:freeform_directives"]`) plus a
/// `obj:curve2 = true` marker and the
/// `obj:curve_kind` / `obj:curve_degree` / `obj:curve_u_range` /
/// `obj:curve_samples` provenance.
fn tessellate_curve2(doc: &ObjDoc, samples: u32) -> Vec<Primitive> {
    let mut out: Vec<Primitive> = Vec::new();

    let mut active_kind: Option<&'static str> = None;
    let mut active_degree: Option<u32> = None;
    let mut parm_u: Vec<f32> = Vec::new();
    let mut bmat_u: Vec<f32> = Vec::new();
    let mut step_u: Option<u32> = None;
    // `curv2` directives queued for this block — evaluated on `end`
    // (mirrors the 3D `curv` two-pass deferral so the body `parm u`
    // knot vector is visible before B-spline evaluation).
    let mut pending: Vec<&Vec<String>> = Vec::new();

    let flush = |out: &mut Vec<Primitive>,
                 active_kind: Option<&'static str>,
                 active_degree: Option<u32>,
                 parm_u: &[f32],
                 bmat_u: &[f32],
                 step_u: Option<u32>,
                 pending: &[&Vec<String>]| {
        flush_curve2_block(
            out,
            doc,
            active_kind,
            active_degree,
            parm_u,
            bmat_u,
            step_u,
            pending,
            samples,
        );
    };

    for entry in &doc.freeform_directives {
        if entry.is_empty() {
            continue;
        }
        match entry[0].as_str() {
            "cstype" => {
                flush(
                    &mut out,
                    active_kind,
                    active_degree,
                    &parm_u,
                    &bmat_u,
                    step_u,
                    &pending,
                );
                pending.clear();
                parm_u.clear();
                bmat_u.clear();
                step_u = None;
                active_degree = None;

                let mut iter = entry.iter().skip(1);
                let first = iter.next().map(String::as_str);
                let second = iter.next().map(String::as_str);
                active_kind = match (first, second) {
                    (Some("bezier"), _) => Some("bezier"),
                    (Some("rat"), Some("bezier")) => Some("rat_bezier"),
                    (Some("bspline"), _) => Some("bspline"),
                    (Some("rat"), Some("bspline")) => Some("rat_bspline"),
                    (Some("cardinal"), _) => Some("cardinal"),
                    (Some("rat"), Some("cardinal")) => Some("cardinal"),
                    (Some("taylor"), _) => Some("taylor"),
                    (Some("rat"), Some("taylor")) => Some("taylor"),
                    (Some("bmatrix"), _) => Some("bmatrix"),
                    (Some("rat"), Some("bmatrix")) => Some("bmatrix"),
                    _ => None,
                };
            }
            "deg" => {
                if let Some(d) = entry.get(1).and_then(|t| t.parse::<u32>().ok()) {
                    active_degree = Some(d);
                }
            }
            "parm" if entry.get(1).map(String::as_str) == Some("u") => {
                parm_u = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f32>().ok())
                    .collect();
            }
            "bmat" if entry.get(1).map(String::as_str) == Some("u") => {
                bmat_u = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f32>().ok())
                    .collect();
            }
            "step" => {
                step_u = entry.get(1).and_then(|t| t.parse::<u32>().ok());
            }
            "curv2" => {
                pending.push(entry);
            }
            "end" => {
                flush(
                    &mut out,
                    active_kind,
                    active_degree,
                    &parm_u,
                    &bmat_u,
                    step_u,
                    &pending,
                );
                pending.clear();
                parm_u.clear();
                bmat_u.clear();
                step_u = None;
                active_kind = None;
                active_degree = None;
            }
            _ => {}
        }
    }
    // Tail flush for a malformed block missing its closing `end`.
    flush(
        &mut out,
        active_kind,
        active_degree,
        &parm_u,
        &bmat_u,
        step_u,
        &pending,
    );
    out
}

/// Evaluate one `curv2` entry under an active `cstype` and block-level
/// `parm u` / `bmat u` / `step` state. Returns `(u_min, u_max,
/// polyline_points)` on success, `None` when the block state is
/// incomplete (no `cstype`, B-spline knot-count mismatch, bad bmatrix
/// sizing, fewer than two control points, etc.). Shared by
/// [`flush_curve2_block`] and [`collect_all_curv2_polylines`] so the
/// surface trim/hole clipping pass (spec §"trim u0 u1 curv2d …",
/// §"hole u0 u1 curv2d …") sees the same polyline a stand-alone
/// `obj:curves2` synthetic primitive does.
#[allow(clippy::too_many_arguments)]
fn evaluate_curv2_entry(
    kind: &'static str,
    active_degree: Option<u32>,
    parm_u: &[f32],
    bmat_u: &[f32],
    step_u: Option<u32>,
    control_points: &[[f32; 3]],
    control_weights: &[f32],
    samples: u32,
) -> Option<(f32, f32, Vec<[f32; 3]>)> {
    // `curv2` carries no inline `u0 u1`; the evaluation range comes from
    // the block's `parm u` knot vector when present (needed for the
    // B-spline window clip), otherwise the canonical `[0, 1]`. Spec
    // §"curv2" + §"parm u/v".
    let (u_min, u_max) = match (parm_u.first(), parm_u.last()) {
        (Some(&a), Some(&b)) if parm_u.len() >= 2 => (a, b),
        _ => (0.0, 1.0),
    };

    let curve_points = match kind {
        "bezier" | "rat_bezier" => {
            sample_bezier(control_points, control_weights, kind, u_min, u_max, samples)
        }
        "bspline" | "rat_bspline" => {
            let degree = active_degree?;
            if parm_u.len() != control_points.len() + degree as usize + 1 {
                return None;
            }
            sample_bspline(
                control_points,
                control_weights,
                kind,
                degree,
                parm_u,
                u_min,
                u_max,
                samples,
            )
        }
        "cardinal" => {
            if active_degree.is_some_and(|d| d != 3) {
                return None;
            }
            if control_points.len() < 4 {
                return None;
            }
            sample_cardinal(control_points, samples)
        }
        "taylor" => {
            let degree = match active_degree {
                Some(d) => d as usize,
                None => control_points.len().saturating_sub(1),
            };
            if control_points.len() != degree + 1 {
                return None;
            }
            sample_taylor(control_points, u_min, u_max, samples)
        }
        "bmatrix" => {
            let degree = active_degree?;
            let step = step_u?;
            let n_plus_1 = (degree as usize).checked_add(1)?;
            let expected_bmat = n_plus_1.checked_mul(n_plus_1)?;
            if bmat_u.len() != expected_bmat {
                return None;
            }
            if step == 0 {
                return None;
            }
            if control_points.len() < n_plus_1 {
                return None;
            }
            sample_bmatrix(control_points, bmat_u, degree, step, samples)
        }
        _ => return None,
    };
    Some((u_min, u_max, curve_points))
}

/// `(u_min, u_max, polyline)` triple captured for one `curv2` entry —
/// see [`collect_all_curv2_polylines`]. Aliased to keep clippy's
/// `type_complexity` lint happy on the `Vec<Option<…>>` table that
/// indexes one such triple per global curv2 occurrence.
type Curv2Polyline = (f32, f32, Vec<[f32; 2]>);
/// `Vec<Option<Curv2Polyline>>` keyed by zero-based global curv2 source
/// order (so a `curv2d` 1-based reference reads `table[idx - 1]`).
type Curv2PolylineTable = Vec<Option<Curv2Polyline>>;

/// Walk `doc.freeform_directives` once and return, for every `curv2`
/// encountered (1-based in source order), the tessellated parameter-space
/// polyline plus its evaluation `(u_min, u_max)` range. Returns `None` at
/// the slot when the enclosing `cstype … end` block is too incomplete to
/// evaluate; the slot indices themselves stay aligned with `curv2 N`
/// references on `trim` / `hole` / `scrv` statements (spec §"trim u0 u1
/// curv2d …", §"hole u0 u1 curv2d …").
///
/// `samples` is the same per-direction sample knob the user supplied via
/// [`ObjDecoder::with_curve_tessellation`]; the resolved polylines feed
/// the surface trim/hole clip pass (the polyline is rasterised at
/// `samples + 1` (u, v) coordinates per curve segment, then point-in-
/// polygon tested against each surface-lattice sample).
fn collect_all_curv2_polylines(doc: &ObjDoc, samples: u32) -> Curv2PolylineTable {
    let mut out: Curv2PolylineTable = Vec::new();
    if samples == 0 {
        return out;
    }

    let n_vp = doc.vp.len() as i64;
    let mut active_kind: Option<&'static str> = None;
    let mut active_degree: Option<u32> = None;
    let mut parm_u: Vec<f32> = Vec::new();
    let mut bmat_u: Vec<f32> = Vec::new();
    let mut step_u: Option<u32> = None;
    // First pass collects per-block state and indexes of every `curv2`
    // entry within that block; we evaluate on `end` (or `cstype` / tail)
    // so the body `parm u` knot vector is fully visible. This mirrors
    // the two-pass deferral used by `tessellate_curve2`.
    let mut pending: Vec<(usize, &Vec<String>)> = Vec::new();

    let flush = |out: &mut Curv2PolylineTable,
                 active_kind: Option<&'static str>,
                 active_degree: Option<u32>,
                 parm_u: &[f32],
                 bmat_u: &[f32],
                 step_u: Option<u32>,
                 pending: &[(usize, &Vec<String>)]| {
        for (idx, entry) in pending {
            // Make sure the output Vec is long enough to address `idx`
            // (1-based — slot 0 is unused so `curv2 1` lands at index 0
            // with a `-1` shift when looked up).
            while out.len() <= *idx {
                out.push(None);
            }
            let Some(kind) = active_kind else {
                continue;
            };
            if entry.len() < 3 {
                continue;
            }
            let mut control_points: Vec<[f32; 3]> = Vec::new();
            let mut control_weights: Vec<f32> = Vec::new();
            let mut bad = false;
            for tok in &entry[1..] {
                let Ok(raw) = tok.parse::<i64>() else {
                    bad = true;
                    break;
                };
                let resolved = if raw < 0 { n_vp + 1 + raw } else { raw };
                if resolved <= 0 || resolved > n_vp {
                    bad = true;
                    break;
                }
                let vp = doc.vp[(resolved as usize) - 1];
                control_points.push([vp[0], vp[1], 0.0]);
                let w = if vp[2] == 0.0 { 1.0 } else { vp[2] };
                control_weights.push(w);
            }
            if bad || control_points.len() < 2 {
                continue;
            }
            let Some((u_min, u_max, pts)) = evaluate_curv2_entry(
                kind,
                active_degree,
                parm_u,
                bmat_u,
                step_u,
                &control_points,
                &control_weights,
                samples,
            ) else {
                continue;
            };
            // Lift the curve's (x, y, z) samples down to 2D — the z
            // component is always 0 for a `curv2` (we lifted the 2D `vp`
            // into a flat 3D control point inside the evaluator), so
            // dropping it is lossless.
            let polyline: Vec<[f32; 2]> = pts.iter().map(|p| [p[0], p[1]]).collect();
            out[*idx] = Some((u_min, u_max, polyline));
        }
    };

    // 0-based global `curv2` counter; the spec's `trim u0 u1 curv2d`
    // numbering is 1-based so we look up at `curv2d - 1`.
    let mut curv2_counter: usize = 0;

    for entry in &doc.freeform_directives {
        if entry.is_empty() {
            continue;
        }
        match entry[0].as_str() {
            "cstype" => {
                flush(
                    &mut out,
                    active_kind,
                    active_degree,
                    &parm_u,
                    &bmat_u,
                    step_u,
                    &pending,
                );
                pending.clear();
                parm_u.clear();
                bmat_u.clear();
                step_u = None;
                active_degree = None;

                let mut iter = entry.iter().skip(1);
                let first = iter.next().map(String::as_str);
                let second = iter.next().map(String::as_str);
                active_kind = match (first, second) {
                    (Some("bezier"), _) => Some("bezier"),
                    (Some("rat"), Some("bezier")) => Some("rat_bezier"),
                    (Some("bspline"), _) => Some("bspline"),
                    (Some("rat"), Some("bspline")) => Some("rat_bspline"),
                    (Some("cardinal"), _) => Some("cardinal"),
                    (Some("rat"), Some("cardinal")) => Some("cardinal"),
                    (Some("taylor"), _) => Some("taylor"),
                    (Some("rat"), Some("taylor")) => Some("taylor"),
                    (Some("bmatrix"), _) => Some("bmatrix"),
                    (Some("rat"), Some("bmatrix")) => Some("bmatrix"),
                    _ => None,
                };
            }
            "deg" => {
                if let Some(d) = entry.get(1).and_then(|t| t.parse::<u32>().ok()) {
                    active_degree = Some(d);
                }
            }
            "parm" if entry.get(1).map(String::as_str) == Some("u") => {
                parm_u = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f32>().ok())
                    .collect();
            }
            "bmat" if entry.get(1).map(String::as_str) == Some("u") => {
                bmat_u = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f32>().ok())
                    .collect();
            }
            "step" => {
                step_u = entry.get(1).and_then(|t| t.parse::<u32>().ok());
            }
            "curv2" => {
                pending.push((curv2_counter, entry));
                curv2_counter += 1;
            }
            "end" => {
                flush(
                    &mut out,
                    active_kind,
                    active_degree,
                    &parm_u,
                    &bmat_u,
                    step_u,
                    &pending,
                );
                pending.clear();
                parm_u.clear();
                bmat_u.clear();
                step_u = None;
                active_kind = None;
                active_degree = None;
            }
            _ => {}
        }
    }
    // Tail flush for blocks missing their closing `end` directive.
    flush(
        &mut out,
        active_kind,
        active_degree,
        &parm_u,
        &bmat_u,
        step_u,
        &pending,
    );
    // Pad the output so every globally-numbered curv2 has a slot, even
    // when the trailing ones evaluated to `None`.
    while out.len() < curv2_counter {
        out.push(None);
    }
    out
}

/// Slice a tessellated `curv2` polyline to the parameter sub-range
/// `[u0, u1]` (spec §"trim u0 u1 curv2d" / §"hole u0 u1 curv2d") and
/// append the resulting (u, v) sample sequence onto `loop_out`. The
/// polyline was sampled uniformly over the curve's own `(u_min, u_max)`
/// range, so we map `[u0, u1]` linearly into that range and pick out
/// the matching slice. Reverses the slice when `u0 > u1` so the loop
/// orientation can be expressed by either ordering.
///
/// To avoid duplicate vertices at segment boundaries on a multi-curve
/// loop, the first vertex of the second-and-later segment is skipped
/// when `loop_out` is non-empty.
fn append_curv2_segment(
    loop_out: &mut Vec<[f32; 2]>,
    polyline: &[[f32; 2]],
    curve_u_min: f32,
    curve_u_max: f32,
    u0: f32,
    u1: f32,
) {
    if polyline.len() < 2 {
        return;
    }
    let n = polyline.len();
    let span = curve_u_max - curve_u_min;
    let to_idx = |u: f32| -> usize {
        if span.abs() < f32::EPSILON {
            0
        } else {
            let t = ((u - curve_u_min) / span).clamp(0.0, 1.0);
            (t * (n - 1) as f32).round() as usize
        }
    };
    let i0 = to_idx(u0);
    let i1 = to_idx(u1);
    let forward = i0 <= i1;
    let (lo, hi) = if forward { (i0, i1) } else { (i1, i0) };
    if hi <= lo {
        return;
    }
    let segment: Vec<[f32; 2]> = if forward {
        polyline[lo..=hi].to_vec()
    } else {
        polyline[lo..=hi].iter().rev().copied().collect()
    };
    let start = if loop_out.is_empty() { 0 } else { 1 };
    for p in &segment[start..] {
        loop_out.push(*p);
    }
}

/// Standard ray-casting point-in-polygon test (Jordan curve theorem).
/// The polygon is treated as closed — the implicit edge from the last
/// vertex back to the first is included so a `curv2` loop that ends on
/// its starting parameter vertex (the typical spec pattern, e.g.
/// `curv2 1 2 3 4 1`) is handled correctly. Vertices on the boundary
/// can resolve either way depending on the epsilon noise of the
/// rasterised polyline; this is fine for the surface-clip use case
/// since we keep triangles only when **all three** vertices pass, so a
/// single ambiguous boundary point can at most lose one cell.
fn point_in_polygon(point: [f32; 2], polygon: &[[f32; 2]]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let [px, py] = point;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let [xi, yi] = polygon[i];
        let [xj, yj] = polygon[j];
        let intersect = (yi > py) != (yj > py) && {
            let denom = yj - yi;
            if denom.abs() < f32::EPSILON {
                false
            } else {
                px < (xj - xi) * (py - yi) / denom + xi
            }
        };
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Evaluate every `curv2` entry queued for the current `cstype … end`
/// block (helper for [`tessellate_curve2`]). A block whose state is
/// incomplete (missing `cstype`, missing knot vector for B-spline,
/// malformed `vp` indices, …) is silently dropped, matching the
/// lenient-loader pattern used throughout the crate.
#[allow(clippy::too_many_arguments)]
fn flush_curve2_block(
    out: &mut Vec<Primitive>,
    doc: &ObjDoc,
    active_kind: Option<&'static str>,
    active_degree: Option<u32>,
    parm_u: &[f32],
    bmat_u: &[f32],
    step_u: Option<u32>,
    pending: &[&Vec<String>],
    samples: u32,
) {
    let Some(kind) = active_kind else {
        return;
    };
    let n_vp = doc.vp.len() as i64;
    for entry in pending {
        // `curv2 vp1 vp2 …` — keyword + at least two control points.
        if entry.len() < 3 {
            continue;
        }
        let mut control_points: Vec<[f32; 3]> = Vec::new();
        let mut control_weights: Vec<f32> = Vec::new();
        let mut bad = false;
        for tok in &entry[1..] {
            let Ok(raw) = tok.parse::<i64>() else {
                bad = true;
                break;
            };
            // Spec §"curv2": control points are parameter vertices;
            // negative values are relative-from-end (spec §"vp").
            let resolved = if raw < 0 { n_vp + 1 + raw } else { raw };
            if resolved <= 0 || resolved > n_vp {
                bad = true;
                break;
            }
            let vp = doc.vp[(resolved as usize) - 1];
            // Lift the 2D parameter coordinate into a flat 3D control
            // point so the existing 1D samplers (which operate on
            // `[f32; 3]` component-wise) evaluate the curve unchanged.
            control_points.push([vp[0], vp[1], 0.0]);
            // The optional 3rd `vp` coordinate is the rational weight
            // (spec §"vp u v w"). `vp` storage pads a missing 3rd
            // coordinate with `0.0`; a 0 weight is degenerate, so read
            // it back as the spec default 1.0.
            let w = if vp[2] == 0.0 { 1.0 } else { vp[2] };
            control_weights.push(w);
        }
        if bad || control_points.len() < 2 {
            continue;
        }

        let Some((u_min, u_max, curve_points)) = evaluate_curv2_entry(
            kind,
            active_degree,
            parm_u,
            bmat_u,
            step_u,
            &control_points,
            &control_weights,
            samples,
        ) else {
            continue;
        };
        if curve_points.len() < 2 {
            continue;
        }

        let mut prim = Primitive::new(Topology::LineStrip);
        let n = curve_points.len() as u32;
        prim.positions = curve_points;
        if n > u16::MAX as u32 {
            prim.indices = Some(Indices::U32((0..n).collect()));
        } else {
            prim.indices = Some(Indices::U16((0..n).map(|i| i as u16).collect()));
        }

        prim.extras.insert(
            "obj:tessellated_curve".to_string(),
            serde_json::Value::Bool(true),
        );
        // 2D-parameter-space marker so consumers can tell a `curv2`
        // polyline apart from a 3D `curv` one (the positions are
        // `(u, v, 0)` parameter-space coordinates, not model space).
        prim.extras
            .insert("obj:curve2".to_string(), serde_json::Value::Bool(true));
        prim.extras.insert(
            "obj:curve_kind".to_string(),
            serde_json::Value::String(kind.to_string()),
        );
        let reported_degree = match kind {
            "bezier" | "rat_bezier" => (control_points.len() - 1) as u64,
            "bspline" | "rat_bspline" => active_degree.unwrap_or(0) as u64,
            "cardinal" => 3,
            "taylor" => active_degree
                .map(u64::from)
                .unwrap_or_else(|| (control_points.len() - 1) as u64),
            "bmatrix" => active_degree.map(u64::from).unwrap_or(0),
            _ => 0,
        };
        prim.extras.insert(
            "obj:curve_degree".to_string(),
            serde_json::Value::Number(serde_json::Number::from(reported_degree)),
        );
        prim.extras.insert(
            "obj:curve_u_range".to_string(),
            serde_json::Value::Array(vec![
                serde_json::Value::from(u_min as f64),
                serde_json::Value::from(u_max as f64),
            ]),
        );
        prim.extras.insert(
            "obj:curve_samples".to_string(),
            serde_json::Value::Number(serde_json::Number::from(samples as u64)),
        );

        out.push(prim);
    }
}

/// Tessellate every `scrv` (special-curve) directive that sits inside a
/// `cstype … end` block into a single parameter-space polyline
/// ([`Topology::LineStrip`]). Spec §"Special curve", §"scrv u0 u1 curv2d
/// u0 u1 curv2d …".
///
/// A `scrv` is structurally identical to `trim` / `hole`: each directive
/// is a sequence of `(u0, u1, curv2d)` triples that select sub-ranges of
/// previously-defined `curv2` parameter-space curves. Unlike trim/hole,
/// the resulting polyline is **not** a closed loop — the spec describes
/// it as "a sequence of curves which lie on a given surface to build a
/// single special curve" that will appear as triangle edges in the
/// surface's final triangulation. Until the surface triangulator
/// honours that constraint, this round emits the special curve as a
/// stand-alone parameter-space polyline on the synthetic `"obj:scrvs"`
/// mesh so consumers that care about the special-curve geometry can
/// resolve it without re-walking the directive stream.
///
/// Per-`scrv` provenance lands on `Primitive::extras`:
///   * `obj:tessellated_curve` — `true` (shared encoder-filter sentinel).
///   * `obj:scrv` — `true` (special-curve marker).
///   * `obj:scrv_segments` — number of `(u0, u1, curv2d)` segments
///     concatenated into the polyline.
///   * `obj:scrv_curv2_refs` — array of `[curv2d_index_1based, u0, u1]`
///     triples in source order (provenance for the segment list).
///
/// The free-form directive sequence still rides on
/// `Scene3D::extras["obj:freeform_directives"]` so a decode → encode
/// cycle replays the original `cstype` / `surf` / `scrv` / `end` block
/// verbatim — the encoder filters the synthetic polyline out via the
/// shared `obj:tessellated_curve` sentinel.
///
/// `curv2d` references are 1-based global per spec §"scrv u0 u1
/// curv2d" — "This curve must have been previously defined with the
/// curv2 statement". Segments whose referenced `curv2` failed to
/// tessellate (incomplete block state, missing knot vector, …) are
/// silently dropped; the surrounding `scrv` may still produce a
/// partial polyline if at least two vertices survive across the
/// successfully-resolved segments.
fn tessellate_scrv(doc: &ObjDoc, samples: u32) -> Vec<Primitive> {
    let mut out: Vec<Primitive> = Vec::new();
    if samples == 0 {
        return out;
    }

    // Reuse the same pre-resolution pass the surface trim/hole clipper
    // uses (spec §"trim u0 u1 curv2d" / §"hole u0 u1 curv2d" /
    // §"scrv u0 u1 curv2d" — all three reference `curv2` by 1-based
    // global index).
    let curv2_polylines = collect_all_curv2_polylines(doc, samples);

    for entry in &doc.freeform_directives {
        if entry.first().map(String::as_str) != Some("scrv") {
            continue;
        }
        // `scrv u0 u1 curv2d u0 u1 curv2d …` — keyword + N triples.
        let toks = &entry[1..];
        if toks.len() < 3 || toks.len() % 3 != 0 {
            continue;
        }

        let mut polyline: Vec<[f32; 2]> = Vec::new();
        let mut refs: Vec<serde_json::Value> = Vec::new();
        let mut segments: u32 = 0;
        let mut bad = false;
        for chunk in toks.chunks(3) {
            let Ok(u0) = chunk[0].parse::<f32>() else {
                bad = true;
                break;
            };
            let Ok(u1) = chunk[1].parse::<f32>() else {
                bad = true;
                break;
            };
            let Ok(idx) = chunk[2].parse::<i64>() else {
                bad = true;
                break;
            };
            // Spec §"scrv u0 u1 curv2d": the curv2 index is 1-based and
            // references an earlier definition. The spec doesn't describe
            // a negative-from-end form here (those would predate the
            // curve being defined), matching the trim/hole semantics.
            if idx <= 0 {
                continue;
            }
            let slot = idx as usize - 1;
            let Some(entry) = curv2_polylines.get(slot).and_then(|e| e.as_ref()) else {
                // The referenced curv2 didn't tessellate (block state
                // incomplete, malformed `vp` index, etc.). Skip this
                // segment but keep walking the rest of the scrv — the
                // spec doesn't require all-or-nothing.
                continue;
            };
            let (curve_u_min, curve_u_max, segment_polyline) = entry;
            append_curv2_segment(
                &mut polyline,
                segment_polyline,
                *curve_u_min,
                *curve_u_max,
                u0,
                u1,
            );
            refs.push(serde_json::Value::Array(vec![
                serde_json::Value::from(idx),
                serde_json::Value::from(u0 as f64),
                serde_json::Value::from(u1 as f64),
            ]));
            segments += 1;
        }
        if bad || polyline.len() < 2 {
            continue;
        }

        // Lift the 2D parameter-space samples into a flat 3D position
        // list (z = 0). Matches the `curv2` synthetic primitive layout
        // so consumers can treat scrv and curv2 polylines uniformly.
        let positions: Vec<[f32; 3]> = polyline.iter().map(|p| [p[0], p[1], 0.0]).collect();
        let n = positions.len() as u32;
        let mut prim = Primitive::new(Topology::LineStrip);
        prim.positions = positions;
        prim.indices = if n > u16::MAX as u32 {
            Some(Indices::U32((0..n).collect()))
        } else {
            Some(Indices::U16((0..n).map(|i| i as u16).collect()))
        };
        prim.extras.insert(
            "obj:tessellated_curve".to_string(),
            serde_json::Value::Bool(true),
        );
        prim.extras
            .insert("obj:scrv".to_string(), serde_json::Value::Bool(true));
        prim.extras.insert(
            "obj:scrv_segments".to_string(),
            serde_json::Value::Number(serde_json::Number::from(segments as u64)),
        );
        prim.extras.insert(
            "obj:scrv_curv2_refs".to_string(),
            serde_json::Value::Array(refs),
        );
        out.push(prim);
    }

    out
}

/// Walk every `cstype` … `end` block once and lift every `sp` (special
/// point) body statement that appears inside it into a typed
/// parameter-space record. Spec §"Special point", §"sp vp1 vp …".
///
/// A special point is an `sp` body statement (spec §"Free-form
/// curve/surface body statements") that lists one or more 1-based
/// references into the `vp` parameter-vertex pool. The kind of element
/// the body sits under decides how many of each `vp`'s components are
/// meaningful:
///   * Inside a `curv` (3D space curve) block — the parameter vertices
///     are 1D (`vp u`), so only the `u` component of each referenced
///     `vp` is meaningful (spec: "For space curves and trimming curves,
///     the parameter vertices must be 1D").
///   * Inside a `curv2` (2D parameter-space trimming curve) block — also
///     1D per spec, but the `vp` line that the curv2 references in turn
///     supplies the surface-space `(u, v)` coordinate of the trimming
///     curve's control point. The spec treats a special point on a
///     trimming curve as "essentially the same as a special point on
///     the surface it trims" — both `u` and `v` are meaningful (spec
///     §"Special point").
///   * Inside a `surf` block — the parameter vertices are 2D (spec:
///     "For surfaces, the parameter vertices must be 2D"), so both `u`
///     and `v` from each referenced `vp` are meaningful.
///
/// The element kind is determined by the first `curv` / `curv2` / `surf`
/// directive seen inside the open `cstype` block. (The spec doesn't
/// allow mixing element kinds inside one `cstype` block — `end` closes
/// the block before a fresh `cstype` reopens one.)
///
/// Typed view: each `sp` directive's resolved references land on
/// `Scene3D::extras["obj:special_points"]` as an array of objects with
/// the stable keys:
///   * `element_kind` — `"curv"` / `"curv2"` / `"surf"`.
///   * `vp_index_1based` — the original 1-based reference (after
///     negative-from-end normalisation).
///   * `u` — `f64`, always present.
///   * `v` — `f64` for `curv2` and `surf`; `null` for `curv` (the spec
///     says space-curve special points are 1D).
///
/// Synthetic primitive: per `sp` directive, a [`Topology::Points`]
/// primitive lands on a synthetic mesh named `"obj:sps"`. Point
/// positions lift each resolved special point into 3D as
/// `[u, v_or_0, 0.0]` so consumers can render them alongside the
/// surrounding tessellated curve / surface lattice without re-walking
/// the directive stream. Each primitive carries provenance extras:
///   * `obj:tessellated_curve` — `true` (shared encoder-filter sentinel,
///     so the encoder's existing `is_tessellated_curve` filter drops the
///     synthetic primitives from a re-emit).
///   * `obj:special_point` — `true` (sp marker).
///   * `obj:special_point_element_kind` — `"curv"` / `"curv2"` / `"surf"`.
///   * `obj:special_point_vp_refs` — array of the resolved 1-based vp
///     indices in source order.
///
/// `vp` references support the spec's negative-from-end shorthand the
/// rest of the free-form path uses (§"Special point" example 9: `sp 1`
/// after a `curv` that itself referenced `-4 -3 -2 -1`); references that
/// land outside the live `vp` pool are silently dropped from both the
/// typed view and the synthetic primitive (no fail-loud — the encoder
/// still replays the original `sp` line verbatim from
/// `Scene3D::extras["obj:freeform_directives"]`).
type SpecialPointPrimData = Vec<(SpecialPointKind, Vec<SpecialPointRef>)>;
type SpecialPointRef = (i64, f32, Option<f32>);

fn collect_special_points(doc: &ObjDoc) -> (Vec<serde_json::Value>, SpecialPointPrimData) {
    let mut typed: Vec<serde_json::Value> = Vec::new();
    let mut prim_data: SpecialPointPrimData = Vec::new();
    let n_vp = doc.vp.len() as i64;
    if n_vp == 0 {
        return (typed, prim_data);
    }

    // Per-block state: the active element kind inside the current
    // `cstype` … `end` block, set when we see the first `curv` /
    // `curv2` / `surf` directive after `cstype` and cleared on `end`.
    let mut active_kind: Option<SpecialPointKind> = None;

    for entry in &doc.freeform_directives {
        let Some(keyword) = entry.first().map(String::as_str) else {
            continue;
        };
        match keyword {
            "cstype" => {
                // Fresh block opens; clear any leftover kind from a
                // previous block that didn't terminate cleanly.
                active_kind = None;
            }
            "end" => {
                active_kind = None;
            }
            "curv" => {
                active_kind = Some(SpecialPointKind::Curv);
            }
            "curv2" => {
                active_kind = Some(SpecialPointKind::Curv2);
            }
            "surf" => {
                active_kind = Some(SpecialPointKind::Surf);
            }
            "sp" => {
                let Some(kind) = active_kind else {
                    // An `sp` line with no enclosing element. The spec
                    // doesn't define behaviour for that — skip but the
                    // free-form-directives replay still re-emits it.
                    continue;
                };
                let mut resolved: Vec<SpecialPointRef> = Vec::new();
                for tok in &entry[1..] {
                    let Ok(raw) = tok.parse::<i64>() else {
                        continue;
                    };
                    if raw == 0 {
                        continue;
                    }
                    let normalised = if raw > 0 { raw } else { n_vp + raw + 1 };
                    if normalised <= 0 || normalised > n_vp {
                        continue;
                    }
                    let slot = (normalised - 1) as usize;
                    let vp = doc.vp[slot];
                    let u = vp[0];
                    let v = match kind {
                        SpecialPointKind::Curv => None,
                        SpecialPointKind::Curv2 | SpecialPointKind::Surf => Some(vp[1]),
                    };
                    let kind_str = kind.as_str();
                    let mut obj = serde_json::Map::new();
                    obj.insert(
                        "element_kind".to_string(),
                        serde_json::Value::String(kind_str.to_string()),
                    );
                    obj.insert(
                        "vp_index_1based".to_string(),
                        serde_json::Value::Number(serde_json::Number::from(normalised)),
                    );
                    obj.insert("u".to_string(), serde_json::Value::from(u as f64));
                    obj.insert(
                        "v".to_string(),
                        match v {
                            Some(value) => serde_json::Value::from(value as f64),
                            None => serde_json::Value::Null,
                        },
                    );
                    typed.push(serde_json::Value::Object(obj));
                    resolved.push((normalised, u, v));
                }
                if !resolved.is_empty() {
                    prim_data.push((kind, resolved));
                }
            }
            _ => {}
        }
    }

    (typed, prim_data)
}

/// Synthetic-primitive companion to [`collect_special_points`]. Emits
/// one [`Topology::Points`] primitive per `sp` directive, with each
/// resolved special point lifted into 3D as `[u, v_or_0, 0.0]` (spec
/// §"Special point"). The primitives land on the `"obj:sps"` synthetic
/// mesh and are filtered out on re-encode via the shared
/// `obj:tessellated_curve` sentinel.
fn tessellate_special_points(doc: &ObjDoc) -> Vec<Primitive> {
    let mut out: Vec<Primitive> = Vec::new();
    let (_, prim_data) = collect_special_points(doc);
    for (kind, points) in prim_data {
        let positions: Vec<[f32; 3]> = points
            .iter()
            .map(|(_, u, v)| [*u, v.unwrap_or(0.0), 0.0])
            .collect();
        let n = positions.len() as u32;
        let mut prim = Primitive::new(Topology::Points);
        prim.positions = positions;
        prim.indices = if n > u16::MAX as u32 {
            Some(Indices::U32((0..n).collect()))
        } else {
            Some(Indices::U16((0..n).map(|i| i as u16).collect()))
        };
        prim.extras.insert(
            "obj:tessellated_curve".to_string(),
            serde_json::Value::Bool(true),
        );
        prim.extras.insert(
            "obj:special_point".to_string(),
            serde_json::Value::Bool(true),
        );
        prim.extras.insert(
            "obj:special_point_element_kind".to_string(),
            serde_json::Value::String(kind.as_str().to_string()),
        );
        let refs: Vec<serde_json::Value> = points
            .iter()
            .map(|(idx, _, _)| serde_json::Value::Number(serde_json::Number::from(*idx)))
            .collect();
        prim.extras.insert(
            "obj:special_point_vp_refs".to_string(),
            serde_json::Value::Array(refs),
        );
        out.push(prim);
    }
    out
}

/// Element-kind classifier for the `sp` body statement (spec §"Special
/// point"). Determines how many of each referenced `vp`'s components are
/// meaningful when surfaced on the typed view + synthetic primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpecialPointKind {
    /// Inside a `curv` (3D space curve) block — `vp` references are 1D.
    Curv,
    /// Inside a `curv2` (2D parameter-space trimming curve) block —
    /// `vp` references are 1D per the curve spec, but the spec
    /// describes a trimming-curve special point as "essentially the
    /// same as a special point on the surface it trims" so we surface
    /// both `u` and `v`.
    Curv2,
    /// Inside a `surf` block — `vp` references are 2D.
    Surf,
}

impl SpecialPointKind {
    fn as_str(self) -> &'static str {
        match self {
            SpecialPointKind::Curv => "curv",
            SpecialPointKind::Curv2 => "curv2",
            SpecialPointKind::Surf => "surf",
        }
    }
}

/// Walk `doc.freeform_directives` for every `con` connectivity statement
/// and return a typed decomposition suitable for surfacing on
/// `Scene3D::extras["obj:connectivity"]`.
///
/// Spec §"Connectivity between free-form surfaces" / §"con surf_1 q0_1
/// q1_1 curv2d_1 surf_2 q0_2 q1_2 curv2d_2": the keyword is followed by
/// exactly eight positional arguments — two `(surf, q0, q1, curv2d)`
/// quadruples — that tie two previously-declared `surf` blocks together
/// along a shared trimming-curve segment for edge merging.
///
/// The returned [`serde_json::Value`] is always an array of objects;
/// each object carries the eight stable, lowercase, underscore-separated
/// keys:
///
/// * `surf_1` — `i64`, the 1-based index of the first surface (negative
///   values, supported elsewhere in the free-form vocabulary, are kept
///   as-is; the typed view does NOT resolve them against the surface
///   stream because surfaces aren't numbered in the captured directive
///   sequence).
/// * `q0_1` — `f64`, starting parameter on `curv2d_1`.
/// * `q1_1` — `f64`, ending parameter on `curv2d_1`.
/// * `curv2d_1` — `i64`, the 1-based index of the trimming curve on the
///   first surface.
/// * `surf_2` — `i64`, the second surface's index.
/// * `q0_2` — `f64`, starting parameter on `curv2d_2`.
/// * `q1_2` — `f64`, ending parameter on `curv2d_2`.
/// * `curv2d_2` — `i64`, the second surface's trimming-curve index.
///
/// A `con` line that doesn't carry exactly eight arguments, or whose
/// integer slots fail to parse as `i64` / float slots fail to parse as
/// `f64`, is dropped from the typed view without failing the parse
/// (the original line still rides on `obj:freeform_directives` for
/// verbatim round-trip). This matches the existing `sp` typed-view
/// policy: parse-time-only, lossy on malformed input, the verbatim
/// channel stays the source of truth for the encoder.
fn collect_connectivity(doc: &ObjDoc) -> Vec<serde_json::Value> {
    let mut typed: Vec<serde_json::Value> = Vec::new();
    for entry in &doc.freeform_directives {
        let Some(keyword) = entry.first().map(String::as_str) else {
            continue;
        };
        if keyword != "con" {
            continue;
        }
        // Spec §"con surf_1 q0_1 q1_1 curv2d_1 surf_2 q0_2 q1_2
        // curv2d_2": keyword + exactly eight positional args.
        if entry.len() != 9 {
            continue;
        }
        let Ok(surf_1) = entry[1].parse::<i64>() else {
            continue;
        };
        let Ok(q0_1) = entry[2].parse::<f64>() else {
            continue;
        };
        let Ok(q1_1) = entry[3].parse::<f64>() else {
            continue;
        };
        let Ok(curv2d_1) = entry[4].parse::<i64>() else {
            continue;
        };
        let Ok(surf_2) = entry[5].parse::<i64>() else {
            continue;
        };
        let Ok(q0_2) = entry[6].parse::<f64>() else {
            continue;
        };
        let Ok(q1_2) = entry[7].parse::<f64>() else {
            continue;
        };
        let Ok(curv2d_2) = entry[8].parse::<i64>() else {
            continue;
        };
        let mut obj = serde_json::Map::new();
        obj.insert(
            "surf_1".to_string(),
            serde_json::Value::Number(serde_json::Number::from(surf_1)),
        );
        obj.insert("q0_1".to_string(), serde_json::Value::from(q0_1));
        obj.insert("q1_1".to_string(), serde_json::Value::from(q1_1));
        obj.insert(
            "curv2d_1".to_string(),
            serde_json::Value::Number(serde_json::Number::from(curv2d_1)),
        );
        obj.insert(
            "surf_2".to_string(),
            serde_json::Value::Number(serde_json::Number::from(surf_2)),
        );
        obj.insert("q0_2".to_string(), serde_json::Value::from(q0_2));
        obj.insert("q1_2".to_string(), serde_json::Value::from(q1_2));
        obj.insert(
            "curv2d_2".to_string(),
            serde_json::Value::Number(serde_json::Number::from(curv2d_2)),
        );
        typed.push(serde_json::Value::Object(obj));
    }
    typed
}

/// Walk `doc.freeform_directives` for every `trim` / `hole` / `scrv`
/// loop statement and return a typed decomposition suitable for
/// surfacing on `Scene3D::extras["obj:trim_loops"]`.
///
/// Spec §"Trimming loops and holes" / §"trim u0 u1 curv2d u0 u1 curv2d
/// …" / §"hole u0 u1 curv2d u0 u1 curv2d …" / §"Special curve" /
/// §"scrv u0 u1 curv2d u0 u1 curv2d …": all three share the identical
/// repeating-triple body shape — the keyword is followed by one or more
/// `(u0, u1, curv2d)` triples, each naming a previously-defined `curv2`
/// parameter-space curve plus the `[u0, u1]` sub-range of that curve to
/// walk. `trim` assembles an outer trimming loop, `hole` an inner
/// (hole) loop, and `scrv` a special curve guaranteed to appear as
/// triangle edges in the surface's final triangulation.
///
/// The returned objects each carry three stable, lowercase,
/// underscore-separated keys:
///
/// * `loop_kind` — exactly `"trim"`, `"hole"`, or `"scrv"` (the
///   keyword the line opened with).
/// * `element_kind` — the directive that opened the enclosing
///   `cstype … end` block (`"surf"` is the only spec-legal host, since
///   trimming loops trim a surface; a loop seen inside a `curv` /
///   `curv2` block, or outside any block, surfaces `"unknown"` so the
///   consumer can still read the segments).
/// * `cstype` — the recognised type slug from the enclosing `cstype`
///   header (one of `"bezier"` / `"rat_bezier"` / `"bspline"` /
///   `"rat_bspline"` / `"cardinal"` / `"taylor"` / `"bmatrix"`), or
///   `"unknown"` when the declared type isn't one of those names or no
///   `cstype` block is open. Same disambiguation table the `parm` /
///   `ctech` / `stech` typed views use.
/// * `segments` — an array of `{u0, u1, curv2d}` objects in source
///   order. `u0` / `u1` are `f64` (the start / end parameter on the
///   referenced curve); `curv2d` is `i64` (the 1-based index of the
///   `curv2` trimming curve — negative-from-end references, which the
///   spec §"Examples" case 8 special-curve example uses, are echoed
///   as-is so the consumer's own resolver can apply relative-from-end
///   semantics).
///
/// A line whose argument count isn't a positive multiple of three, or
/// any of whose `u0` / `u1` tokens fail to parse as `f64` or `curv2d`
/// token fails to parse as `i64`, is dropped from the typed view
/// without failing the parse — the original line still rides on
/// `obj:freeform_directives` for verbatim round-trip. Mirrors the
/// lossy-on-malformed policy of the existing `sp` / `con` / `parm`
/// typed views; the verbatim channel stays the source of truth for the
/// encoder.
fn collect_trim_loops(doc: &ObjDoc) -> Vec<serde_json::Value> {
    let mut typed: Vec<serde_json::Value> = Vec::new();
    // Per-block state mirrored from `collect_parms`: a `cstype` opens a
    // block and pins the type slug, a `curv` / `curv2` / `surf` pins the
    // element kind, and `end` clears both.
    let mut active_cstype: Option<&'static str> = None;
    let mut active_kind: Option<&'static str> = None;
    for entry in &doc.freeform_directives {
        let Some(keyword) = entry.first().map(String::as_str) else {
            continue;
        };
        match keyword {
            "cstype" => {
                let mut iter = entry.iter().skip(1);
                let first = iter.next().map(String::as_str);
                let second = iter.next().map(String::as_str);
                active_cstype = match (first, second) {
                    (Some("bezier"), _) => Some("bezier"),
                    (Some("rat"), Some("bezier")) => Some("rat_bezier"),
                    (Some("bspline"), _) => Some("bspline"),
                    (Some("rat"), Some("bspline")) => Some("rat_bspline"),
                    (Some("cardinal"), _) => Some("cardinal"),
                    (Some("rat"), Some("cardinal")) => Some("cardinal"),
                    (Some("taylor"), _) => Some("taylor"),
                    (Some("rat"), Some("taylor")) => Some("taylor"),
                    (Some("bmatrix"), _) => Some("bmatrix"),
                    (Some("rat"), Some("bmatrix")) => Some("bmatrix"),
                    _ => None,
                };
                active_kind = None;
            }
            "end" => {
                active_cstype = None;
                active_kind = None;
            }
            "curv" => active_kind = Some("curv"),
            "curv2" => active_kind = Some("curv2"),
            "surf" => active_kind = Some("surf"),
            "trim" | "hole" | "scrv" => {
                // Spec §"trim/hole/scrv u0 u1 curv2d …": keyword + one or
                // more `(u0, u1, curv2d)` triples.
                let toks = &entry[1..];
                if toks.is_empty() || toks.len() % 3 != 0 {
                    continue;
                }
                let mut segments: Vec<serde_json::Value> = Vec::new();
                let mut ok = true;
                for chunk in toks.chunks(3) {
                    let (Ok(u0), Ok(u1), Ok(curv2d)) = (
                        chunk[0].parse::<f64>(),
                        chunk[1].parse::<f64>(),
                        chunk[2].parse::<i64>(),
                    ) else {
                        ok = false;
                        break;
                    };
                    let mut seg = serde_json::Map::new();
                    seg.insert("u0".to_string(), serde_json::Value::from(u0));
                    seg.insert("u1".to_string(), serde_json::Value::from(u1));
                    seg.insert(
                        "curv2d".to_string(),
                        serde_json::Value::Number(serde_json::Number::from(curv2d)),
                    );
                    segments.push(serde_json::Value::Object(seg));
                }
                if !ok {
                    // A single malformed triple drops the whole line from
                    // the typed view; the verbatim channel still replays
                    // it byte-faithful.
                    continue;
                }
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "loop_kind".to_string(),
                    serde_json::Value::String(keyword.to_string()),
                );
                obj.insert(
                    "element_kind".to_string(),
                    serde_json::Value::String(active_kind.unwrap_or("unknown").to_string()),
                );
                obj.insert(
                    "cstype".to_string(),
                    serde_json::Value::String(active_cstype.unwrap_or("unknown").to_string()),
                );
                obj.insert("segments".to_string(), serde_json::Value::Array(segments));
                typed.push(serde_json::Value::Object(obj));
            }
            _ => {}
        }
    }
    typed
}

/// Walk `doc.freeform_directives` for every `parm u …` / `parm v …` body
/// statement and return a typed decomposition suitable for surfacing on
/// `Scene3D::extras["obj:parms"]`.
///
/// Spec §"parm u/v" / §"Free-form curve/surface body statements": a
/// `parm` line carries the **global parameters** (for Bezier / Cardinal /
/// Taylor / basis-matrix curves and surfaces) or the **knot vector** (for
/// B-spline / NURBS curves and surfaces) that fix the evaluation domain
/// of the surrounding free-form element. Each line writes one parametric
/// direction at a time: `parm u p1 p2 …` for `u`, `parm v p1 p2 …` for
/// `v`. A surface block can therefore carry two `parm` lines (one per
/// direction); curves and trimming curves only ever write `parm u`.
///
/// The returned [`serde_json::Value`] is always an array of objects, one
/// entry per source `parm` directive in source order; each object carries
/// the four stable, lowercase, underscore-separated keys:
///
/// * `direction` — `String`, exactly `"u"` or `"v"`.
/// * `element_kind` — `String`, the kind of the enclosing free-form
///   element (`"curv"` / `"curv2"` / `"surf"`) — decided by the most
///   recent `curv` / `curv2` / `surf` directive inside the current
///   `cstype … end` block. A `parm` line that sits outside any element
///   (no `curv` / `curv2` / `surf` seen since the last `cstype`) is
///   dropped from the typed view.
/// * `cstype` — `String`, the type slug declared by the enclosing
///   `cstype` directive — one of `"bezier"` / `"rat_bezier"` /
///   `"bspline"` / `"rat_bspline"` / `"cardinal"` / `"taylor"` /
///   `"bmatrix"`. A `parm` line that sits outside any `cstype` block
///   (i.e. the most-recent `cstype` token wasn't a recognised type)
///   surfaces `"unknown"` in this slot so consumers can still see the
///   raw values. Per spec the same domain semantics apply: B-spline
///   reads the values as the knot vector; Bezier / Cardinal / Taylor
///   / basis-matrix reads them as global parameters that define the
///   per-segment break points.
/// * `values` — array of `f64`, the parsed floating-point values from
///   the `parm` line (`p1 p2 p3 …`). Tokens that fail to parse as
///   `f64` are dropped — same lenient policy the rest of the typed
///   accessors use. The spec requires a minimum of two parameter
///   values per line; lines that fall short still surface their
///   surviving values without failing the parse.
///
/// The encoder is still driven by the verbatim
/// `obj:freeform_directives` channel; the typed view exists purely so
/// consumers don't have to walk the directive sequence themselves to
/// pair every `parm` with its enclosing element + `cstype` block.
///
/// Lines whose `parm` direction token isn't exactly `"u"` or `"v"` (the
/// only two values the spec defines) are dropped from the typed view
/// without failing the parse; the verbatim channel still replays them
/// byte-faithful.
fn collect_parms(doc: &ObjDoc) -> Vec<serde_json::Value> {
    let mut typed: Vec<serde_json::Value> = Vec::new();
    // Per-block state mirrored from the round-11/12/13/14/182 surface
    // tessellation pass: a `cstype` directive opens a new block, sets
    // the active type slug, and clears any previously-tracked element
    // kind; a `curv` / `curv2` / `surf` directive inside the block
    // pins the element kind for the following body statements; `end`
    // clears both.
    let mut active_cstype: Option<&'static str> = None;
    let mut active_kind: Option<&'static str> = None;
    for entry in &doc.freeform_directives {
        let Some(keyword) = entry.first().map(String::as_str) else {
            continue;
        };
        match keyword {
            "cstype" => {
                // Re-derive the type slug from `cstype [rat] type` per
                // spec §"Curve and surface type". Mirrors the matching
                // table used by `tessellate_surfaces`.
                let mut iter = entry.iter().skip(1);
                let first = iter.next().map(String::as_str);
                let second = iter.next().map(String::as_str);
                active_cstype = match (first, second) {
                    (Some("bezier"), _) => Some("bezier"),
                    (Some("rat"), Some("bezier")) => Some("rat_bezier"),
                    (Some("bspline"), _) => Some("bspline"),
                    (Some("rat"), Some("bspline")) => Some("rat_bspline"),
                    (Some("cardinal"), _) => Some("cardinal"),
                    (Some("rat"), Some("cardinal")) => Some("cardinal"),
                    (Some("taylor"), _) => Some("taylor"),
                    (Some("rat"), Some("taylor")) => Some("taylor"),
                    (Some("bmatrix"), _) => Some("bmatrix"),
                    (Some("rat"), Some("bmatrix")) => Some("bmatrix"),
                    _ => None,
                };
                active_kind = None;
            }
            "end" => {
                active_cstype = None;
                active_kind = None;
            }
            "curv" => active_kind = Some("curv"),
            "curv2" => active_kind = Some("curv2"),
            "surf" => active_kind = Some("surf"),
            "parm" => {
                // Spec §"parm u/v": exactly `parm u …` or `parm v …`.
                // Anything else (no direction token, or a token that
                // isn't u/v) drops from the typed view.
                let Some(direction) = entry.get(1).map(String::as_str) else {
                    continue;
                };
                if direction != "u" && direction != "v" {
                    continue;
                }
                let Some(kind) = active_kind else {
                    // `parm` outside any element. The spec doesn't
                    // define this; we drop it from the typed view (the
                    // verbatim channel still replays the line).
                    continue;
                };
                let values: Vec<f64> = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f64>().ok())
                    .collect();
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "direction".to_string(),
                    serde_json::Value::String(direction.to_string()),
                );
                obj.insert(
                    "element_kind".to_string(),
                    serde_json::Value::String(kind.to_string()),
                );
                obj.insert(
                    "cstype".to_string(),
                    serde_json::Value::String(active_cstype.unwrap_or("unknown").to_string()),
                );
                obj.insert(
                    "values".to_string(),
                    serde_json::Value::Array(
                        values.into_iter().map(serde_json::Value::from).collect(),
                    ),
                );
                typed.push(serde_json::Value::Object(obj));
            }
            _ => {}
        }
    }
    typed
}

/// Walk `doc.freeform_directives` for every `ctech` curve-approximation
/// and `stech` surface-approximation directive, returning a typed
/// decomposition suitable for surfacing on
/// `Scene3D::extras["obj:approximations"]`.
///
/// Spec §"ctech technique resolution" — three mutually-exclusive sub-
/// forms:
///   * `ctech cparm res` — constant parametric subdivision. One f64
///     resolution parameter scaling per-segment subdivision count by
///     curve degree.
///   * `ctech cspace maxlength` — constant spatial subdivision. One f64
///     real-space line-segment length cap.
///   * `ctech curv maxdist maxangle` — curvature-dependent subdivision.
///     Two f64 parameters: object-space chord-to-curve distance and
///     curve-normal-angle bound in degrees.
///
/// Spec §"stech technique resolution" — four mutually-exclusive sub-
/// forms:
///   * `stech cparma ures vres` — constant parametric subdivision with
///     separate u/v resolution parameters (two f64).
///   * `stech cparmb uvres` — constant parametric subdivision with one
///     unified resolution parameter (one f64).
///   * `stech cspace maxlength` — constant spatial subdivision (one
///     f64, same shape as the `ctech` sibling).
///   * `stech curv maxdist maxangle` — curvature-dependent subdivision
///     (two f64, same shape as the `ctech` sibling).
///
/// The returned [`serde_json::Value`] is always an array of objects, one
/// entry per source `ctech` / `stech` directive in source order; each
/// object carries the four stable, lowercase, underscore-separated keys:
///
/// * `element_kind` — `String`, exactly `"curve"` for a `ctech` line and
///   `"surface"` for an `stech` line. Pinned per spec text — `ctech`
///   "specifies a curve approximation technique", `stech` "specifies a
///   surface approximation technique".
/// * `technique` — `String`, the spec-defined sub-form slug, one of
///   `"cparm"` / `"cspace"` / `"curv"` (curve forms) or `"cparma"` /
///   `"cparmb"` / `"cspace"` / `"curv"` (surface forms). Unrecognised
///   technique tokens drop the whole line from the typed view (the
///   verbatim channel still replays it byte-faithful).
/// * `parameters` — array of `f64`, the parsed resolution arguments in
///   source order. Length follows the spec's per-form arity:
///   `cparm` / `cspace` / `cparmb` are 1; `curv` / `cparma` are 2.
///   Tokens that fail to parse as `f64` drop the whole line from the
///   typed view (we don't surface partial parameter arrays — a
///   resolution argument that doesn't decode would mislead consumers
///   into rendering against zero / NaN).
/// * `cstype` — `String`, the type slug declared by the enclosing
///   `cstype` directive — one of `"bezier"` / `"rat_bezier"` /
///   `"bspline"` / `"rat_bspline"` / `"cardinal"` / `"taylor"` /
///   `"bmatrix"`, or `"unknown"` when the declared type isn't one of
///   those names. Same disambiguation table the `parm` typed view uses.
///
/// Per spec the `ctech` / `stech` directives sit inside a `cstype` … `end`
/// block alongside `curv` / `surf` / `parm`. A line that appears outside
/// any block (no `cstype` seen since the last `end`) still surfaces with
/// `cstype = "unknown"` so consumers can see the resolution parameters;
/// dropping the line entirely would lose data the verbatim channel
/// still carries.
///
/// The encoder is still driven by the verbatim
/// `obj:freeform_directives` channel; the typed view exists purely so
/// consumers don't have to re-parse the per-technique positional tokens
/// to pair every `ctech` / `stech` with its enclosing `cstype` block.
///
/// Lines whose argument count doesn't match the spec's per-form arity
/// (e.g. `ctech curv 0.1` missing the `maxangle` argument) drop from the
/// typed view without failing the parse. Mirrors the lossy-on-malformed
/// policy of the existing `sp` / `con` / `parm` typed views.
fn collect_approximation_techniques(doc: &ObjDoc) -> Vec<serde_json::Value> {
    let mut typed: Vec<serde_json::Value> = Vec::new();
    // Track the enclosing `cstype` slug so each line carries its block
    // context. Mirrors the `collect_parms` state machine.
    let mut active_cstype: Option<&'static str> = None;
    for entry in &doc.freeform_directives {
        let Some(keyword) = entry.first().map(String::as_str) else {
            continue;
        };
        match keyword {
            "cstype" => {
                let mut iter = entry.iter().skip(1);
                let first = iter.next().map(String::as_str);
                let second = iter.next().map(String::as_str);
                active_cstype = match (first, second) {
                    (Some("bezier"), _) => Some("bezier"),
                    (Some("rat"), Some("bezier")) => Some("rat_bezier"),
                    (Some("bspline"), _) => Some("bspline"),
                    (Some("rat"), Some("bspline")) => Some("rat_bspline"),
                    (Some("cardinal"), _) => Some("cardinal"),
                    (Some("rat"), Some("cardinal")) => Some("cardinal"),
                    (Some("taylor"), _) => Some("taylor"),
                    (Some("rat"), Some("taylor")) => Some("taylor"),
                    (Some("bmatrix"), _) => Some("bmatrix"),
                    (Some("rat"), Some("bmatrix")) => Some("bmatrix"),
                    _ => None,
                };
            }
            "end" => {
                active_cstype = None;
            }
            "ctech" | "stech" => {
                // Spec §"ctech technique resolution" / §"stech technique
                // resolution": keyword + technique token + N resolution
                // parameters.
                let Some(technique) = entry.get(1).map(String::as_str) else {
                    continue;
                };
                let element_kind = if keyword == "ctech" {
                    "curve"
                } else {
                    "surface"
                };
                // Per-form expected argument arity. See doc-comment table
                // above; unrecognised techniques drop the line.
                let expected_args: usize = match (keyword, technique) {
                    ("ctech", "cparm") => 1,
                    ("ctech", "cspace") => 1,
                    ("ctech", "curv") => 2,
                    ("stech", "cparma") => 2,
                    ("stech", "cparmb") => 1,
                    ("stech", "cspace") => 1,
                    ("stech", "curv") => 2,
                    _ => continue,
                };
                // Keyword + technique slug + expected_args parameters.
                if entry.len() != 2 + expected_args {
                    continue;
                }
                // Parse every resolution parameter; bail (without partial
                // surfacing) if any token fails — see doc-comment.
                let mut params: Vec<f64> = Vec::with_capacity(expected_args);
                let mut ok = true;
                for raw in &entry[2..] {
                    match raw.parse::<f64>() {
                        Ok(v) => params.push(v),
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    continue;
                }
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "element_kind".to_string(),
                    serde_json::Value::String(element_kind.to_string()),
                );
                obj.insert(
                    "technique".to_string(),
                    serde_json::Value::String(technique.to_string()),
                );
                obj.insert(
                    "parameters".to_string(),
                    serde_json::Value::Array(
                        params.into_iter().map(serde_json::Value::from).collect(),
                    ),
                );
                obj.insert(
                    "cstype".to_string(),
                    serde_json::Value::String(active_cstype.unwrap_or("unknown").to_string()),
                );
                typed.push(serde_json::Value::Object(obj));
            }
            _ => {}
        }
    }
    typed
}

/// Tessellate every `surf` element that sits under a supported `cstype`
/// header into a triangulated [`Topology::Triangles`] primitive. Mirrors
/// [`tessellate_curves`] but evaluates a bivariate tensor product (spec
/// §"Rational and non-rational curves and surfaces", §"Bezier",
/// §"B-spline", §"Surface vertex data — control points").
///
/// Supported `cstype` values:
///   * `bezier` / `rat bezier` (round 11) — bivariate tensor-product de
///     Casteljau; single patch of `(degu + 1) × (degv + 1)` control
///     points.
///   * `bspline` / `rat bspline` (round 12) — bivariate tensor-product
///     Cox-deBoor evaluation; the `parm u` / `parm v` knot vectors define
///     the control-grid extents (`(len(parm u) − degu − 1) ×
///     (len(parm v) − degv − 1)` per spec §"B-spline" condition 6).
///   * `cardinal` / `rat cardinal` (round 13) — cubic-only bivariate
///     tensor-product Cardinal (Catmull-Rom) evaluation via the spec
///     §"Cardinal" Cardinal→Bezier conversion applied per parametric
///     direction over a sliding 4-point window; the control grid is the
///     `parm`-derived extent (`parm_count + 1` per direction) or a
///     square single patch when `parm` only carries the 2-value range.
///   * `taylor` (round 14) — bivariate tensor-product polynomial
///     evaluation `S(u, v) = Σ_i Σ_j c_{i,j} · u^i · v^j` per spec
///     §"Taylor" (the control points are the polynomial coefficients).
///     Single patch of `(degu + 1) × (degv + 1)` coefficient vectors.
///     `rat taylor` routes to the same evaluator without weight
///     blending — the spec note in §"Free-form curve/surface body
///     statements" explicitly says the rational form "does not make
///     sense for Taylor".
///   * `bmatrix` / `rat bmatrix` (round 182) — bivariate tensor-product
///     basis-matrix evaluation `S(u, v) = Σ_a Σ_b (Σ_p B_u[a][p] u^p)
///     (Σ_q B_v[b][q] v^q) · c_{base_u + a, base_v + b}` per spec
///     §"Basis matrix". The per-direction control-grid extent is
///     `(parm − 2) · s + n + 1` (inverse of spec §"Basis matrix"
///     `parm = (K − n) / s + 2`); patch decomposition uses the
///     per-direction `step stepu stepv` strides. Multi-patch grids
///     are now supported (e.g. the spec §"Examples" cubic Bezier
///     basis-matrix surface). The `rat bmatrix` form routes to the
///     same evaluator without per-vertex weight blending, matching
///     the round-10 1D curve path.
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
///   * `obj:surface_kind` — `"bezier"` / `"rat_bezier"` / `"bspline"` /
///     `"rat_bspline"` / `"cardinal"` / `"taylor"` / `"bmatrix"`.
///   * `obj:surface_degree` — `[degu, degv]`.
///   * `obj:surface_u_range` / `obj:surface_v_range` — `[s0, s1]` /
///     `[t0, t1]` from the `surf` directive.
///   * `obj:surface_samples` — sample count per parametric direction.
fn tessellate_surfaces(doc: &ObjDoc, samples: u32) -> Vec<Primitive> {
    let mut out: Vec<Primitive> = Vec::new();
    if samples == 0 {
        return out;
    }

    // Pre-resolve every `curv2` directive in the document into a 2D
    // parameter-space polyline keyed by 1-based source order — spec
    // §"trim u0 u1 curv2d" / §"hole u0 u1 curv2d" reference these by
    // global index so the per-surface trim/hole clip pass needs them all
    // available regardless of which `cstype … end` block originally
    // declared them.
    let curv2_polylines = collect_all_curv2_polylines(doc, samples);

    // Block state, accumulated between `cstype` … `end`. Like the curve
    // tessellator, a `surf` header is syntactically ahead of the `parm u`
    // / `parm v` body statements that supply the B-spline knot vectors,
    // so the whole block is buffered and evaluated on `end` (or `cstype`
    // / tail flush) once the body is fully visible.
    let mut active_kind: Option<&'static str> = None;
    let mut deg_u: Option<u32> = None;
    let mut deg_v: Option<u32> = None;
    // Spec §"parm u/v": for B-spline surfaces these are the u/v knot
    // vectors (unused by the Bezier basis but parsed regardless).
    let mut parm_u: Vec<f32> = Vec::new();
    let mut parm_v: Vec<f32> = Vec::new();
    // Spec §"bmat u/v matrix": for `cstype bmatrix` surfaces the per-
    // direction basis matrices supply the polynomial coefficients of
    // each `(n + 1)`-row in row-major form with column index `j`
    // varying fastest (round 10 reuses the same layout for curves).
    let mut bmat_u: Vec<f32> = Vec::new();
    let mut bmat_v: Vec<f32> = Vec::new();
    // Spec §"step stepu stepv": the per-direction segment stride
    // controls patch decomposition for both bmatrix curves and bmatrix
    // surfaces. `stepu` is mandatory for both; `stepv` is required
    // only for surfaces.
    let mut step_u: Option<u32> = None;
    let mut step_v: Option<u32> = None;
    let mut pending_surfs: Vec<&Vec<String>> = Vec::new();
    // Trim / hole loops accumulated for the current surface block —
    // each is a closed (u, v) polygon assembled from one or more
    // `(u0, u1, curv2d)` segments. The spec (§"trim", §"hole" /
    // "Trimming Loops") says: "To cut one or more holes in a region,
    // use a trim statement followed by one or more hole statements.
    // To introduce another trimmed region in the same surface, use
    // another trim statement followed by one or more hole statements."
    // We therefore keep trims and holes in their source order so the
    // clip pass can pair holes with their enclosing trim region.
    let mut pending_trims: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut pending_holes: Vec<Vec<[f32; 2]>> = Vec::new();

    let resolve_loop = |entry: &Vec<String>| -> Option<Vec<[f32; 2]>> {
        // `trim u0 u1 curv2d u0 u1 curv2d …` — one or more (u0, u1,
        // curv2d) triples after the keyword. Build the closed loop by
        // concatenating each segment in source order.
        let toks = &entry[1..];
        if toks.len() < 3 || toks.len() % 3 != 0 {
            return None;
        }
        let mut polygon: Vec<[f32; 2]> = Vec::new();
        for chunk in toks.chunks(3) {
            let u0 = chunk[0].parse::<f32>().ok()?;
            let u1 = chunk[1].parse::<f32>().ok()?;
            let idx = chunk[2].parse::<i64>().ok()?;
            // 1-based, positive only (the spec doesn't define a
            // negative `curv2d` here — those references would predate
            // the curve being defined and are rejected).
            if idx <= 0 {
                return None;
            }
            let slot = idx as usize - 1;
            let entry = curv2_polylines.get(slot).and_then(|e| e.as_ref())?;
            let (curve_u_min, curve_u_max, polyline) = entry;
            append_curv2_segment(&mut polygon, polyline, *curve_u_min, *curve_u_max, u0, u1);
        }
        if polygon.len() < 3 {
            return None;
        }
        Some(polygon)
    };

    #[allow(clippy::too_many_arguments)]
    let flush = |out: &mut Vec<Primitive>,
                 kind: Option<&'static str>,
                 deg_u: Option<u32>,
                 deg_v: Option<u32>,
                 parm_u: &[f32],
                 parm_v: &[f32],
                 bmat_u: &[f32],
                 bmat_v: &[f32],
                 step_u: Option<u32>,
                 step_v: Option<u32>,
                 surfs: &[&Vec<String>],
                 trims: &[Vec<[f32; 2]>],
                 holes: &[Vec<[f32; 2]>]| {
        let Some(kind) = kind else {
            return;
        };
        for entry in surfs {
            if let Some(prim) = flush_surface(
                doc, kind, deg_u, deg_v, parm_u, parm_v, bmat_u, bmat_v, step_u, step_v, entry,
                samples, trims, holes,
            ) {
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
                flush(
                    &mut out,
                    active_kind,
                    deg_u,
                    deg_v,
                    &parm_u,
                    &parm_v,
                    &bmat_u,
                    &bmat_v,
                    step_u,
                    step_v,
                    &pending_surfs,
                    &pending_trims,
                    &pending_holes,
                );
                pending_surfs.clear();
                pending_trims.clear();
                pending_holes.clear();
                deg_u = None;
                deg_v = None;
                parm_u.clear();
                parm_v.clear();
                bmat_u.clear();
                bmat_v.clear();
                step_u = None;
                step_v = None;
                // Spec §"Curve and surface type": `cstype [rat] type`.
                let mut iter = entry.iter().skip(1);
                let first = iter.next().map(String::as_str);
                let second = iter.next().map(String::as_str);
                active_kind = match (first, second) {
                    (Some("bezier"), _) => Some("bezier"),
                    (Some("rat"), Some("bezier")) => Some("rat_bezier"),
                    (Some("bspline"), _) => Some("bspline"),
                    (Some("rat"), Some("bspline")) => Some("rat_bspline"),
                    // Spec §"Cardinal": cubic, first-derivative-continuous
                    // surface (round 13). The `rat` qualifier maps to the
                    // same kind — the spec note (§"Free-form curve/surface
                    // body statements") says the unit-weight default is
                    // reasonable for Cardinal because its basis functions
                    // sum to 1, so we don't differentiate `rat cardinal`.
                    (Some("cardinal"), _) => Some("cardinal"),
                    (Some("rat"), Some("cardinal")) => Some("cardinal"),
                    // Spec §"Taylor": arbitrary-degree polynomial surface
                    // S(u,v) = Σ_i Σ_j c_{i,j} · u^i · v^j (round 14).
                    // The spec note in §"Free-form curve/surface body
                    // statements" says the unit-weight default "does
                    // not make sense for Taylor"; we accept `rat
                    // taylor` for syntactic compatibility but evaluate
                    // it the same way (no per-vertex weights).
                    (Some("taylor"), _) => Some("taylor"),
                    (Some("rat"), Some("taylor")) => Some("taylor"),
                    // Spec §"Basis matrix" (round 182 surfaces): the
                    // user supplies `bmat u` + `bmat v` plus
                    // `step stepu stepv` body statements; per spec
                    // §"Free-form curve/surface body statements" the
                    // `rat` form just signals per-vertex weight
                    // blending, which we currently don't apply to the
                    // bmatrix path (matches the round-10 curve
                    // behaviour), so both forms map to the same kind.
                    (Some("bmatrix"), _) => Some("bmatrix"),
                    (Some("rat"), Some("bmatrix")) => Some("bmatrix"),
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
            // Spec §"parm u/v": `parm u p1 p2 …` / `parm v p1 p2 …`. For
            // B-spline surfaces these are the knot vectors in each
            // parametric direction.
            "parm" if entry.get(1).map(String::as_str) == Some("u") => {
                parm_u = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f32>().ok())
                    .collect();
            }
            "parm" if entry.get(1).map(String::as_str) == Some("v") => {
                parm_v = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f32>().ok())
                    .collect();
            }
            // Spec §"bmat u/v matrix": `bmat u m_00 m_01 … m_nn` (and
            // `bmat v` for surfaces) supplies the row-major
            // `(n + 1) × (n + 1)` basis matrix with the column index
            // varying fastest. Captured for the basis-matrix surface
            // path; ignored by the other `cstype` branches.
            "bmat" if entry.get(1).map(String::as_str) == Some("u") => {
                bmat_u = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f32>().ok())
                    .collect();
            }
            "bmat" if entry.get(1).map(String::as_str) == Some("v") => {
                bmat_v = entry[2..]
                    .iter()
                    .filter_map(|t| t.parse::<f32>().ok())
                    .collect();
            }
            // Spec §"step stepu stepv": `step stepu [stepv]`. `stepu`
            // is mandatory; `stepv` is required only for surfaces and
            // controls the v-direction patch decomposition.
            "step" => {
                step_u = entry.get(1).and_then(|t| t.parse::<u32>().ok());
                step_v = entry.get(2).and_then(|t| t.parse::<u32>().ok());
            }
            "surf" => pending_surfs.push(entry),
            // Spec §"trim u0 u1 curv2d u0 u1 curv2d …": outer trimming
            // loop assembled from one or more curv2 segments. Resolved
            // here to a closed (u, v) polygon so the eventual
            // `flush_surface` call can point-in-polygon test each
            // surface-lattice vertex against it.
            "trim" => {
                if let Some(loop_uv) = resolve_loop(entry) {
                    pending_trims.push(loop_uv);
                }
            }
            // Spec §"hole u0 u1 curv2d u0 u1 curv2d …": inner trimming
            // loop ("hole"). Same shape as `trim`, but each surface
            // vertex inside any hole loop is excluded.
            "hole" => {
                if let Some(loop_uv) = resolve_loop(entry) {
                    pending_holes.push(loop_uv);
                }
            }
            "end" => {
                flush(
                    &mut out,
                    active_kind,
                    deg_u,
                    deg_v,
                    &parm_u,
                    &parm_v,
                    &bmat_u,
                    &bmat_v,
                    step_u,
                    step_v,
                    &pending_surfs,
                    &pending_trims,
                    &pending_holes,
                );
                pending_surfs.clear();
                pending_trims.clear();
                pending_holes.clear();
                active_kind = None;
                deg_u = None;
                deg_v = None;
                parm_u.clear();
                parm_v.clear();
                bmat_u.clear();
                bmat_v.clear();
                step_u = None;
                step_v = None;
            }
            _ => {}
        }
    }
    // Tail flush — defensive against a missing closing `end`.
    flush(
        &mut out,
        active_kind,
        deg_u,
        deg_v,
        &parm_u,
        &parm_v,
        &bmat_u,
        &bmat_v,
        step_u,
        step_v,
        &pending_surfs,
        &pending_trims,
        &pending_holes,
    );
    out
}

/// Sub-cell boundary re-mesher for the surface trim/hole clip
/// (spec §"Trimming loops and holes", §"trim u0 u1 curv2d …",
/// §"hole u0 u1 curv2d …").
///
/// The lattice classification pass marks each `(samples + 1)²` sample
/// vertex as kept (inside at least one trim loop — or no trim loops at
/// all, per spec "If the first trim statement in the sequence is
/// omitted, the enclosing outer trimming loop is taken to be the
/// parameter range of the surface" — AND outside every hole loop) or
/// dropped. Fully-kept triangles emit unchanged and fully-dropped
/// triangles vanish, exactly like the conservative clip; the
/// previously-dropped **straddling** triangles (1 or 2 corners kept)
/// are now clipped against the in/out classification function instead
/// of being discarded wholesale:
///
///   * each lattice edge whose endpoints classify differently is
///     bisected in parameter space until the inside/outside frontier
///     is pinned to float precision, yielding a boundary vertex on
///     the trimming-loop polygon;
///   * the kept sub-polygon (a triangle for 1-kept corners, a quad
///     split into two triangles for 2-kept corners) is emitted with
///     the original winding;
///   * crossings are cached per undirected lattice edge so adjacent
///     straddling triangles share their boundary vertex and the
///     re-meshed rim stays watertight;
///   * sub-triangles whose parameter-space area collapses below a
///     small fraction of the cell area (loops grazing a lattice line)
///     are suppressed rather than emitted as degenerate slivers.
///
/// The synthesised boundary vertex's 3D position is interpolated
/// linearly along the lattice edge at the bisected parameter — the
/// same piecewise-linear approximation the triangle lattice itself
/// carries, so the re-meshed boundary is exactly as accurate as the
/// surrounding mesh. Boundary vertices that end up referenced by no
/// surviving sub-triangle (every candidate was a suppressed sliver)
/// are garbage-collected by [`TrimRemesh::finish`] so the vertex pool
/// only grows where geometry actually appeared.
struct TrimRemesh<'a> {
    /// `(samples + 1)` — lattice vertices per row.
    stride: usize,
    samples: u32,
    /// Parameter-rectangle origin + spans (`surf s0 s1 t0 t1`).
    s0: f32,
    span_s: f32,
    t0: f32,
    span_t: f32,
    trims: &'a [Vec<[f32; 2]>],
    holes: &'a [Vec<[f32; 2]>],
    /// The lattice 3D positions (row-major, `stride²` entries).
    lattice: &'a [[f32; 3]],
    /// Per-lattice-vertex kept mask from the classification pass.
    kept: &'a [bool],
    /// Suppress emitted sub-triangles below this parameter-space area.
    area_eps: f32,
    /// Synthesised boundary vertices (positions + parameter coords),
    /// indexed from `lattice.len()` upward.
    boundary_positions: Vec<[f32; 3]>,
    boundary_uvs: Vec<[f32; 2]>,
    /// Undirected lattice edge → synthesised boundary vertex index.
    edge_cache: HashMap<(u32, u32), u32>,
}

impl<'a> TrimRemesh<'a> {
    /// Parameter-space coordinate of any vertex index (lattice or
    /// synthesised boundary vertex).
    fn uv_of(&self, i: u32) -> [f32; 2] {
        let lattice_count = self.lattice.len() as u32;
        if i < lattice_count {
            let su = (i as usize) % self.stride;
            let sv = (i as usize) / self.stride;
            [
                self.s0 + (su as f32 / self.samples as f32) * self.span_s,
                self.t0 + (sv as f32 / self.samples as f32) * self.span_t,
            ]
        } else {
            self.boundary_uvs[(i - lattice_count) as usize]
        }
    }

    /// The trim/hole classification function — identical to the
    /// per-lattice-vertex pass in [`flush_surface`] so the bisected
    /// frontier converges onto the same region boundary.
    fn inside(&self, uv: [f32; 2]) -> bool {
        let in_trim = self.trims.is_empty()
            || self
                .trims
                .iter()
                .any(|loop_uv| point_in_polygon(uv, loop_uv));
        let in_hole = self
            .holes
            .iter()
            .any(|loop_uv| point_in_polygon(uv, loop_uv));
        in_trim && !in_hole
    }

    /// Boundary vertex on the lattice edge `inside_idx → outside_idx`,
    /// synthesised by bisecting the classification function along the
    /// edge in parameter space. Cached per undirected edge so the two
    /// triangles sharing the edge agree on the vertex.
    fn crossing(&mut self, inside_idx: u32, outside_idx: u32) -> u32 {
        let key = if inside_idx < outside_idx {
            (inside_idx, outside_idx)
        } else {
            (outside_idx, inside_idx)
        };
        if let Some(&idx) = self.edge_cache.get(&key) {
            return idx;
        }
        let uv_in = self.uv_of(inside_idx);
        let uv_out = self.uv_of(outside_idx);
        // Invariant: `lo` classifies inside, `hi` outside. 24 rounds
        // pin the frontier to ~2⁻²⁴ of the edge length — beyond f32
        // lattice resolution.
        let mut lo = 0.0f32;
        let mut hi = 1.0f32;
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            let uv = [
                uv_in[0] + (uv_out[0] - uv_in[0]) * mid,
                uv_in[1] + (uv_out[1] - uv_in[1]) * mid,
            ];
            if self.inside(uv) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        // Land on the last known-inside parameter so the synthesised
        // vertex itself still classifies inside the trimmed region.
        let t = lo;
        let pa = self.lattice[inside_idx as usize];
        let pb = self.lattice[outside_idx as usize];
        let idx = self.lattice.len() as u32 + self.boundary_positions.len() as u32;
        self.boundary_positions.push([
            pa[0] + (pb[0] - pa[0]) * t,
            pa[1] + (pb[1] - pa[1]) * t,
            pa[2] + (pb[2] - pa[2]) * t,
        ]);
        self.boundary_uvs.push([
            uv_in[0] + (uv_out[0] - uv_in[0]) * t,
            uv_in[1] + (uv_out[1] - uv_in[1]) * t,
        ]);
        self.edge_cache.insert(key, idx);
        idx
    }

    /// Push triangle `(a, b, c)` unless its parameter-space area is a
    /// degenerate sliver (loop boundary grazing a lattice line).
    fn push_triangle(&self, indices: &mut Vec<u32>, a: u32, b: u32, c: u32) {
        let p = self.uv_of(a);
        let q = self.uv_of(b);
        let r = self.uv_of(c);
        let area2 = ((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])).abs();
        if area2 > self.area_eps {
            indices.push(a);
            indices.push(b);
            indices.push(c);
        }
    }

    /// Clip one CCW lattice triangle against the trim/hole region and
    /// append the surviving (sub-)triangles to `indices`.
    fn clip_triangle(&mut self, indices: &mut Vec<u32>, a: u32, b: u32, c: u32) {
        let ka = self.kept[a as usize];
        let kb = self.kept[b as usize];
        let kc = self.kept[c as usize];
        match (ka, kb, kc) {
            (true, true, true) => {
                indices.push(a);
                indices.push(b);
                indices.push(c);
            }
            (false, false, false) => {}
            // Exactly one corner kept: rotate it to the front (winding
            // preserved) and keep the corner triangle bounded by the
            // two edge crossings.
            (true, false, false) => self.clip_one_kept(indices, a, b, c),
            (false, true, false) => self.clip_one_kept(indices, b, c, a),
            (false, false, true) => self.clip_one_kept(indices, c, a, b),
            // Exactly two corners kept: rotate the dropped corner to
            // the back and keep the quad `(i1, i2, cross(i2→o),
            // cross(i1→o))` split into two triangles.
            (true, true, false) => self.clip_two_kept(indices, a, b, c),
            (false, true, true) => self.clip_two_kept(indices, b, c, a),
            (true, false, true) => self.clip_two_kept(indices, c, a, b),
        }
    }

    /// `(i, o1, o2)` with only `i` kept → triangle
    /// `(i, cross(i→o1), cross(i→o2))`.
    fn clip_one_kept(&mut self, indices: &mut Vec<u32>, i: u32, o1: u32, o2: u32) {
        let p1 = self.crossing(i, o1);
        let p2 = self.crossing(i, o2);
        self.push_triangle(indices, i, p1, p2);
    }

    /// `(i1, i2, o)` with `o` dropped → quad
    /// `(i1, i2, cross(i2→o), cross(i1→o))` as two triangles.
    fn clip_two_kept(&mut self, indices: &mut Vec<u32>, i1: u32, i2: u32, o: u32) {
        let p1 = self.crossing(i2, o);
        let p2 = self.crossing(i1, o);
        self.push_triangle(indices, i1, i2, p1);
        self.push_triangle(indices, i1, p1, p2);
    }

    /// Garbage-collect boundary vertices that no surviving sub-triangle
    /// references (every candidate was a suppressed sliver), remap the
    /// indices, and return the referenced boundary positions in index
    /// order ready to append after the lattice vertices.
    fn finish(self, indices: &mut [u32]) -> Vec<[f32; 3]> {
        let lattice_count = self.lattice.len() as u32;
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let mut compacted: Vec<[f32; 3]> = Vec::new();
        for idx in indices.iter_mut() {
            let old = *idx;
            if old < lattice_count {
                continue;
            }
            let next = lattice_count + remap.len() as u32;
            let slot = *remap.entry(old).or_insert_with(|| {
                compacted.push(self.boundary_positions[(old - lattice_count) as usize]);
                next
            });
            *idx = slot;
        }
        compacted
    }
}

/// Evaluate one `surf` element against an active Bezier / B-spline /
/// Cardinal / Taylor `cstype` and return the triangulated primitive,
/// or `None` when the directive is incomplete / malformed (lenient-
/// loader pattern — the directive still round-trips through
/// `obj:freeform_directives`).
#[allow(clippy::too_many_arguments)]
fn flush_surface(
    doc: &ObjDoc,
    kind: &'static str,
    deg_u: Option<u32>,
    deg_v: Option<u32>,
    parm_u: &[f32],
    parm_v: &[f32],
    bmat_u: &[f32],
    bmat_v: &[f32],
    step_u: Option<u32>,
    step_v: Option<u32>,
    entry: &[String],
    samples: u32,
    trims: &[Vec<[f32; 2]>],
    holes: &[Vec<[f32; 2]>],
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

    let bspline = matches!(kind, "bspline" | "rat_bspline");
    let cardinal = kind == "cardinal";
    let taylor = kind == "taylor";
    let bmatrix = kind == "bmatrix";
    // Determine the expected single-patch control grid.
    //   * Bezier: a single patch is exactly (degu + 1) × (degv + 1)
    //     control points (spec §"Bezier"). Larger grids are multi-patch
    //     and need a `step` stride the Bezier basis doesn't carry, so they
    //     stay captured-only.
    //   * B-spline: the control-point count per direction is fixed by the
    //     knot vector — spec §"B-spline" condition 6, `K = q − n − 1`, so
    //     there are `len(parm) − deg − 1` control points in that
    //     direction. A single `surf` already covers the whole grid (the
    //     knot vector internally encodes the piecewise segments), so no
    //     patch decomposition is needed.
    //   * Cardinal: cubic-only (spec §"Cardinal": "only defined for the
    //     cubic case"). The control count per direction relates to the
    //     `parm` count by the spec condition `parm = K − n + 2` (n = 3),
    //     i.e. `K_dir = parm_count + 1`. When a `parm` directive only
    //     spells out the 2-value global parameter range (as the spec
    //     Cardinal-surface example does), there is no per-direction split
    //     to read, so the grid is taken to be square — `cols = rows =
    //     sqrt(total)` — which recovers the canonical single 4×4 patch.
    //   * Taylor: the control points are the polynomial coefficients
    //     `c_{i,j}` for `S(u,v) = Σ_i Σ_j c_{i,j} · u^i · v^j` (spec
    //     §"Taylor"). A single Taylor "patch" of declared degree
    //     `deg degu degv` therefore needs exactly
    //     `(degu + 1) × (degv + 1)` coefficient vectors, matching the
    //     Bezier control-grid extents.
    let (cols, rows) = if bspline {
        // Need at least `deg + 2` knots per direction for ≥ 1 control
        // point. The `du + 2` / `dv + 2` arithmetic guards against
        // attacker-supplied `deg` values that would overflow `usize` on
        // the subsequent subtraction; an out-of-range degree leaves the
        // surface captured-only.
        let need_u = du.checked_add(2)?;
        let need_v = dv.checked_add(2)?;
        if parm_u.len() < need_u || parm_v.len() < need_v {
            return None;
        }
        (parm_u.len() - du - 1, parm_v.len() - dv - 1) // (K1 + 1, K2 + 1)
    } else if bmatrix {
        // Spec §"Basis matrix" / §"step stepu stepv": the per-direction
        // control-vertex count is K = (parm_count − 2) · s + n + 1 (the
        // inverse of the spec's `parm = (K − n) / s + 2`). Both `parm u`
        // / `parm v` and `step stepu stepv` are required for a surface;
        // missing either leaves the surface captured-only.
        let su = step_u? as usize;
        let sv = step_v? as usize;
        if su == 0 || sv == 0 || parm_u.len() < 2 || parm_v.len() < 2 {
            return None;
        }
        let cols = (parm_u.len() - 2)
            .checked_mul(su)?
            .checked_add(du)?
            .checked_add(1)?;
        let rows = (parm_v.len() - 2)
            .checked_mul(sv)?
            .checked_add(dv)?
            .checked_add(1)?;
        (cols, rows)
    } else if cardinal {
        // Cardinal must be cubic per spec; reject any other degree (the
        // directive still round-trips verbatim through extras).
        if du != 3 || dv != 3 {
            return None;
        }
        let total = entry.len() - 5; // control-vertex token count.
        // Prefer the per-direction `parm` extents when they carry more
        // than just the range endpoints (`parm = K − n + 2`); otherwise
        // fall back to a square single-patch grid.
        let cols = if parm_u.len() > 2 {
            parm_u.len() + 1
        } else {
            isqrt_exact(total)?
        };
        let rows = if parm_v.len() > 2 {
            parm_v.len() + 1
        } else if cols != 0 && total % cols == 0 {
            total / cols
        } else {
            return None;
        };
        (cols, rows)
    } else if taylor {
        // Taylor: `(degu + 1) × (degv + 1)` polynomial coefficients per
        // single patch (spec §"Taylor"). `checked_add` guards against
        // attacker-supplied huge degree values (e.g. `deg 111111`)
        // whose `+1` would still fit in `usize` but whose product
        // blows past available memory in the `Vec::with_capacity`
        // below.
        (du.checked_add(1)?, dv.checked_add(1)?)
    } else {
        // Bezier / rat_bezier — spec §"Bezier": "the number of global
        // parameter values given with the parm statement must be
        // K/n + 1, where K is the number of control points. For
        // surfaces, this requirement applies independently for the u
        // and v parametric directions." Inverting that:
        // `K = degu × (parm_u_count − 1)`, with adjacent patches
        // sharing their boundary control points (spec §"Surface
        // vertex data — Control points": "For surfaces made up of
        // many patches, …, the control points are ordered as if the
        // surface were a single large patch"), so the total
        // per-direction grid extent is `K + 1 = degu × patches_u + 1`
        // where `patches_u = parm_u_count − 1`. The single-patch case
        // (`parm u v0 v1`) collapses to the canonical
        // `(degu + 1) × (degv + 1)`. When the `parm` directive is
        // missing entirely (some loaders elide it for the default
        // single-patch case), fall back to the single-patch grid.
        let patches_u = if parm_u.len() >= 2 {
            parm_u.len() - 1
        } else {
            1
        };
        let patches_v = if parm_v.len() >= 2 {
            parm_v.len() - 1
        } else {
            1
        };
        let cols = du.checked_mul(patches_u)?.checked_add(1)?;
        let rows = dv.checked_mul(patches_v)?.checked_add(1)?;
        (cols, rows)
    };
    // Cap the expected control-grid size: a single `surf` line carries
    // `entry.len() - 5` control-vertex tokens, so any `expected` that
    // doesn't match that count is captured-only anyway (per the
    // `grid.len() != expected` check at the end of the read loop). Bail
    // here before the `Vec::with_capacity(expected)` allocation to keep
    // attacker `deg` / `parm` values from triggering an
    // allocation-size-too-big abort.
    let expected = cols.checked_mul(rows)?;
    if expected != entry.len().saturating_sub(5) {
        return None;
    }

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
        // Not a single patch of the declared degree (Bezier) or the knot-
        // vector-implied grid size (B-spline) — leave it captured-only
        // rather than guessing the patch decomposition.
        return None;
    }

    let mut positions = if bspline {
        sample_bspline_surface(
            &grid, &weights, kind, du as u32, dv as u32, parm_u, parm_v, s0, s1, t0, t1, cols,
            rows, samples,
        )
    } else if cardinal {
        sample_cardinal_surface(&grid, cols, rows, samples)
    } else if taylor {
        sample_taylor_surface(&grid, cols, rows, s0, s1, t0, t1, samples)
    } else if bmatrix {
        // Spec §"Basis matrix": validate the basis-matrix sizes
        // (n + 1)² before evaluating. `flush_surface` already enforced
        // the per-direction control-vertex count via the `parm` / `step`
        // inverse formula, so a bmat-size mismatch here is the only
        // remaining captured-only condition.
        let need_u = du.checked_add(1)?.checked_mul(du.checked_add(1)?)?;
        let need_v = dv.checked_add(1)?.checked_mul(dv.checked_add(1)?)?;
        if bmat_u.len() != need_u || bmat_v.len() != need_v {
            return None;
        }
        let su = step_u?;
        let sv = step_v?;
        sample_bmatrix_surface(
            &grid, bmat_u, bmat_v, du as u32, dv as u32, su, sv, cols, rows, samples,
        )
    } else {
        // Bezier: walk the `patches_u × patches_v` patch grid (each
        // patch is `(du + 1) × (dv + 1)` control points, with adjacent
        // patches sharing their boundary column / row). Single-patch
        // inputs (the common case) route through the same loop with a
        // 1×1 patch grid, matching the legacy behaviour.
        let patches_u = cols.saturating_sub(1).checked_div(du).unwrap_or(1).max(1);
        let patches_v = rows.saturating_sub(1).checked_div(dv).unwrap_or(1).max(1);
        sample_bezier_surface_multipatch(
            &grid, &weights, kind, cols, rows, du, dv, patches_u, patches_v, samples,
        )
    };
    if positions.is_empty() {
        return None;
    }

    // Per-lattice-vertex parameter-space coordinates, only built when
    // there is at least one trim or hole loop to test against. Each
    // sample lives at `(s0 + u_frac · (s1 − s0), t0 + v_frac · (t1 − t0))`
    // — the same uniform sampling the per-`cstype` evaluators above use.
    // Vertex `(su, sv)` is kept iff it lies inside any trim loop (or
    // there are no trim loops, in which case "inside the parameter
    // rectangle" is assumed — spec §"Trimming Loops": "If no trim or
    // hole statements are specified, then the surface is trimmed at
    // its parameter range") AND outside every hole loop.
    let stride = samples as usize + 1;
    let trimming = !trims.is_empty() || !holes.is_empty();
    let kept: Vec<bool> = if trimming {
        let mut k = Vec::with_capacity(stride * stride);
        let span_s = s1 - s0;
        let span_t = t1 - t0;
        for sv in 0..stride {
            for su in 0..stride {
                let u_frac = su as f32 / samples as f32;
                let v_frac = sv as f32 / samples as f32;
                let u = s0 + u_frac * span_s;
                let v = t0 + v_frac * span_t;
                let in_trim = trims.is_empty()
                    || trims
                        .iter()
                        .any(|loop_uv| point_in_polygon([u, v], loop_uv));
                let in_hole = holes
                    .iter()
                    .any(|loop_uv| point_in_polygon([u, v], loop_uv));
                k.push(in_trim && !in_hole);
            }
        }
        k
    } else {
        Vec::new()
    };

    // Build a triangle grid over the (samples + 1) × (samples + 1)
    // sample lattice. Vertex (su, sv) lives at index sv * stride + su.
    // Two CCW triangles per cell (spec §"surf" note: the front of the
    // surface is the side where u increases to the right and v
    // increases upward). When trim/hole loops are active, straddling
    // boundary triangles are sub-cell re-meshed against the loop
    // polygon via [`TrimRemesh`] instead of dropped wholesale, so the
    // trimmed rim follows the loop boundary rather than staying
    // jagged at the lattice grain.
    let mut indices: Vec<u32> = Vec::with_capacity((samples as usize) * (samples as usize) * 6);
    let mut boundary_vertex_count = 0usize;
    if trimming {
        let span_s = s1 - s0;
        let span_t = t1 - t0;
        // Sliver threshold: 10⁻⁶ of one lattice cell's parameter-space
        // area (×2 because `push_triangle` compares doubled areas).
        // Loops that graze a lattice line produce crossings at the
        // line itself; the resulting zero-width sub-triangles are
        // suppressed instead of emitted as degenerate slivers.
        let cell_area2 = (span_s / samples as f32).abs() * (span_t / samples as f32).abs() * 2.0;
        let mut remesh = TrimRemesh {
            stride,
            samples,
            s0,
            span_s,
            t0,
            span_t,
            trims,
            holes,
            lattice: &positions,
            kept: &kept,
            area_eps: cell_area2 * 1e-6,
            boundary_positions: Vec::new(),
            boundary_uvs: Vec::new(),
            edge_cache: HashMap::new(),
        };
        for sv in 0..samples as usize {
            for su in 0..samples as usize {
                let i00 = (sv * stride + su) as u32;
                let i10 = (sv * stride + su + 1) as u32;
                let i01 = ((sv + 1) * stride + su) as u32;
                let i11 = ((sv + 1) * stride + su + 1) as u32;
                remesh.clip_triangle(&mut indices, i00, i10, i11);
                remesh.clip_triangle(&mut indices, i00, i11, i01);
            }
        }
        let boundary = remesh.finish(&mut indices);
        boundary_vertex_count = boundary.len();
        positions.extend(boundary);
    } else {
        for sv in 0..samples as usize {
            for su in 0..samples as usize {
                let i00 = (sv * stride + su) as u32;
                let i10 = (sv * stride + su + 1) as u32;
                let i01 = ((sv + 1) * stride + su) as u32;
                let i11 = ((sv + 1) * stride + su + 1) as u32;
                indices.push(i00);
                indices.push(i10);
                indices.push(i11);
                indices.push(i00);
                indices.push(i11);
                indices.push(i01);
            }
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
    // Spec §"Bezier" multi-patch decomposition — when the synthesised
    // grid contains more than one patch per direction, surface the
    // per-direction patch count so downstream consumers can recognise
    // the boundary structure inside the otherwise-uniform triangle
    // lattice. Other `cstype` paths already encode their segment count
    // through their own provenance (B-spline knot vector, basis-matrix
    // step), so the marker is Bezier-specific.
    if matches!(kind, "bezier" | "rat_bezier") && du > 0 && dv > 0 {
        let patches_u = (cols.saturating_sub(1)) / du;
        let patches_v = (rows.saturating_sub(1)) / dv;
        if patches_u > 1 || patches_v > 1 {
            prim.extras.insert(
                "obj:surface_patches".to_string(),
                serde_json::Value::Array(vec![
                    serde_json::Value::from(patches_u as u64),
                    serde_json::Value::from(patches_v as u64),
                ]),
            );
        }
    }
    if trimming {
        // Spec §"Trimming Loops" — record how many outer/inner loops
        // contributed to the clip so downstream consumers (or
        // round-trip verifiers) can tell the synthetic mesh apart from
        // an un-clipped one.
        prim.extras.insert(
            "obj:surface_trimmed".to_string(),
            serde_json::Value::Bool(true),
        );
        prim.extras.insert(
            "obj:surface_trim_loops".to_string(),
            serde_json::Value::Number(serde_json::Number::from(trims.len() as u64)),
        );
        prim.extras.insert(
            "obj:surface_hole_loops".to_string(),
            serde_json::Value::Number(serde_json::Number::from(holes.len() as u64)),
        );
        // Sub-cell boundary re-mesh provenance: how many vertices were
        // synthesised on the trim/hole loop boundary (0 when every
        // straddling cell collapsed to suppressed slivers, e.g. loops
        // aligned exactly with lattice lines). Boundary vertices sit
        // after the `(samples + 1)²` lattice block in `positions`.
        prim.extras.insert(
            "obj:surface_trim_boundary_vertices".to_string(),
            serde_json::Value::Number(serde_json::Number::from(boundary_vertex_count as u64)),
        );
    }

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

/// Evaluate a multi-patch Bezier (or rational-Bezier) surface at a
/// `(samples + 1) × (samples + 1)` lattice over the global parameter
/// rectangle.
///
/// Spec §"Bezier" gives the per-direction control count as
/// `K = degu × patches_u` (with `parm_u_count = K/degu + 1 = patches_u + 1`),
/// and §"Surface vertex data — Control points" arranges the global
/// control mesh "as if the surface were a single large patch" with
/// adjacent patches sharing their boundary control row / column. The
/// total per-direction grid extent is therefore `cols = degu × patches_u + 1`
/// (and `rows = degv × patches_v + 1`), with patch `(pu, pv)` owning the
/// `(degu + 1) × (degv + 1)` sub-window starting at
/// `(pu × degu, pv × degv)`.
///
/// Each global lattice sample `(su, sv)` maps to a global parameter
/// `(u_g, v_g) ∈ [0, patches_u] × [0, patches_v]`; its integer part
/// selects the patch, fractional part is the local Bezier parameter
/// `t ∈ [0, 1]` for tensor-product de Casteljau (delegated to
/// [`sample_bezier_surface`] on a freshly-windowed sub-grid). The single-
/// patch case (`patches_u = patches_v = 1`) collapses to the legacy
/// behaviour: one lattice over the whole grid, one de Casteljau pass per
/// sample.
///
/// Output vertices are ordered row-major in the sample lattice: sample
/// `(su, sv)` lands at index `sv × (samples + 1) + su`.
#[allow(clippy::too_many_arguments)]
fn sample_bezier_surface_multipatch(
    grid: &[[f32; 3]],
    weights: &[f32],
    kind: &str,
    cols: usize,
    rows: usize,
    deg_u: usize,
    deg_v: usize,
    patches_u: usize,
    patches_v: usize,
    samples: u32,
) -> Vec<[f32; 3]> {
    if samples == 0
        || cols == 0
        || rows == 0
        || deg_u == 0
        || deg_v == 0
        || patches_u == 0
        || patches_v == 0
        || grid.len() != cols * rows
        || weights.len() != grid.len()
    {
        return Vec::new();
    }
    // Single-patch fast path: identical to the original
    // `sample_bezier_surface` traversal.
    if patches_u == 1 && patches_v == 1 {
        return sample_bezier_surface(grid, weights, kind, cols, rows, samples);
    }
    let rational = kind == "rat_bezier";
    let homo: Vec<[f32; 4]> = grid
        .iter()
        .zip(weights.iter())
        .map(|(p, w)| {
            let weight = if rational { *w } else { 1.0 };
            [p[0] * weight, p[1] * weight, p[2] * weight, weight]
        })
        .collect();

    let n = samples as usize + 1;
    let patch_cols = deg_u + 1;
    let patch_rows = deg_v + 1;
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n * n);

    // Pre-allocated scratch buffers reused per sample.
    let mut sub_window: Vec<[f32; 4]> = Vec::with_capacity(patch_cols * patch_rows);
    let mut col_pts: Vec<[f32; 4]> = Vec::with_capacity(patch_rows);

    for sv in 0..n {
        // Global v ∈ [0, patches_v]: integer part = v-patch index,
        // fractional part = local Bezier parameter t_v ∈ [0, 1].
        let v_global = if n == 1 {
            0.0
        } else {
            sv as f32 * (patches_v as f32) / (n - 1) as f32
        };
        let mut pv = v_global.floor() as isize;
        if pv < 0 {
            pv = 0;
        }
        if pv as usize >= patches_v {
            pv = patches_v as isize - 1;
        }
        let pv = pv as usize;
        let t_v = (v_global - pv as f32).clamp(0.0, 1.0);

        for su in 0..n {
            let u_global = if n == 1 {
                0.0
            } else {
                su as f32 * (patches_u as f32) / (n - 1) as f32
            };
            let mut pu = u_global.floor() as isize;
            if pu < 0 {
                pu = 0;
            }
            if pu as usize >= patches_u {
                pu = patches_u as isize - 1;
            }
            let pu = pu as usize;
            let t_u = (u_global - pu as f32).clamp(0.0, 1.0);

            // Copy the active patch's `(deg_u + 1) × (deg_v + 1)`
            // homogeneous sub-grid. Spec §"Surface vertex data —
            // Control points": the active patch starts at
            // `(pu · deg_u, pv · deg_v)` and ends at
            // `(pu · deg_u + deg_u, pv · deg_v + deg_v)` inclusive,
            // sharing its boundary with neighbouring patches.
            sub_window.clear();
            let base_u = pu * deg_u;
            let base_v = pv * deg_v;
            for j in 0..patch_rows {
                let row_start = (base_v + j) * cols + base_u;
                sub_window.extend_from_slice(&homo[row_start..row_start + patch_cols]);
            }

            col_pts.clear();
            for r in 0..patch_rows {
                let row = &sub_window[r * patch_cols..(r + 1) * patch_cols];
                col_pts.push(de_casteljau_4d(row, t_u));
            }
            let pt = de_casteljau_4d(&col_pts, t_v);
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

/// Evaluate a basis-matrix surface patch (spec §"Basis matrix",
/// §"step stepu stepv") at a `(samples + 1) × (samples + 1)` lattice
/// via the bivariate tensor-product polynomial
///
///   S(u, v) = Σ_a Σ_b ( Σ_p B_u[a][p] · u^p )
///                     ( Σ_q B_v[b][q] · v^q )
///                     · c_{base_u + a, base_v + b}
///
/// where `B_u` / `B_v` are the per-direction basis matrices supplied by
/// `bmat u` / `bmat v` (row-major, column index `j` varying fastest per
/// spec §"bmat u/v matrix"), `deg_u` / `deg_v` are the per-direction
/// polynomial degrees from `deg degu degv`, and `step_u` / `step_v` are
/// the per-direction segment strides from `step stepu stepv`.
///
/// `grid` is the control mesh in row-major u-fastest order (spec
/// §"Surface vertex data — control points": "i = 0 to K1 for j = 0,
/// …"): `cols` control points per v-row, `rows` v-rows. Spec
/// §"Basis matrix" gives the per-direction control count as
/// `K = (parm − 2) · s + n + 1` (inverse of `parm = (K − n) / s + 2`);
/// the caller in [`flush_surface`] enforces that `cols` and `rows`
/// match this size before this routine runs.
///
/// Patch decomposition: each `(seg_u, seg_v)` pair traces a tensor-
/// product polynomial segment whose control window starts at
/// `(base_u, base_v) = (seg_u · step_u, seg_v · step_v)`. The total
/// per-direction segment count is `(K − n − 1) / s + 1`, derived in the
/// same way as the round-10 1D curve path (`sample_bmatrix`).
///
/// Output vertices are ordered row-major in the sample lattice:
/// sample `(su, sv)` lands at index `sv · (samples + 1) + su`.
///
/// Spec §"Free-form curve/surface body statements" notes the rational
/// `rat bmatrix` form would blend per-vertex `w` weights; we match the
/// round-10 curve path and do not apply them here (the `rat bmatrix`
/// kind routes to this same evaluator without weights), which keeps
/// the basis-matrix path consistent with the user-authored polynomial
/// definition.
#[allow(clippy::too_many_arguments)]
fn sample_bmatrix_surface(
    grid: &[[f32; 3]],
    bmat_u: &[f32],
    bmat_v: &[f32],
    deg_u: u32,
    deg_v: u32,
    step_u: u32,
    step_v: u32,
    cols: usize,
    rows: usize,
    samples: u32,
) -> Vec<[f32; 3]> {
    let n_plus_1 = match (deg_u as usize).checked_add(1) {
        Some(v) => v,
        None => return Vec::new(),
    };
    let m_plus_1 = match (deg_v as usize).checked_add(1) {
        Some(v) => v,
        None => return Vec::new(),
    };
    let need_bmat_u = match n_plus_1.checked_mul(n_plus_1) {
        Some(v) => v,
        None => return Vec::new(),
    };
    let need_bmat_v = match m_plus_1.checked_mul(m_plus_1) {
        Some(v) => v,
        None => return Vec::new(),
    };
    if samples == 0
        || cols == 0
        || rows == 0
        || step_u == 0
        || step_v == 0
        || grid.len() != cols * rows
        || bmat_u.len() != need_bmat_u
        || bmat_v.len() != need_bmat_v
        || cols < n_plus_1
        || rows < m_plus_1
    {
        return Vec::new();
    }
    let su_stride = step_u as usize;
    let sv_stride = step_v as usize;
    // Per-direction segment count: largest `i` with `i · s + n + 1 ≤ K`.
    // Matches the round-10 1D derivation, applied independently to u
    // and v per spec §"step stepu stepv" ("For surfaces, the above
    // description applies independently to each parametric direction.").
    let n_seg_u = (cols - n_plus_1) / su_stride + 1;
    let n_seg_v = (rows - m_plus_1) / sv_stride + 1;
    let n = samples as usize + 1;
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n * n);

    for sv_i in 0..n {
        // Global v ∈ [0, n_seg_v]: integer part = segment, fractional
        // part = local `t ∈ [0, 1]` within that segment. The last sample
        // is pinned to the upper endpoint of the final segment so the
        // surface closes on the spec-defined boundary.
        let gv = if sv_i == n - 1 {
            n_seg_v as f32
        } else {
            sv_i as f32 * n_seg_v as f32 / (n - 1) as f32
        };
        let mut seg_v = gv.floor() as usize;
        let mut tv = gv - seg_v as f32;
        if seg_v >= n_seg_v {
            seg_v = n_seg_v - 1;
            tv = 1.0;
        }
        let base_v = seg_v * sv_stride;

        // tv^0 .. tv^m once per row.
        let mut tv_pow: Vec<f32> = Vec::with_capacity(m_plus_1);
        let mut pv = 1.0_f32;
        for _ in 0..m_plus_1 {
            tv_pow.push(pv);
            pv *= tv;
        }
        // Row b's v-basis coefficient: Σ_q B_v[b][q] · tv^q.
        let mut v_coef: Vec<f32> = Vec::with_capacity(m_plus_1);
        for b in 0..m_plus_1 {
            let mut c = 0.0_f32;
            for q in 0..m_plus_1 {
                c += bmat_v[b * m_plus_1 + q] * tv_pow[q];
            }
            v_coef.push(c);
        }

        for su_i in 0..n {
            let gu = if su_i == n - 1 {
                n_seg_u as f32
            } else {
                su_i as f32 * n_seg_u as f32 / (n - 1) as f32
            };
            let mut seg_u = gu.floor() as usize;
            let mut tu = gu - seg_u as f32;
            if seg_u >= n_seg_u {
                seg_u = n_seg_u - 1;
                tu = 1.0;
            }
            let base_u = seg_u * su_stride;

            // tu^0 .. tu^n once per (su, sv) sample.
            let mut tu_pow: Vec<f32> = Vec::with_capacity(n_plus_1);
            let mut pu = 1.0_f32;
            for _ in 0..n_plus_1 {
                tu_pow.push(pu);
                pu *= tu;
            }
            // Column a's u-basis coefficient: Σ_p B_u[a][p] · tu^p.
            let mut u_coef: Vec<f32> = Vec::with_capacity(n_plus_1);
            for a in 0..n_plus_1 {
                let mut c = 0.0_f32;
                for p in 0..n_plus_1 {
                    c += bmat_u[a * n_plus_1 + p] * tu_pow[p];
                }
                u_coef.push(c);
            }

            // S(u, v) = Σ_a Σ_b u_coef[a] · v_coef[b] · grid[base_v+b][base_u+a].
            let mut accum = [0.0_f32; 3];
            for (b, vc) in v_coef.iter().enumerate() {
                let row = (base_v + b) * cols;
                for (a, uc) in u_coef.iter().enumerate() {
                    let cp = grid[row + base_u + a];
                    let w = uc * vc;
                    accum[0] += w * cp[0];
                    accum[1] += w * cp[1];
                    accum[2] += w * cp[2];
                }
            }
            out.push(accum);
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

/// Evaluate a B-spline (or rational B-spline / NURBS) surface patch at a
/// `(samples + 1) × (samples + 1)` lattice via the bivariate
/// tensor-product Cox-deBoor formula (spec §"B-spline", §"Rational and
/// non-rational curves and surfaces", §"Surface vertex data — control
/// points").
///
/// `grid` is the control mesh in row-major order with the u index varying
/// fastest (`cols` control points per v-row, `rows` v-rows). The surface
/// is
///
///   S(u, v) = Σ_i Σ_j N_{i,nu}(u) · N_{j,nv}(v) · d_{i,j}
///
/// for the non-rational case and
///
///   S(u, v) = Σ_i Σ_j N_{i,nu}(u) · N_{j,nv}(v) · w_{i,j} · d_{i,j}
///             ─────────────────────────────────────────────────────
///                  Σ_i Σ_j N_{i,nu}(u) · N_{j,nv}(v) · w_{i,j}
///
/// for the rational (NURBS) case. `nu` / `nv` are the u / v degrees and
/// `knots_u` (`parm u`) / `knots_v` (`parm v`) are the per-direction knot
/// vectors. The basis functions are evaluated with the same
/// [`bspline_basis`] routine the 1D curve path uses.
///
/// `s0`..`s1` and `t0`..`t1` are the `surf` parameter ranges; each is
/// clipped against the spec §"B-spline" condition-5 evaluation window
/// `[x_n, x_{K+1}]` of its direction's knot vector. The half-open
/// knot-span convention `x_i ≤ t < x_{i+1}` means an endpoint exactly at
/// the upper bound would yield an all-zero basis, so the last sample in
/// each direction is nudged fractionally below the bound (the same
/// standard NURBS-evaluator pattern as [`sample_bspline`]).
///
/// Output vertices are ordered row-major in the sample lattice: sample
/// `(su, sv)` lands at index `sv * (samples + 1) + su`.
#[allow(clippy::too_many_arguments)]
fn sample_bspline_surface(
    grid: &[[f32; 3]],
    weights: &[f32],
    kind: &str,
    deg_u: u32,
    deg_v: u32,
    knots_u: &[f32],
    knots_v: &[f32],
    s0: f32,
    s1: f32,
    t0: f32,
    t1: f32,
    cols: usize,
    rows: usize,
    samples: u32,
) -> Vec<[f32; 3]> {
    if samples == 0 || cols == 0 || rows == 0 || grid.len() != cols * rows {
        return Vec::new();
    }
    let nu = deg_u as usize;
    let nv = deg_v as usize;
    // Spec §"B-spline" condition 6: q + 1 knots ⇒ K + 1 = q − n control
    // points ⇒ knots.len() == control_count + degree + 1.
    if knots_u.len() != cols + nu + 1 || knots_v.len() != rows + nv + 1 {
        return Vec::new();
    }

    // Per-direction evaluation windows (spec condition 5:
    // x_n ≤ t_min < t_max ≤ x_{K+1}). Clip the `surf` ranges into the
    // valid span of each knot vector.
    let u_lo_bound = knots_u[nu];
    let u_hi_bound = knots_u[cols]; // x_{K1+1}, K1+1 = cols.
    let v_lo_bound = knots_v[nv];
    let v_hi_bound = knots_v[rows]; // x_{K2+1}, K2+1 = rows.
    let u_min = s0.max(u_lo_bound);
    let u_max = s1.min(u_hi_bound);
    let v_min = t0.max(v_lo_bound);
    let v_max = t1.min(v_hi_bound);
    if u_min > u_max || v_min > v_max {
        return Vec::new();
    }

    let rational = kind == "rat_bspline";
    let n = samples as usize + 1;

    // Precompute one row of u-basis values per sample column and one
    // column of v-basis values per sample row; the tensor product reuses
    // them across the lattice.
    let nudge = |t: f32, lo: f32, hi: f32| -> f32 {
        // When t lands exactly on the upper bound the half-open spans give
        // an all-zero basis; bias it fractionally inside the last span.
        if t >= hi {
            let biased = hi - (hi - lo).abs() * 1e-7 - f32::EPSILON;
            if biased < lo { lo } else { biased }
        } else {
            t
        }
    };

    let u_basis_rows: Vec<Vec<f32>> = (0..n)
        .map(|i| {
            let t01 = if n == 1 {
                0.0
            } else {
                i as f32 / (n - 1) as f32
            };
            let u = nudge(u_min + t01 * (u_max - u_min), u_lo_bound, u_hi_bound);
            bspline_basis(u, knots_u, nu)
        })
        .collect();
    let v_basis_rows: Vec<Vec<f32>> = (0..n)
        .map(|j| {
            let t01 = if n == 1 {
                0.0
            } else {
                j as f32 / (n - 1) as f32
            };
            let v = nudge(v_min + t01 * (v_max - v_min), v_lo_bound, v_hi_bound);
            bspline_basis(v, knots_v, nv)
        })
        .collect();

    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n * n);
    for vb in v_basis_rows.iter() {
        for ub in u_basis_rows.iter() {
            // Tensor product: S = Σ_j vb[j] · Σ_i ub[i] · w_{i,j} · d_{i,j}
            // accumulated together with the weighted denominator.
            let mut acc = [0.0f32; 3];
            let mut wsum = 0.0f32;
            for (j, &bv) in vb.iter().enumerate().take(rows) {
                if bv == 0.0 {
                    continue;
                }
                for (i, &bu) in ub.iter().enumerate().take(cols) {
                    if bu == 0.0 {
                        continue;
                    }
                    let idx = j * cols + i;
                    let w = if rational { weights[idx] } else { 1.0 };
                    let coeff = bu * bv * w;
                    if coeff == 0.0 {
                        continue;
                    }
                    wsum += coeff;
                    acc[0] += coeff * grid[idx][0];
                    acc[1] += coeff * grid[idx][1];
                    acc[2] += coeff * grid[idx][2];
                }
            }
            if wsum.abs() > f32::EPSILON {
                // Non-rational basis functions form a partition of unity
                // inside the valid window, so the division is a no-op there
                // (wsum ≈ 1); the rational form needs it. Dividing in both
                // cases keeps a single code path and is numerically safe.
                out.push([acc[0] / wsum, acc[1] / wsum, acc[2] / wsum]);
            } else {
                // Sample fell outside the support of every basis function
                // (pathological knot vector); emit the zero accumulator so
                // the lattice size still matches (samples + 1)^2.
                out.push(acc);
            }
        }
    }
    out
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

/// Evaluate a single cubic Cardinal (Catmull-Rom) control polygon at the
/// global parameter `s ∈ [0, len − 3]`, where the integer part of `s`
/// selects the 4-point segment window and the fractional part is the
/// local `t ∈ [0, 1]` inside that segment.
///
/// Spec §"Cardinal": each segment over `c0, c1, c2, c3` converts to a
/// cubic Bezier (`b0 = c1`, `b1 = c1 + (c2 − c0) / 6`,
/// `b2 = c2 − (c3 − c1) / 6`, `b3 = c2`) and is then evaluated with the
/// Bernstein cubic basis. The curve interpolates every interior control
/// point exactly. This is the 1D building block the tensor-product
/// surface evaluator reuses in both parametric directions.
fn cardinal_eval_1d(points: &[[f32; 3]], s: f32) -> [f32; 3] {
    // Caller guarantees `points.len() >= 4`.
    let n_segments = points.len() - 3;
    let mut seg = s.floor() as isize;
    let mut t = s - seg as f32;
    if seg < 0 {
        seg = 0;
        t = 0.0;
    } else if seg as usize >= n_segments {
        seg = n_segments as isize - 1;
        t = 1.0;
    }
    let seg = seg as usize;
    let c0 = points[seg];
    let c1 = points[seg + 1];
    let c2 = points[seg + 2];
    let c3 = points[seg + 3];
    // Spec §"Cardinal" Bezier conversion (component-wise per axis).
    let mut b: [[f32; 3]; 4] = [[0.0; 3]; 4];
    for a in 0..3 {
        b[0][a] = c1[a];
        b[1][a] = c1[a] + (c2[a] - c0[a]) / 6.0;
        b[2][a] = c2[a] - (c3[a] - c1[a]) / 6.0;
        b[3][a] = c2[a];
    }
    let u = 1.0 - t;
    let w0 = u * u * u;
    let w1 = 3.0 * u * u * t;
    let w2 = 3.0 * u * t * t;
    let w3 = t * t * t;
    [
        w0 * b[0][0] + w1 * b[1][0] + w2 * b[2][0] + w3 * b[3][0],
        w0 * b[0][1] + w1 * b[1][1] + w2 * b[2][1] + w3 * b[3][1],
        w0 * b[0][2] + w1 * b[1][2] + w2 * b[2][2] + w3 * b[3][2],
    ]
}

/// Evaluate a cubic Cardinal (Catmull-Rom) surface patch at a
/// `(samples + 1) × (samples + 1)` lattice via the bivariate
/// tensor-product Cardinal evaluation (spec §"Cardinal").
///
/// `grid` is the control mesh in row-major order with the u index varying
/// fastest (`cols` control points per v-row, `rows` v-rows; spec
/// §"Surface vertex data — control points"). The surface is the tensor
/// product of two cubic Cardinal bases:
///
///   S(u, v) = Σ_i Σ_j C_i(u) · C_j(v) · d_{i,j}
///
/// where `C_·` are the cubic Cardinal basis functions. We collapse the
/// inner u sum first by running the 1D Cardinal evaluator on each v-row,
/// then a second 1D Cardinal evaluation in the v direction over the
/// `rows` collapsed points (spec §"Cardinal": "For surfaces, all but the
/// first and last row and column of control points are interpolated").
///
/// The global parameter domain is `[0, cols − 3] × [0, rows − 3]` (one
/// unit per Cardinal segment); samples are spread uniformly over it. The
/// `surf` range scalars are provenance only (Cardinal is segment-
/// normalised, like the round-9 curve path), so they are not used to
/// re-parameterise the evaluation.
///
/// Weights / rationality: spec §"Free-form curve/surface body
/// statements" notes the unit-weight default is reasonable for Cardinal
/// (its basis functions sum to 1), so per-vertex `w` weights are not
/// applied — `rat cardinal` routes here too.
///
/// Output vertices are ordered row-major in the sample lattice: sample
/// `(su, sv)` lands at index `sv * (samples + 1) + su`.
fn sample_cardinal_surface(
    grid: &[[f32; 3]],
    cols: usize,
    rows: usize,
    samples: u32,
) -> Vec<[f32; 3]> {
    // Cardinal needs at least a 4×4 control window per direction.
    if samples == 0 || cols < 4 || rows < 4 || grid.len() != cols * rows {
        return Vec::new();
    }
    let n = samples as usize + 1;
    let u_span = (cols - 3) as f32; // number of u-segments.
    let v_span = (rows - 3) as f32; // number of v-segments.

    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n * n);
    for sv in 0..n {
        let v = if n == 1 {
            0.0
        } else {
            sv as f32 / (n - 1) as f32 * v_span
        };
        for su in 0..n {
            let u = if n == 1 {
                0.0
            } else {
                su as f32 / (n - 1) as f32 * u_span
            };
            // Inner pass: evaluate each v-row's 1D Cardinal curve at u,
            // leaving one point per row.
            let mut col_pts: Vec<[f32; 3]> = Vec::with_capacity(rows);
            for r in 0..rows {
                let row = &grid[r * cols..r * cols + cols];
                col_pts.push(cardinal_eval_1d(row, u));
            }
            // Outer pass: 1D Cardinal evaluation in v over the collapsed
            // points.
            out.push(cardinal_eval_1d(&col_pts, v));
        }
    }
    out
}

/// Evaluate a Taylor polynomial surface patch at a
/// `(samples + 1) × (samples + 1)` lattice via direct bivariate
/// polynomial evaluation.
///
/// Spec §"Taylor": the control points are the polynomial coefficients
/// `c_{i,j}` for the bivariate polynomial:
///
/// ```text
///   S(u, v) = Σ_{i=0..degu} Σ_{j=0..degv} c_{i,j} · u^i · v^j
/// ```
///
/// Applied component-wise per axis. Each of the three output channels
/// (x, y, z) is an independent polynomial in u and v whose coefficients
/// are taken from the corresponding component of the control points.
/// The control grid is row-major with the u index varying fastest (spec
/// §"Surface vertex data — control points"), so the coefficient
/// `c_{i,j}` lives at `grid[j * cols + i]` where `cols = degu + 1` and
/// `rows = degv + 1`.
///
/// The `surf s0 s1 t0 t1` range supplies the global parameter clip
/// (spec §"surf": "the [s0, s1] range gives the start/end values for
/// the curve in the u direction" — analogous for `[t0, t1]` in v).
/// Taylor curves and surfaces evaluate against the raw parameter values
/// directly (not a normalised `[0, 1]` re-parameterisation), so we
/// sample at `u_i = s0 + i / samples · (s1 - s0)` and similarly for v.
///
/// Implementation: we collapse the inner u sum first by Horner-rule
/// evaluation across each v-row, leaving one point per row, then a
/// second Horner-rule pass in v over the collapsed points. The inner-
/// loop scratch buffer is heap-allocated once per `(su, sv)` sample at
/// modest cost; the total surface sample count is `(samples + 1)²`.
///
/// Rationality: the spec note in §"Free-form curve/surface body
/// statements" explicitly says the rational form "does not make sense
/// for Taylor", so `rat taylor` routes here without weight blending.
///
/// Output vertices are ordered row-major in the sample lattice: sample
/// `(su, sv)` lands at index `sv * (samples + 1) + su`.
#[allow(clippy::too_many_arguments)]
fn sample_taylor_surface(
    grid: &[[f32; 3]],
    cols: usize,
    rows: usize,
    s0: f32,
    s1: f32,
    t0: f32,
    t1: f32,
    samples: u32,
) -> Vec<[f32; 3]> {
    if samples == 0 || cols == 0 || rows == 0 || grid.len() != cols * rows {
        return Vec::new();
    }
    let n = samples as usize + 1;
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(n * n);
    // Scratch for the inner Horner-rule pass: one collapsed point per
    // v-row at the current u sample.
    let mut col_pts: Vec<[f32; 3]> = Vec::with_capacity(rows);
    for sv in 0..n {
        let v = if n == 1 {
            0.0
        } else {
            t0 + (sv as f32 / (n - 1) as f32) * (t1 - t0)
        };
        for su in 0..n {
            let u = if n == 1 {
                0.0
            } else {
                s0 + (su as f32 / (n - 1) as f32) * (s1 - s0)
            };
            // Inner pass: Horner's rule in u across each v-row,
            // collapsing each row to a single point at the sample u.
            //
            //   row(u) = (((c_{degu,j} · u + c_{degu-1,j}) · u + …) · u
            //            + c_{0,j})
            col_pts.clear();
            for r in 0..rows {
                let row_start = r * cols;
                let mut acc = grid[row_start + cols - 1];
                for i in (0..cols - 1).rev() {
                    let cij = grid[row_start + i];
                    acc[0] = acc[0] * u + cij[0];
                    acc[1] = acc[1] * u + cij[1];
                    acc[2] = acc[2] * u + cij[2];
                }
                col_pts.push(acc);
            }
            // Outer pass: Horner's rule in v over the collapsed points.
            let mut acc = col_pts[rows - 1];
            for j in (0..rows - 1).rev() {
                let cj = col_pts[j];
                acc[0] = acc[0] * v + cj[0];
                acc[1] = acc[1] * v + cj[1];
                acc[2] = acc[2] * v + cj[2];
            }
            out.push(acc);
        }
    }
    out
}

/// Integer square root that returns `Some(r)` only when `n == r * r`
/// (i.e. `n` is a perfect square). Used to recover the square single-
/// patch control-grid dimension for a Cardinal `surf` whose `parm`
/// directives carry only the 2-value global parameter range.
fn isqrt_exact(n: usize) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let mut r = (n as f64).sqrt() as usize;
    // Guard against floating-point rounding on either side.
    while r * r > n {
        r -= 1;
    }
    while (r + 1) * (r + 1) <= n {
        r += 1;
    }
    if r * r == n { Some(r) } else { None }
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
    // `checked_add` / `checked_mul` are defensive — the public-facing
    // `flush_block` caller already filters degrees whose `(n+1)²`
    // overflows `usize`, but this helper is also reachable from future
    // call sites and the cost of the saturation check is negligible.
    let Some(n_plus_1) = (degree as usize).checked_add(1) else {
        return Vec::new();
    };
    let Some(expected_bmat) = n_plus_1.checked_mul(n_plus_1) else {
        return Vec::new();
    };
    if control_points.len() < n_plus_1 || bmat_u.len() != expected_bmat || step == 0 || samples == 0
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
    if let Some(s) = &prim_acc.usemap {
        prim.extras.insert(
            "obj:usemap".to_string(),
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

    // 2D trimming-curve (`curv2`) tessellation pass — the same sample
    // knob evaluates the parameter-space trimming / special /
    // connectivity curves (spec §"curv2") into `LineStrip` polylines on
    // a dedicated `obj:curves2` mesh. The directives still ride on
    // `Scene3D::extras["obj:freeform_directives"]` for verbatim
    // round-trip; the encoder filters the synthetic primitives out.
    let tessellated_curve2 = if options.curve_tessellation_samples > 0 {
        tessellate_curve2(&doc, options.curve_tessellation_samples)
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

    // Special-curve (`scrv`) tessellation pass (round 206) — evaluates
    // every `scrv` directive into a parameter-space LineStrip polyline
    // (spec §"Special curve", §"scrv u0 u1 curv2d u0 u1 curv2d …"). The
    // directives still ride on `Scene3D::extras["obj:freeform_directives"]`
    // for verbatim round-trip; the encoder filters the synthetic
    // primitives out via the shared `obj:tessellated_curve` sentinel.
    let tessellated_scrv = if options.curve_tessellation_samples > 0 {
        tessellate_scrv(&doc, options.curve_tessellation_samples)
    } else {
        Vec::new()
    };

    // Special-point (`sp`) synthetic-primitive pass (round 246) — gated
    // on the same `curve_tessellation_samples` knob the curv / scrv /
    // surf passes use, but the special-points pass doesn't sample at a
    // density; it emits exactly one [`Topology::Points`] primitive per
    // `sp` directive (lifted from the resolved `vp` parameter-vertex
    // pool, spec §"Special point", §"sp vp1 vp …"). The directives
    // still ride on `Scene3D::extras["obj:freeform_directives"]` for
    // verbatim round-trip; the encoder filters the synthetic primitives
    // out via the shared `obj:tessellated_curve` sentinel.
    let tessellated_sp = if options.curve_tessellation_samples > 0 {
        tessellate_special_points(&doc)
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

    if !tessellated_curve2.is_empty() {
        let mut mesh = Mesh::new(Some("obj:curves2".to_string()));
        for prim in tessellated_curve2 {
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

    if !tessellated_scrv.is_empty() {
        let mut mesh = Mesh::new(Some("obj:scrvs".to_string()));
        for prim in tessellated_scrv {
            mesh.primitives.push(prim);
        }
        scene.add_mesh(mesh);
    }

    if !tessellated_sp.is_empty() {
        let mut mesh = Mesh::new(Some("obj:sps".to_string()));
        for prim in tessellated_sp {
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
    // Spec §"maplib filename1 filename2 ..." — texture-map library
    // declarations. Emit each name on its own line (the spec accepts
    // a multi-name line but a one-per-line emit keeps re-encoded diffs
    // grep-friendly and matches the per-line `mtllib` emit above).
    if let Some(serde_json::Value::Array(list)) = scene.extras.get("obj:maplibs") {
        for entry in list {
            if let Some(s) = entry.as_str() {
                writeln!(out, "maplib {s}").unwrap();
            }
        }
    }

    // Spec §"shadow_obj filename" / §"trace_obj filename": top-level
    // directives that nominate companion files for shadow casting and
    // ray-traced reflections. The spec is silent on placement but the
    // worked examples in §"Examples" (cases 2 and 3) put them between
    // `mtllib` and the vertex pool, so we mirror that. `Scene3D::extras`
    // carries plain strings populated by the decoder; absent keys leave
    // the preamble unchanged.
    if let Some(serde_json::Value::String(name)) = scene.extras.get("obj:shadow_obj") {
        if !name.is_empty() {
            writeln!(out, "shadow_obj {name}").unwrap();
        }
    }
    if let Some(serde_json::Value::String(name)) = scene.extras.get("obj:trace_obj") {
        if !name.is_empty() {
            writeln!(out, "trace_obj {name}").unwrap();
        }
    }

    // Spec §"General statement" — replay any captured `call` /
    // `csh` lines in document order. Source position relative to the
    // polygonal section isn't preserved (see
    // `ObjDoc::general_directives` docstring), so we emit them once,
    // at the top of the preamble right after the companion-file
    // block. Empty arrays / non-string tokens are skipped lenient-loader
    // style; absent key leaves the preamble unchanged.
    if let Some(serde_json::Value::Array(generals)) = scene.extras.get("obj:general_directives") {
        for entry in generals {
            if let serde_json::Value::Array(toks) = entry {
                let parts: Vec<&str> = toks.iter().filter_map(|v| v.as_str()).collect();
                if parts.is_empty() {
                    continue;
                }
                writeln!(out, "{}", parts.join(" ")).unwrap();
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

            // Spec §"usemap map_name/off" — texture-map rendering
            // identifier. Emit verbatim from the round-trip extras
            // slot populated by the decoder. The literal string is
            // preserved (so `usemap off` re-emits as `usemap off`,
            // and `usemap MyTex` re-emits as `usemap MyTex`); the
            // same per-primitive emit pattern as `usemtl` above.
            if let Some(name) = prim.extras.get("obj:usemap").and_then(|v| v.as_str()) {
                writeln!(out, "usemap {name}").unwrap();
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
