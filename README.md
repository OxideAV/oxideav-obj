# oxideav-obj

Pure-Rust Wavefront OBJ + MTL 3D mesh codec. Implements the
`oxideav_mesh3d::Mesh3DDecoder` and `Mesh3DEncoder` traits, plugging into
the wider OxideAV codec ecosystem.

OBJ is the universal mesh-interchange format published by Wavefront
Technologies in the early 1990s as Appendix B of the Advanced Visualizer
manual. This crate implements the polygonal subset (the part that
modern loaders actually load):

- `v` / `vt` / `vn` vertex data — with the optional `w` 4th component on
  positions (rational weight per spec §"v x y z w" — preserved verbatim
  through `Primitive::extras["obj:vertex_weight"]`) and the optional
  `v` / `w` extra components on UVs. Texture coordinates round-trip
  byte-faithfully across the full 1D / 2D / 3D range per spec §"vt u v w"
  (both `v` and `w` are optional, defaulting to `0`): each `vt` line's
  source token width rides on `Scene3D::extras["obj:texcoord_widths"]`
  (parallel to the `obj:texcoords` source pool) and the otherwise-dropped
  3rd `w` depth coordinate on `["obj:texcoord_w"]`, so a `vt 0.25` (1D)
  re-emits as one token and a `vt u v w` (3D) keeps its depth rather than
  both collapsing onto the canonical `vt u v`. The widely-deployed MeshLab /
  libigl / Meshroom / OpenCV `v x y z r g b` per-vertex-colour
  extension is accepted at parse time, surfaced through
  `Primitive::colors[0]` (alpha pinned to 1.0 since the extension only
  spells out three channels), and re-emitted at the original token
  width — `xyz`, `xyzw`, `xyzrgb`, or `xyzwrgb` — using the
  `Primitive::extras["obj:vertex_color_present"]` bitmap so partial-
  colouring inputs preserve their per-vertex partition on round-trip
  rather than fabricating synthetic white. 5-float `v` lines are
  rejected as ambiguous.
- `f` faces in all four index syntaxes (`v`, `v/vt`, `v//vn`,
  `v/vt/vn`), with 1-based indexing and the negative-index relative-from-end
  shorthand. Polygons (n-gons) are fan-triangulated on read; the original
  per-face arity is stashed in `Mesh::extras["obj:original_face_arities"]`
  so the encoder can re-emit n-gons rather than triangles.
- `l` line elements → `Topology::LineStrip` for a single `l` element
  with three or more distinct vertices, `Topology::LineLoop` when
  the polyline closes (last vertex equals the first; redundant
  closing index dropped), or `Topology::Lines` for multi-`l`
  primitives and 2-vertex segments. The encoder picks the matching
  emit shape: `LineStrip` writes the natural index list,
  `LineLoop` re-appends the first index to spell out the closing
  edge, and `Lines` rejoins contiguous segment pairs into one
  polyline rather than emitting one `l v1 v2` per pair.
- `p` point elements → `Topology::Points`. Multi-vertex `p v1 v2 v3 …`
  lines pack onto one element list; mixing point and face/line elements
  under one `usemtl` splits into one primitive per topology.
- `mg <group_number> [res]` merging-group state-setting → preserved
  verbatim in `Primitive::extras["obj:merging_group"]`; a change
  mid-stream splits the primitive (mirrors `s` behaviour).
- `bevel on/off`, `c_interp on/off`, `d_interp on/off`, and
  `lod <level>` display attributes → captured per-primitive in
  `Primitive::extras["obj:bevel"]` / `["obj:c_interp"]` /
  `["obj:d_interp"]` / `["obj:lod"]`. Mid-stream changes split the
  primitive so each one carries one consistent assignment per
  attribute.
- `o <name>` → one `Mesh` per object directive (or a single mesh if the
  file has no `o`).
- `g name1 name2 …` → multiple group names per line, captured in
  `Primitive::extras["obj:groups"]` and re-emitted on a single `g` line.
  `g` is state-setting (spec §"Grouping"): a mid-stream membership change
  splits the primitive so the new groups apply only to subsequent
  elements, not the ones already accumulated.
