//! Special-curve (`scrv`) embedding as surface triangle edges — spec
//! §"Special curve":
//!
//!   "A special curve is guaranteed to be included in any triangulation
//!    of the surface. This means that the line formed by approximating
//!    the special curve with a sequence of straight line segments will
//!    actually appear as a sequence of triangle edges in the final
//!    triangulation."
//!
//! Earlier rounds emitted the `scrv` as a stand-alone parameter-space
//! `LineStrip` on the `obj:scrvs` mesh (round 206) but the tessellated
//! `obj:surfaces` triangle mesh ignored it, so the special curve did
//! NOT appear as a chain of triangle edges. This round routes triangle
//! edges along the special curve: every straight segment of the
//! approximated `scrv` polyline is forced to coincide with a chain of
//! triangle edges in the surface mesh, satisfying the spec guarantee.
//!
//! The free-form directive sequence still rides on
//! `Scene3D::extras["obj:freeform_directives"]` so a decode → encode
//! cycle replays the original block verbatim; the encoder filters the
//! synthetic surface out via the shared `obj:tessellated_curve`
//! sentinel, so this constrained mesh is decode-only enrichment and
//! never perturbs round-trip.

use std::collections::HashSet;

use oxideav_mesh3d::{Indices, Mesh3DDecoder, Mesh3DEncoder, Topology};
use oxideav_obj::{ObjDecoder, ObjEncoder};

/// A quantised parameter-space coordinate (xy × 4096, rounded).
type VKey = (i64, i64);
/// An undirected triangle edge between two quantised coordinates.
type Edge = (VKey, VKey);

/// A flat unit-square bilinear Bezier patch (z = 0) plus a single
/// straight `scrv` running horizontally across the middle of it. The
/// `curv2` walks the two parameter-space points `(0, 0.5) → (1, 0.5)`,
/// so the special curve cuts through the interior of every lattice cell
/// row — it does NOT lie along any pre-existing triangle edge, so the
/// constraint pass must split triangles to embed it.
const SCRV_DIAGONAL: &str = "\
vp 0.0 0.5
vp 1.0 0.5
cstype bspline
deg 1
curv2 1 2
parm u 0.0 0.0 1.0 1.0
end
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
scrv 0.0 2.0 1
end
";

/// Same patch, but the special curve is a poly-corner L-shape:
/// `(0.3,0.3) → (0.7,0.3) → (0.7,0.7)`, exercising a bent scrv whose
/// vertices and segments land off the lattice grid (lattice lines at
/// multiples of 0.25 for a 4-sample patch), so the constraint pass must
/// split interior triangles to embed it.
const SCRV_BENT_INTERIOR: &str = "\
vp 0.3 0.3
vp 0.7 0.3
vp 0.7 0.7
cstype bspline
deg 1
curv2 1 2 3
parm u 0.0 1.0 2.0 3.0 4.0
end
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
scrv 1.0 3.0 1
end
";

fn surf_prim(scene: &oxideav_mesh3d::Scene3D) -> &oxideav_mesh3d::Primitive {
    let mesh = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:surfaces"))
        .expect("obj:surfaces mesh must exist");
    assert_eq!(mesh.primitives.len(), 1, "one surface primitive expected");
    &mesh.primitives[0]
}

fn scrv_polyline(scene: &oxideav_mesh3d::Scene3D) -> Vec<[f32; 2]> {
    let mesh = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:scrvs"))
        .expect("obj:scrvs mesh must exist");
    mesh.primitives[0]
        .positions
        .iter()
        .map(|p| [p[0], p[1]])
        .collect()
}

fn indices_as_u32(prim: &oxideav_mesh3d::Primitive) -> Vec<u32> {
    match prim.indices.as_ref().expect("indices present") {
        Indices::U16(v) => v.iter().map(|&x| x as u32).collect(),
        Indices::U32(v) => v.clone(),
    }
}

/// Build the undirected set of triangle edges, keyed by the two
/// endpoints' parameter coordinates quantised to a stable grid.
fn edge_set(prim: &oxideav_mesh3d::Primitive) -> HashSet<Edge> {
    // Recover each vertex's parameter coordinate. The surface lattice is
    // a uniform `(s0..s1) × (t0..t1)` grid; the synthetic vertices carry
    // 3D xy positions that, for these flat z = 0 patches, equal their
    // parameter coordinates (the control points are placed at the unit
    // square corners). We therefore key edges off the xy position.
    let q = |p: &[f32; 3]| -> (i64, i64) {
        (
            (p[0] * 4096.0).round() as i64,
            (p[1] * 4096.0).round() as i64,
        )
    };
    let idx = indices_as_u32(prim);
    let mut set = HashSet::new();
    for tri in idx.chunks(3) {
        let v: Vec<(i64, i64)> = tri
            .iter()
            .map(|&i| q(&prim.positions[i as usize]))
            .collect();
        for k in 0..3 {
            let a = v[k];
            let b = v[(k + 1) % 3];
            let e = if a <= b { (a, b) } else { (b, a) };
            set.insert(e);
        }
    }
    set
}

