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
  `v` / `w` extra components on UVs. The widely-deployed MeshLab /
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
- `s 1` / `s off` / `s 0` smoothing groups → preserved verbatim in
  `Primitive::extras["obj:smoothing_group"]`; a smoothing-group change
  mid-object splits the primitive so each one carries a single
  consistent assignment.
- `mtllib <file.mtl> [<file2.mtl> …]` and `usemtl <name>` — each
  `usemtl` switch starts a fresh `Primitive` so a multi-material OBJ
  becomes a `Mesh` with N primitives, each with its own `MaterialId`.
- Free-form geometry (`vp` parameter-space vertices, `cstype`,
  `deg`, `curv`, `curv2`, `surf`, `parm`, `trim`, `hole`, `scrv`,
  `sp`, `end`, plus superseded `bzp` / `bsp` patches) — captured
  verbatim into `Scene3D::extras["obj:vp"]` (1-based parallel vertex
  pool) and `Scene3D::extras["obj:freeform_directives"]` (sequence
  of `[keyword, arg1, arg2, …]` arrays). The encoder replays both
  after the polygonal section so a decode → encode round-trip
  preserves the directive order and arguments. Verbatim by default;
  opt-in tessellation of `curv` curves and Bezier / B-spline /
  Cardinal `surf` surfaces is available via
  `ObjDecoder::with_curve_tessellation(samples)` (see the per-round
  notes below).

The companion **MTL** parser/serialiser handles:

- Phong colours (`Ka` / `Kd` / `Ks` / `Ke`) → glTF `base_color`
  (from `Kd`) + `emissive_factor` (from `Ke`); `Ka` / `Ks` and the
  `Ns` exponent are stashed in `Material::extras` for round-trip.
- Transparency (`d` dissolve / `Tr = 1 - d`) → `AlphaMode::Blend`
  + `base_color.a`. The `d -halo factor` orientation-dependent
  variant is detected and re-emitted via
  `Material::extras["mtl:d_halo_factor"]`.
- Index of refraction (`Ni`) and illumination model (`illum`) → extras.
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
  option chunks (`-blendu`, `-bm`, `-mm`, `-clamp`, `-imfchan`, `-o`,
  `-s`, `-t`, `-texres`) are parsed out of the filename and surfaced
  via `Material::extras["mtl:<map_name>:options"]`; the encoder
  splices them back inline.
- Wavefront-PBR extension (`Pr` roughness, `Pm` metallic, `Pc`
  clearcoat, `Ps` sheen, `map_Pr` / `map_Pm`) → `Material::roughness`
  / `Material::metallic` / `metallic_roughness_texture`.

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

This crate was implemented strictly from those references — no
existing OBJ loader (tinyobj, assimp, blender io_scene_obj, three.js
OBJLoader) was consulted.

## Status

