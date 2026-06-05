//! Wavefront MTL (material library) ASCII parser + serialiser.
//!
//! The grammar mirrors OBJ's: line-oriented, whitespace-separated,
//! `#` introduces a comment to end of line. Each `newmtl <name>`
//! opens a fresh material; subsequent lines populate the material's
//! parameters until the next `newmtl` or end of file.
//!
//! This crate maps the Phong-Blinn vocabulary onto the glTF
//! metallic-roughness model in [`Material`], preserving the original
//! field values in [`Material::extras`] so a re-serialise reproduces
//! the input. The Wavefront-PBR extension (`Pr`, `Pm`, `Pc`, `Ps`,
//! `map_Pr`, `map_Pm`) lands directly in the corresponding PBR slots.

use oxideav_mesh3d::{AlphaMode, Error, ImageData, Material, Result, Sampler, Texture, TextureRef};

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Pending texture references from the parser. We can't allocate a
/// `TextureRef` at parse time because that needs a `TextureId` (only
/// known once textures land in the [`Scene3D`](oxideav_mesh3d::Scene3D)).
/// The OBJ→Scene3D path bridges this in
/// [`merge_materials_into_scene`].
#[derive(Debug, Default, Clone)]
struct PendingTextures {
    base_color: Option<String>,
    normal: Option<String>,
    metallic_roughness: Option<String>,
    emissive: Option<String>,
}

/// Parsed material plus its yet-to-be-resolved texture URIs.
#[derive(Debug, Clone)]
struct ParsedMaterial {
    material: Material,
    pending: PendingTextures,
}

/// Parse an MTL document.
///
/// Returns one [`Material`] per `newmtl` block. Texture references
/// are resolved lazily by [`merge_materials_into_scene`] (used by the
/// OBJ decoder) — direct callers get materials with `*_texture` slots
/// wired to fresh textures stored in the same returned vector via
/// the `extras["mtl:pending_textures"]` side-channel; consumers
/// integrating with a real `Scene3D` should use [`parse_mtl_with_scene`]
/// instead.
pub fn parse_mtl(text: &str) -> Result<Vec<Material>> {
    let parsed = parse_mtl_internal(text)?;
    let mut out: Vec<Material> = Vec::with_capacity(parsed.len());
    for pm in parsed {
        let mut mat = pm.material;
        // Stash pending texture URIs in extras so a downstream pass
        // can hoist them into a Scene3D's texture pool. Direct callers
        // who want the URIs without a Scene3D can pull them from here.
        let mut pending_obj = serde_json::Map::new();
        if let Some(p) = pm.pending.base_color {
            pending_obj.insert("base_color".into(), serde_json::Value::String(p));
        }
        if let Some(p) = pm.pending.normal {
            pending_obj.insert("normal".into(), serde_json::Value::String(p));
        }
        if let Some(p) = pm.pending.metallic_roughness {
            pending_obj.insert("metallic_roughness".into(), serde_json::Value::String(p));
        }
        if let Some(p) = pm.pending.emissive {
            pending_obj.insert("emissive".into(), serde_json::Value::String(p));
        }
        if !pending_obj.is_empty() {
            mat.extras.insert(
                "mtl:pending_textures".to_string(),
                serde_json::Value::Object(pending_obj),
            );
        }
        out.push(mat);
    }
    Ok(out)
}

/// Hoist pending texture URIs into the supplied scene as
/// [`Texture`]s and bind the result on each material via
/// [`TextureRef`]. Materials are also added to the scene; returns the
/// `MaterialId` for each input material in declaration order.
///
/// Provided as a convenience for `obj.rs` and direct MTL-decoder
/// callers; symmetrical with the OBJ→Scene3D pipeline so reload of an
/// MTL standalone produces the same in-scene structure as a full OBJ
/// decode would.
pub fn merge_materials_into_scene(
    scene: &mut oxideav_mesh3d::Scene3D,
    materials: Vec<Material>,
) -> Vec<oxideav_mesh3d::MaterialId> {
    let mut ids = Vec::with_capacity(materials.len());
    for mut mat in materials {
        // Resolve any `mtl:pending_textures` field into real Textures.
        let pending = mat.extras.remove("mtl:pending_textures");
        if let Some(serde_json::Value::Object(obj)) = pending {
            for (slot, val) in obj {
                let serde_json::Value::String(uri) = val else {
                    continue;
                };
                let tex = Texture {
                    name: Some(uri.clone()),
                    image: ImageData::External {
                        uri: uri.clone(),
                        mime: None,
                    },
                    sampler: Sampler::default_sampler(),
                };
                let tex_id = scene.add_texture(tex);
                let tex_ref = TextureRef::new(tex_id);
                match slot.as_str() {
                    "base_color" => mat.base_color_texture = Some(tex_ref),
                    "normal" => mat.normal_texture = Some(tex_ref),
                    "metallic_roughness" => mat.metallic_roughness_texture = Some(tex_ref),
                    "emissive" => mat.emissive_texture = Some(tex_ref),
                    _ => {}
                }
            }
        }
        ids.push(scene.add_material(mat));
    }
    ids
}

/// One-shot parse + scene-hoist for direct MTL-decoder callers.
pub fn parse_mtl_with_scene(text: &str) -> Result<oxideav_mesh3d::Scene3D> {
    let mut scene = oxideav_mesh3d::Scene3D::new();
    let materials = parse_mtl(text)?;
    let _ = merge_materials_into_scene(&mut scene, materials);
    Ok(scene)
}

fn parse_mtl_internal(text: &str) -> Result<Vec<ParsedMaterial>> {
    let mut out: Vec<ParsedMaterial> = Vec::new();
    let mut current: Option<ParsedMaterial> = None;

    fn strip_comment(line: &str) -> &str {
        match line.find('#') {
            Some(idx) => &line[..idx],
            None => line,
        }
    }

    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let Some(keyword) = tokens.next() else {
            continue;
        };

        match keyword {
            "newmtl" => {
                if let Some(prev) = current.take() {
                    out.push(prev);
                }
                let name: String = tokens.collect::<Vec<_>>().join(" ");
                let mut mat = Material::new();
                // Spec primer says fallback to metallic=0/roughness=0.5 when
                // PBR fields aren't present.
                mat.metallic = 0.0;
                mat.roughness = 0.5;
                mat.name = Some(name);
                current = Some(ParsedMaterial {
                    material: mat,
                    pending: PendingTextures::default(),
                });
            }
            other => {
                let Some(pm) = current.as_mut() else {
                    return Err(Error::invalid(format!(
                        "MTL: {other:?} appears before any newmtl directive"
                    )));
                };
                apply_directive(other, &mut tokens, pm)?;
            }
        }
    }

    if let Some(last) = current.take() {
        out.push(last);
    }
    Ok(out)
}

fn parse_floats<'a, I: Iterator<Item = &'a str>>(tokens: I, keyword: &str) -> Result<Vec<f32>> {
    tokens
        .map(str::parse::<f32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::invalid(format!("MTL {keyword}: bad float ({e})")))
}

/// One of the three mutually-exclusive forms a `K*` colour statement
/// (or `Tf`) can take per Wavefront MTL spec §"Ka r g b" / §"Kd r g b"
/// / §"Ks r g b" / §"Tf r g b":
///
/// * Plain RGB triple (g/b default to r when omitted).
/// * `spectral file.rfl factor` (factor defaults to 1.0).
/// * `xyz x y z` CIEXYZ tristimulus (y/z default to x when omitted).
///
/// Used to keep the four colour-statement parsers consistent.
#[derive(Debug, Clone)]
enum ColorStatement {
    Rgb { r: f32, g: f32, b: f32 },
    Spectral { file: String, factor: f32 },
    Xyz { x: f32, y: f32, z: f32 },
}