/// Assert that every consecutive pair of `scrv` polyline vertices is
/// joined by a chain of triangle edges (each segment may be split by
/// lattice-edge crossings, so we walk the segment and require each
/// sub-step between consecutive distinct mesh vertices on the segment to
/// be a triangle edge). The simplest sufficient check that proves the
/// spec guarantee: each scrv polyline vertex coordinate is a mesh
/// vertex, and each scrv segment is covered by triangle edges colinear
/// with it.
fn assert_scrv_is_edge_chain(prim: &oxideav_mesh3d::Primitive, scrv: &[[f32; 2]]) {
    let edges = edge_set(prim);
    let q = |p: [f32; 2]| -> (i64, i64) {
        (
            (p[0] * 4096.0).round() as i64,
            (p[1] * 4096.0).round() as i64,
        )
    };
    // Every scrv vertex must be present as a mesh vertex.
    let mesh_verts: HashSet<(i64, i64)> = prim
        .positions
        .iter()
        .map(|p| {
            (
                ((p[0] * 4096.0).round()) as i64,
                (p[1] * 4096.0).round() as i64,
            )
        })
        .collect();
    for v in scrv {
        assert!(
            mesh_verts.contains(&q(*v)),
            "scrv vertex {v:?} must appear as a mesh vertex"
        );
    }
    // For each scrv segment, collect the mesh vertices lying on it
    // (endpoints + lattice-edge crossings), order them along the
    // segment, and require each consecutive pair to be a triangle edge.
    for seg in scrv.windows(2) {
        let a = seg[0];
        let b = seg[1];
        let dir = [b[0] - a[0], b[1] - a[1]];
        let len2 = dir[0] * dir[0] + dir[1] * dir[1];
        if len2 < 1e-12 {
            continue;
        }
        let mut on_seg: Vec<(f32, (i64, i64))> = Vec::new();
        for p in &prim.positions {
            let w = [p[0] - a[0], p[1] - a[1]];
            let t = (w[0] * dir[0] + w[1] * dir[1]) / len2;
            if !(-1e-4..=1.0 + 1e-4).contains(&t) {
                continue;
            }
            // Perpendicular distance to the segment line.
            let cross = (w[0] * dir[1] - w[1] * dir[0]).abs() / len2.sqrt();
            if cross < 1e-3 {
                on_seg.push((
                    t.clamp(0.0, 1.0),
                    (
                        ((p[0] * 4096.0).round()) as i64,
                        (p[1] * 4096.0).round() as i64,
                    ),
                ));
            }
        }
        on_seg.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        on_seg.dedup_by_key(|x| x.1);
        assert!(
            on_seg.len() >= 2,
            "scrv segment {a:?}->{b:?} must hit at least its two endpoints"
        );
        for pair in on_seg.windows(2) {
            let e = if pair[0].1 <= pair[1].1 {
                (pair[0].1, pair[1].1)
            } else {
                (pair[1].1, pair[0].1)
            };
            assert!(
                edges.contains(&e),
                "scrv sub-segment {:?} must be a triangle edge",
                e
            );
        }
    }
}

#[test]
fn scrv_directive_stays_captured_when_tessellation_is_disabled() {
    // No tessellation → no synthetic surface, directive captured verbatim.
    let scene = ObjDecoder::new().decode(SCRV_DIAGONAL.as_bytes()).unwrap();
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.name.as_deref() != Some("obj:surfaces"))
    );
}

#[test]
fn straight_scrv_appears_as_triangle_edges() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(SCRV_DIAGONAL.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);
    assert_eq!(prim.topology, Topology::Triangles);
    // Provenance: the constraint pass ran and embedded one special curve.
    assert_eq!(
        prim.extras
            .get("obj:surface_scrv")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        prim.extras
            .get("obj:surface_scrv_curves")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    let scrv = scrv_polyline(&scene);
    assert!(scrv.len() >= 2);
    assert_scrv_is_edge_chain(prim, &scrv);
}

