//! Wavefront MTL `Tf` alternative-form coverage.
//!
//! Spec §"Tf" lists three mutually-exclusive forms:
//!
//!   Tf r g b
//!   Tf spectral file.rfl factor
//!   Tf xyz x y z
//!
//! Round 4 only handled the RGB form. Round 5 lifts the spectral and
//! XYZ forms into [`oxideav_mesh3d::Material::extras`] under the
//! sibling keys `mtl:Tf:spectral` (a `{file, factor}` object) and
//! `mtl:Tf:xyz` (an `[x, y, z]` array). The encoder picks the first
//! present key on a per-material basis.

use oxideav_obj::mtl;

#[test]
fn tf_spectral_with_factor_round_trips() {
    let text = "newmtl Smoke\nKd 0.5 0.5 0.5\nTf spectral smoke.rfl 0.75\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let m = &mats[0];
    let obj = m
        .extras
        .get("mtl:Tf:spectral")
        .and_then(|v| v.as_object())
        .expect("Tf spectral captured");
    assert_eq!(obj.get("file").and_then(|v| v.as_str()), Some("smoke.rfl"));
    let factor = obj.get("factor").and_then(|v| v.as_f64()).unwrap();
    assert!((factor - 0.75).abs() < 1e-6);

    // The plain RGB key must NOT be set — the forms are mutually exclusive.
    assert!(
        !m.extras.contains_key("mtl:Tf"),
        "spectral form must not also populate mtl:Tf"
    );

    // Re-encode and verify the directive shape.
    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let line = s
        .lines()
        .find(|l| l.starts_with("Tf "))
        .expect("Tf line emitted");
    let toks: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(toks, vec!["Tf", "spectral", "smoke.rfl", "0.75"]);
}

#[test]
fn tf_spectral_default_factor_is_implicit_on_emit() {
    // Per spec, factor defaults to 1.0 when omitted; the encoder should
    // omit the explicit "1" so the round-trip matches the original
    // spelling.
    let text = "newmtl Sm\nKd 1 1 1\nTf spectral fog.rfl\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let line = s.lines().find(|l| l.starts_with("Tf ")).unwrap();
    let toks: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(toks, vec!["Tf", "spectral", "fog.rfl"]);
}

#[test]
fn tf_xyz_round_trips_three_values() {
    let text = "newmtl Glass\nKd 0.5 0.5 0.5\nTf xyz 0.1 0.2 0.3\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let arr = mats[0]
        .extras
        .get("mtl:Tf:xyz")
        .and_then(|v| v.as_array())
        .expect("Tf xyz captured");
    assert_eq!(arr.len(), 3);
    let xs: Vec<f64> = arr.iter().map(|v| v.as_f64().unwrap()).collect();
    assert!((xs[0] - 0.1).abs() < 1e-6);
    assert!((xs[1] - 0.2).abs() < 1e-6);
    assert!((xs[2] - 0.3).abs() < 1e-6);

    // Re-encode and verify.
    let bytes = mtl::serialize_mtl(&mats, &[]).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    let line = s.lines().find(|l| l.starts_with("Tf ")).unwrap();
    let toks: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(toks, vec!["Tf", "xyz", "0.1", "0.2", "0.3"]);
}

#[test]
fn tf_xyz_single_value_broadcasts_to_three() {
    // Spec: "y and z arguments are optional. If only x is specified,
    // then y and z are assumed to be equal to x."
    let text = "newmtl C\nKd 1 1 1\nTf xyz 0.4\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let arr = mats[0]
        .extras
        .get("mtl:Tf:xyz")
        .and_then(|v| v.as_array())
        .unwrap();
    let xs: Vec<f64> = arr.iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(xs.len(), 3);
    assert!((xs[0] - 0.4).abs() < 1e-6);
    assert!((xs[1] - 0.4).abs() < 1e-6);
    assert!((xs[2] - 0.4).abs() < 1e-6);
}

#[test]
fn tf_rgb_form_unaffected_by_alt_form_support() {
    // Regression: round-4 RGB-form behaviour must still hold.
    let text = "newmtl C\nKd 1 1 1\nTf 0.9 0.85 0.8\n";
    let mats = mtl::parse_mtl(text).unwrap();
    let arr = mats[0]
        .extras
        .get("mtl:Tf")
        .and_then(|v| v.as_array())
        .unwrap();
    let xs: Vec<f64> = arr.iter().map(|v| v.as_f64().unwrap()).collect();
    assert!((xs[0] - 0.9).abs() < 1e-6);
    assert!((xs[1] - 0.85).abs() < 1e-6);
    assert!((xs[2] - 0.8).abs() < 1e-6);
    assert!(!mats[0].extras.contains_key("mtl:Tf:spectral"));
    assert!(!mats[0].extras.contains_key("mtl:Tf:xyz"));
}
