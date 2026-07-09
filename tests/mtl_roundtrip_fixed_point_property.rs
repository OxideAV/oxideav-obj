//! Generative fixed-point property for the MTL serialiser.
//!
//! The companion of `roundtrip_fixed_point_property.rs` (which pins the
//! OBJ geometry serialiser): a decode → encode of an MTL document must
//! reach a textual fixed point after one round-trip, i.e.
//!
//! ```text
//! serialize(parse(serialize(parse(x)))) == serialize(parse(x))
//! ```
//!
//! across documents mixing every material-statement family (Phong
//! colours in RGB / spectral / xyz forms, `d` / `Tr`, `Ni`, `illum`,
//! `Tf`, `sharpness`, the Wavefront-PBR scalars, and the full texture-
//! map family with `-flag value` option chunks). A deterministic
//! pseudo-random generator walks the statement space so the assertion
//! covers a broad matrix without a hand-written corpus.
//!
//! This caught two real serialiser-fidelity bugs:
//!
//!  * `map_Pr` / `map_Pm` both map onto glTF's single packed
//!    `metallic_roughness_texture`. The typed slot was emitted as a
//!    hard-wired `map_Pr` *in addition to* the pass-through re-emit of
//!    the operator's original `mtl:map_Pr` / `mtl:map_Pm` extras, so the
//!    map double-emitted — and a `map_Pm`-only source grew a spurious
//!    `map_Pr` line on every round-trip.
//!  * `Material::extras` is a `HashMap`, so the pass-through map
//!    emission (`map_Ka` / `map_Ks` / `decal` / …) rode on the map's
//!    randomised iteration order; a re-parse built a fresh map with a
//!    different order, so the second serialisation reshuffled the lines.
//!
//! Both are now fixed; the property asserts panic-freedom *and* the
//! one-round-trip fixed point together.

use oxideav_obj::mtl;

/// Tiny deterministic xorshift PRNG so the corpus is reproducible and
/// the test carries no external `rand` dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, m: u64) -> u64 {
        self.next() % m
    }
    fn choice<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len() as u64) as usize]
    }
    fn unit(&mut self) -> f64 {
        (self.below(1000) as f64) / 1000.0
    }
}

const MAPS: &[&str] = &[
    "map_Kd",
    "map_Ka",
    "map_Ks",
    "map_Ke",
    "map_Ns",
    "map_d",
    "map_Bump",
    "disp",
    "decal",
    "map_Pr",
    "map_Pm",
    "map_Ps",
    "map_Pc",
    "map_Pcr",
    "map_aniso",
    "map_anisor",
];

fn colour(r: &mut Rng) -> String {
    match r.below(6) {
        0 => "spectral file.rfl 1.0".to_string(),
        1 => "xyz 0.1 0.2 0.3".to_string(),
        _ => format!("{:.3} {:.3} {:.3}", r.unit(), r.unit(), r.unit()),
    }
}

fn options(r: &mut Rng) -> String {
    let mut o = String::new();
    if r.below(2) == 0 {
        o.push_str(&format!("-o {:.2} ", r.unit() * 2.0));
    }
    if r.below(2) == 0 {
        o.push_str(&format!("-s {:.2} {:.2} ", r.unit() * 2.0, r.unit() * 2.0));
    }
    if r.below(2) == 0 {
        o.push_str("-clamp on ");
    }
    if r.below(2) == 0 {
        o.push_str(&format!("-bm {:.2} ", r.unit() * 2.0));
    }
    if r.below(2) == 0 {
        o.push_str("-blendu off ");
    }
    if r.below(2) == 0 {
        o.push_str("-imfchan r ");
    }
    o
}