- `s 1` / `s off` / `s 0` smoothing groups → preserved verbatim in
  `Primitive::extras["obj:smoothing_group"]`; a smoothing-group change
  mid-object splits the primitive so each one carries a single
  consistent assignment. The spec calls smoothing groups "a quick way
  to specify vertex normals", so
  `ObjDecoder::with_normal_generation(NormalGeneration::FromSmoothingGroups)`
  (opt-in) synthesises vertex normals for `vn`-less faces: an active
  group (`s 1`, …) gives smooth area-weighted averaged normals across
  shared vertices, while smoothing off (`s off` / `s 0` / no `s`) gives
  faceted per-face normals (vertices de-shared so the hard edge between
  adjacent faces survives). Explicit `vn` data supersede smoothing
  groups, so those primitives are untouched. Generated normals are
  flagged `Primitive::extras["obj:generated_normals"]` (`"smooth"` /
  `"flat"`) and the encoder skips them, keeping the round-trip
  `vn`-free.
- `mtllib <file.mtl> [<file2.mtl> …]` and `usemtl <name>` — each
  `usemtl` switch starts a fresh `Primitive` so a multi-material OBJ
  becomes a `Mesh` with N primitives, each with its own `MaterialId`.
- `maplib <lib1.map> [<lib2.map> …]` and `usemap <name>` / `usemap off`
  — the texture-map sibling of `mtllib` / `usemtl` per spec
  §"maplib" / §"usemap". Library names ride on
  `Scene3D::extras["obj:maplibs"]`; the per-primitive binding lands in
  `Primitive::extras["obj:usemap"]`. A mid-stream `usemap` switch
  opens a fresh primitive (same state-setter shape as `usemtl` / `s`
  / `mg`); a `usemtl` switch inherits the active `usemap` binding
  (the two operate independently per spec).
- Free-form geometry (`vp` parameter-space vertices, `cstype`,
  `deg`, `curv`, `curv2`, `surf`, `parm`, `trim`, `hole`, `scrv`,
  `sp`, `end`, plus the superseded `bzp` / `bsp` patches and the
  superseded `cdc` Cardinal-curve / `cdp` Cardinal-patch / `res`
  segment-count statements) — captured
  verbatim into `Scene3D::extras["obj:vp"]` (1-based parallel vertex
  pool) and `Scene3D::extras["obj:freeform_directives"]` (sequence
  of `[keyword, arg1, arg2, …]` arrays). The encoder replays both
  after the polygonal section so a decode → encode round-trip
  preserves the directive order and arguments. Each `vp` line's
  original token width (1, 2, or 3 coordinates per spec §"vp u v w")
  rides on a parallel `Scene3D::extras["obj:vp_widths"]` vector so the
  re-emit is byte-faithful even when a trailing coordinate is a genuine
  zero (`vp 0.5 0.0` for a `v = 0` surface special point, or
  `vp 0.5 0.5 0.0` for a zero-weight rational trim control point) rather
  than collapsing on a strip-trailing-zeros heuristic. Verbatim by default;
  opt-in tessellation of `curv` 3D space curves, `curv2` 2D
  parameter-space trimming curves, and Bezier / B-spline / Cardinal /
  Taylor / basis-matrix `surf` surfaces is available via
  `ObjDecoder::with_curve_tessellation(samples)` (see the per-round
  notes below). Tessellated surfaces carry smooth per-vertex normals
  (area-weighted face-normal averages over the emitted triangle lattice,
  front-oriented per spec §"surf": u-right / v-up CCW winding) so a
  downstream renderer or glTF exporter shades curved surfaces smoothly
  rather than flat; the normal buffer covers the base lattice plus
  trim-boundary and `scrv`-constraint vertices. A `ctech cparm <res>`
  curve-approximation directive (spec §"ctech technique resolution") in a
  `cstype … end` block overrides the uniform tessellation budget for that
  block: each `curv` is evaluated at `n = round(res × degree)`
  subdivisions per the spec's constant-parametric formula (`res 0` ⇒ a
  single line segment), with the effective count surfaced through
  `Primitive::extras["obj:curve_samples"]` /
  `["obj:curve_ctech_cparm_res"]`. The geometric `ctech cspace` /
  `ctech curv` techniques (iterative chord-length / curvature refinement)
  stay on the uniform budget. Its surface analog, `stech cparma ures vres`
  / `stech cparmb uvres` (spec §"stech technique resolution"), overrides
  the surface lattice density the same way: each `surf` patch is evaluated
  at `n = round(res × degree)` subdivisions per parametric direction, with
  the shared isotropic lattice driven from the finer (max) of the two
  per-direction counts so the coarser direction is never under-sampled
  (`cparma 0 0` ⇒ two triangles per patch; `cparmb` applies its one value
  to both directions). The effective count rides on
  `Primitive::extras["obj:surface_samples"]` and the source `[ures, vres]`
  pair on `["obj:surface_stech_cparm_res"]`; the geometric `stech cspace`
  / `stech curv` techniques stay on the uniform budget.