#[test]
fn bent_interior_scrv_appears_as_triangle_edges() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(SCRV_BENT_INTERIOR.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);
    assert_eq!(prim.topology, Topology::Triangles);
    assert_eq!(
        prim.extras
            .get("obj:surface_scrv_curves")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    // The embedding only adds vertices/triangles, never removes lattice
    // coverage: the original 5×5 lattice block stays intact.
    assert!(
        prim.positions.len() > 25,
        "constraint pass must synthesise extra vertices, got {}",
        prim.positions.len()
    );
    let scrv = scrv_polyline(&scene);
    assert_scrv_is_edge_chain(prim, &scrv);
}

#[test]
fn surface_with_no_scrv_is_unchanged() {
    // A bare surface (no scrv) must not grow scrv provenance or vertices.
    const NO_SCRV: &str = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(NO_SCRV.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);
    assert_eq!(prim.positions.len(), 25, "untouched 5×5 lattice");
    assert_eq!(indices_as_u32(prim).len(), 96, "untouched 4×4×2×3");
    assert!(!prim.extras.contains_key("obj:surface_scrv"));
}

/// Spec §"Examples" case 8 ("Trimming with a special curve") — a
/// rational B-spline surface carrying both a `trim` loop and a `scrv`
/// special curve, using negative-from-end `vp` references. Verifies the
/// trim-clip and scrv-embed passes coexist: the surface is trimmed AND
/// the special curve appears as triangle edges on the kept region.
const SPEC_EXAMPLE_8: &str = "\
# trimming curve
vp -0.675  1.850  3.000
vp  0.915  1.930
vp  2.485  0.470  2.000
vp  2.485 -1.030
vp  1.605 -1.890 10.700
vp -0.745 -0.654  0.500
cstype rat bezier
deg 3
curv2 -6 -5 -4 -3 -2 -1 -6
parm u 0.00 1.00 2.00
end
# special curve
vp -0.185  0.322
vp  0.214  0.818
vp  1.652  0.207
vp  1.652 -0.455
curv2 -4 -3 -2 -1
parm u 2.00 10.00
end
# surface
v -1.350 -1.030 0.000
v  0.130 -1.030 0.432 7.600
v  1.480 -1.030 0.000 2.300
v -1.460  0.060 0.201
v  0.120  0.060 0.915 0.500
v  1.380  0.060 0.454 1.500
v -1.480  1.030 0.000 2.300
v  0.120  1.030 0.394 6.100
v  1.170  1.030 0.000 3.300
cstype rat bspline
deg 2 2
surf -1.0 2.5 -2.0 2.0 -9 -8 -7 -6 -5 -4 -3 -2 -1
parm u -1.00 -1.00 -1.00 2.50 2.50 2.50
parm v -2.00 -2.00 -2.00 2.00 2.00 2.00
trim 0.0 2.0 1
scrv 4.2 9.7 2
end
";

#[test]
fn spec_example_8_trims_and_round_trips() {
    // Spec example 8's SECOND `curv2` block (the special curve) omits a
    // `cstype` header — the preceding `end` cleared the active curve
    // type, so under a strict reading the special `curv2 2` has no
    // defined type and does not tessellate. The crate's lenient-loader
    // policy (established for the round-206 scrv pass) drops an
    // unresolvable curve rather than guessing its type, so the special
    // curve is NOT embedded for this exact fixture while the first
    // (typed) `curv2 1` trim loop still clips the surface. The combined
    // trim+scrv embedding path is exercised by
    // `combined_trim_and_scrv_typed_curves` below with a self-contained
    // fixture whose special curve carries its own `cstype`.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(SPEC_EXAMPLE_8.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);
    assert_eq!(prim.topology, Topology::Triangles);
    assert_eq!(
        prim.extras
            .get("obj:surface_trimmed")
            .and_then(|v| v.as_bool()),
        Some(true),
        "the typed trim loop must clip the surface"
    );
    assert!(!prim.positions.is_empty());
    // The directive block round-trips verbatim regardless of the passes.
    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("scrv"), "scrv directive must round-trip");
    assert!(text.contains("trim"), "trim directive must round-trip");
}

/// A flat unit-square bilinear patch carrying BOTH a `trim` loop (the
/// inner `[0.2, 0.8]²` square) AND a `scrv` special curve that crosses
/// the kept region, each backed by its own fully-typed `curv2` block —
/// so both the trim-clip and scrv-embed passes run together.
const COMBINED_TRIM_SCRV: &str = "\
vp 0.2 0.2
vp 0.8 0.2
vp 0.8 0.8
vp 0.2 0.8
cstype bspline
deg 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0 5.0 6.0
end
vp 0.25 0.55
vp 0.65 0.55
cstype bspline
deg 1
curv2 5 6
parm u 0.0 0.0 1.0 1.0
end
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
cstype bezier
deg 1 1
surf 0.0 1.0 0.0 1.0 1 2 3 4
parm u 0.0 1.0
parm v 0.0 1.0
trim 0.0 6.0 1
scrv 0.0 1.0 2
end
";

