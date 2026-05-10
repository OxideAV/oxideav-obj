# oxideav-obj

Pure-Rust Wavefront OBJ + MTL 3D mesh codec. Implements the
`oxideav_mesh3d::Mesh3DDecoder` and `Mesh3DEncoder` traits, plugging into
the wider OxideAV codec ecosystem.

OBJ is the universal mesh-interchange format published by Wavefront
Technologies in the early 1990s as Appendix B of the Advanced Visualizer
manual. This crate implements the polygonal subset (the part that
modern loaders actually load):

- `v` / `vt` / `vn` vertex data — with the optional `w` 4th component on
  positions and the optional `v` / `w` extra components on UVs.
- `f` faces in all four index syntaxes (`v`, `v/vt`, `v//vn`,
  `v/vt/vn`), with 1-based indexing and the negative-index relative-from-end
  shorthand. Polygons (n-gons) are fan-triangulated on read; the original
  per-face arity is stashed in `Mesh::extras["obj:original_face_arities"]`
  so the encoder can re-emit n-gons rather than triangles.
- `l` line elements → `Topology::Lines`.
- `o <name>` → one `Mesh` per object directive (or a single mesh if the
  file has no `o`).
- `g <name>` → group-name capture in `Primitive::extras["obj:groups"]`.
- `s` smoothing groups → `Primitive::extras["obj:smoothing_group"]`.
- `mtllib <file.mtl> [<file2.mtl> …]` and `usemtl <name>` — each
  `usemtl` switch starts a fresh `Primitive` so a multi-material OBJ
  becomes a `Mesh` with N primitives, each with its own `MaterialId`.

The companion **MTL** parser/serialiser handles:

- Phong colours (`Ka` / `Kd` / `Ks` / `Ke`) → glTF `base_color`
  (from `Kd`) + `emissive_factor` (from `Ke`); `Ka` / `Ks` and the
  `Ns` exponent are stashed in `Material::extras` for round-trip.
- Transparency (`d` dissolve / `Tr = 1 - d`) → `AlphaMode::Blend`
  + `base_color.a`.
- Index of refraction (`Ni`) and illumination model (`illum`) → extras.
- Texture references (`map_Kd` → `base_color_texture`,
  `map_Bump` → `normal_texture`, `map_d` etc.) emitted as
  `ImageData::External { uri, mime: None }` — the caller resolves
  paths against the OBJ file's directory.
- Wavefront-PBR extension (`Pr` roughness, `Pm` metallic, `Pc`
  clearcoat, `Ps` sheen, `map_Pr` / `map_Pm`) → `Material::roughness`
  / `Material::metallic` / `metallic_roughness_texture`.

Both decoders are registered against `Mesh3DRegistry` under the
default-on `registry` cargo feature; drop the feature for a free-standing
build that only exposes `ObjDecoder` / `ObjEncoder` and the
`oxideav_mesh3d` standalone trait surface.

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

Round 1 (this commit): polygonal subset + MTL Phong + Wavefront-PBR
extension. Free-form curves/surfaces (`curv`, `surf`, `parm`, `trim`,
`hole`, `scrv`, `sp`, `end`), the `vp` parameter-space vertex, the
`p` point element, the `mg` merging-group, the `bevel` /
`c_interp` / `d_interp` / `lod` display attributes, and the `.mod`
binary form are not implemented. Files using only those features will
decode to an empty scene; files that mix them with polygonal data will
decode the polygonal part and ignore the rest.

## License

MIT. See [LICENSE](./LICENSE).