/// Discriminate and parse a `K* … ` / `Tf …` argument list. The first
/// token decides the shape:
///
/// * Numeric → RGB form (spec §"…r g b": g and b default to r when
///   omitted; we eagerly broadcast so the canonical 3-tuple is what
///   lands in `extras`).
/// * `spectral` → `spectral file.rfl [factor]` (spec §"… spectral
///   file.rfl factor": factor defaults to 1.0 when omitted).
/// * `xyz` → `xyz x [y z]` (spec §"… xyz x y z": y and z default to x
///   when omitted).
///
/// `keyword` names the originating directive for error messages.
fn parse_color_statement(toks: &[&str], keyword: &str) -> Result<ColorStatement> {
    if toks.is_empty() {
        return Err(Error::invalid(format!(
            "{keyword}: needs at least 1 argument"
        )));
    }
    match toks[0] {
        "spectral" => {
            if toks.len() < 2 {
                return Err(Error::invalid(format!(
                    "{keyword} spectral: missing file.rfl"
                )));
            }
            let file = toks[1].to_string();
            let factor: f32 = if let Some(f) = toks.get(2) {
                f.parse()
                    .map_err(|e| Error::invalid(format!("{keyword} spectral: bad factor ({e})")))?
            } else {
                1.0
            };
            Ok(ColorStatement::Spectral { file, factor })
        }
        "xyz" => {
            let v: Vec<f32> = toks[1..]
                .iter()
                .map(|s| s.parse::<f32>())
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| Error::invalid(format!("{keyword} xyz: bad float ({e})")))?;
            if v.is_empty() {
                return Err(Error::invalid(format!(
                    "{keyword} xyz: needs at least 1 float"
                )));
            }
            let x = v[0];
            let y = v.get(1).copied().unwrap_or(x);
            let z = v.get(2).copied().unwrap_or(x);
            Ok(ColorStatement::Xyz { x, y, z })
        }
        _ => {
            let v: Vec<f32> = toks
                .iter()
                .map(|s| s.parse::<f32>())
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| Error::invalid(format!("{keyword}: bad float ({e})")))?;
            let r = v[0];
            let g = v.get(1).copied().unwrap_or(r);
            let b = v.get(2).copied().unwrap_or(r);
            Ok(ColorStatement::Rgb { r, g, b })
        }
    }
}

