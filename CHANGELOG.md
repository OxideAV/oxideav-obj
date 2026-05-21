# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cardinal (Catmull-Rom) + Taylor polynomial curve tessellation under
  `ObjDecoder::with_curve_tessellation(samples: u32)`. The decoder now
  evaluates `cstype cardinal` `curv` directives via the spec §"Cardinal"
  conversion to Bezier control points
  (`b0 = c1`, `b1 = c1 + (c2 − c0) / 6`, `b2 = c2 − (c3 − c1) / 6`,
  `b3 = c2`, then cubic Bezier blend) on a sliding 4-point window across
  the control polygon, producing C¹-continuous polylines that
  interpolate every interior control point exactly. Cardinal is cubic
  only per spec; non-cubic `deg` is silently rejected (the directive
  itself remains captured for round-trip).
  `cstype taylor` `curv` directives evaluate via Horner's-rule
  polynomial evaluation `P(t) = Σ_{i=0..n} c_i · t^i` per spec §"Taylor"
  (control points are the polynomial coefficients) with sampling across
  the `[u_min, u_max]` range supplied on the `curv` line. Both `rat
  cardinal` and `rat taylor` qualifiers are accepted but route to the
  same evaluator (the spec note says the unit-weight default is
  reasonable for Cardinal because its basis functions sum to 1, and
  explicitly that the rational form "does not make sense for Taylor").
  The resulting polylines land on the existing `"obj:curves"` synthetic
  mesh with `obj:tessellated_curve = true` / `obj:curve_kind`
  (`"cardinal"` / `"taylor"`) / `obj:curve_degree` (3 for Cardinal,
  the `deg` value for Taylor) / `obj:curve_u_range` / `obj:curve_samples`
  provenance extras. The encoder filters synthetic primitives out of
  the polygonal section so a re-encode replays the original
  `cstype cardinal` / `cstype taylor` blocks unchanged.
- B-spline / NURBS curve tessellation under
  `ObjDecoder::with_curve_tessellation(samples: u32)`. The decoder now
  evaluates `cstype bspline` and `cstype rat bspline` `curv` directives
  via the Cox-deBoor recursive basis-function formula (spec §"B-spline"),
  clipped against the `[x_n, x_{K+1}]` evaluation window of the knot
  vector supplied by the most-recent `parm u …` body statement. The
  resulting polyline lands on the existing `"obj:curves"` synthetic
  mesh with the same `obj:tessellated_curve = true` /
  `obj:curve_kind` (`"bspline"` / `"rat_bspline"`) /
  `obj:curve_degree` / `obj:curve_u_range` / `obj:curve_samples`
  provenance extras. Rational form (NURBS) uses the per-vertex 4th `w`
  weight and projects the weighted blend back to 3D, matching the
  spec's `Σ N_{i,n} · w_i · d_i / Σ N_{i,n} · w_i` formulation.
  Knot-vector length is validated against the spec condition
  `len == K + degree + 2` and incomplete curves are skipped silently
  (the directive itself remains captured for round-trip). The
  tessellator now does two-pass per-block traversal so the `curv`
  header (which precedes the `parm u` body statement per spec
  §"Specifying free-form curves/surfaces") still resolves its knot
  vector. Cardinal / Taylor / basis-matrix bases and `surf`
  2-parameter surfaces remain captured-only.
- `ObjDecoder::with_curve_tessellation(samples: u32)` evaluates every
  `cstype bezier` (and `cstype rat bezier`) `curv` directive via de
  Casteljau's algorithm at `samples + 1` uniformly-spaced parameter
  values, producing real `Topology::LineStrip` primitives on a synthetic
  mesh named `"obj:curves"`. Rational form uses the per-vertex 4th `w`
  weight (`v x y z w`) and projects the homogeneous blend back to 3D.
  Each tessellated primitive carries `obj:tessellated_curve = true`,
  `obj:curve_kind` (`"bezier"` / `"rat_bezier"`), `obj:curve_degree`,
  `obj:curve_u_range`, and `obj:curve_samples` in `extras` so consumers
  can filter / inspect derived geometry. `samples == 0` (the default)
  preserves the round 1-6 behaviour: directives ride as
  `Scene3D::extras["obj:freeform_directives"]` only, no synthetic mesh.
  The encoder skips synthetic curve primitives so re-encoding produces
  the original `cstype` / `curv` / `end` section unchanged.
- Source position pool round-trip: when an OBJ's free-form section
  (`curv` / `curv2` / `surf` / `bzp` / `bsp`) references positions by
  absolute index, those positions now ride on
  `Scene3D::extras["obj:positions"]` (and parallel
  `obj:position_weights` / `obj:position_colors` arrays for the
  extension widths) so the encoder re-emits the full `v` block in
  source order. Previously, positions only referenced by free-form
  directives were silently dropped on re-encode, which made absolute
  curve-control-point indices drift after a decode → encode → decode
  cycle. The fix is invisible to polygon-only OBJs; the extras are
  populated only when free-form directives that reference indices are
  present.