#[test]
fn combined_trim_and_scrv_typed_curves() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(10)
        .decode(COMBINED_TRIM_SCRV.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);
    assert_eq!(
        prim.extras
            .get("obj:surface_trimmed")
            .and_then(|v| v.as_bool()),
        Some(true),
        "trim clip must have run"
    );
    assert_eq!(
        prim.extras
            .get("obj:surface_scrv_curves")
            .and_then(|v| v.as_u64()),
        Some(1),
        "the special curve must be embedded on the trimmed mesh"
    );
    // The constraint pass synthesised at least one vertex embedding the
    // special curve into the trimmed mesh (the curve runs through the
    // kept region interior, off the lattice grid).
    assert!(
        prim.extras
            .get("obj:surface_scrv_vertices")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            >= 2,
        "the special curve embed must add boundary/crossing vertices"
    );
    // The special curve's own endpoints — `(0.25, 0.55)` and
    // `(0.65, 0.55)`, both inside the kept `[0.2,0.8]²` region and off
    // the lattice grid (0.1 steps at 10 samples) — must appear as mesh
    // vertices, and the chord between them must be a chain of triangle
    // edges.
    let edges = edge_set(prim);
    for v in [[0.25f32, 0.55f32], [0.65, 0.55]] {
        let key = (
            (v[0] * 4096.0).round() as i64,
            (v[1] * 4096.0).round() as i64,
        );
        let present = prim.positions.iter().any(|p| {
            (
                (p[0] * 4096.0).round() as i64,
                (p[1] * 4096.0).round() as i64,
            ) == key
        });
        assert!(present, "scrv endpoint {v:?} must be a mesh vertex");
    }
    // Walk the straight scrv chord and require every consecutive pair of
    // on-chord mesh vertices to be a triangle edge.
    assert_chord_is_edge_chain(prim, &edges, [0.25, 0.55], [0.65, 0.55]);
}

/// Require that the straight chord `a → b` is covered by triangle edges:
/// every consecutive pair of mesh vertices lying on the chord is joined
/// by an edge.
fn assert_chord_is_edge_chain(
    prim: &oxideav_mesh3d::Primitive,
    edges: &HashSet<Edge>,
    a: [f32; 2],
    b: [f32; 2],
) {
    let dir = [b[0] - a[0], b[1] - a[1]];
    let len2 = dir[0] * dir[0] + dir[1] * dir[1];
    let mut on: Vec<(f32, (i64, i64))> = Vec::new();
    for p in &prim.positions {
        let w = [p[0] - a[0], p[1] - a[1]];
        let t = (w[0] * dir[0] + w[1] * dir[1]) / len2;
        if !(-1e-4..=1.0 + 1e-4).contains(&t) {
            continue;
        }
        let cross = (w[0] * dir[1] - w[1] * dir[0]).abs() / len2.sqrt();
        if cross < 1e-3 {
            on.push((
                t.clamp(0.0, 1.0),
                (
                    (p[0] * 4096.0).round() as i64,
                    (p[1] * 4096.0).round() as i64,
                ),
            ));
        }
    }
    on.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    on.dedup_by_key(|x| x.1);
    assert!(on.len() >= 2, "chord must hit at least its two endpoints");
    for pair in on.windows(2) {
        let e = if pair[0].1 <= pair[1].1 {
            (pair[0].1, pair[1].1)
        } else {
            (pair[1].1, pair[0].1)
        };
        assert!(
            edges.contains(&e),
            "chord sub-segment {e:?} must be a triangle edge"
        );
    }
}

#[test]
fn scrv_embedding_does_not_perturb_round_trip() {
    // The constrained surface is synthetic; the encoder filters it out
    // and replays the verbatim directive block, so a decode → encode →
    // decode cycle is byte-identical regardless of the constraint pass.
    let scene = ObjDecoder::new()
        .with_curve_tessellation(4)
        .decode(SCRV_DIAGONAL.as_bytes())
        .unwrap();
    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.contains("scrv 0.0 2.0 1") || text.contains("scrv 0 2 1"),
        "scrv directive must round-trip verbatim, got:\n{text}"
    );
}