fn apply_directive(
    keyword: &str,
    tokens: &mut std::str::SplitWhitespace<'_>,
    pm: &mut ParsedMaterial,
) -> Result<()> {
    let mat = &mut pm.material;
    match keyword {
        "Ka" => {
            // Ambient reflectivity. Spec §"Ka r g b" lists three
            // mutually-exclusive forms (RGB / spectral / xyz); the alt
            // forms ride on sibling extras keys (`mtl:Ka:spectral` /
            // `mtl:Ka:xyz`) so an MTL emit reproduces the operator's
            // chosen spelling.
            let toks: Vec<&str> = tokens.collect();
            match parse_color_statement(&toks, "Ka")? {
                ColorStatement::Rgb { r, g, b } => {
                    mat.extras
                        .insert("mtl:Ka".to_string(), serde_json::json!([r, g, b]));
                }
                ColorStatement::Spectral { file, factor } => {
                    mat.extras.insert(
                        "mtl:Ka:spectral".to_string(),
                        serde_json::json!({ "file": file, "factor": factor }),
                    );
                }
                ColorStatement::Xyz { x, y, z } => {
                    mat.extras
                        .insert("mtl:Ka:xyz".to_string(), serde_json::json!([x, y, z]));
                }
            }
        }
        "Kd" => {
            // Diffuse reflectivity. Spec §"Kd r g b" — RGB form sets
            // `base_color[0..3]` (canonical glTF base colour); the
            // mutually-exclusive `Kd spectral` / `Kd xyz` forms ride
            // on sibling extras (`mtl:Kd:spectral` / `mtl:Kd:xyz`)
            // and leave `base_color` untouched so the encoder
            // suppresses the canonical `Kd r g b` emit.
            let toks: Vec<&str> = tokens.collect();
            match parse_color_statement(&toks, "Kd")? {
                ColorStatement::Rgb { r, g, b } => {
                    // Preserve the alpha channel that may have been set
                    // by an earlier `d` line so the assignment ordering
                    // matches the file (`d` typically follows `Kd`,
                    // but defensive).
                    let alpha = mat.base_color[3];
                    mat.base_color = [r, g, b, alpha];
                }
                ColorStatement::Spectral { file, factor } => {
                    mat.extras.insert(
                        "mtl:Kd:spectral".to_string(),
                        serde_json::json!({ "file": file, "factor": factor }),
                    );
                }
                ColorStatement::Xyz { x, y, z } => {
                    mat.extras
                        .insert("mtl:Kd:xyz".to_string(), serde_json::json!([x, y, z]));
                }
            }
        }
        "Ks" => {
            // Specular reflectivity. Spec §"Ks r g b" — same three
            // mutually-exclusive forms as `Ka` / `Kd`.
            let toks: Vec<&str> = tokens.collect();
            match parse_color_statement(&toks, "Ks")? {
                ColorStatement::Rgb { r, g, b } => {
                    mat.extras
                        .insert("mtl:Ks".to_string(), serde_json::json!([r, g, b]));
                }
                ColorStatement::Spectral { file, factor } => {
                    mat.extras.insert(
                        "mtl:Ks:spectral".to_string(),
                        serde_json::json!({ "file": file, "factor": factor }),
                    );
                }
                ColorStatement::Xyz { x, y, z } => {
                    mat.extras
                        .insert("mtl:Ks:xyz".to_string(), serde_json::json!([x, y, z]));
                }
            }
        }
        "Ke" => {
            let v = parse_floats(tokens.by_ref(), keyword)?;
            if v.len() < 3 {
                return Err(Error::invalid(format!(
                    "Ke: needs 3 floats, got {}",
                    v.len()
                )));
            }
            mat.emissive_factor = [v[0], v[1], v[2]];
        }
        "Tf" => {
            // Transmission filter. Spec §"Tf r g b" lists the same
            // three mutually-exclusive forms as `Ka` / `Kd` / `Ks`:
            //
            //   Tf r g b               — RGB triple (g/b default to r)
            //   Tf spectral file.rfl factor    — spectral .rfl curve
            //   Tf xyz x y z           — CIEXYZ tristimulus (y/z default to x)
            //
            // The RGB form lands in `extras["mtl:Tf"]` as an
            // `[r,g,b]` array (the round-1 behaviour); the alt forms
            // land under sibling keys (`mtl:Tf:spectral` /
            // `mtl:Tf:xyz`) so a re-emit reproduces the operator's
            // chosen spelling. PBR transmission is its own KHR
            // extension on the glTF side, so we don't model any of
            // the variants as a first-class `Material` field.
            let toks: Vec<&str> = tokens.collect();
            match parse_color_statement(&toks, "Tf")? {
                ColorStatement::Rgb { r, g, b } => {
                    mat.extras
                        .insert("mtl:Tf".to_string(), serde_json::json!([r, g, b]));
                }
                ColorStatement::Spectral { file, factor } => {
                    mat.extras.insert(
                        "mtl:Tf:spectral".to_string(),
                        serde_json::json!({ "file": file, "factor": factor }),
                    );
                }
                ColorStatement::Xyz { x, y, z } => {
                    mat.extras
                        .insert("mtl:Tf:xyz".to_string(), serde_json::json!([x, y, z]));
                }
            }
        }
        "sharpness" => {
            // Reflection-map sharpness; spec range 0..1000, default 60.
            let v: f32 = tokens
                .next()
                .ok_or_else(|| Error::invalid("sharpness: missing value"))?
                .parse()
                .map_err(|e| Error::invalid(format!("sharpness: bad float ({e})")))?;
            mat.extras
                .insert("mtl:sharpness".to_string(), serde_json::json!(v));
        }
        "Ns" => {
            let v: f32 = tokens
                .next()
                .ok_or_else(|| Error::invalid("Ns: missing value"))?
                .parse()
                .map_err(|e| Error::invalid(format!("Ns: bad float ({e})")))?;
            mat.extras
                .insert("mtl:Ns".to_string(), serde_json::json!(v));
        }
        "Ni" => {
            let v: f32 = tokens
                .next()
                .ok_or_else(|| Error::invalid("Ni: missing value"))?
                .parse()
                .map_err(|e| Error::invalid(format!("Ni: bad float ({e})")))?;
            mat.extras
                .insert("mtl:Ni".to_string(), serde_json::json!(v));
        }
        "d" => {
            // The first non-flag token is the dissolve value. The
            // optional `-halo` flag (per spec §"d -halo factor")
            // makes the dissolve orientation-dependent — surface it
            // via extras so the round-trip emits the same form.
            let mut halo = false;
            let mut value: Option<f32> = None;
            for tok in tokens.by_ref() {
                if tok == "-halo" {
                    halo = true;
                    continue;
                }
                value = Some(
                    tok.parse()
                        .map_err(|e| Error::invalid(format!("d: bad float ({e})")))?,
                );
                break;
            }
            let v = value.ok_or_else(|| Error::invalid("d: missing value"))?;
            mat.base_color[3] = v;
            if v < 1.0 {
                mat.alpha_mode = AlphaMode::Blend;
            }
            if halo {
                mat.extras
                    .insert("mtl:d_halo_factor".to_string(), serde_json::json!(v));
            }
        }
        "Tr" => {
            // Tr = 1 - d (Wavefront alternate dissolve form).
            let v: f32 = tokens
                .next()
                .ok_or_else(|| Error::invalid("Tr: missing value"))?
                .parse()
                .map_err(|e| Error::invalid(format!("Tr: bad float ({e})")))?;
            let d = 1.0 - v;
            mat.base_color[3] = d;
            if d < 1.0 {
                mat.alpha_mode = AlphaMode::Blend;
            }
        }
        "illum" => {
            let v: i32 = tokens
                .next()
                .ok_or_else(|| Error::invalid("illum: missing value"))?
                .parse()
                .map_err(|e| Error::invalid(format!("illum: bad integer ({e})")))?;
            mat.extras
                .insert("mtl:illum".to_string(), serde_json::json!(v));
            // Surface the spec's per-model property breakdown alongside
            // the raw integer so consumers can branch on shading flags
            // without re-deriving the table. Spec §"illum illum_#"
            // (Wavefront Advanced Visualizer manual p.5-30, summary
            // table) enumerates which lighting terms each model turns
            // on; we mirror that table verbatim into `mtl:illum_props`.
            // For values outside 0..=10 (out-of-spec) we still record
            // the integer but emit a null props object so downstream
            // can tell "unknown model" from "model 0 with no flags".
            if let Some(props) = illum_property_map(v) {
                mat.extras.insert("mtl:illum_props".to_string(), props);
            }
        }
        "Pr" => {
            let v: f32 = tokens
                .next()
                .ok_or_else(|| Error::invalid("Pr: missing value"))?
                .parse()
                .map_err(|e| Error::invalid(format!("Pr: bad float ({e})")))?;
            mat.roughness = v;
        }
        "Pm" => {
            let v: f32 = tokens
                .next()
                .ok_or_else(|| Error::invalid("Pm: missing value"))?
                .parse()
                .map_err(|e| Error::invalid(format!("Pm: bad float ({e})")))?;
            mat.metallic = v;
        }
        "Pc" => {
            let v: f32 = tokens
                .next()
                .ok_or_else(|| Error::invalid("Pc: missing value"))?
                .parse()
                .map_err(|e| Error::invalid(format!("Pc: bad float ({e})")))?;
            mat.extras
                .insert("mtl:Pc".to_string(), serde_json::json!(v));
        }
        "Pcr" => {
            let v: f32 = tokens
                .next()
                .ok_or_else(|| Error::invalid("Pcr: missing value"))?
                .parse()
                .map_err(|e| Error::invalid(format!("Pcr: bad float ({e})")))?;
            mat.extras
                .insert("mtl:Pcr".to_string(), serde_json::json!(v));
        }
        "Ps" => {
            let v: f32 = tokens
                .next()
                .ok_or_else(|| Error::invalid("Ps: missing value"))?
                .parse()
                .map_err(|e| Error::invalid(format!("Ps: bad float ({e})")))?;
            mat.extras
                .insert("mtl:Ps".to_string(), serde_json::json!(v));
        }
        "aniso" | "anisor" => {
            let v: f32 = tokens
                .next()
                .ok_or_else(|| Error::invalid(format!("{keyword}: missing value")))?
                .parse()
                .map_err(|e| Error::invalid(format!("{keyword}: bad float ({e})")))?;
            mat.extras
                .insert(format!("mtl:{keyword}"), serde_json::json!(v));
        }
        "map_Kd" => {
            pm.pending.base_color = Some(parse_map_with_options(keyword, tokens, &mut mat.extras));
        }
        "map_Bump" | "map_bump" | "bump" | "norm" => {
            pm.pending.normal = Some(parse_map_with_options(keyword, tokens, &mut mat.extras));
        }
        "map_Ke" => {
            pm.pending.emissive = Some(parse_map_with_options(keyword, tokens, &mut mat.extras));
        }
        "map_Pr" | "map_Pm" => {
            // Either of the two PBR maps lands in metallic_roughness — the
            // glTF channel-packing convention is B = metallic, G = roughness.
            // We can't fuse two file references into one packed texture
            // without decoding pixels, so the last-seen wins; the other
            // is stashed in extras for round-trip.
            let s = parse_map_with_options(keyword, tokens, &mut mat.extras);
            if let Some(prev) = pm.pending.metallic_roughness.replace(s.clone()) {
                mat.extras.insert(
                    "mtl:displaced_pbr_map".to_string(),
                    serde_json::Value::String(prev),
                );
            }
            mat.extras
                .insert(format!("mtl:{keyword}"), serde_json::Value::String(s));
        }
        "refl" | "map_refl" => {
            // Reflection-map statements per spec §"Reflection Map" come
            // in three discriminated forms via the `-type` flag:
            //
            //   refl -type sphere -options -args filename
            //   refl -type cube_top|cube_bottom|cube_front|cube_back|cube_left|cube_right ... filename
            //
            // (plus the legacy bare-`refl filename` form which we
            // preserve under `mtl:refl` as before).
            //
            // Cube faces span SIX separate `refl` lines that together
            // describe one cubemap; bundle them into a single
            // `mtl:refl:cube` object keyed by face name so consumers
            // see one cubemap declaration rather than six unrelated
            // textures. Sphere lands as `mtl:refl:sphere = filename`.
            //
            // Per-line option flags (`-blendu`, `-mm`, …) attached to
            // a typed reflection-map line live next to the filename in
            // a `{file, options: [...]}` object so the round-trip is
            // bit-stable.
            let toks: Vec<&str> = tokens.collect();
            let mut iter = toks.iter().copied().peekable();
            // Pull a `-type <kind>` flag out of the option stream when
            // it is the first option; bare-refl with no `-type` falls
            // through to the legacy single-string form.
            let mut refl_kind: Option<&'static str> = None;
            if iter.peek() == Some(&"-type") {
                let _ = iter.next();
                if let Some(kind) = iter.next() {
                    refl_kind = match kind {
                        "sphere" => Some("sphere"),
                        "cube_top" => Some("cube_top"),
                        "cube_bottom" => Some("cube_bottom"),
                        "cube_front" => Some("cube_front"),
                        "cube_back" => Some("cube_back"),
                        "cube_left" => Some("cube_left"),
                        "cube_right" => Some("cube_right"),
                        // Spec also lists the legacy `cube_side` keyword
                        // as an alias-shape; surface it verbatim.
                        "cube_side" => Some("cube_side"),
                        _ => None,
                    };
                    if refl_kind.is_none() {
                        // Unknown -type kind — preserve verbatim via
                        // the legacy single-string slot below.
                    }
                }
            }
            // Re-collect the remaining tokens into a SplitWhitespace-
            // shaped helper so `map_options_and_filename` can work over
            // them without regressing the existing API.
            let remaining: Vec<&str> = iter.collect();
            let joined = remaining.join(" ");
            let mut split = joined.split_whitespace();
            let (opts, filename) = map_options_and_filename(&mut split);

            match refl_kind {
                Some(face) if face != "sphere" && face != "cube_side" => {
                    // Cube face — fold into the per-material cubemap
                    // bundle. Each face is a `{file, options}` object;
                    // missing options arrays are omitted.
                    let mut entry = serde_json::Map::new();
                    entry.insert(
                        "file".to_string(),
                        serde_json::Value::String(filename.clone()),
                    );
                    if !opts.is_empty() {
                        if let Some(typed) = decompose_map_options(&opts) {
                            entry.insert("options_typed".to_string(), typed);
                        }
                        entry.insert(
                            "options".to_string(),
                            serde_json::Value::Array(
                                opts.iter()
                                    .map(|s| serde_json::Value::String(s.clone()))
                                    .collect(),
                            ),
                        );
                    }
                    let cube_key = "mtl:refl:cube".to_string();
                    let cube_obj = match mat.extras.remove(&cube_key) {
                        Some(serde_json::Value::Object(map)) => map,
                        _ => serde_json::Map::new(),
                    };
                    let mut cube_obj = cube_obj;
                    cube_obj.insert(face.to_string(), serde_json::Value::Object(entry));
                    mat.extras
                        .insert(cube_key, serde_json::Value::Object(cube_obj));
                }
                Some("sphere") => {
                    let mut entry = serde_json::Map::new();
                    entry.insert(
                        "file".to_string(),
                        serde_json::Value::String(filename.clone()),
                    );
                    if !opts.is_empty() {
                        if let Some(typed) = decompose_map_options(&opts) {
                            entry.insert("options_typed".to_string(), typed);
                        }
                        entry.insert(
                            "options".to_string(),
                            serde_json::Value::Array(
                                opts.iter()
                                    .map(|s| serde_json::Value::String(s.clone()))
                                    .collect(),
                            ),
                        );
                    }
                    mat.extras.insert(
                        "mtl:refl:sphere".to_string(),
                        serde_json::Value::Object(entry),
                    );
                }
                _ => {
                    // Bare `refl filename` (legacy) or unknown -type
                    // kind — preserve via the original single-string
                    // slot used in r3.
                    if !opts.is_empty() {
                        if let Some(typed) = decompose_map_options(&opts) {
                            mat.extras
                                .insert(format!("mtl:{keyword}:options_typed"), typed);
                        }
                        mat.extras.insert(
                            format!("mtl:{keyword}:options"),
                            serde_json::Value::Array(
                                opts.into_iter().map(serde_json::Value::String).collect(),
                            ),
                        );
                    }
                    mat.extras.insert(
                        format!("mtl:{keyword}"),
                        serde_json::Value::String(filename),
                    );
                }
            }
        }
        "map_Ka" | "map_Ks" | "map_Ns" | "map_d" | "disp" | "map_disp" | "decal" | "map_decal" => {
            // Less-PBR-friendly maps preserved in extras for round-trip.
            // Both the bare (`disp`, `decal`) and `map_*` variants are
            // accepted; the original spelling is kept as the extras key
            // so the encoder re-emits the same form.
            let s = parse_map_with_options(keyword, tokens, &mut mat.extras);
            mat.extras
                .insert(format!("mtl:{keyword}"), serde_json::Value::String(s));
        }
        // Unknown directives are silently skipped (lenient-loader convention).
        _ => {}
    }
    Ok(())
}

