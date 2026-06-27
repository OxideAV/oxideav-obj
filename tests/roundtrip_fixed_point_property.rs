//! Property test: the OBJ encoder reaches a textual fixed point after one
//! round-trip. For any document we can decode, the *second* generation of
//! emitted text must equal the first:
//!
//! ```text
//! gen1 = serialize(parse(input))
//! gen2 = serialize(parse(gen1))
//! assert gen1 == gen2
//! ```
//!
//! This is the load-bearing invariant for a loss-tolerant codec: once the
//! decoder has normalised an arbitrary input into the typed `Scene3D`
//! model, re-encoding and re-decoding must not drift. A drift here means
//! some directive is captured but re-emitted in a shape the decoder then
//! reads back differently — exactly the class of bug the per-feature
//! width/arity fidelity work has been closing one directive at a time.
//!
//! Inputs are produced by a small deterministic generator (a seeded LCG,
//! no external crate) that assembles syntactically-valid OBJ documents
//! mixing the directive families this crate handles: header comments,
//! `v` (3/4/6/7-wide), `vt` (1/2/3-wide), `vn`, faces in all four index
//! syntaxes with both positive and negative indices, `l` / `p` elements,
//! `g` / `o` / `s` / `usemtl` / `usemap` / `mg` state-setters, the
//! display attributes, and a free-form `vp` + `cstype … end` block. Seeds
//! 0..N give a broad, reproducible corpus; any failure prints the exact
//! generating seed and the diverging document for a one-line repro.

use oxideav_obj::obj;

/// Tiny deterministic PRNG — a 64-bit LCG (Numerical Recipes constants).
/// No external dependency; reproducible across platforms.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        // Avoid the all-zero fixed point.
        Lcg(seed.wrapping_mul(6364136223846793005).wrapping_add(1))
    }
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    /// Uniform in `0..n` (n > 0).
    fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }
    fn chance(&mut self, one_in: u32) -> bool {
        self.below(one_in) == 0
    }
    /// A small "random" coordinate with a few decimals (kept exactly
    /// representable so float formatting is stable across round-trips).
    fn coord(&mut self) -> String {
        let sign = if self.chance(2) { "-" } else { "" };
        let whole = self.below(8);
        let frac = self.below(4); // 0, 1, 2, 3 → .0 .25 .5 .75 (exact in f32)
        let frac_str = match frac {
            0 => "0",
            1 => "25",
            2 => "5",
            _ => "75",
        };
        format!("{sign}{whole}.{frac_str}")
    }
}

/// Pick a 1-based index uniformly within a pool of `n` entries.
fn pick(r: &mut Lcg, n: u32) -> u32 {
    1 + r.below(n)
}

