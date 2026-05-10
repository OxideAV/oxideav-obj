# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
- MTL `disp` ↔ `map_disp`, `decal` ↔ `map_decal`, and `refl` ↔
  `map_refl` keyword aliases land in extras with the original
  spelling preserved as the key.
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