/// Split a `map_*` token stream into `(options, filename)`.
///
/// `map_Kd -blendu off -clamp on -mm 0 1 path/to/diffuse.png`
/// returns `(["-blendu off", "-clamp on", "-mm 0 1"], "path/to/diffuse.png")`.
///
/// Each leading `-flag` token consumes a known number of arguments
/// per the MTL spec ("Options for texture map statements", Bourke
/// mirror line 540 onwards). Once a token that is neither a flag nor
/// a flag argument is encountered, the rest of the line is treated
/// as the filename (joined with single spaces so paths with embedded
/// whitespace round-trip).
fn map_options_and_filename(tokens: &mut std::str::SplitWhitespace<'_>) -> (Vec<String>, String) {
    let toks: Vec<&str> = tokens.collect();
    let mut opts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        // Only `-letter…` is a flag; bare integers / negative numbers
        // for paths starting with `-` would also start with `-`,
        // but the second char is the discriminator (alphabetic ⇒ flag).
        let is_flag = t.starts_with('-')
            && t.len() > 1
            && t.chars().nth(1).is_some_and(|c| c.is_ascii_alphabetic());
        if !is_flag {
            break;
        }
        let arg_count = flag_arg_count(t);
        if arg_count == 0 {
            // Unknown flag — preserve verbatim and hope the next token
            // is the filename. Bumps the index by 1.
            opts.push(t.to_string());
            i += 1;
            continue;
        }
        // Make sure we have enough remaining tokens; if not, the file
        // name was truncated mid-flag and we surface the original
        // tail verbatim so the user sees the malformed input.
        let end = (i + 1 + arg_count).min(toks.len());
        let chunk: Vec<&str> = toks[i..end].to_vec();
        opts.push(chunk.join(" "));
        i = end;
    }
    let filename = toks[i..].join(" ");
    (opts, filename)
}

/// Number of arguments that follow a known `map_*` option flag, per
/// the MTL spec. Unknown flags return 0 → the parser preserves the
/// flag literally and treats the next token as the filename.
fn flag_arg_count(flag: &str) -> usize {
    match flag {
        "-blendu" | "-blendv" | "-cc" | "-clamp" => 1, // on | off
        "-bm" | "-boost" | "-texres" => 1,             // single float / int
        "-imfchan" | "-type" => 1,                     // single char / keyword
        "-mm" => 2,                                    // base gain
        // `-o`, `-s`, `-t` are documented as `u [v] [w]` — variable
        // arity. We greedily consume up to three numeric tokens after
        // the flag in `consume_uvw`, but the static count is 3 so
        // well-formed inputs round-trip cleanly. If a path follows
        // earlier than expected (e.g. `-o 1 path.png`), the path
        // accidentally absorbs the missing v / w; users who need that
        // edge case can supply explicit zeros.
        "-o" | "-s" | "-t" => 3,
        _ => 0,
    }
}