## [0.0.1](https://github.com/OxideAV/oxideav-obj/compare/v0.0.0...v0.0.1) - 2026-05-10

### Other

- Round 6: per-vertex colour extension + v 4th weight preservation
- round-5 status — Tf alt forms, typed refl, line strip/loop
- Round 5: refl -type sphere / cube_* typed reflection-map sets
- Round 5: promote single-l polylines to LineStrip / LineLoop topology
- Round 5: MTL Tf spectral / Tf xyz alternative forms
- Round 4: free-form geometry directives round-trip via Scene3D::extras
- Round 3: encoder rejoins polyline segment chains into one l line
- Round 3: MTL map_* option flags + d -halo dissolve
- Round 3: bevel / c_interp / d_interp / lod display attributes
- Round 3: p point elements + mg merging-group state-setting
- Round 2: multi-name groups, smoothing-group split, MTL extras, path loader, negative-index encoder

### Added

- `v x y z r g b` per-vertex colour extension (MeshLab / libigl /
  Meshroom / OpenCV de-facto). The decoder accepts `v` lines with 3, 4,
  6, or 7 floats — `xyz`, `xyzw` (rational weight per spec
  §"v x y z w"), `xyzrgb` (vertex-colour extension), or `xyzwrgb`
  (both). Colours land on `Primitive::colors[0]` as
  `[r, g, b, 1.0]`; rational weights land in
  `Primitive::extras["obj:vertex_weight"]`. A per-vertex bitmap in
  `Primitive::extras["obj:vertex_color_present"]` records *which*
  source vertices originally carried RGB so the encoder can re-emit
  the same 3-/4-/6-/7-token width on round-trip rather than
  fabricating synthetic white for vertices that didn't spell out
  colour. Mixed-colouring primitives round-trip with the partition
  preserved (some `v` lines stay 3-token, others go to 6). The
  loader rejects 5-float `v` lines as ambiguous (neither `xyzw` nor
  `xyzrgb` per any extant convention).
- Free-form geometry directives (`vp` parameter-space vertices,
  `cstype`, `deg`, `curv`, `curv2`, `surf`, `parm`, `trim`, `hole`,
  `scrv`, `sp`, `end`, plus the older superseded `bzp` / `bsp`
  patches per spec §"Superseded statements") are captured into
  `Scene3D::extras["obj:vp"]` (list of `[u, v, w]` 3-tuples in 1-based
  numbering parallel to `v` / `vt` / `vn`) and
  `Scene3D::extras["obj:freeform_directives"]` (sequence of
  `[keyword, arg1, arg2, …]` arrays preserving directive order and
  arguments verbatim). The encoder replays both after the polygonal
  section so a decode → encode round-trip is bit-stable for the
  free-form portion. No semantic interpretation — consumers that
  need to evaluate the curves/surfaces walk the captured directive
  sequence themselves; this crate guarantees lossless transit.
  `vp` lines are emitted with only as many coordinates as carry
  meaningful information (`vp u`, `vp u v`, or `vp u v w`).
- `p v1 v2 v3 …` point elements decode to a `Topology::Points`
  primitive (multi-vertex `p` lines pack onto one element list);
  mixing point and face/line elements under one `usemtl` splits into
  one primitive per topology.
- `mg <group_number> [res]` merging-group state-setting is preserved
  verbatim in `Primitive::extras["obj:merging_group"]`; a change
  mid-stream splits the primitive (mirrors `s` smoothing-group
  behaviour). The encoder re-emits an `mg <token>` line ahead of the
  affected elements.
- Display-attribute state-setters `bevel on/off`, `c_interp on/off`,
  `d_interp on/off`, and `lod <level>` are captured per-primitive in
  `Primitive::extras["obj:bevel"]` / `["obj:c_interp"]` /
  `["obj:d_interp"]` / `["obj:lod"]`. Mid-stream changes split the
  primitive so each one carries one consistent assignment per
  attribute.
- MTL `map_*` directive option flags (`-blendu`, `-blendv`, `-cc`,
  `-clamp`, `-bm`, `-boost`, `-mm`, `-o`, `-s`, `-t`, `-texres`,
  `-imfchan`, `-type`) are stripped out of the filename at parse
  time and preserved in `Material::extras["mtl:<map_name>:options"]`
  as an array of `"<flag> <args>"` strings. The encoder splices the
  saved options back ahead of the filename so a round-trip emits
  `map_Kd -clamp on path.png` rather than dropping the flags.
- MTL `d -halo factor` orientation-dependent dissolve is detected on
  parse, surfaced via `Material::extras["mtl:d_halo_factor"]`, and
  re-emitted as `d -halo <factor>` rather than the plain `d` form.