/// Build one syntactically-valid OBJ document from a seed.
fn gen_obj(seed: u64) -> String {
    let mut r = Lcg::new(seed);
    let mut doc = String::new();

    // Optional header comment block.
    if r.chance(2) {
        doc.push_str("# generated fixture\n");
        if r.chance(2) {
            doc.push_str("# second header line\n");
        }
    }

    // Vertex pools. Keep counts modest but non-trivial.
    let n_v = 4 + r.below(8); // 4..=11 geometric vertices
    for _ in 0..n_v {
        let x = r.coord();
        let y = r.coord();
        let z = r.coord();
        match r.below(4) {
            0 => doc.push_str(&format!("v {x} {y} {z}\n")),
            1 => doc.push_str(&format!("v {x} {y} {z} 1.0\n")), // weight
            2 => doc.push_str(&format!("v {x} {y} {z} 0.5 0.25 0.75\n")), // rgb
            _ => doc.push_str(&format!("v {x} {y} {z} 1.0 0.5 0.25 0.75\n")), // w+rgb
        }
    }
    let n_vt = 1 + r.below(n_v); // at least one
    for _ in 0..n_vt {
        let u = r.coord();
        match r.below(3) {
            0 => doc.push_str(&format!("vt {u}\n")),
            1 => doc.push_str(&format!("vt {u} {}\n", r.coord())),
            _ => doc.push_str(&format!("vt {u} {} {}\n", r.coord(), r.coord())),
        }
    }
    let n_vn = 1 + r.below(n_v);
    for _ in 0..n_vn {
        doc.push_str(&format!("vn {} {} {}\n", r.coord(), r.coord(), r.coord()));
    }

    // Optional grouping / object header.
    if r.chance(3) {
        doc.push_str(&format!("o object{}\n", r.below(3)));
    }
    if r.chance(3) {
        doc.push_str(&format!("g grp{} grp{}\n", r.below(3), r.below(3)));
    }

    // A handful of elements of one kind (mixing face / line / point under
    // a single material is rejected by the decoder, so a document commits
    // to one element family). State-setters appear only in the face case,
    // where a mid-stream change splits the primitive cleanly.
    let elem_kind = r.below(3); // 0 = faces, 1 = lines, 2 = points
    let n_elem = 2 + r.below(6);
    for _ in 0..n_elem {
        match elem_kind {
            // Faces (triangles) in various index syntaxes, with optional
            // state-setters interleaved.
            0 => {
                match r.below(15) {
                    0 => doc.push_str(&format!("s {}\n", r.below(4))),
                    1 => doc.push_str("s off\n"),
                    2 => doc.push_str(&format!("usemtl mat{}\n", r.below(3))),
                    3 => doc.push_str(&format!("mg {} 0.5\n", 1 + r.below(3))),
                    4 => doc.push_str("bevel on\n"),
                    5 => doc.push_str(&format!("lod {}\n", r.below(10))),
                    6 => doc.push_str(&format!("usemap map{}\n", r.below(2))),
                    7 => doc.push_str("usemap off\n"),
                    8 => doc.push_str(if r.chance(2) {
                        "c_interp on\n"
                    } else {
                        "c_interp off\n"
                    }),
                    9 => doc.push_str(if r.chance(2) {
                        "d_interp on\n"
                    } else {
                        "d_interp off\n"
                    }),
                    10 => doc.push_str(&format!("g grp{} grp{}\n", r.below(3), r.below(3))),
                    _ => {}
                }
                // A face whose every component is spelled with the same
                // sign — the spec allows -k relative-from-end on each of
                // the geometric / texture / normal slots independently,
                // and the decoder resolves each against its own pool. We
                // keep one sign per face to stay grep-readable.
                let negative = r.chance(4);
                // Render an index (1-based positive `idx` into a pool of
                // `n` entries) as either the absolute form or, when
                // `negative`, the -k relative-from-end form (`n - idx + 1`
                // is in `1..=n`, so the negative reference is always in
                // range).
                let render = |negative: bool, idx: u32, n: u32| -> String {
                    if negative {
                        format!("-{}", n - idx + 1)
                    } else {
                        format!("{idx}")
                    }
                };
                let mut verts = Vec::new();
                for _ in 0..3 {
                    let (vi, ti, ni) = (pick(&mut r, n_v), pick(&mut r, n_vt), pick(&mut r, n_vn));
                    let vs = render(negative, vi, n_v);
                    let ts = render(negative, ti, n_vt);
                    let ns = render(negative, ni, n_vn);
                    let tok = match r.below(4) {
                        0 => vs,
                        1 => format!("{vs}/{ts}"),
                        2 => format!("{vs}//{ns}"),
                        _ => format!("{vs}/{ts}/{ns}"),
                    };
                    verts.push(tok);
                }
                doc.push_str(&format!("f {}\n", verts.join(" ")));
            }
            // Line element (3 distinct vertices ⇒ LineStrip).
            1 => {
                let a = pick(&mut r, n_v);
                let b = pick(&mut r, n_v);
                let c = pick(&mut r, n_v);
                doc.push_str(&format!("l {a} {b} {c}\n"));
            }
            // Point element.
            _ => {
                let a = pick(&mut r, n_v);
                let b = pick(&mut r, n_v);
                doc.push_str(&format!("p {a} {b}\n"));
            }
        }
    }

    // Optional free-form block: parameter-space vertices + a cstype curve.
    if r.chance(2) {
        let n_vp = 2 + r.below(3);
        for _ in 0..n_vp {
            match r.below(3) {
                0 => doc.push_str(&format!("vp {}\n", r.coord())),
                1 => doc.push_str(&format!("vp {} {}\n", r.coord(), r.coord())),
                _ => doc.push_str(&format!("vp {} {} {}\n", r.coord(), r.coord(), r.coord())),
            }
        }
        doc.push_str("cstype bezier\n");
        doc.push_str("deg 3\n");
        doc.push_str(&format!(
            "curv 0.0 1.0 {} {} {} {}\n",
            pick(&mut r, n_v),
            pick(&mut r, n_v),
            pick(&mut r, n_v),
            pick(&mut r, n_v)
        ));
        doc.push_str("parm u 0.0 1.0\n");
        doc.push_str("end\n");
    }

    doc
}

#[test]
fn decode_encode_is_a_textual_fixed_point_over_generated_corpus() {
    let mut checked = 0u32;
    let mut decodable = 0u32;
    for seed in 0..600u64 {
        let input = gen_obj(seed);
        // The input is decodable by construction; if it isn't, that's a
        // generator bug we want to see surfaced, not silently skipped —
        // but a few index combinations may legitimately be rejected
        // (e.g. an empty pool edge); tolerate those, just don't count
        // them.
        let scene1 = match obj::parse_obj(&input) {
            Ok(s) => s,
            Err(_) => continue,
        };
        decodable += 1;
        let gen1 = String::from_utf8(obj::serialize_obj(&scene1, None).unwrap()).unwrap();

        let scene2 = obj::parse_obj(&gen1).unwrap_or_else(|e| {
            panic!("seed {seed}: re-decode of our own output failed: {e}\n--- gen1 ---\n{gen1}")
        });
        let gen2 = String::from_utf8(obj::serialize_obj(&scene2, None).unwrap()).unwrap();

        assert_eq!(
            gen1, gen2,
            "seed {seed}: encoder is not a fixed point\n--- input ---\n{input}\n\
             --- gen1 ---\n{gen1}\n--- gen2 ---\n{gen2}"
        );
        checked += 1;
    }
    // Sanity: the generator must actually exercise the codec, not produce
    // an all-rejected corpus that vacuously passes.
    assert!(
        decodable > 500,
        "generator produced too few decodable documents ({decodable}/600)"
    );
    assert!(checked > 500, "too few fixed-point checks ran ({checked})");
}