/// Parse a `map_*`-style keyword: split into (options, filename),
/// stash the options in `extras["mtl:<keyword>:options"]`, and return
/// the bare filename for caller-side TextureRef wiring.
///
/// In addition to the verbatim `:options` array (which drives encoder
/// round-trip), a decomposed typed view of each recognised flag lands
/// on `extras["mtl:<keyword>:options_typed"]`. See
/// [`decompose_map_options`] for the schema.
fn parse_map_with_options(
    keyword: &str,
    tokens: &mut std::str::SplitWhitespace<'_>,
    extras: &mut std::collections::HashMap<String, serde_json::Value>,
) -> String {
    let (opts, filename) = map_options_and_filename(tokens);
    if !opts.is_empty() {
        if let Some(typed) = decompose_map_options(&opts) {
            extras.insert(format!("mtl:{keyword}:options_typed"), typed);
        }
        extras.insert(
            format!("mtl:{keyword}:options"),
            serde_json::Value::Array(opts.into_iter().map(serde_json::Value::String).collect()),
        );
    }
    filename
}

/// Decompose a parsed `:options` array into a typed object per spec
/// §"Options for texture map statements".
///
/// The spec defines twelve flags that may appear before a `map_*` /
/// `bump` / `disp` / `decal` / `refl` filename. Each parsed `-flag
/// args` chunk lands under a stable lowercase key with a per-flag
/// value shape; flags the parser didn't recognise are silently skipped
/// so unknown chunks don't pollute the typed view (they still ride on
/// the raw `:options` array, which drives the encoder).
///
/// Key / value schema (each present only when the flag appeared):
///
/// * `blendu`, `blendv`, `clamp`, `cc` — `bool`. The spec writes these
///   as `on` / `off`; the typed value is `true` for `on` and `false`
///   for `off`. Any other argument value drops the flag from the typed
///   view (the raw array still preserves it).
/// * `bm`, `boost`, `texres` — `f64`. The spec writes these as a
///   single positive (`boost`, `texres`) or signed (`bm`) float; the
///   typed value is the parsed number.
/// * `imfchan` — `String` over the spec alphabet `r | g | b | m | l |
///   z` (§"-imfchan"). Anything else drops the flag.
/// * `type` — `String` over the spec alphabet `sphere | cube_top |
///   cube_bottom | cube_front | cube_back | cube_left | cube_right`
///   (§"refl -type"); other values drop the flag.
/// * `mm` — `[base, gain]` as a two-element `[f64; 2]` array (spec
///   §"-mm base gain"). Both arguments are required.
/// * `o`, `s`, `t` — `[u, v, w]` as a three-element `[f64; 3]` array
///   (spec §"-o u v w" / §"-s u v w" / §"-t u v w"). The spec marks
///   `v` and `w` optional (defaulting to 0 for `-o` / `-t` and 1 for
///   `-s`); the typed view fills the omitted slots accordingly so the
///   array shape stays stable.
///
/// Returns `None` when none of the recognised flags appeared, so
/// callers can skip the `options_typed` key entirely for option lists
/// composed of only unknown flags. The raw `:options` array still
/// drives encoder output; this typed view is parse-time-only and is
/// not consulted by the encoder (so the encoder still round-trips the
/// exact source-order tokens).
fn decompose_map_options(opts: &[String]) -> Option<serde_json::Value> {
    let mut obj = serde_json::Map::new();
    for chunk in opts {
        let mut it = chunk.split_whitespace();
        let flag = it.next()?;
        let args: Vec<&str> = it.collect();
        match flag {
            // Boolean on|off flags
            "-blendu" | "-blendv" | "-clamp" | "-cc" => {
                if args.len() != 1 {
                    continue;
                }
                let v = match args[0] {
                    "on" => true,
                    "off" => false,
                    _ => continue,
                };
                let key = &flag[1..];
                obj.insert(key.to_string(), serde_json::Value::Bool(v));
            }
            // Single-float flags
            "-bm" | "-boost" | "-texres" => {
                if args.len() != 1 {
                    continue;
                }
                let Ok(v) = args[0].parse::<f64>() else {
                    continue;
                };
                let key = &flag[1..];
                obj.insert(
                    key.to_string(),
                    serde_json::Number::from_f64(v)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null),
                );
            }
            // -imfchan: single-letter channel selector per spec alphabet
            "-imfchan" => {
                if args.len() != 1 {
                    continue;
                }
                let v = args[0];
                if !matches!(v, "r" | "g" | "b" | "m" | "l" | "z") {
                    continue;
                }
                obj.insert(
                    "imfchan".to_string(),
                    serde_json::Value::String(v.to_string()),
                );
            }
            // -type: keyword selector used by `refl` per spec §"refl -type"
            "-type" => {
                if args.len() != 1 {
                    continue;
                }
                let v = args[0];
                if !matches!(
                    v,
                    "sphere"
                        | "cube_top"
                        | "cube_bottom"
                        | "cube_front"
                        | "cube_back"
                        | "cube_left"
                        | "cube_right"
                ) {
                    continue;
                }
                obj.insert("type".to_string(), serde_json::Value::String(v.to_string()));
            }
            // -mm base gain: exactly two floats
            "-mm" => {
                if args.len() != 2 {
                    continue;
                }
                let Ok(base) = args[0].parse::<f64>() else {
                    continue;
                };
                let Ok(gain) = args[1].parse::<f64>() else {
                    continue;
                };
                obj.insert(
                    "mm".to_string(),
                    serde_json::Value::Array(vec![
                        serde_json::Number::from_f64(base)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null),
                        serde_json::Number::from_f64(gain)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null),
                    ]),
                );
            }
            // -o / -s / -t: 1..=3 floats, defaults fill the spec-defined slots
            "-o" | "-s" | "-t" => {
                if args.is_empty() || args.len() > 3 {
                    continue;
                }
                let mut parsed: Vec<f64> = Vec::with_capacity(args.len());
                let mut ok = true;
                for a in &args {
                    match a.parse::<f64>() {
                        Ok(n) => parsed.push(n),
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    continue;
                }
                // Spec defaults per §"-o u v w" / §"-s u v w" / §"-t u v w":
                //   -o: default (0, 0, 0)
                //   -s: default (1, 1, 1)
                //   -t: default (0, 0, 0)
                let default = if flag == "-s" { 1.0 } else { 0.0 };
                while parsed.len() < 3 {
                    parsed.push(default);
                }
                let arr: Vec<serde_json::Value> = parsed
                    .into_iter()
                    .map(|v| {
                        serde_json::Number::from_f64(v)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null)
                    })
                    .collect();
                let key = &flag[1..];
                obj.insert(key.to_string(), serde_json::Value::Array(arr));
            }
            // Unknown flag — leave to the raw :options array.
            _ => {}
        }
    }
    if obj.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(obj))
    }
}