- Encoder rejoins contiguous `Topology::Lines` segment pairs into a
  single polyline `l v1 v2 v3 …` line whenever segment N's end index
  equals segment N+1's start index, rather than emitting one
  `l v1 v2` per pair (lossless for the typical decode→encode round
  trip of polyline-heavy OBJ inputs).
- A primitive with exactly one `l` element promotes to the more
  specific `Topology::LineStrip` (or `Topology::LineLoop` when the
  last vertex equals the first) instead of `Topology::Lines`. The
  encoder is symmetric: `LineStrip` emits the natural index list,
  `LineLoop` re-appends the first index so the round-trip parser
  re-detects the closure. Multi-`l` primitives and 2-vertex
  segments stay on `Topology::Lines` so the existing
  contiguous-chain re-emit path still applies.
- Multi-name `g` lines: `g name1 name2 …` captures every name as a
  distinct group entry in `Primitive::extras["obj:groups"]` and the
  encoder re-emits them on a single `g` line.
- Smoothing-group state-setting: a mid-object `s` change splits the
  current primitive so each `Primitive` carries a single
  `obj:smoothing_group` value; `s 0` and `s off` are preserved verbatim
  through the round-trip.
- MTL `Tf r g b` (transmission filter, with `g` / `b` defaulting to
  `r`) and `sharpness <value>` directives parse into
  `Material::extras` and re-emit on serialisation.
- MTL `Tf` alternative-form support — `Tf spectral file.rfl factor`
  lands as `Material::extras["mtl:Tf:spectral"] = {file, factor}` and
  `Tf xyz x y z` lands as `Material::extras["mtl:Tf:xyz"] = [x, y, z]`
  (with `y` / `z` defaulting to `x` per spec). The three forms are
  mutually exclusive on emit; the factor `1.0` default is omitted from
  the spectral re-emit so it matches the most common operator-written
  spelling.
- MTL `disp` ↔ `map_disp`, `decal` ↔ `map_decal`, and `refl` ↔
  `map_refl` keyword aliases land in extras with the original
  spelling preserved as the key.
- MTL `refl -type sphere` and `refl -type cube_*` typed reflection-
  map sets land as structured extras: `mtl:refl:sphere = {file,
  options?}` and `mtl:refl:cube = {cube_top, cube_bottom, cube_front,
  cube_back, cube_left, cube_right, cube_side}` (each face an
  optional `{file, options?}` entry). Six separate `cube_*` lines
  bundle into one cubemap rather than overwriting each other under
  the legacy single-string slot. The encoder re-emits one line per
  face / sphere with options spliced ahead of the filename, in a
  fixed face order so the round-trip diff is deterministic. The bare
  legacy `refl filename` form still lands in `mtl:refl` for
  backwards compatibility.
- `obj::parse_obj_from_path` convenience loader resolves `mtllib`
  references (single or multi-file per line) against the OBJ's parent
  directory; missing libraries surface a clean `Error::invalid` with
  the offending path.
- `ObjEncoder::with_negative_indices(true)` (and the underlying
  `obj::SerializeOptions::negative_indices`) emit face / line vertex
  indices in relative-from-end form (`f -3 -2 -1`) for round-trip
  parity with inputs that used negative indices.

### Initial scaffold

- Wavefront OBJ + companion MTL parser/serialiser implementing
  `oxideav_mesh3d::Mesh3DDecoder` / `Mesh3DEncoder` traits.
- OBJ decoder: `v` / `vt` / `vn` vertex data, `f` faces (1-based + negative
  indices, all four `v` / `v/vt` / `v//vn` / `v/vt/vn` syntaxes), `l` lines,
  `o` object split, `g` group, `s` smoothing-group capture (extras),
  `usemtl` material switch (one `Primitive` per switch), `mtllib` material
  library load, polygon fan triangulation with original-arity capture in
  `Mesh::extras["obj:original_face_arities"]`.
- OBJ encoder: per-mesh `o` directive, per-primitive `usemtl`, deduplicated
  `v` / `vt` / `vn` lists with shared 1-based indices, `f`-face emission
  matching the available attribute set, polygon re-emission when the
  matching `obj:original_face_arities` extra is present, `l` line elements
  for `Topology::Lines`.
- MTL decoder: `Ka` / `Kd` / `Ks` / `Ke` Phong colours, `Ns` / `Ni` / `d`
  / `Tr` / `illum` scalar parameters, `map_Kd` / `map_Ks` / `map_Ka` /
  `map_Bump` / `map_d` / `map_Ns` texture references, Wavefront-PBR
  extension (`Pr` roughness, `Pm` metallic, `Pc` clearcoat, `Ps` sheen,
  `map_Pr` / `map_Pm` PBR maps).
- MTL encoder: `newmtl` blocks with the same vocabulary; `d`-from-base-color
  alpha, `Pr` / `Pm` for PBR-aware rendering pipelines.
