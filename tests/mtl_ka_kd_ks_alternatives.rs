//! Wavefront MTL `Ka` / `Kd` / `Ks` alternative-form coverage.
//!
//! Spec §"Ka r g b" / §"Kd r g b" / §"Ks r g b" each list the same
//! three mutually-exclusive forms (mirroring `Tf`):
//!
//!   K{a,d,s} r g b
//!   K{a,d,s} spectral file.rfl factor
//!   K{a,d,s} xyz x y z
//!
//! Rounds 1..5 only handled the RGB form. This round lifts the
//! `spectral` and `xyz` forms into [`oxideav_mesh3d::Material::extras`]
//! under the sibling keys
//! `mtl:K{a,d,s}:spectral` (a `{file, factor}` object) and
//! `mtl:K{a,d,s}:xyz` (an `[x, y, z]` array), mirroring the existing
//! `Tf:spectral` / `Tf:xyz` channel shape. The encoder picks the first
//! present key on a per-material basis.
//!
//! Also covers the spec's defaulting rules:
//! * Plain RGB: "If only r is specified, then g, and b are assumed to
//!   be equal to r."
//! * `xyz`: "y and z arguments are optional. If only x is specified,
//!   then y and z are assumed to be equal to x."
//! * `spectral`: "factor is an optional argument … defaults to 1.0, if
//!   not specified."

use oxideav_obj::mtl;

#[test]
fn ka_spectral_with_factor_round_trips() {
    let text = "newmtl A\nKd 1 1 1\nKa spectral ambient.rfl 0.5\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let m = &mats[0];
    let obj = m
        .extras
        .get("mtl:Ka:spectral")
        .and_then(|v| v.as_object())
        .expect("Ka spectral captured");
    assert_eq!(
        obj.get("file").and_then(|v| v.as_str()),
        Some("ambient.rfl")
    );
    let factor = obj.get("factor").and_then(|v| v.as_f64()).unwrap();
    assert!((factor - 0.5).abs() < 1e-6);
    // The mutually-exclusive RGB form must NOT be populated.
    assert!(!m.extras.contains_key("mtl:Ka"));
    assert!(!m.extras.contains_key("mtl:Ka:xyz"));

    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let line = s
        .lines()
        .find(|l| l.starts_with("Ka "))
        .expect("Ka line emitted");
    let toks: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(toks, vec!["Ka", "spectral", "ambient.rfl", "0.5"]);
}

#[test]
fn ka_spectral_default_factor_is_implicit_on_emit() {
    let text = "newmtl A\nKd 1 1 1\nKa spectral amb.rfl\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let factor = mats[0]
        .extras
        .get("mtl:Ka:spectral")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("factor"))
        .and_then(|v| v.as_f64())
        .unwrap();
    assert!((factor - 1.0).abs() < 1e-6);

    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let line = s.lines().find(|l| l.starts_with("Ka ")).unwrap();
    let toks: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(toks, vec!["Ka", "spectral", "amb.rfl"]);
}

#[test]
fn ka_xyz_round_trips_three_values() {
    let text = "newmtl A\nKd 1 1 1\nKa xyz 0.2 0.3 0.4\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let arr = mats[0]
        .extras
        .get("mtl:Ka:xyz")
        .and_then(|v| v.as_array())
        .expect("Ka xyz captured");
    let xs: Vec<f64> = arr.iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(xs.len(), 3);
    assert!((xs[0] - 0.2).abs() < 1e-6);
    assert!((xs[1] - 0.3).abs() < 1e-6);
    assert!((xs[2] - 0.4).abs() < 1e-6);

    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let line = s.lines().find(|l| l.starts_with("Ka ")).unwrap();
    let toks: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(toks, vec!["Ka", "xyz", "0.2", "0.3", "0.4"]);
}

#[test]
fn ka_xyz_single_value_broadcasts_to_three() {
    // Spec §"Ka xyz x y z": y and z default to x when omitted.
    let text = "newmtl A\nKd 1 1 1\nKa xyz 0.3\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let arr = mats[0]
        .extras
        .get("mtl:Ka:xyz")
        .and_then(|v| v.as_array())
        .unwrap();
    let xs: Vec<f64> = arr.iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(xs.len(), 3);
    assert!((xs[0] - 0.3).abs() < 1e-6);
    assert!((xs[1] - 0.3).abs() < 1e-6);
    assert!((xs[2] - 0.3).abs() < 1e-6);
}

#[test]
fn ka_rgb_single_value_broadcasts_to_three() {
    // Spec §"Ka r g b": g and b default to r when omitted.
    let text = "newmtl A\nKd 1 1 1\nKa 0.4\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let arr = mats[0]
        .extras
        .get("mtl:Ka")
        .and_then(|v| v.as_array())
        .unwrap();
    let xs: Vec<f64> = arr.iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(xs.len(), 3);
    assert!((xs[0] - 0.4).abs() < 1e-6);
    assert!((xs[1] - 0.4).abs() < 1e-6);
    assert!((xs[2] - 0.4).abs() < 1e-6);

    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let line = s.lines().find(|l| l.starts_with("Ka ")).unwrap();
    let toks: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(toks, vec!["Ka", "0.4", "0.4", "0.4"]);
}