/// Decompose an `illum` integer into the spec's property table.
///
/// The Wavefront MTL spec §"illum illum_#" summarises each model
/// (0..=10) as a small set of shading-property flags ("Color on /
/// Ambient on / Highlight on / Reflection on / Ray trace on /
/// Transparency: Glass on / Transparency: Refraction on /
/// Reflection: Fresnel on / Casts shadows onto invisible surfaces").
/// This routine mirrors that table verbatim so consumers can
/// introspect a material's shading intent without re-deriving it.
///
/// The returned [`serde_json::Value`] is always an object with **all**
/// boolean flag keys present, set `true` or `false` per the spec
/// table, so callers can safely `.get(key).and_then(as_bool)` without
/// distinguishing "key missing" from "explicitly false". Values
/// outside `0..=10` return `None` (the raw integer still lands in
/// `mtl:illum`).
///
/// Flag keys (stable, lowercase, underscore-separated):
///
/// * `color` — true for models 0–9 (model 10 is a shadowmatte, no
///   colour). Per spec, every non-shadowmatte model emits Kd-driven
///   colour.
/// * `ambient` — true for models 1–9 (model 0 is "Color on and
///   Ambient *off*"; model 10 has no shading at all).
/// * `highlight` — true for models with a specular term (2–9).
/// * `reflection` — true when the model includes a reflection term
///   (3, 4, 5, 6, 7, 8, 9).
/// * `ray_trace` — true when the spec table says "Ray trace on"
///   (3, 4, 5, 6, 7); models 8 and 9 explicitly say "Ray trace off".
/// * `transparency_glass` — true for models 4 and 9 ("Transparency:
///   Glass on" per spec).
/// * `transparency_refraction` — true for models 6 and 7
///   ("Transparency: Refraction on" per spec).
/// * `fresnel` — true for models 5 and 7 ("Reflection: Fresnel on");
///   explicitly false for 6 ("Reflection: Fresnel off"). Other
///   models leave Fresnel unmentioned and the flag is false.
/// * `casts_shadow_on_invisible` — true only for model 10
///   ("Casts shadows onto invisible surfaces").
fn illum_property_map(n: i32) -> Option<serde_json::Value> {
    if !(0..=10).contains(&n) {
        return None;
    }
    // Spec table from p.5-30, mirrored verbatim per row:
    //   0   Color on and Ambient off
    //   1   Color on and Ambient on
    //   2   Highlight on
    //   3   Reflection on and Ray trace on
    //   4   Transparency: Glass on; Reflection: Ray trace on
    //   5   Reflection: Fresnel on and Ray trace on
    //   6   Transparency: Refraction on; Reflection: Fresnel off, Ray trace on
    //   7   Transparency: Refraction on; Reflection: Fresnel on, Ray trace on
    //   8   Reflection on and Ray trace off
    //   9   Transparency: Glass on; Reflection: Ray trace off
    //   10  Casts shadows onto invisible surfaces
    //
    // Model 2's spec row ("Highlight on") doesn't restate "Color on /
    // Ambient on" because those carry over from model 1; models 3..=9
    // similarly inherit the diffuse+ambient base by virtue of starting
    // from model 2's equation. We surface that inheritance explicitly:
    // every non-shadowmatte (0..=9) gets `color = true`, and every
    // shaded non-flat model (1..=9) gets `ambient = true`.
    let color = (0..=9).contains(&n);
    let ambient = (1..=9).contains(&n);
    let highlight = (2..=9).contains(&n);
    let reflection = matches!(n, 3..=9);
    let ray_trace = matches!(n, 3..=7);
    let transparency_glass = matches!(n, 4 | 9);
    let transparency_refraction = matches!(n, 6 | 7);
    let fresnel = matches!(n, 5 | 7);
    let casts_shadow_on_invisible = n == 10;
    Some(serde_json::json!({
        "color": color,
        "ambient": ambient,
        "highlight": highlight,
        "reflection": reflection,
        "ray_trace": ray_trace,
        "transparency_glass": transparency_glass,
        "transparency_refraction": transparency_refraction,
        "fresnel": fresnel,
        "casts_shadow_on_invisible": casts_shadow_on_invisible,
    }))
}

// ---------------------------------------------------------------------------
// Serialisation
// ---------------------------------------------------------------------------

