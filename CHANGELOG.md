# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Basis-matrix `surf` surface tessellation under
  `ObjDecoder::with_curve_tessellation(samples: u32)`. The decoder now
  evaluates `surf` elements that sit under a `cstype bmatrix` (or
  `cstype rat bmatrix`) header into a triangulated `Topology::Triangles`
  primitive on the synthetic `"obj:surfaces"` mesh, via the bivariate
  tensor-product polynomial
  `S(u, v) = Σ_a Σ_b (Σ_p B_u[a][p] · u^p) (Σ_q B_v[b][q] · v^q) ·
  c_{base_u + a, base_v + b}` (spec §"Basis matrix",
  §"bmat u/v matrix", §"step stepu stepv"). Per-direction basis
  matrices come from `bmat u` / `bmat v` (row-major, column index
  varying fastest); per-direction segment strides come from
  `step stepu stepv`. The per-direction control-grid extent is the
  inverse of the spec relation `parm = (K − n) / s + 2`, i.e.
  `K = (parm − 2) · s + n + 1`, applied independently in u and v per
  spec §"step stepu stepv" ("For surfaces, the above description
  applies independently to each parametric direction."). Multi-patch
  grids are now decomposed into `(K_u − n − 1) / stepu + 1` × `(K_v −
  m − 1) / stepv + 1` polynomial segments; the global parameter `(u,
  v)` partitions into per-segment `(seg_u, seg_v, t_u, t_v)` with the
  patch-local control window starting at `(seg_u · stepu, seg_v ·
  stepv)`. The cubic Bezier basis-matrix surface from the spec
  §"Examples" reproduces the equivalent `cstype bezier` patch sample-
  for-sample on its single-patch form. The `rat bmatrix` qualifier
  routes to the same evaluator without per-vertex weight blending
  (matches the round-10 1D curve behaviour — the user's basis is the
  authoritative source). Malformed blocks (missing `bmat u` / `bmat v`,
  missing `step`, wrong-size matrices, mismatched control-vertex
  count) are silently dropped — the directive sequence still rides on
  `Scene3D::extras["obj:freeform_directives"]` for round-trip.
  Synthetic primitives carry `obj:tessellated_surface = true`,
  `obj:surface_kind` (`"bmatrix"`), `obj:surface_degree`,
  `obj:surface_u_range`, `obj:surface_v_range`, and
  `obj:surface_samples` provenance plus the shared
  `obj:tessellated_curve = true` sentinel so the encoder filters the
  synthetic geometry out and replays the original
  `cstype` / `deg` / `bmat u` / `bmat v` / `step` / `parm u` /
  `parm v` / `surf` / `end` block verbatim. Trim/hole loop
  evaluation remains out of scope.
- Taylor polynomial `surf` surface tessellation under
  `ObjDecoder::with_curve_tessellation(samples: u32)`. The decoder now
  evaluates `surf` elements that sit under a `cstype taylor` (or
  `cstype rat taylor`) header into a triangulated `Topology::Triangles`
  primitive on the synthetic `"obj:surfaces"` mesh, via the bivariate
  tensor-product Horner-rule evaluation
  `S(u, v) = Σ_i Σ_j c_{i,j} · u^i · v^j` (spec §"Taylor"). Control
  points are the polynomial coefficients in spec §"Surface vertex data
  — control points" row-major u-fastest order; a single Taylor patch
  of declared degree `deg degu degv` needs exactly
  `(degu + 1) × (degv + 1)` coefficient vectors. The `surf s0 s1 t0 t1`
  range supplies the global parameter clip; Taylor surfaces evaluate
  against the raw `[s0, s1]` × `[t0, t1]` window directly (not a
  normalised `[0, 1]` re-parameterisation). The implementation
  collapses the inner u sum via Horner's rule across each v-row, then
  a second Horner-rule pass in v over the collapsed points; total
  surface sample count is `(samples + 1)²`. The spec note in
  §"Free-form curve/surface body statements" says the rational form
  "does not make sense for Taylor", so `rat taylor` routes to the same
  evaluator without per-vertex weight blending. Synthetic primitives
  carry `obj:tessellated_surface = true`, `obj:surface_kind`
  (`"taylor"`), `obj:surface_degree`, `obj:surface_u_range`,
  `obj:surface_v_range`, and `obj:surface_samples` provenance plus the
  shared `obj:tessellated_curve = true` sentinel so the encoder filters
  the synthetic geometry out and replays the original
  `cstype` / `deg` / `surf` / `parm` / `end` block verbatim from
  `Scene3D::extras["obj:freeform_directives"]`. Basis-matrix `surf`
  surfaces remain captured-only.

## [0.0.2](https://github.com/OxideAV/oxideav-obj/compare/v0.0.1...v0.0.2) - 2026-05-24

### Other

- Round 13: Cardinal (Catmull-Rom) surf surface tessellation
- Round 12: B-spline / NURBS surf surface tessellation
- round 11: Bezier surf surface tessellation (tensor-product de Casteljau)
- round 10: basis-matrix curve tessellation (cstype bmatrix + bmat + step)
- Round 9: Cardinal (Catmull-Rom) + Taylor curve tessellation
- Round 8: B-spline / NURBS curve tessellation ([#3](https://github.com/OxideAV/oxideav-obj/pull/3))
- Round 7: Bezier curve tessellation evaluator

### Added

- `cargo fuzz` harness — two libfuzzer-driven panic-freedom targets
  (`fuzz/fuzz_targets/parse_obj.rs` and `fuzz/fuzz_targets/parse_mtl.rs`)
  that drive attacker-controlled bytes through every public decoder
  entry point and assert no call panics, aborts, debug-overflows, or
  indexes out of bounds. `parse_obj` exercises `ObjDecoder::decode`
  (the trait surface), `ObjDecoder::with_curve_tessellation(8).decode`
  (the free-form evaluator path — Bezier / B-spline / Cardinal /
  Taylor / basis-matrix curves and Bezier / B-spline / Cardinal
  surfaces), the lower-level `obj::parse_obj` free function, the
  explicit `obj::parse_obj_with_options` entry, and 4 truncated
  prefixes per input. `parse_mtl` exercises `MtlDecoder::decode`,
  `mtl::parse_mtl`, and `mtl::parse_mtl_with_scene`. The fuzz
  subcrate's `Cargo.lock` is tracked under `fuzz/` for reproducible
  builds; `fuzz/target` / `fuzz/corpus` / `fuzz/artifacts` are
  `.gitignore`-d.
- `tests/fuzz_regressions.rs` — three panic-freedom regression tests
  pinning the crashes discovered by the first 180-second `parse_obj`
  fuzz run (see "Fixed" below).

### Fixed

- `parse_face_vertex` rejected an empty leading slot in `f` / `l` / `p`
  index tokens (e.g. `f /1/2 /3/4 /5/6` or `p /13`) at parse time
  instead of letting them coalesce to `v == 0` and trip the downstream
  `(fv.v - 1) as usize` underflow inside `build_scene`. The position
  component is mandatory per spec ("v is the index of the geometric
  vertex … required for every reference"); the parser now surfaces
  the missing-position case as `Err(Error::invalid)` so the `fv.v >= 1`
  invariant holds end-to-end. Found by libfuzzer on the new
  `parse_obj` target.
- `tessellate_surfaces` Bezier branch capped attacker-controlled
  `(degu + 1, degv + 1)` grid extents with `checked_add` /
  `checked_mul` and an early "expected == entry-control-count" gate
  so a malformed `deg 111111` (or other huge value) bails before
  `Vec::with_capacity(expected)` would request a multi-gibibyte
  allocation. The same defence covers the `cstype bmatrix`
  `(n + 1) × (n + 1)` basis-matrix size check in `flush_block` and
  the corresponding helper `sample_bmatrix`. Found by libfuzzer +
  AddressSanitizer's `allocation-size-too-big` detector on the new
  `parse_obj` target.

### Added

- Cardinal (Catmull-Rom) `surf` surface tessellation under
  `ObjDecoder::with_curve_tessellation(samples: u32)`. The decoder now
  evaluates `surf` elements that sit under a `cstype cardinal` (or
  `cstype rat cardinal`) header into a triangulated
  `Topology::Triangles` primitive on the synthetic `"obj:surfaces"`
  mesh, via the bivariate tensor-product Cardinal evaluation
  `S(u, v) = Σ_i Σ_j C_i(u) · C_j(v) · d_{i,j}` (spec §"Cardinal"). Each
  parametric direction reuses the spec's Cardinal→Bezier per-segment
  conversion (`b0 = c1`, `b1 = c1 + (c2 − c0) / 6`,
  `b2 = c2 − (c3 − c1) / 6`, `b3 = c2`, then a cubic Bernstein blend)
  over a sliding 4-point window: the inner pass collapses every v-row at
  the sample u, then a second 1-D Cardinal pass runs in v over the
  collapsed points. Cardinal is cubic-only per spec ("Cardinal splines
  are only defined for the cubic case"), so any `deg` other than `3 3`
  leaves the surface captured-only. The control grid is read from the
  `parm u` / `parm v` extents (`K = parm_count + 1` per direction, from
  the spec relation `parm = K − n + 2` with `n = 3`); when `parm` only
  carries the 2-value global parameter range (as the spec's
  Cardinal-surface example does), the grid is taken to be the square
  single patch (`cols = rows = √total`). Per spec §"Cardinal" — "For
  surfaces, all but the first and last row and column of control points
  are interpolated" — a single bicubic patch's parametric corners land
  exactly on the interior 2×2 control block, which the tests verify, and
  a cross-check confirms the tensor-product evaluator matches an
  independent Cardinal→Bezier reference sample-for-sample. The
  `rat cardinal` qualifier routes to the same evaluator (spec
  §"Free-form curve/surface body statements" notes the unit-weight
  default is reasonable for Cardinal because its basis functions sum to
  1), so per-vertex `w` weights are not applied. Synthetic primitives
  carry `obj:tessellated_surface = true`, `obj:surface_kind`
  (`"cardinal"`), `obj:surface_degree`, `obj:surface_u_range`,
  `obj:surface_v_range`, and `obj:surface_samples` provenance plus the
  shared `obj:tessellated_curve = true` sentinel so the encoder filters
  the synthetic geometry out and replays the original
  `cstype` / `deg` / `surf` / `parm u` / `parm v` / `end` block verbatim
  from `Scene3D::extras["obj:freeform_directives"]`. Non-cubic Cardinal,
  Taylor, and basis-matrix `surf` bases remain captured-only.
- B-spline / NURBS `surf` surface tessellation under
  `ObjDecoder::with_curve_tessellation(samples: u32)`. The decoder now
  evaluates `surf` elements that sit under a `cstype bspline` (or
  `cstype rat bspline`) header into a triangulated `Topology::Triangles`
  primitive on the synthetic `"obj:surfaces"` mesh, via the bivariate
  tensor-product Cox-deBoor formula
  `S(u, v) = Σ_i Σ_j N_{i,nu}(u) · N_{j,nv}(v) · d_{i,j}` (spec
  §"B-spline" + §"Rational and non-rational curves and surfaces"). The
  per-direction control-grid extents are derived from the `parm u` /
  `parm v` knot vectors (`(len(parm u) − degu − 1) ×
  (len(parm v) − degv − 1)` per spec §"B-spline" condition 6, applied
  independently in u and v); control points are read in the spec's
  row-major u-fastest order (§"Surface vertex data — control points")
  with negative relative-from-end indices honoured. Each `surf` line's
  `s0 s1 t0 t1` range is clipped against the condition-5 evaluation
  window `[x_n, x_{K+1}]` of its direction's knot vector, and the last
  sample per direction is nudged fractionally below the upper bound so
  the half-open knot-span convention doesn't zero the basis at the
  endpoint (same NURBS-evaluator pattern as the round-8 curve path). The
  rational (NURBS) form blends the per-vertex `w` weights from the `v`
  lines and projects via the weighted denominator
  `Σ N·N·w·d / Σ N·N·w`. The basis is evaluated with the same
  `bspline_basis` Cox-deBoor routine the 1D `curv` path uses, so a
  clamped quadratic B-spline patch (`parm 0 0 0 1 1 1`) reproduces the
  equivalent quadratic Bezier patch sample-for-sample, and the spec's
  cubic B-spline surface example tessellates inside its control-net
  convex hull. Synthetic primitives carry
  `obj:tessellated_surface = true`, `obj:surface_kind`
  (`"bspline"` / `"rat_bspline"`), `obj:surface_degree`,
  `obj:surface_u_range`, `obj:surface_v_range`, and `obj:surface_samples`
  provenance plus the shared `obj:tessellated_curve = true` sentinel so
  the encoder filters the synthetic geometry out and replays the original
  `cstype` / `deg` / `surf` / `parm u` / `parm v` / `end` block verbatim
  from `Scene3D::extras["obj:freeform_directives"]`. Knot/control-count
  mismatches and malformed blocks are left captured-only. Cardinal /
  Taylor / basis-matrix `surf` bases remain captured-only.
- Bezier `surf` surface tessellation under
  `ObjDecoder::with_curve_tessellation(samples: u32)`. The decoder now
  evaluates `surf` elements that sit under a `cstype bezier` (or
  `cstype rat bezier`) header into a triangulated `Topology::Triangles`
  primitive on a synthetic mesh named `"obj:surfaces"`, via the
  bivariate tensor-product de Casteljau algorithm (spec §"Rational and
  non-rational curves and surfaces" + §"Bezier"). Control points are
  read in the spec's row-major u-fastest order (§"Surface vertex data —
  control points": "listed in the order i = 0 to K1 for j = 0, followed
  by i = 0 to K1 for j = 1, …"); the `surf` line's `v/vt/vn` control-
  vertex references are parsed for their leading position index, with
  negative relative-from-end indices honoured. A single Bezier patch of
  declared degree `deg degu degv` requires exactly
  `(degu + 1) × (degv + 1)` control points; counts that don't match a
  single patch (multi-patch grids, which the Bezier basis can't
  decompose without a `step` stride) are left captured-only. The patch
  is sampled at a `(samples + 1) × (samples + 1)` lattice and
  triangulated counter-clockwise (front = u-increases-right,
  v-increases-up per the spec `surf` note). The rational form lifts each
  control point to its homogeneous `(w·x, w·y, w·z, w)` form, runs both
  de Casteljau passes in 4D, and projects back via `x / w`. Synthetic
  primitives carry `obj:tessellated_surface = true`, `obj:surface_kind`
  (`"bezier"` / `"rat_bezier"`), `obj:surface_degree` (`[degu, degv]`),
  `obj:surface_u_range` (`[s0, s1]`), `obj:surface_v_range`
  (`[t0, t1]`), and `obj:surface_samples` provenance extras, plus the
  shared `obj:tessellated_curve = true` sentinel so the encoder filters
  the synthetic geometry out and replays the original
  `cstype` / `deg` / `surf` / `parm` / `end` block verbatim from
  `Scene3D::extras["obj:freeform_directives"]`. Non-Bezier `surf` bases
  remain captured-only.
- Basis-matrix curve tessellation under
  `ObjDecoder::with_curve_tessellation(samples: u32)`. The decoder now
  evaluates `cstype bmatrix` `curv` directives per spec §"Basis matrix"
  using the user-supplied `(n + 1) × (n + 1)` basis from `bmat u` and the
  segment stride from `step <stepu>` (spec §"bmat u/v matrix" and
  §"step stepu stepv"). Each polynomial segment `i` consumes the
  control-point window `c_{i·step + 1} .. c_{i·step + n + 1}` (1-based)
  and evaluates `P(t) = Σ_i Σ_j B[i][j] · t^j · p_{base + i}` per axis,
  where `B[i][j]` is the row-major basis-matrix element with column
  index `j` varying fastest (spec §"bmat u/v matrix": "matrix lists the
  contents of the basis matrix with column subscript j varying the
  fastest"). The `rat bmatrix` qualifier is accepted but does not apply
  per-vertex weights (the spec note says the unit-weight default "may
  or may not make sense for a representation given in basis-matrix form",
  so the user's basis is the authoritative source). Synthetic primitives
  carry the same `obj:tessellated_curve = true` / `obj:curve_kind` (`"bmatrix"`) /
  `obj:curve_degree` (from `deg`) / `obj:curve_u_range` / `obj:curve_samples`
  provenance extras. Malformed blocks (missing `bmat u`, missing `step`,
  wrong-size matrix, fewer than `n + 1` control points) are silently
  dropped — the directive sequence still rides on
  `Scene3D::extras["obj:freeform_directives"]` for round-trip.
- `bmat` and `step` free-form directive tracking — the parser now
  captures these keywords into the `obj:freeform_directives` extra
  alongside the existing `cstype` / `deg` / `curv` / `parm` / `surf` /
  `trim` / `hole` / `scrv` / `sp` / `end` / `bzp` / `bsp` directives,
  so a `cstype bmatrix` block round-trips through a decode → encode →
  decode cycle bit-exactly. The encoder replays the captured directive
  sequence verbatim with no semantic interpretation.
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