#[test]
fn kd_spectral_suppresses_canonical_rgb_emit() {
    // `Kd spectral` is mutually exclusive with `Kd r g b`; the encoder
    // must NOT also emit the default `base_color` triple.
    let text = "newmtl D\nKd spectral diff.rfl 0.8\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let obj = mats[0]
        .extras
        .get("mtl:Kd:spectral")
        .and_then(|v| v.as_object())
        .expect("Kd spectral captured");
    assert_eq!(obj.get("file").and_then(|v| v.as_str()), Some("diff.rfl"));
    let factor = obj.get("factor").and_then(|v| v.as_f64()).unwrap();
    assert!((factor - 0.8).abs() < 1e-6);

    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    // Exactly one Kd line and it should be the spectral form.
    let kd_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("Kd ")).collect();
    assert_eq!(kd_lines.len(), 1);
    let toks: Vec<&str> = kd_lines[0].split_whitespace().collect();
    assert_eq!(toks, vec!["Kd", "spectral", "diff.rfl", "0.8"]);
}

#[test]
fn kd_xyz_suppresses_canonical_rgb_emit() {
    let text = "newmtl D\nKd xyz 0.1 0.2 0.3\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let arr = mats[0]
        .extras
        .get("mtl:Kd:xyz")
        .and_then(|v| v.as_array())
        .expect("Kd xyz captured");
    assert_eq!(arr.len(), 3);

    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let kd_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("Kd ")).collect();
    assert_eq!(kd_lines.len(), 1);
    let toks: Vec<&str> = kd_lines[0].split_whitespace().collect();
    assert_eq!(toks, vec!["Kd", "xyz", "0.1", "0.2", "0.3"]);
}

#[test]
fn ks_spectral_round_trips() {
    let text = "newmtl S\nKd 1 1 1\nKs spectral spec.rfl 0.25\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let obj = mats[0]
        .extras
        .get("mtl:Ks:spectral")
        .and_then(|v| v.as_object())
        .expect("Ks spectral captured");
    let factor = obj.get("factor").and_then(|v| v.as_f64()).unwrap();
    assert!((factor - 0.25).abs() < 1e-6);
    assert!(!mats[0].extras.contains_key("mtl:Ks"));

    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let line = s.lines().find(|l| l.starts_with("Ks ")).unwrap();
    let toks: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(toks, vec!["Ks", "spectral", "spec.rfl", "0.25"]);
}

#[test]
fn ks_xyz_round_trips() {
    let text = "newmtl S\nKd 1 1 1\nKs xyz 0.7\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let arr = mats[0]
        .extras
        .get("mtl:Ks:xyz")
        .and_then(|v| v.as_array())
        .unwrap();
    let xs: Vec<f64> = arr.iter().map(|v| v.as_f64().unwrap()).collect();
    assert!((xs[0] - 0.7).abs() < 1e-6);
    assert!((xs[1] - 0.7).abs() < 1e-6);
    assert!((xs[2] - 0.7).abs() < 1e-6);

    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let line = s.lines().find(|l| l.starts_with("Ks ")).unwrap();
    let toks: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(toks, vec!["Ks", "xyz", "0.7", "0.7", "0.7"]);
}

#[test]
fn rgb_form_unaffected_by_alt_form_support() {
    // Regression: the existing RGB-form behaviour for the three
    // statements must still hold and remain mutually exclusive with
    // the alt-form sibling keys.
    let text = "newmtl C\nKa 0.1 0.2 0.3\nKd 0.4 0.5 0.6\nKs 0.7 0.8 0.9\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let m = &mats[0];

    let ka: Vec<f64> = m
        .extras
        .get("mtl:Ka")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert!((ka[0] - 0.1).abs() < 1e-6);
    assert!((ka[1] - 0.2).abs() < 1e-6);
    assert!((ka[2] - 0.3).abs() < 1e-6);
    assert!(!m.extras.contains_key("mtl:Ka:spectral"));
    assert!(!m.extras.contains_key("mtl:Ka:xyz"));

    assert!((m.base_color[0] - 0.4).abs() < 1e-6);
    assert!((m.base_color[1] - 0.5).abs() < 1e-6);
    assert!((m.base_color[2] - 0.6).abs() < 1e-6);
    assert!(!m.extras.contains_key("mtl:Kd:spectral"));
    assert!(!m.extras.contains_key("mtl:Kd:xyz"));

    let ks: Vec<f64> = m
        .extras
        .get("mtl:Ks")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert!((ks[0] - 0.7).abs() < 1e-6);
    assert!((ks[1] - 0.8).abs() < 1e-6);
    assert!((ks[2] - 0.9).abs() < 1e-6);
    assert!(!m.extras.contains_key("mtl:Ks:spectral"));
    assert!(!m.extras.contains_key("mtl:Ks:xyz"));
}