/// Serialise a slice of materials to MTL format.
///
/// Texture references are emitted via the `External { uri, .. }`
/// variant — the URI is written verbatim. Embedded / Source textures
/// are skipped (no on-disk path to point at); a one-line comment
/// identifies the gap so the file is round-trip-able under the same
/// invariants as the decoder.
pub fn serialize_mtl(materials: &[Material], textures: &[Texture]) -> Result<Vec<u8>> {
    use std::fmt::Write;
    let mut out = String::new();
    writeln!(out, "# MTL generated by oxideav-obj").unwrap();

    for (i, mat) in materials.iter().enumerate() {
        let name = mat.name.clone().unwrap_or_else(|| format!("material_{i}"));
        writeln!(out, "newmtl {name}").unwrap();

        // Ka — one of three mutually-exclusive forms per spec §"Ka r g b" /
        // §"Ka spectral" / §"Ka xyz". The sibling-key precedence order
        // mirrors the source ordering in the spec listing.
        emit_color_statement(&mut out, "Ka", &mat.extras);
        // Kd — same three forms per spec §"Kd r g b" / §"Kd spectral" /
        // §"Kd xyz". The canonical RGB form populates `base_color`
        // directly, so we emit it from there; the alt forms ride on
        // extras (`mtl:Kd:spectral` / `mtl:Kd:xyz`) and suppress the
        // canonical line so the round-trip matches the operator-written
        // spelling.
        if let Some(serde_json::Value::Object(o)) = mat.extras.get("mtl:Kd:spectral") {
            let file = o.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let factor = o.get("factor").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
            if (factor - 1.0).abs() < f32::EPSILON {
                writeln!(out, "Kd spectral {file}").unwrap();
            } else {
                writeln!(out, "Kd spectral {file} {}", fmt_f(factor)).unwrap();
            }
        } else if let Some(serde_json::Value::Array(v)) = mat.extras.get("mtl:Kd:xyz") {
            if let [a, b, c] = v.as_slice() {
                writeln!(
                    out,
                    "Kd xyz {} {} {}",
                    fmt_f(a.as_f64().unwrap_or(0.0) as f32),
                    fmt_f(b.as_f64().unwrap_or(0.0) as f32),
                    fmt_f(c.as_f64().unwrap_or(0.0) as f32)
                )
                .unwrap();
            }
        } else {
            // Canonical RGB form: always emit (it's the glTF base
            // color → MTL Phong diffuse).
            writeln!(
                out,
                "Kd {} {} {}",
                fmt_f(mat.base_color[0]),
                fmt_f(mat.base_color[1]),
                fmt_f(mat.base_color[2])
            )
            .unwrap();
        }
        // Ks — same three forms per spec §"Ks r g b" / §"Ks spectral" /
        // §"Ks xyz".
        emit_color_statement(&mut out, "Ks", &mat.extras);
        if mat.emissive_factor != [0.0, 0.0, 0.0] {
            writeln!(
                out,
                "Ke {} {} {}",
                fmt_f(mat.emissive_factor[0]),
                fmt_f(mat.emissive_factor[1]),
                fmt_f(mat.emissive_factor[2])
            )
            .unwrap();
        }
        if let Some(v) = mat.extras.get("mtl:Ns").and_then(|v| v.as_f64()) {
            writeln!(out, "Ns {}", fmt_f(v as f32)).unwrap();
        }
        if let Some(v) = mat.extras.get("mtl:Ni").and_then(|v| v.as_f64()) {
            writeln!(out, "Ni {}", fmt_f(v as f32)).unwrap();
        }
        // Tf transmission filter — one of three mutually exclusive
        // forms per spec §"Tf". Only the first present extras key is
        // emitted (per the spec's mutual-exclusion clause).
        if let Some(serde_json::Value::Array(v)) = mat.extras.get("mtl:Tf") {
            if let [a, b, c] = v.as_slice() {
                writeln!(
                    out,
                    "Tf {} {} {}",
                    fmt_f(a.as_f64().unwrap_or(0.0) as f32),
                    fmt_f(b.as_f64().unwrap_or(0.0) as f32),
                    fmt_f(c.as_f64().unwrap_or(0.0) as f32)
                )
                .unwrap();
            }
        } else if let Some(serde_json::Value::Object(o)) = mat.extras.get("mtl:Tf:spectral") {
            // `Tf spectral file.rfl factor` — `factor` defaults to 1.0
            // and is omitted from the emit when it equals the default,
            // so the round-trip matches the most common operator-written
            // form.
            let file = o.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let factor = o.get("factor").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
            if (factor - 1.0).abs() < f32::EPSILON {
                writeln!(out, "Tf spectral {file}").unwrap();
            } else {
                writeln!(out, "Tf spectral {file} {}", fmt_f(factor)).unwrap();
            }
        } else if let Some(serde_json::Value::Array(v)) = mat.extras.get("mtl:Tf:xyz") {
            if let [a, b, c] = v.as_slice() {
                writeln!(
                    out,
                    "Tf xyz {} {} {}",
                    fmt_f(a.as_f64().unwrap_or(0.0) as f32),
                    fmt_f(b.as_f64().unwrap_or(0.0) as f32),
                    fmt_f(c.as_f64().unwrap_or(0.0) as f32)
                )
                .unwrap();
            }
        }
        // sharpness — scalar, MTL spec §"sharpness value".
        if let Some(v) = mat.extras.get("mtl:sharpness").and_then(|v| v.as_f64()) {
            writeln!(out, "sharpness {}", fmt_f(v as f32)).unwrap();
        }
        if mat.base_color[3] < 1.0 || matches!(mat.alpha_mode, AlphaMode::Blend) {
            // Emit `d -halo <factor>` when the parser captured a halo
            // dissolve, otherwise the canonical `d <value>` form.
            if mat.extras.contains_key("mtl:d_halo_factor") {
                writeln!(out, "d -halo {}", fmt_f(mat.base_color[3])).unwrap();
            } else {
                writeln!(out, "d {}", fmt_f(mat.base_color[3])).unwrap();
            }
        }
        if let Some(v) = mat.extras.get("mtl:illum").and_then(|v| v.as_i64()) {
            writeln!(out, "illum {v}").unwrap();
        }
        // PBR fields — only emit when the user actually carries PBR values.
        // The mesh3d default is metallic=1.0 / roughness=1.0; our parser
        // resets those to 0 / 0.5 when constructing from MTL, so any
        // non-default value is taken to indicate "PBR is in use".
        let pbr_in_use = mat.metallic != 0.0
            || (mat.roughness - 0.5).abs() > f32::EPSILON
            || mat.metallic_roughness_texture.is_some()
            || mat.extras.contains_key("mtl:Pc")
            || mat.extras.contains_key("mtl:Ps");
        if pbr_in_use {
            writeln!(out, "Pr {}", fmt_f(mat.roughness)).unwrap();
            writeln!(out, "Pm {}", fmt_f(mat.metallic)).unwrap();
        }
        if let Some(v) = mat.extras.get("mtl:Pc").and_then(|v| v.as_f64()) {
            writeln!(out, "Pc {}", fmt_f(v as f32)).unwrap();
        }
        if let Some(v) = mat.extras.get("mtl:Ps").and_then(|v| v.as_f64()) {
            writeln!(out, "Ps {}", fmt_f(v as f32)).unwrap();
        }

        // Texture references — splice any saved `-flag value` option
        // chunks back ahead of the filename so the round-trip emits
        // `map_Kd -clamp on path.png` instead of just `map_Kd path.png`.
        write_tex_ref(
            &mut out,
            "map_Kd",
            mat.base_color_texture,
            textures,
            &mat.extras,
        );
        write_tex_ref(
            &mut out,
            "map_Bump",
            mat.normal_texture,
            textures,
            &mat.extras,
        );
        write_tex_ref(
            &mut out,
            "map_Pr",
            mat.metallic_roughness_texture,
            textures,
            &mat.extras,
        );
        write_tex_ref(
            &mut out,
            "map_Ke",
            mat.emissive_texture,
            textures,
            &mat.extras,
        );

        // Typed reflection-map sets per spec §"Reflection Map":
        // `refl -type sphere file` and the six `refl -type cube_*`
        // faces. Each face emits as its own line; option flags
        // captured per-face are spliced ahead of the filename.
        if let Some(serde_json::Value::Object(o)) = mat.extras.get("mtl:refl:sphere") {
            let file = o.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let opts: Vec<&str> = o
                .get("options")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
                .unwrap_or_default();
            if opts.is_empty() {
                writeln!(out, "refl -type sphere {file}").unwrap();
            } else {
                writeln!(out, "refl -type sphere {} {file}", opts.join(" ")).unwrap();
            }
        }
        if let Some(serde_json::Value::Object(faces)) = mat.extras.get("mtl:refl:cube") {
            // Fixed face order — keeps the round-trip diff stable
            // regardless of HashMap insertion order.
            for face in [
                "cube_top",
                "cube_bottom",
                "cube_front",
                "cube_back",
                "cube_left",
                "cube_right",
                "cube_side",
            ] {
                let Some(serde_json::Value::Object(entry)) = faces.get(face) else {
                    continue;
                };
                let file = entry.get("file").and_then(|v| v.as_str()).unwrap_or("");
                let opts: Vec<&str> = entry
                    .get("options")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
                    .unwrap_or_default();
                if opts.is_empty() {
                    writeln!(out, "refl -type {face} {file}").unwrap();
                } else {
                    writeln!(out, "refl -type {face} {} {file}", opts.join(" ")).unwrap();
                }
            }
        }

        // Pass-through extras — `mtl:*` keys we didn't consume above.
        for (k, v) in &mat.extras {
            if !k.starts_with("mtl:") {
                continue;
            }
            // Skip the keys we already printed above.
            match k.as_str() {
                "mtl:Ka"
                | "mtl:Ka:spectral"
                | "mtl:Ka:xyz"
                | "mtl:Kd:spectral"
                | "mtl:Kd:xyz"
                | "mtl:Ks"
                | "mtl:Ks:spectral"
                | "mtl:Ks:xyz"
                | "mtl:Ns"
                | "mtl:Ni"
                | "mtl:illum"
                | "mtl:illum_props"
                | "mtl:Pc"
                | "mtl:Ps"
                | "mtl:Tf"
                | "mtl:Tf:spectral"
                | "mtl:Tf:xyz"
                | "mtl:sharpness"
                | "mtl:displaced_pbr_map"
                | "mtl:d_halo_factor"
                | "mtl:refl:sphere"
                | "mtl:refl:cube" => continue,
                _ => {}
            }
            // `mtl:<map>:options` chunks are spliced inline by
            // write_tex_ref / the bare-`disp`-etc pass-through; skip
            // them here so they don't double-emit as a standalone line.
            // `mtl:<map>:options_typed` is the parse-time-only
            // decomposed view of the same data — never emitted.
            if k.ends_with(":options") || k.ends_with(":options_typed") {
                continue;
            }
            // Only emit string-valued passthrough keys (textures we didn't model);
            // numeric ones we don't consume just stay as side-channel metadata.
            if let Some(s) = v.as_str() {
                let kw = k.strip_prefix("mtl:").unwrap_or(k.as_str());
                // Splice options ahead of the filename for keys that
                // have an associated `:options` companion (disp /
                // decal / refl / map_Ka / map_Ks / map_Ns / map_d).
                let opts_key = format!("mtl:{kw}:options");
                if let Some(serde_json::Value::Array(opts)) = mat.extras.get(&opts_key) {
                    let parts: Vec<&str> = opts.iter().filter_map(|o| o.as_str()).collect();
                    writeln!(out, "{kw} {} {s}", parts.join(" ")).unwrap();
                } else {
                    writeln!(out, "{kw} {s}").unwrap();
                }
            }
        }

        out.push('\n');
    }

    Ok(out.into_bytes())
}