The companion **MTL** parser/serialiser handles:

- Phong colours (`Ka` / `Kd` / `Ks` / `Ke`) → glTF `base_color`
  (from `Kd`) + `emissive_factor` (from `Ke`); `Ka` / `Ks` and the
  `Ns` exponent are stashed in `Material::extras` for round-trip.
  Each of `Ka` / `Kd` / `Ks` accepts the three mutually-exclusive
  forms per spec — plain RGB (`r g b`, with g/b defaulting to r),
  `spectral file.rfl [factor]` (factor defaults to 1.0), and
  `xyz x [y z]` CIEXYZ (y/z defaulting to x). The spectral and
  xyz variants ride on sibling extras keys
  (`mtl:K{a,d,s}:spectral` / `mtl:K{a,d,s}:xyz`) so a re-emit
  reproduces the operator's chosen form; the `Kd spectral` /
  `Kd xyz` variants additionally suppress the canonical
  `Kd r g b` emit driven by `base_color`.
- Transparency (`d` dissolve / `Tr = 1 - d`) → `AlphaMode::Blend`
  + `base_color.a`. The `d -halo factor` orientation-dependent
  variant is detected and re-emitted via
  `Material::extras["mtl:d_halo_factor"]`.
- Index of refraction (`Ni`) and illumination model (`illum`) → extras.
  The raw `illum` integer lands in `Material::extras["mtl:illum"]`
  unchanged; for in-spec models 0–10, the parser additionally surfaces
  the spec's per-model "Properties that are turned on in the Property
  Editor" summary table (Wavefront MTL spec §"illum illum_#") as a
  decomposed object in `Material::extras["mtl:illum_props"]` with the
  nine stable flag keys `color` / `ambient` / `highlight` /
  `reflection` / `ray_trace` / `transparency_glass` /
  `transparency_refraction` / `fresnel` /
  `casts_shadow_on_invisible`. Out-of-spec integers (negative or
  `> 10`) keep the raw integer but omit `mtl:illum_props`. The
  decomposition is parse-time-only; the encoder still emits exactly
  one `illum N` line per material.
- Transmission filter — three mutually-exclusive forms per spec:
  `Tf r g b` (with `g`/`b` defaulting to `r`),
  `Tf spectral file.rfl factor` → `Material::extras["mtl:Tf:spectral"]`,
  and `Tf xyz x y z` → `Material::extras["mtl:Tf:xyz"]`.
- Reflection sharpness (`sharpness`) and displacement / decal /
  reflection maps (`disp` ↔ `map_disp`, `decal` ↔ `map_decal`,
  `refl` ↔ `map_refl`) round-trip via extras.
- Typed reflection maps — `refl -type sphere file` and the six-face
  `refl -type cube_top|cube_bottom|cube_front|cube_back|cube_left|cube_right`
  cubemap. Sphere lands as `Material::extras["mtl:refl:sphere"]`;
  the six face lines bundle into `Material::extras["mtl:refl:cube"]`
  (one entry per face) so they don't collapse onto each other under
  last-write-wins; the encoder re-emits one `refl -type <kind> ... file`
  line per face / sphere in deterministic order.
