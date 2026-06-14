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
  `sp`, `end`, plus superseded `bzp` / `bsp` patches) — captured
  verbatim into `Scene3D::extras["obj:vp"]` (1-based parallel vertex
  pool) and `Scene3D::extras["obj:freeform_directives"]` (sequence
  of `[keyword, arg1, arg2, …]` arrays). The encoder replays both
  after the polygonal section so a decode → encode round-trip
  preserves the directive order and arguments. Verbatim by default;
  opt-in tessellation of `curv` 3D space curves, `curv2` 2D
  parameter-space trimming curves, and Bezier / B-spline / Cardinal /
  Taylor / basis-matrix `surf` surfaces is available via
  `ObjDecoder::with_curve_tessellation(samples)` (see the per-round
  notes below).

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
  option chunks (`-blendu`, `-bm`, `-mm`, `-clamp`, `-imfchan`, `-o`,
  `-s`, `-t`, `-texres`) are parsed out of the filename and surfaced
  via `Material::extras["mtl:<map_name>:options"]`; the encoder
  splices them back inline. A parallel typed view rides on
  `Material::extras["mtl:<map_name>:options_typed"]` with stable
  primitive-valued keys per flag (`bool` for the `on`/`off` flags,
  `f64` for `bm` / `boost` / `texres`, `[base, gain]` for `mm`,
  `[u, v, w]` for `o` / `s` / `t`, `String` for `imfchan` and `type`)
  so consumers can read each option without re-parsing the raw token
  array. The typed key is parse-time-only; encoder output is still
  driven by the raw `:options` array.
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
Round 14: Taylor polynomial `surf` surface tessellation — the same
`with_curve_tessellation(samples)` knob now also evaluates `surf`
elements under a `cstype taylor` (or `cstype rat taylor`) header into
a `Topology::Triangles` grid on the synthetic `"obj:surfaces"` mesh,
via the bivariate tensor-product Horner-rule polynomial evaluation
`S(u, v) = Σ_i Σ_j c_{i,j} · u^i · v^j` (spec §"Taylor"). Control
points are the polynomial coefficients laid out row-major u-fastest
(spec §"Surface vertex data — control points"); a single Taylor
patch of declared degree `deg degu degv` needs exactly
`(degu + 1) × (degv + 1)` coefficient vectors. The `surf s0 s1 t0 t1`
range supplies the global parameter clip; Taylor surfaces evaluate
against the raw parameter values directly (not a normalised `[0, 1]`
re-parameterisation). The spec note in §"Free-form curve/surface
body statements" says the rational form "does not make sense for
Taylor", so `rat taylor` routes to the same evaluator without per-
vertex weight blending. Synthetic primitives carry the same
`obj:tessellated_surface` / `obj:surface_kind` (`"taylor"`) /
`obj:surface_degree` / `obj:surface_u_range` / `obj:surface_v_range`
/ `obj:surface_samples` provenance and the encoder filters them out,
replaying the original `cstype` / `deg` / `surf` / `parm` / `end`
block unchanged.
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
Round 182: basis-matrix `surf` surface tessellation — the same
`with_curve_tessellation(samples)` knob now also evaluates `surf`
elements under a `cstype bmatrix` (or `cstype rat bmatrix`) header
into a `Topology::Triangles` grid on the synthetic `"obj:surfaces"`
mesh, via the bivariate tensor-product polynomial
`S(u, v) = Σ_a Σ_b (Σ_p B_u[a][p] · u^p) (Σ_q B_v[b][q] · v^q) ·
c_{base_u + a, base_v + b}` (spec §"Basis matrix",
§"bmat u/v matrix", §"step stepu stepv"). Per-direction basis
matrices come from `bmat u` / `bmat v` (row-major, column index
varying fastest); per-direction segment strides come from
`step stepu stepv`. The per-direction control-grid extent inverts the
spec relation `parm = (K − n) / s + 2` to `K = (parm − 2) · s + n + 1`,
applied independently in u and v ("For surfaces, the above description
applies independently to each parametric direction."). Multi-patch
grids decompose into per-segment patch windows starting at
`(seg_u · stepu, seg_v · stepv)`, so the spec §"Examples" cubic
Bezier-as-bmatrix surface single-patch case matches the equivalent
`cstype bezier` patch sample-for-sample. The `rat bmatrix` qualifier
routes to the same evaluator without per-vertex weight blending
(matches the round-10 1D curve behaviour). Synthetic primitives carry
the same `obj:tessellated_surface` / `obj:surface_kind` (`"bmatrix"`) /
`obj:surface_degree` / `obj:surface_u_range` / `obj:surface_v_range` /
`obj:surface_samples` provenance and the encoder filters them out,
replaying the original `cstype` / `deg` / `bmat u` / `bmat v` /
`step` / `parm u` / `parm v` / `surf` / `end` block unchanged.
Round 201: surface trim/hole clipping — the same
`with_curve_tessellation(samples)` knob now also applies the
`trim u0 u1 curv2d …` (outer) and `hole u0 u1 curv2d …` (inner)
trimming loops declared inside a `surf` block (spec §"Trimming
Loops", §"trim", §"hole") to the tessellated surface mesh. Every
`curv2` referenced by an enclosing `trim` / `hole` is resolved to its
parameter-space `(u, v)` polyline (via the same Bezier / B-spline /
Cardinal / Taylor / basis-matrix evaluator the round-188 stand-alone
`curv2` path uses), and the per-`trim` / per-`hole` segments are
concatenated into a closed polygon. Each `(samples + 1)²` lattice
vertex of the surface is then point-in-polygon-tested via Jordan-
curve ray casting: a triangle is kept iff all three vertices lie
inside at least one trim loop (or there are no trim loops — spec:
"If no trim or hole statements are specified, then the surface is
trimmed at its parameter range") AND outside every hole loop. This
is a conservative clip — boundary cells whose corners straddle a
loop edge are dropped wholesale rather than sub-cell re-meshed, so
the trim edge stays jagged at the lattice grain. The free-form
directive sequence still rides on
`Scene3D::extras["obj:freeform_directives"]` so a decode → encode
cycle replays the original `cstype` / `deg` / `surf` / `parm` /
`trim` / `hole` / `end` block verbatim; the encoder filters the
synthetic clipped surface out via the shared
`obj:tessellated_curve` sentinel. Per-clipped primitive,
provenance lands on `obj:surface_trimmed = true`,
`obj:surface_trim_loops` (count), and `obj:surface_hole_loops`
(count). Curv2 references on `trim` / `hole` are 1-based global
(spec §"trim u0 u1 curv2d" — "This curve must have been previously
defined with the curv2 statement"); a one-pass walk over
`freeform_directives` resolves every curv2 polyline up-front so a
`trim` declared in one block can reference a `curv2` first defined
in any earlier block.
Round 188: 2D trimming-curve (`curv2`) tessellation — the same
`with_curve_tessellation(samples)` knob now also evaluates every
`curv2` directive (the parameter-space curve referenced by
`trim` / `hole` / `scrv`, spec §"curv2") into a `Topology::LineStrip`
polyline on a new synthetic mesh named `"obj:curves2"`. A `curv2`
references `vp` parameter vertices (spec §"vp u v w") and lies in the
2D parameter space of the surface it trims, so each `vp (u, v)` is
lifted into a flat `[u, v, 0.0]` control point and run through the
same Bezier / B-spline / Cardinal / Taylor / basis-matrix evaluators
the 3D `curv` path uses — the sampled `x`/`y` are the parameter-space
coordinates, `z` stays `0.0`. Unlike `curv`, a `curv2` line carries
no inline `u0 u1`; the B-spline evaluation window comes from the
block's `parm u` knot vector. The optional 3rd `vp` coordinate is the
rational weight (default `1.0`; the `vp` `0.0` padding default reads
back as `1.0` for the rational forms). Negative `curv2` indices
resolve relative-from-end against the `vp` pool (spec §"Special point"
example `curv2 -6 -5 …`). Synthetic primitives carry the shared
`obj:tessellated_curve` sentinel plus a `obj:curve2` marker and the
`obj:curve_kind` / `obj:curve_degree` / `obj:curve_u_range` /
`obj:curve_samples` provenance; the encoder filters them out and
replays the original `cstype` / `curv2` / `parm` / `end` block
unchanged from `Scene3D::extras["obj:freeform_directives"]`.
Round 206: special-curve (`scrv`) tessellation — the same
`with_curve_tessellation(samples)` knob now also evaluates every
`scrv` directive (spec §"Special curve", §"scrv u0 u1 curv2d u0 u1
curv2d …") into a parameter-space `Topology::LineStrip` polyline on
a new synthetic mesh named `"obj:scrvs"`. A `scrv` shares the
`(u0, u1, curv2d)` triple shape `trim` / `hole` use, but unlike
those it is **not** a closed loop — the spec describes it as a
"sequence of curves which lie on a given surface to build a single
special curve" that must appear as a sequence of triangle edges in
the surface's final triangulation. This round emits the special
curve as a stand-alone parameter-space polyline so consumers can
resolve it without re-walking the directive stream; round 290
additionally embeds it as actual triangle edges on the `obj:surfaces`
mesh (see below). The `curv2d` references
are 1-based global per spec ("This curve must have been previously
defined with the curv2 statement"), resolved against the same
`collect_all_curv2_polylines` pre-pass the round-201 trim/hole
clipper uses, so a `scrv` declared in one block can still reference
a `curv2` first defined in any earlier block. Segments whose
referenced `curv2` failed to tessellate (incomplete block state,
missing knot vector, …) are silently dropped; the surrounding
`scrv` still produces a partial polyline if at least two vertices
survive across the successfully-resolved segments. Per-`scrv`
primitives carry the shared `obj:tessellated_curve` sentinel plus an
`obj:scrv` marker, an `obj:scrv_segments` count, and an
`obj:scrv_curv2_refs` array of `[curv2d_index, u0, u1]` provenance
triples in source order; the encoder filters them out and replays
the original `cstype` / `surf` / `scrv` / `end` block unchanged
from `Scene3D::extras["obj:freeform_directives"]`.
Round 218: multi-patch Bezier `surf` surface decomposition — the same
`with_curve_tessellation(samples)` knob now also evaluates `surf`
elements under a `cstype bezier` (or `cstype rat bezier`) header
whose control mesh spans more than one Bezier patch per parametric
direction. Spec §"Bezier" gives the per-direction control count as
`K = degu × (parm_u_count − 1)` (inverting "the number of global
parameter values given with the parm statement must be K/n + 1"),
and spec §"Surface vertex data — Control points" lays the global
control mesh out "as if the surface were a single large patch" with
adjacent patches sharing their boundary row / column. Each lattice
sample maps to a global parameter `(u_g, v_g) ∈ [0, patches_u] ×
[0, patches_v]`; its integer part selects the patch, fractional part
is the local Bezier parameter `t ∈ [0, 1]` for tensor-product de
Casteljau on the active `(degu + 1) × (degv + 1)` sub-window. The
single-patch case (`parm` length 2 per direction, the common form)
collapses to the legacy single-`sample_bezier_surface` path. The
rational form blends the per-vertex `w` weights through the same
sub-window and projects via the weighted denominator. Synthetic
primitives gain a new `obj:surface_patches = [patches_u, patches_v]`
provenance extra when either count exceeds 1, so downstream
consumers can recognise the patch seams inside the otherwise-
uniform triangle lattice; single-patch surfaces still omit the
marker. Multi-patch grids whose control count doesn't satisfy the
spec equality `K = degu × patches_u` stay captured-only, matching
the existing single-patch mismatch behaviour.

Round 223: approximation-technique directives + companion-object
references — four previously-dropped spec-defined directives now
round-trip.

Round 273: typed decomposition of the `trim` / `hole` / `scrv` loop
body statements per spec §"Trimming loops and holes" / §"trim u0 u1
curv2d …" / §"hole u0 u1 curv2d …" and §"Special curve" / §"scrv u0 u1
curv2d …" — the three loop statements all share the identical
repeating-triple body shape. Parallel to the verbatim
`obj:freeform_directives` channel (which still carries every line for
round-trip), a parse-time-only typed view now lands on
`Scene3D::extras["obj:trim_loops"]` as an array of objects with the
four stable, lowercase, underscore-separated keys `loop_kind` /
`element_kind` / `cstype` / `segments`. The `loop_kind` is exactly
`"trim"` / `"hole"` / `"scrv"`; the `element_kind` is the directive
that opened the enclosing `cstype … end` block (`"surf"` for the
spec-legal host, `"unknown"` outside a surface block); the `cstype`
slug reuses the `parm` / `ctech` / `stech` disambiguation table
(`"bezier"` / `"rat_bezier"` / `"bspline"` / `"rat_bspline"` /
`"cardinal"` / `"taylor"` / `"bmatrix"`, or `"unknown"`). The
`segments` array decomposes every `(u0, u1, curv2d)` triple in source
order — `u0` / `u1` as `f64`, `curv2d` as `i64` (negative-from-end
references per spec §"Examples" case 8 echoed as-is without
resolution). A line whose argument count isn't a positive multiple of
three, or any of whose tokens fail to parse, drops from the typed view
without failing the parse; the verbatim channel stays the encoder's
source of truth. Mirrors the lossy-on-malformed policy of the existing
`sp` / `con` / `parm` typed views.
Round 266: typed decomposition of the `ctech` / `stech` approximation-
technique directives per spec §"ctech technique resolution" + §"stech
technique resolution". Parallel to the verbatim `obj:freeform_directives`
channel (which still carries every `ctech` / `stech` line for
round-trip), a parse-time-only typed view now lands on
`Scene3D::extras["obj:approximations"]` as an array of objects with the
four stable, lowercase, underscore-separated keys `element_kind` /
`technique` / `parameters` / `cstype`. The `element_kind` is exactly
`"curve"` for a `ctech` line and `"surface"` for an `stech` line per
spec ("specifies a curve approximation technique" / "specifies a
surface approximation technique"). The `technique` is the spec's
sub-form slug — one of `"cparm"` / `"cspace"` / `"curv"` for curves
and `"cparma"` / `"cparmb"` / `"cspace"` / `"curv"` for surfaces.
The `parameters` array is the parsed `f64` resolution arguments in
source order, with the spec-defined arities `cparm`/`cspace`/`cparmb`
= 1 and `curv`/`cparma` = 2; a malformed parameter token (or wrong
argument arity, or an unrecognised technique slug) drops the line from
the typed view without failing the parse — the verbatim channel still
replays the line byte-faithful. The `cstype` slug reuses the existing
`parm` typed view's disambiguation table (one of `"bezier"` /
`"rat_bezier"` / `"bspline"` / `"rat_bspline"` / `"cardinal"` /
`"taylor"` / `"bmatrix"`); lines that sit outside any open
`cstype … end` block still surface in the typed view with
`cstype = "unknown"` so consumers can read the resolution parameters.
The encoder is still driven by the verbatim channel so the typed view
exists purely to spare consumers from re-parsing the per-technique
positional tokens.

Round 254: typed decomposition of the `parm u …` / `parm v …` body
statement per spec §"parm u/v" + §"Free-form curve/surface body
statements". Parallel to the verbatim `obj:freeform_directives`
channel (which still carries every `parm` line for round-trip), a
parse-time-only typed view now lands on
`Scene3D::extras["obj:parms"]` as an array of objects with the four
stable, lowercase, underscore-separated keys `direction` /
`element_kind` / `cstype` / `values`. The `direction` is exactly
`"u"` or `"v"` per the only two values the spec defines; the
`element_kind` (`"curv"` / `"curv2"` / `"surf"`) is pinned by the
most recent `curv` / `curv2` / `surf` directive inside the
surrounding `cstype … end` block; the `cstype` slug carries the
recognised type from the enclosing `cstype` header (one of
`"bezier"` / `"rat_bezier"` / `"bspline"` / `"rat_bspline"` /
`"cardinal"` / `"taylor"` / `"bmatrix"`), or `"unknown"` when the
declared type isn't one of those names. `values` is the parsed
array of `f64` — the global parameters for Bezier / Cardinal /
Taylor / basis-matrix elements, or the knot vector for B-spline /
NURBS elements per the spec's twin role for the `parm` keyword. The
encoder is still driven by the verbatim channel so the typed view
exists purely to spare consumers from walking the directive
sequence to pair every `parm` line with its enclosing `cstype` block
+ element kind. Lines whose direction token isn't exactly `"u"` /
`"v"`, or that sit outside any element (no `curv` / `curv2` / `surf`
seen since the last `cstype`), drop from the typed view without
failing the parse (the verbatim channel still replays them
byte-faithful). Non-numeric value tokens drop from the per-line
`values` array — mirrors the lenient-on-malformed policy of the
existing `sp` / `con` typed views.

Round 251: typed decomposition of the `con` connectivity statement
per spec §"Connectivity between free-form surfaces" / §"con surf_1
q0_1 q1_1 curv2d_1 surf_2 q0_2 q1_2 curv2d_2". Parallel to the
verbatim `obj:freeform_directives` channel (which still carries every
`con` line for round-trip), a parse-time-only typed view now lands
on `Scene3D::extras["obj:connectivity"]` as an array of objects with
the eight stable, lowercase, underscore-separated keys `surf_1` /
`q0_1` / `q1_1` / `curv2d_1` / `surf_2` / `q0_2` / `q1_2` /
`curv2d_2`. Integer slots (`surf_*`, `curv2d_*`) land as `i64`;
parameter slots (`q0_*`, `q1_*`) land as `f64`. The encoder is still
driven by the verbatim channel so the typed view exists purely to
spare consumers a second pass over the eight positional tokens. Lines
that don't carry exactly eight arguments, or whose integer / float
slots fail to parse, drop from the typed view without failing the
parse (the verbatim channel still replays them byte-faithful).
Negative indices in the `surf_*` / `curv2d_*` slots are echoed as-is —
the typed view doesn't resolve them against the surface / curv2
streams because surfaces aren't numbered in the captured directive
sequence; consumers that want negative-from-end semantics walk the
typed value through their own resolver.

Round 246: typed decomposition of the `sp` (special-point) body
statement per spec §"Special point", §"sp vp1 vp …". The verbatim
free-form-directives channel still carries every `sp` line for round-
trip, but a parse-time-only typed view now lands on
`Scene3D::extras["obj:special_points"]` as an array of objects with the
stable keys `element_kind` / `vp_index_1based` / `u` / `v`, in source
order. The element kind is decided by the directive that opened the
enclosing `cstype` … `end` block: `curv` keeps `v = null` (spec: "For
space curves and trimming curves, the parameter vertices must be 1D");
`curv2` surfaces both `u` and `v` because the spec describes a
trimming-curve special point as "essentially the same as a special
point on the surface it trims"; `surf` carries both components (spec:
"For surfaces, the parameter vertices must be 2D"). A companion
synthetic `Topology::Points` primitive lands on a new `"obj:sps"` mesh
under the same `with_curve_tessellation(samples)` knob the other
free-form passes use, one primitive per `sp` directive, with positions
lifted as `[u, v_or_0, 0.0]` plus per-primitive provenance extras
(`obj:special_point` marker, `obj:special_point_element_kind` string,
`obj:special_point_vp_refs` array of resolved 1-based vp indices). The
synthetic primitives carry the shared `obj:tessellated_curve` sentinel
so the encoder's existing `is_tessellated_curve` filter drops them on
re-emit; the `sp` line itself replays unchanged from the
`obj:freeform_directives` array. Negative `vp` references resolve
relative-from-end per the surrounding free-form pattern; references
outside the live `vp` pool (including `0`) drop silently from both the
typed view and the synthetic primitive without failing the parse, since
the verbatim directive replay handles round-trip independently. `sp`
lines outside any open `cstype` block are omitted from the typed view
(no enclosing element kind to resolve against) but still appear in the
verbatim replay.

Round 243: OBJ rendering-identifier pair `maplib` / `usemap` per spec
§"maplib filename1 filename2 ..." and §"usemap map_name/off". Both
are siblings to the already-supported material identifiers (`mtllib` /
`usemtl`) but cover the texture-map library rather than the material
library. `maplib lib1.map lib2.map ...` lines land on
`Scene3D::extras["obj:maplibs"]` as a verbatim string array (same
de-duplication policy as `mtllib` — a name that appears twice on the
same line or repeats on a later `maplib` line is suppressed). The
per-primitive binding from `usemap <name>` or `usemap off` lands on
`Primitive::extras["obj:usemap"]`. State-setter semantics mirror
`usemtl`: a mid-stream `usemap` switch opens a fresh primitive that
inherits the active groups, smoothing / merging group, display
attributes, and `usemtl` material. A `usemtl` switch likewise
inherits the active `usemap` binding (the two operate independently
per spec — one selects a material, the other a texture-map
definition). The encoder replays `maplib` after `mtllib` (one
line per unique name to keep diffs grep-friendly) and `usemap` after
`usemtl` (one line per primitive carrying the binding). Documents
that don't carry either directive produce neither extras key and
neither encode line.

Round 240: typed decomposition of `map_*` option flags per MTL spec
§"Options for texture map statements". Parallel to the existing raw
`mtl:<map>:options` string array (which still drives encoder round-
trip), a typed view lands on `mtl:<map>:options_typed` whose stable
lowercase keys carry primitive values: `blendu` / `blendv` / `clamp`
/ `cc` decode to `bool` (`on` → `true`, `off` → `false`); `bm` /
`boost` / `texres` decode to `f64`; `mm` decodes to a two-element
`[base, gain]` array; `o` / `s` / `t` decode to a three-element
`[u, v, w]` array; `imfchan` and `type` decode to a `String` over
their spec-defined alphabets (`r | g | b | m | l | z` and `sphere |
cube_top | … | cube_right`). Each key appears only when the source
line carried the matching flag; values that don't match the spec's
expected shape (e.g. `-clamp maybe`, `-imfchan q`) drop silently from
the typed view but stay verbatim on the raw `:options` array, so
encoder output keeps its byte-for-byte source-order guarantee. The
typed key is parse-time-only — the encoder filters `:options_typed`
out of its passthrough loop so it never appears in serialised MTL.
Nested options inside `mtl:refl:sphere` and per-face entries under
`mtl:refl:cube[<face>]` also gain a sibling `options_typed` field,
so per-face reflection metadata is uniformly structured.

Round 236: MTL `Ka` / `Kd` / `Ks` `spectral` and `xyz` alternative
forms — the three Phong-colour statements now accept the same triplet
of mutually-exclusive forms `Tf` already did (plain RGB, `spectral
file.rfl [factor]`, and `xyz x [y z]`), matching the spec listings at
§"Ka r g b", §"Kd r g b", §"Ks r g b". The spectral form lands on
`Material::extras["mtl:K{a,d,s}:spectral"]` as a `{file, factor}`
object (`factor` defaults to 1.0 per spec, and the encoder omits the
explicit `1.0` token in that case); the xyz form lands on
`Material::extras["mtl:K{a,d,s}:xyz"]` as an `[x, y, z]` array (y and
z default to x per spec). The plain RGB form additionally honours
the spec's single-value broadcast ("If only r is specified, then g,
and b are assumed to be equal to r") for all three statements — the
previous parser required exactly three floats. The encoder picks the
first present sibling key per material in precedence order
`spectral` → `xyz` → RGB; `Kd spectral` / `Kd xyz` additionally
suppress the canonical `Kd r g b` line driven by `base_color` so the
forms remain mutually exclusive on round-trip. `Tf` was refactored
to share the same `parse_color_statement` helper without changing
its observable behaviour.

Round 229: connectivity (`con`) + general-statement (`call` / `csh`)
round-trip — three more previously-dropped spec-defined directives now
survive a decode → encode cycle verbatim.
`con surf_1 q0_1 q1_1 curv2d_1 surf_2 q0_2 q1_2 curv2d_2` (spec
§"Connectivity between free-form surfaces", §"con surf_1 q0_1 q1_1
curv2d_1 surf_2 q0_2 q1_2 curv2d_2") is a top-level free-form geometry
statement that ties two previously-declared `surf` blocks together
along a shared trimming-curve segment for edge merging; captured into
the existing `Scene3D::extras["obj:freeform_directives"]` verbatim-
replay channel alongside the other free-form geometry directives so
its source-order position relative to the `surf` blocks it references
is preserved. `call filename.ext arg1 arg2 …` (spec §"General
statement", §"call filename.ext arg1 arg2 …") is the inline include
of a sibling `.obj` / `.mod` file with positional argument substitution
and `csh command` / `csh -command` (spec §"General statement", §"csh
command") is the shell-execute directive (leading `-` flagging "ignore
error on non-zero exit"). Both general statements are captured-only
into a new `Scene3D::extras["obj:general_directives"]` side-channel
array of `[keyword, arg1, …]` entries in document order; the encoder
replays them at the top of the preamble right after the
`shadow_obj` / `trace_obj` companion-file block. Source-line position
relative to the polygonal section is NOT preserved by design (the
spec is silent on placement — "The call statement can be inserted
into .obj files using a text editor"). The parser deliberately does
NOT recursively resolve `call` (would require IO + nested-call depth
tracking outside the clean-room parser's scope) nor execute `csh`
(sandbox-escape trapdoor for any consumer that round-trips untrusted
OBJ input); consumers walk the captured directive sequence themselves. `ctech technique resolution` (spec §"ctech technique
resolution", three forms `cparm res` / `cspace maxlength` /
`curv maxdist maxangle`) and `stech technique resolution` (spec
§"stech technique resolution", four forms `cparma ures vres` /
`cparmb uvres` / `cspace maxlength` / `curv maxdist maxangle`) are
captured into the same `Scene3D::extras["obj:freeform_directives"]`
verbatim-replay channel as the existing free-form curve/surface
attributes; the encoder emits them inside the original `cstype` /
`deg` / `parm` / `end` block they were sourced from, preserving
source order. `shadow_obj filename` (spec §"shadow_obj filename") and
`trace_obj filename` (spec §"trace_obj filename") surface as plain
strings on `Scene3D::extras["obj:shadow_obj"]` /
`Scene3D::extras["obj:trace_obj"]` with per-spec last-wins collapse
("Only one shadow object can be stored in a file. If more than one
shadow object is specified, the last one specified will be used.")
and re-emit in the preamble right after the `mtllib` block, matching
the placement in the spec §"Examples" cases 2 and 3. Empty filenames
on either companion directive are dropped at parse time. No semantic
interpretation of the technique parameters — the tessellator's
`with_curve_tessellation(samples)` knob still controls sample counts
independently.

Round 282: sub-cell trim/hole boundary re-meshing — the round-201
conservative clip dropped any lattice triangle whose three corners
didn't all classify inside the trimmed region, leaving the trim edge
jagged at the lattice grain. Straddling boundary triangles (1 or 2
corners kept) are now clipped against the in/out classification
function (inside ≥ 1 trim loop — or no trim loops, per spec
§"Trimming loops and holes" "If the first trim statement in the
sequence is omitted, the enclosing outer trimming loop is taken to be
the parameter range of the surface" — AND outside every hole loop)
instead of dropped wholesale: each crossing lattice edge is bisected
in parameter space until the inside/outside frontier is pinned
(24 rounds ≈ 2⁻²⁴ of the edge length), the synthesised boundary
vertex — 3D position interpolated linearly along the lattice edge,
i.e. the same piecewise-linear approximation the triangle lattice
itself carries — is appended after the `(samples + 1)²` lattice
block, and the kept sub-polygon (corner triangle for 1-kept, quad
split into two triangles for 2-kept) is emitted with the original
CCW winding. Crossings are cached per undirected lattice edge so
adjacent straddling cells share their boundary vertex and the
re-meshed rim stays watertight; sub-triangles whose parameter-space
area collapses below 10⁻⁶ of a lattice cell (loops grazing a lattice
line exactly) are suppressed rather than emitted as degenerate
slivers, and boundary vertices left unreferenced by that suppression
are garbage-collected from the vertex pool. On an axis-aligned square
loop sitting between lattice lines the kept area now matches the
analytic trimmed area to within the chord-across-corner error (~0.4 %
observed at 8 samples) where the conservative whole-cell staircase
missed ≈ 60 % of the loop on the same fixture. New per-primitive
provenance `obj:surface_trim_boundary_vertices` counts the
synthesised vertices (0 when every straddling cell collapsed to
suppressed slivers). Verbatim round-trip is untouched — the encoder
still filters synthetic surfaces via the shared
`obj:tessellated_curve` sentinel and replays the original `trim` /
`hole` block from `Scene3D::extras["obj:freeform_directives"]`.

Round 290: special-curve (`scrv`) embedding as surface triangle edges
— spec §"Special curve": "A special curve is guaranteed to be included
in any triangulation of the surface. … the line formed by
approximating the special curve with a sequence of straight line
segments will actually appear as a sequence of triangle edges in the
final triangulation." The round-206 `scrv` pass emitted the special
curve only as a stand-alone parameter-space polyline on the
`obj:scrvs` mesh; the tessellated `obj:surfaces` triangle grid ignored
it, so the spec's triangle-edge guarantee was unmet. Now every `surf`
whose enclosing `cstype … end` block carries a `scrv` directive has
the special curve embedded into its triangulation: the `scrv` is
resolved to a parameter-space polyline (the same `(u0, u1, curv2d)`
body grammar and `collect_all_curv2_polylines` pre-pass `trim` /
`hole` use, but left open — a special curve is a constraint, not a
closed region), then each straight segment is forced to coincide with
a chain of triangle edges. The constraint runs on the final kept
geometry **after** the round-282 trim/hole re-mesh, so trimming and
special-curve embedding compose. The embedder works on the triangle
soup with no adjacency structure: any triangle whose interior a
segment crosses is split so the chord between the two boundary hits
becomes an edge, crossing vertices are deduplicated on a quantised
parameter grid (watertight across adjacent splits), each synthesised
vertex's 3D position is the barycentric blend of the host triangle's
corners (so the embedded curve is exactly as accurate as the
surrounding piecewise-linear facet — no new surface evaluation), and a
segment that already lies along existing lattice edges counts as
embedded with no split. New per-surface provenance: `obj:surface_scrv`
(marker), `obj:surface_scrv_curves` (count of special curves that
overlapped the meshed surface), and `obj:surface_scrv_vertices` (count
of synthesised constraint vertices). Verbatim round-trip is untouched
— the synthetic surface still carries the shared
`obj:tessellated_curve` sentinel so the encoder filters it and replays
the original `scrv` block from
`Scene3D::extras["obj:freeform_directives"]`.

Round 295: connectivity (`con`) seam tessellation — spec §"Connectivity
between free-form surfaces", §"con surf_1 q0_1 q1_1 curv2d_1 surf_2 q0_2
q1_2 curv2d_2": "Connectivity connects two surfaces along their trimming
curves. … This information is useful for edge merging." The round-251
pass surfaced the eight raw `con` arguments as the typed
`Scene3D::extras["obj:connectivity"]` view; this round draws the seam
itself as drawable geometry. With `with_curve_tessellation(samples)`
enabled, every `con` emits a pair of parameter-space
`Topology::LineStrip` seams — one per joined surface edge — on a new
synthetic mesh named `"obj:cons"`. Each side's `curv2d` is resolved
through the same `collect_all_curv2_polylines` pre-pass the `trim` /
`hole` / `scrv` passes use ("This curve must have been previously
defined with the curv2 statement"), and the `[q0, q1]` sub-range is
walked with the shared `append_curv2_segment` so a connectivity seam is
sampled identically to a special-curve segment. The appendix fixes the
correspondence — the seam is `S1(T1(t1))` for `t1 ∈ [q0_1, q1_1]` and
`S2(T2(t2))` for `t2 ∈ [q0_2, q1_2]`, "identical up to
reparameterization" with endpoints meeting exactly — so the two emitted
seams are the two sides of one weld. Per-seam provenance: the shared
`obj:tessellated_curve` sentinel, an `obj:con` marker, `obj:con_side`
(`1` / `2`), `obj:con_surf` and `obj:con_peer_surf` (the joined
surface's index and its mate's, so a consumer holding one seam finds the
other without re-parsing), `obj:con_curv2d`, and `obj:con_q0` /
`obj:con_q1`. A `con` without exactly eight arguments is dropped from
the geometry view (like the typed view); a side whose curve doesn't
resolve (non-positive / undefined `curv2d`, or a zero-length parameter
range — e.g. the spec example's `2.0 2.0` point-join on side 1) drops on
its own while the other side still emits. The merging-group filter the
spec mentions ("Connectivity between surfaces in different merging
groups is ignored") is a renderer-side pruning decision over the `mg`
state and is left to the consumer. Verbatim round-trip is untouched —
the encoder filters the seams via the shared sentinel and replays the
original `con` line from `Scene3D::extras["obj:freeform_directives"]`.

The `.mod` binary form remains out of scope. Round 282 upgraded the
round-201 conservative trim/hole clip to sub-cell boundary re-meshing,
so the trim edge now follows the loop polygon at bisection precision
rather than the lattice grain (residual approximation: the straight
chord across a loop corner inside its straddling cell); round 290
embeds the `scrv` special curve as triangle edges on the surface mesh;
round 295 draws the `con` connectivity seam as a parameter-space
polyline pair on the `obj:cons` mesh.

## License

MIT. See [LICENSE](./LICENSE).