fn gen_mtl(r: &mut Rng) -> String {
    let mut s = String::new();
    let n_mat = 1 + r.below(4);
    for m in 0..n_mat {
        s.push_str(&format!("newmtl mat{m}\n"));
        if r.below(2) == 0 {
            s.push_str(&format!("Ka {}\n", colour(r)));
        }
        if r.below(2) == 0 {
            s.push_str(&format!("Kd {}\n", colour(r)));
        }
        if r.below(2) == 0 {
            s.push_str(&format!("Ks {}\n", colour(r)));
        }
        if r.below(2) == 0 {
            s.push_str(&format!(
                "Ke {:.3} {:.3} {:.3}\n",
                r.unit(),
                r.unit(),
                r.unit()
            ));
        }
        if r.below(2) == 0 {
            s.push_str(&format!("Ns {:.2}\n", r.unit() * 100.0));
        }
        if r.below(2) == 0 {
            s.push_str(&format!("Ni {:.3}\n", 1.0 + r.unit()));
        }
        if r.below(2) == 0 {
            s.push_str(&format!("d {:.3}\n", r.unit()));
        }
        if r.below(3) == 0 {
            s.push_str(&format!("Tf {}\n", colour(r)));
        }
        if r.below(2) == 0 {
            s.push_str(&format!("illum {}\n", r.below(11)));
        }
        if r.below(3) == 0 {
            s.push_str(&format!("sharpness {}\n", r.below(1000)));
        }
        if r.below(2) == 0 {
            s.push_str(&format!("Pr {:.3}\n", r.unit()));
        }
        if r.below(2) == 0 {
            s.push_str(&format!("Pm {:.3}\n", r.unit()));
        }
        if r.below(3) == 0 {
            s.push_str(&format!("Pc {:.3}\n", r.unit()));
        }
        if r.below(3) == 0 {
            s.push_str(&format!("Ps {:.3}\n", r.unit()));
        }
        let n_maps = r.below(6);
        for _ in 0..n_maps {
            let mk = r.choice(MAPS);
            s.push_str(&format!("{mk} {}tex_{}.png\n", options(r), r.below(3)));
        }
        if r.below(4) == 0 {
            s.push_str("map_aat on\n");
        }
        if r.below(4) == 0 {
            s.push_str(&format!("refl -type sphere {}reflect.mpc\n", options(r)));
        }
    }
    s
}

fn serialize_once(text: &str) -> Option<String> {
    let scene = mtl::parse_mtl_with_scene(text).ok()?;
    let bytes = mtl::serialize_mtl(&scene.materials, &scene.textures).ok()?;
    Some(String::from_utf8(bytes).expect("MTL serialiser emits UTF-8"))
}

#[test]
fn mtl_serializer_reaches_a_fixed_point() {
    let mut rng = Rng(0x0BAD_C0DE_1234_5678);
    let mut checked = 0u32;
    for _ in 0..4000 {
        let doc = gen_mtl(&mut rng);
        let Some(once) = serialize_once(&doc) else {
            continue;
        };
        let twice = serialize_once(&once).expect("re-parse of our own output must succeed");
        assert_eq!(
            once, twice,
            "serialize(parse(x)) is not a fixed point.\n\
             --- input ---\n{doc}\n--- once ---\n{once}\n--- twice ---\n{twice}"
        );
        checked += 1;
    }
    assert!(checked > 3000, "generator produced too few parseable docs");
}

/// Minimised pin for the `map_Pm`-only phantom-`map_Pr` growth bug.
#[test]
fn map_pm_only_does_not_grow_a_phantom_map_pr() {
    let once = serialize_once("newmtl m\nmap_Pm metallic.png\n").expect("parses");
    assert!(
        once.contains("map_Pm metallic.png"),
        "map_Pm must survive the round-trip: {once}"
    );
    assert!(
        !once.contains("map_Pr"),
        "a map_Pm-only source must not fabricate a map_Pr line: {once}"
    );
    let twice = serialize_once(&once).expect("re-parses");
    assert_eq!(once, twice, "map_Pm round-trip must be a fixed point");
}

/// Minimised pin for the unstable pass-through map ordering bug.
#[test]
fn passthrough_map_order_is_stable() {
    let src = "newmtl m\nmap_Ns a.png\ndecal b.png\nmap_Ka c.png\n";
    let once = serialize_once(src).expect("parses");
    let twice = serialize_once(&once).expect("re-parses");
    assert_eq!(
        once, twice,
        "pass-through map emission order must not depend on HashMap seed"
    );
}