- Texture references (`map_Kd` → `base_color_texture`,
  `map_Bump` → `normal_texture`, `map_d` etc.) emitted as
  `ImageData::External { uri, mime: None }` — the caller resolves
  paths against the OBJ file's directory. Leading `-flag value`
  option chunks (`-blendu`, `-blendv`, `-cc`, `-bm`, `-boost`, `-mm`,
  `-clamp`, `-imfchan`, `-o`, `-s`, `-t`, `-texres`) are parsed out of
  the filename and surfaced via `Material::extras["mtl:<map_name>:options"]`;
  the encoder splices them back inline. The offset / scale / turbulence
  flags `-o` / `-s` / `-t` are variable-arity per spec (`u` mandatory,
  `v` / `w` optional): the parser consumes only the numeric run that
  follows each flag, so `map_Kd -o 0.2 logo.mpc` keeps `logo.mpc` as the
  filename rather than swallowing it as the omitted `v`. A parallel typed
  view rides on
  `Material::extras["mtl:<map_name>:options_typed"]` with stable
  primitive-valued keys per flag (`bool` for the `on`/`off` flags,
  `f64` for `bm` / `boost` / `texres`, `[base, gain]` for `mm`,
  `[u, v, w]` for `o` / `s` / `t`, `String` for `imfchan` and `type`)
  so consumers can read each option without re-parsing the raw token
  array. The typed key is parse-time-only; encoder output is still
  driven by the raw `:options` array.
- Wavefront-PBR extension scalars (`Pr` roughness, `Pm` metallic, `Pc`
  clearcoat, `Pcr` clearcoat-roughness, `Ps` sheen, `aniso` /
  `anisor` anisotropy) → `Material::roughness` / `Material::metallic`
  for `Pr` / `Pm`, with the rest stashed on extras. The metallic-roughness
  maps `map_Pr` / `map_Pm` drive `metallic_roughness_texture`; the
  remaining PBR map siblings `map_Ps` / `map_Pc` / `map_Pcr` /
  `map_aniso` / `map_anisor` — which glTF can't channel-map — ride
  verbatim on `Material::extras["mtl:<map>"]` (with `-flag value` option
  chunks parsed out) so a decode → encode round-trip is lossless for the
  full PBR map family rather than dropping the unrepresentable ones.
- `map_aat on` per-material texture anti-aliasing toggle (spec
  §"map_aat on") → boolean `Material::extras["mtl:map_aat"]`,
  round-tripped as the exact `on` / `off` token.

Both decoders are registered against `Mesh3DRegistry` under the
default-on `registry` cargo feature; drop the feature for a free-standing
build that only exposes `ObjDecoder` / `ObjEncoder` and the
`oxideav_mesh3d` standalone trait surface.

For path-based loading, `obj::parse_obj_from_path` resolves
`mtllib foo.mtl` references against the OBJ file's parent directory
(handles multiple MTL files per line). For round-trip mirroring of
inputs that used negative-from-end indices, the encoder accepts
`ObjEncoder::new().with_negative_indices(true)` (or the same flag on
`obj::SerializeOptions`).

## Sourcing

The Wavefront spec is mirrored in the OxideAV docs repository:

- `docs/3d/obj/wavefront-obj-spec.txt` — Martin Reddy plain-text
  Appendix B1 mirror.
- `docs/3d/obj/wavefront-mtl-spec.html` — Paul Bourke MTL mirror
  (carries the original `Copyright 1995 Alias|Wavefront, Inc.` notice).
- `docs/3d/obj/paulbourke-obj-reference.html` — Paul Bourke OBJ
  cross-check mirror.

This crate was implemented strictly from those references.

## Status

Production-ready for the polygonal OBJ + MTL feature set, with broad
free-form (curve/surface) support behind an opt-in tessellator.

**Polygonal core:** `v` / `vt` / `vn` (with optional `w` weight and the
`v x y z [w] r g b` per-vertex-colour extension), `f` faces in all four
index syntaxes (1-based, negative-from-end), `l` line elements
(`LineStrip` / `LineLoop` / `Lines`), `p` point elements, `o` objects,
`g` groups (multi-name), `s` smoothing groups, `mg` merging groups,
`bevel` / `c_interp` / `d_interp` / `lod` display attributes,
`mtllib` / `usemtl`, and `maplib` / `usemap`. State-setter directives
split a primitive on mid-stream change; original n-gon arities are
preserved so the encoder re-emits polygons rather than triangles. With
`ObjDecoder::with_normal_generation(…)`, `vn`-less faces get vertex
normals synthesised from their smoothing-group state (smooth-averaged
inside a group, faceted when smoothing is off) for direct rendering.