Round 1: polygonal subset + MTL Phong + Wavefront-PBR extension.
Round 2: multi-name `g` lines, smoothing-group state-setting (split-on-
change), `Tf` / `sharpness` / displacement-map round-trip, path-based
loader with `mtllib` resolution, and an opt-in negative-index encoder.
Round 3: `p` point elements, `mg` merging groups, `bevel` / `c_interp`
/ `d_interp` / `lod` display attributes, MTL `map_*` option flags
(`-blendu`, `-clamp`, `-bm`, …) preserved through round-trip, MTL
`d -halo factor`, encoder polyline rejoin.
Round 4: free-form geometry (`vp` parameter-space vertex pool plus
the verbatim `cstype` / `deg` / `curv` / `curv2` / `surf` / `parm` /
`trim` / `hole` / `scrv` / `sp` / `end` / `bzp` / `bsp` directive
sequence) — round-trips through `Scene3D::extras` without
tessellation.
Round 5: MTL `Tf spectral` / `Tf xyz` alternative transmission-filter
forms, `refl -type sphere` / `refl -type cube_*` typed reflection-map
sets bundled into `mtl:refl:sphere` / `mtl:refl:cube` extras, and
single-`l` polylines promoted to `Topology::LineStrip` /
`Topology::LineLoop` (with closure detection at decode time).
Round 6: per-vertex colour extension (`v x y z r g b`, MeshLab /
libigl / Meshroom de-facto) accepted on parse, populated on
`Primitive::colors[0]`, and re-emitted at the source's original
3-/4-/6-/7-token width via the `obj:vertex_color_present` bitmap.
The `v` 4th `w` weight component is now preserved through
`Primitive::extras["obj:vertex_weight"]` rather than silently dropped.
Round 7: opt-in Bezier curve tessellation —
`ObjDecoder::with_curve_tessellation(samples)` evaluates every
`cstype bezier` (and `cstype rat bezier`) `curv` directive via de
Casteljau's algorithm and emits a real `Topology::LineStrip`
primitive on a synthetic `"obj:curves"` mesh; the rational form
uses the per-vertex 4th `w` weight and projects back to 3D. Each
tessellated primitive carries provenance extras
(`obj:tessellated_curve`, `obj:curve_kind`, `obj:curve_degree`,
`obj:curve_u_range`, `obj:curve_samples`). The free-form directive
sequence still rides on `Scene3D::extras["obj:freeform_directives"]`
so re-encoding regenerates the original `cstype` / `curv` / `end`
section unchanged; the encoder filters synthetic curve primitives
out of the polygonal output. Free-form-section position pool now
rides on `Scene3D::extras["obj:positions"]` (plus parallel
`obj:position_weights` / `obj:position_colors`) so `curv` /
`surf` absolute-index references stay valid across a
decode → encode → decode cycle.
Round 8: B-spline / NURBS curve tessellation — the same
`with_curve_tessellation(samples)` knob now also evaluates
`cstype bspline` and `cstype rat bspline` `curv` directives via the
Cox-deBoor recursive basis-function formula (spec §"B-spline"),
clipped against the `[x_n, x_{K+1}]` evaluation window of the knot
vector supplied by the most-recent `parm u …` body statement.
Rational form uses the per-vertex 4th `w` weight (NURBS form) and
projects the weighted blend back to 3D. The tessellator now does
two-pass per-`cstype/end` block traversal so the `curv` header
(which comes before the `parm u …` body statement per spec) can
still resolve its knot vector. Knot-vector length is validated
against the spec condition `len == K + degree + 2` and curves with
incomplete data are skipped silently (the directive remains
captured for round-trip).
Round 9: Cardinal (Catmull-Rom) + Taylor polynomial curve
tessellation — `with_curve_tessellation(samples)` now also evaluates
`cstype cardinal` `curv` directives via the spec §"Cardinal"
conversion to Bezier control points (`b0 = c1`,
`b1 = c1 + (c2 − c0) / 6`, `b2 = c2 − (c3 − c1) / 6`, `b3 = c2`,
then cubic Bezier blend) on a sliding 4-point window, and `cstype
taylor` `curv` directives via Horner's-rule polynomial evaluation
`P(t) = Σ_{i=0..n} c_i · t^i` per spec §"Taylor". Cardinal is cubic
only (non-cubic `deg` is rejected, matching the spec's "only
defined for the cubic case" requirement); Taylor honours the
`[u_min, u_max]` parameter clip directly on the `curv` line.
Synthetic primitives carry the same `obj:tessellated_curve` /
`obj:curve_kind` (`"cardinal"` / `"taylor"`) / `obj:curve_degree` /
`obj:curve_u_range` / `obj:curve_samples` provenance and the
encoder filters them out so the source `cstype` / `curv` / `end`
block replays unchanged.
Round 10: basis-matrix curve tessellation — `with_curve_tessellation(samples)`
now also evaluates `cstype bmatrix` `curv` directives per spec
§"Basis matrix" using the user-supplied `(n + 1) × (n + 1)` basis
from `bmat u` (row-major, column index `j` varying fastest per
spec §"bmat u/v matrix") and the segment stride from
`step <stepu>` (spec §"step stepu stepv"). Each segment evaluates
`P(t) = Σ_i Σ_j B[i][j] · t^j · p_{base + i}` over the
control-point window `c_{base+1} .. c_{base+n+1}` (1-based, `base = i·stepu`).
The `bmat` and `step` keywords are now tracked alongside the other
free-form directives, so they round-trip verbatim through
`Scene3D::extras["obj:freeform_directives"]` and the encoder
replays the original `cstype bmatrix` block unchanged. Cubic
Bezier expressed as `cstype bmatrix` matches the closed-form
Bernstein evaluation; the Hermite spec example interpolates its
endpoints.
Round 11: Bezier `surf` surface tessellation — the same
`with_curve_tessellation(samples)` knob now also evaluates `surf`
elements under a `cstype bezier` (or `cstype rat bezier`) header
into a real `Topology::Triangles` grid on a synthetic mesh named
`"obj:surfaces"`, via the bivariate tensor-product de Casteljau
algorithm (spec §"Rational and non-rational curves and surfaces",
§"Bezier"). Control points are read in the spec's row-major
u-fastest order (§"Surface vertex data — control points": "i = 0
to K1 for j = 0, …"); the `surf` line's `v/vt/vn` references are
parsed for their leading position index (negative relative-from-end
indices honoured). A single patch of declared degree `deg degu degv`
needs exactly `(degu + 1) × (degv + 1)` control points; mismatched
counts (multi-patch grids, which Bezier can't decompose without a
`step` stride) are left captured-only. The surface is sampled at a
`(samples + 1) × (samples + 1)` lattice and triangulated CCW
(front = u-right, v-up per the spec note). Each synthetic primitive
carries provenance extras (`obj:tessellated_surface`,
`obj:surface_kind`, `obj:surface_degree`, `obj:surface_u_range`,
`obj:surface_v_range`, `obj:surface_samples`) plus the shared
`obj:tessellated_curve` sentinel so the encoder filters it out and
replays the original `cstype` / `deg` / `surf` / `parm` / `end`
block unchanged from `Scene3D::extras["obj:freeform_directives"]`.
The rational form uses each control point's 4th `w` weight and
projects the weighted blend back to 3D.
Round 12: B-spline / NURBS `surf` surface tessellation — the same
`with_curve_tessellation(samples)` knob now also evaluates `surf`
elements under a `cstype bspline` (or `cstype rat bspline`) header
into a `Topology::Triangles` grid on the synthetic `"obj:surfaces"`
mesh, via the bivariate tensor-product Cox-deBoor formula
`S(u, v) = Σ_i Σ_j N_{i,nu}(u) · N_{j,nv}(v) · d_{i,j}` (spec
§"B-spline"). The per-direction control-grid size comes from the
`parm u` / `parm v` knot vectors (`(len(parm u) − degu − 1) ×
(len(parm v) − degv − 1)`, spec §"B-spline" condition 6 applied
independently in u and v); the `surf` range is clipped against each
direction's `[x_n, x_{K+1}]` evaluation window. The rational (NURBS)
form blends the per-vertex `w` weights and projects via the weighted
denominator. Reuses the round-8 `bspline_basis` Cox-deBoor routine,
so a clamped quadratic patch matches the equivalent Bezier patch
sample-for-sample. Synthetic primitives carry the same
`obj:tessellated_surface` / `obj:surface_kind`
(`"bspline"` / `"rat_bspline"`) / `obj:surface_degree` /
`obj:surface_u_range` / `obj:surface_v_range` / `obj:surface_samples`
provenance and the encoder filters them out, replaying the original
`cstype` / `deg` / `surf` / `parm u` / `parm v` / `end` block
unchanged.
Round 13: Cardinal (Catmull-Rom) `surf` surface tessellation — the
same `with_curve_tessellation(samples)` knob now also evaluates `surf`
elements under a `cstype cardinal` (or `cstype rat cardinal`) header
into a `Topology::Triangles` grid on the synthetic `"obj:surfaces"`
mesh, via the bivariate tensor-product Cardinal evaluation
`S(u, v) = Σ_i Σ_j C_i(u) · C_j(v) · d_{i,j}` (spec §"Cardinal").
Each parametric direction reuses the spec's Cardinal→Bezier per-
segment conversion (`b0 = c1`, `b1 = c1 + (c2 − c0) / 6`,
`b2 = c2 − (c3 − c1) / 6`, `b3 = c2`) on a sliding 4-point window: the
u pass collapses every v-row, then a v pass runs over the collapsed
points. Cardinal is cubic-only per spec, so non-`3 3` degrees stay
captured-only; the control grid comes from the `parm u` / `parm v`
extents (`K = parm_count + 1` per direction) or, when `parm` carries
only the 2-value range, the square single patch (`√total`). A single
bicubic patch's parametric corners interpolate the interior 2×2
control block exactly (spec: "all but the first and last row and
column of control points are interpolated"). The `rat cardinal` form
routes to the same evaluator (unit-weight default). Synthetic
primitives carry the same `obj:tessellated_surface` / `obj:surface_kind`
(`"cardinal"`) / `obj:surface_degree` / `obj:surface_u_range` /
`obj:surface_v_range` / `obj:surface_samples` provenance and the
encoder filters them out, replaying the original
`cstype` / `deg` / `surf` / `parm` / `end` block unchanged.
Round 14 (depth): `cargo fuzz` harness — `fuzz/fuzz_targets/parse_obj.rs`
and `fuzz/fuzz_targets/parse_mtl.rs` drive attacker-controlled bytes
through every public decoder entry point and assert panic-freedom (no
panic / abort / debug-overflow / out-of-bounds index for any input).
The first 180-second `parse_obj` run found two real crashes that are
now fixed and pinned by regression tests in `tests/fuzz_regressions.rs`:
  * `parse_face_vertex` admitted an empty leading slot (so `f /1 /2`
    and `p /13` and `l /1 /2` produced `fv.v == 0` which then panicked
    on `(fv.v - 1) as usize` underflow downstream). Fix: require a
    non-empty position component at parse time so the `fv.v >= 1`
    invariant holds end-to-end.
  * `tessellate_surfaces` allocated `Vec::with_capacity(cols * rows)`
    for the Bezier control-grid pool without bounding the product
    against the declared control-vertex count, so `deg 111111` blew
    past available memory. Fix: `checked_add` / `checked_mul` on the
    grid extents and an early "expected != entry control-count" bail
    so the allocation never runs for mismatched grids. Same defence
    applied to the `cstype bmatrix` `(n + 1) × (n + 1)` basis-size
    check.
Subsequent 180-second runs against `parse_obj` (corpus grew to ~8.8k
discovered inputs) and `parse_mtl` (corpus grew to ~1.1k) finished
without further crashes / OOM / timeouts. The fuzz subcrate's
`Cargo.lock` is tracked for reproducible builds; transient
`fuzz/target/`, `fuzz/corpus/`, and `fuzz/artifacts/` paths sit on
`.gitignore`.

The `.mod` binary form remains out of scope; non-Bezier/non-B-spline/
non-Cardinal `surf` surfaces (Taylor / basis-matrix), multi-patch
Bezier surface decomposition, and trim/hole loop evaluation are the
remaining gaps.

## License

MIT. See [LICENSE](./LICENSE).