fn write_tex_ref(
    out: &mut String,
    keyword: &str,
    ref_: Option<TextureRef>,
    textures: &[Texture],
    extras: &std::collections::HashMap<String, serde_json::Value>,
) {
    use std::fmt::Write;
    let Some(r) = ref_ else { return };
    let Some(tex) = textures.get(r.texture.0 as usize) else {
        return;
    };
    if let ImageData::External { uri, .. } = &tex.image {
        // Splice any saved option flags ahead of the filename. The
        // options key uses the canonical map keyword (e.g. `map_Bump`)
        // even when the user originally wrote `bump` / `map_bump` /
        // `norm` — those alias keywords store options under whatever
        // spelling the user used, so try both.
        let opts_key = format!("mtl:{keyword}:options");
        let alt_keys: &[&str] = match keyword {
            "map_Bump" => &[
                "mtl:map_bump:options",
                "mtl:bump:options",
                "mtl:norm:options",
            ],
            _ => &[],
        };
        let opts = extras
            .get(&opts_key)
            .or_else(|| alt_keys.iter().find_map(|k| extras.get(*k)));
        if let Some(serde_json::Value::Array(arr)) = opts {
            let parts: Vec<&str> = arr.iter().filter_map(|o| o.as_str()).collect();
            if parts.is_empty() {
                writeln!(out, "{keyword} {uri}").unwrap();
            } else {
                writeln!(out, "{keyword} {} {uri}", parts.join(" ")).unwrap();
            }
        } else {
            writeln!(out, "{keyword} {uri}").unwrap();
        }
    }
}

/// Emit a `Ka` / `Ks` MTL colour statement, picking whichever of the
/// three mutually-exclusive forms (RGB / `spectral` / `xyz`) the parser
/// captured into extras. The sibling-key precedence is `spectral` →
/// `xyz` → plain `mtl:<keyword>` array (the RGB form). When no key is
/// present the statement is omitted entirely.
///
/// `Kd` is handled inline because its canonical RGB form is mirrored on
/// `Material::base_color` (the glTF base colour), not on
/// `extras["mtl:Kd"]`.
fn emit_color_statement(
    out: &mut String,
    keyword: &str,
    extras: &std::collections::HashMap<String, serde_json::Value>,
) {
    use std::fmt::Write;
    let spectral_key = format!("mtl:{keyword}:spectral");
    let xyz_key = format!("mtl:{keyword}:xyz");
    let rgb_key = format!("mtl:{keyword}");
    if let Some(serde_json::Value::Object(o)) = extras.get(&spectral_key) {
        let file = o.get("file").and_then(|v| v.as_str()).unwrap_or("");
        let factor = o.get("factor").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
        if (factor - 1.0).abs() < f32::EPSILON {
            writeln!(out, "{keyword} spectral {file}").unwrap();
        } else {
            writeln!(out, "{keyword} spectral {file} {}", fmt_f(factor)).unwrap();
        }
    } else if let Some(serde_json::Value::Array(v)) = extras.get(&xyz_key) {
        if let [a, b, c] = v.as_slice() {
            writeln!(
                out,
                "{keyword} xyz {} {} {}",
                fmt_f(a.as_f64().unwrap_or(0.0) as f32),
                fmt_f(b.as_f64().unwrap_or(0.0) as f32),
                fmt_f(c.as_f64().unwrap_or(0.0) as f32)
            )
            .unwrap();
        }
    } else if let Some(serde_json::Value::Array(v)) = extras.get(&rgb_key) {
        if let [a, b, c] = v.as_slice() {
            writeln!(
                out,
                "{keyword} {} {} {}",
                fmt_f(a.as_f64().unwrap_or(0.0) as f32),
                fmt_f(b.as_f64().unwrap_or(0.0) as f32),
                fmt_f(c.as_f64().unwrap_or(0.0) as f32)
            )
            .unwrap();
        }
    }
}

fn fmt_f(x: f32) -> String {
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_phong() {
        let text = "newmtl Red\nKd 1.0 0.0 0.0\nKa 0.1 0.1 0.1\nNs 32\n";
        let mats = parse_mtl(text).unwrap();
        assert_eq!(mats.len(), 1);
        let m = &mats[0];
        assert_eq!(m.name.as_deref(), Some("Red"));
        assert_eq!(m.base_color[0..3], [1.0, 0.0, 0.0]);
        assert_eq!(
            m.extras
                .get("mtl:Ka")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(3)
        );
        assert_eq!(m.extras.get("mtl:Ns").and_then(|v| v.as_f64()), Some(32.0));
    }

    #[test]
    fn dissolve_sets_alpha_blend() {
        let mats = parse_mtl("newmtl Glass\nKd 0.5 0.5 0.5\nd 0.4\n").unwrap();
        assert_eq!(mats[0].base_color[3], 0.4);
        assert!(matches!(mats[0].alpha_mode, AlphaMode::Blend));
    }

    #[test]
    fn tr_alternate_dissolve() {
        let mats = parse_mtl("newmtl Glass\nKd 0.5 0.5 0.5\nTr 0.4\n").unwrap();
        // Tr = 1 - d  ⇒  d = 0.6
        assert!((mats[0].base_color[3] - 0.6).abs() < 1e-6);
        assert!(matches!(mats[0].alpha_mode, AlphaMode::Blend));
    }

    #[test]
    fn pbr_extension_lands_in_pbr_slots() {
        let mats =
            parse_mtl("newmtl Steel\nKd 0.7 0.7 0.7\nPr 0.25\nPm 0.95\nPc 0.5\nPs 0.1\n").unwrap();
        let m = &mats[0];
        assert!((m.roughness - 0.25).abs() < 1e-6);
        assert!((m.metallic - 0.95).abs() < 1e-6);
        let pc = m.extras.get("mtl:Pc").and_then(|v| v.as_f64()).unwrap();
        assert!((pc - 0.5).abs() < 1e-6);
        let ps = m.extras.get("mtl:Ps").and_then(|v| v.as_f64()).unwrap();
        assert!((ps - 0.1).abs() < 1e-6);
    }

    #[test]
    fn map_kd_pending_uri_round_trips() {
        let mats = parse_mtl("newmtl Tex\nKd 1 1 1\nmap_Kd diffuse.png\n").unwrap();
        let pending = mats[0].extras.get("mtl:pending_textures").unwrap();
        assert_eq!(pending["base_color"].as_str(), Some("diffuse.png"));
    }
}