**MTL:** Phong colours (`Ka` / `Kd` / `Ks` / `Ke`, each in plain-RGB /
`spectral` / `xyz` forms), transparency (`d` / `Tr`, including
`d -halo`), `Ni`, `illum` (with the spec property-table decomposition
for in-spec models 0–10), `Tf` (all three forms), `sharpness`,
displacement / decal / reflection maps (`refl -type sphere` / cube),
texture references with full `-flag value` option parsing (both raw and
typed views), the Wavefront-PBR extension (`Pr` / `Pm` / `Pc` / `Pcr` /
`Ps` / `aniso` / `anisor` scalars; `map_Pr` / `map_Pm` metallic-roughness
maps; `map_Ps` / `map_Pc` / `map_Pcr` / `map_aniso` / `map_anisor`
verbatim-round-trip maps), and `map_aat`.

**Free-form geometry:** `vp`, `cstype`, `deg`, `curv`, `curv2`, `surf`,
`parm`, `trim`, `hole`, `scrv`, `sp`, `con`, `ctech` / `stech`,
`call` / `csh`, `shadow_obj` / `trace_obj`, and the superseded
`bzp` / `bsp` / `cdc` / `cdp` / `res` statements all round-trip
verbatim. With `ObjDecoder::with_curve_tessellation(samples)`, `curv` /
`curv2` curves and `surf` surfaces of every basis kind — Bezier,
B-spline / NURBS, Cardinal (Catmull-Rom), Taylor, and basis-matrix
(`bmatrix`) — are evaluated to real `LineStrip` / `Triangles` geometry
on synthetic meshes, including multi-patch decomposition, `trim` / `hole`
sub-cell boundary re-meshing, `scrv` special-curve triangle-edge
embedding, `sp` special points, and `con` connectivity seams. The
superseded `cdc` (Cardinal curve) and `bzp` (16-point bicubic Bezier
patch) statements tessellate too — to `obj:superseded_curves` /
`obj:superseded_surfaces` meshes — reusing the modern Cardinal / Bezier
evaluators, with the superseded `res useg vseg` statement modulating
their subdivision density (segment count clamped to the spec's 3..=120
range). A block-local `ctech cparm` (curves) or `stech cparma` /
`cparmb` (surfaces) directive overrides the uniform subdivision budget
with the spec's constant-parametric `n = round(res × degree)` density. The
triangulated surfaces carry smooth per-vertex normals for direct
smooth-shaded rendering. Typed
decompositions of `parm` / `con` / `sp` / `trim`-loop / `ctech` /
`stech` body statements and of the superseded `bzp` / `bsp` / `cdc` /
`cdp` geometry statements (`Scene3D::extras["obj:superseded"]`, paired
with the active `res` segments) ride alongside the verbatim channel for
consumers that don't want to re-parse positional tokens.

**Comments:** the file's leading comment block (spec §"Comments")
round-trips verbatim — exporter banners (`# Blender v2.93.0 OBJ File:`,
Maya / Meshroom) and the spec's `# 4 vertices` count comments are captured
into `Scene3D::extras["obj:header_comments"]` and re-emitted in place of
the synthetic `# OBJ generated by oxideav-obj` banner. Internal blank
separator lines are preserved; trailing blanks are dropped so repeated
round-trips stay a fixed point. Comments after the first directive aren't
positionally anchored by the line-oriented parser and are discarded.

The encoder reaches a textual fixed point after one round-trip: a
600-seed generative property test (`tests/roundtrip_fixed_point_property.rs`)
asserts `serialize(parse(serialize(parse(x)))) == serialize(parse(x))`
across documents mixing every directive family. This caught a `vt`
index-fidelity bug: two `vt` lines sharing a `[u, v]` value but different
source indices (a 1D `vt 0` padded to `[0, 0]` and a `vt 0 0`) collapse to
one UV in the typed model, so a face referencing the later duplicate was
re-resolved by value to the first slot (`f v/10` → `f v/3`). The decoder
now records the per-vertex source index in
`Primitive::extras["obj:vt_src_index"]` whenever the pool holds a duplicate
value, and the encoder restores the exact slot.

A `cargo fuzz` harness (`fuzz/fuzz_targets/parse_obj.rs` and
`parse_mtl.rs`) asserts panic-freedom across the public decoder entry
points; regression cases are pinned in `tests/fuzz_regressions.rs`.

The `.mod` binary form remains out of scope.

## License

MIT. See [LICENSE](./LICENSE).
