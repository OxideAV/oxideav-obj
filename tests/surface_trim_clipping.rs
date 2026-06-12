//! Surface trim/hole clipping — under the
//! `ObjDecoder::with_curve_tessellation(N)` knob, every `surf` element
//! whose enclosing `cstype … end` block also carries one or more
//! `trim` / `hole` directives has its tessellated triangle grid
//! clipped against the parameter-space loop(s) those directives
//! assemble from `curv2` segments. Spec §"Trimming Loops",
//! §"trim u0 u1 curv2d …", §"hole u0 u1 curv2d …", §"curv2".
//!
//! Spec semantics:
//!
//!   * A `trim` builds an **outer** trimming loop (the surface is
//!     visible inside it).
//!   * A `hole` cuts an **inner** trimming loop out of the enclosing
//!     trim region (the surface is invisible inside it).
//!   * "If no trim or hole statements are specified, then the surface
//!     is trimmed at its parameter range." We honour this implicitly:
//!     no clipping happens when both lists are empty.
//!   * Curv2 index references are 1-based and global — independent of
//!     which `cstype … end` block first declared the curv2.
//!
//! Implementation note: the surface is rasterised at the same
//! `(samples + 1) × (samples + 1)` lattice the un-clipped path uses;
//! each lattice vertex's `(u, v)` parameter coordinate is point-in-
//! polygon-tested against the assembled loops. Fully-kept triangles
//! emit unchanged and fully-dropped triangles vanish; **straddling**
//! boundary triangles (1 or 2 corners kept) are sub-cell re-meshed —
//! each crossing lattice edge is bisected in parameter space until the
//! in/out frontier is pinned, the synthesised boundary vertex is
//! appended after the lattice block, and the kept sub-polygon (corner
//! triangle or quad) is emitted with the original winding. Crossings
//! are cached per undirected edge so the re-meshed rim is watertight;
//! degenerate slivers (loops grazing a lattice line) are suppressed
//! and their unreferenced boundary vertices garbage-collected.
//!
//! The free-form directive sequence itself still rides on
//! `Scene3D::extras["obj:freeform_directives"]` so a decode → encode
//! cycle replays the original `cstype` / `surf` / `trim` / `hole` /
//! `end` block verbatim — the encoder filters out synthetic clipped
//! geometry via the shared `obj:tessellated_curve` sentinel exactly
//! like the un-clipped surface path.

use oxideav_mesh3d::{Indices, Mesh3DDecoder, Topology};
use oxideav_obj::ObjDecoder;

/// A flat bilinear Bezier patch on the unit square in xy (z = 0 for
/// every control point) with a single square `trim` loop whose corners
/// are the four `vp` parameter vertices and whose perimeter is spelled
/// out by a degree-1 (linear) `curv2`. The loop range is `[0.2, 0.8]`
/// in both u and v, so a 10-sample lattice keeps only the inner 6×6
/// vertex block (60 % of the parameter rectangle in each direction).
const TRIMMED_BILINEAR_SURF: &str = "\
vp 0.2 0.2
vp 0.8 0.2
vp 0.8 0.8
vp 0.2 0.8
cstype bspline
deg 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0 5.0 6.0
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
end
";

fn surf_prim(scene: &oxideav_mesh3d::Scene3D) -> &oxideav_mesh3d::Primitive {
    // The synthetic mesh holding the clipped triangle grid.
    let mesh = scene
        .meshes
        .iter()
        .find(|m| m.name.as_deref() == Some("obj:surfaces"))
        .expect("obj:surfaces mesh must exist");
    assert_eq!(mesh.primitives.len(), 1, "one surface primitive expected");
    &mesh.primitives[0]
}

fn indices_as_u32(prim: &oxideav_mesh3d::Primitive) -> Vec<u32> {
    match prim.indices.as_ref().expect("indices present") {
        Indices::U16(v) => v.iter().map(|&x| x as u32).collect(),
        Indices::U32(v) => v.clone(),
    }
}

#[test]
fn surface_with_no_trim_or_hole_emits_a_full_lattice() {
    // Baseline: same surface without the `trim` line emits the full
    // 4 × 4 cell grid = 32 triangles (96 indices). Sanity-checks that
    // the trim/hole branch doesn't run when neither directive is set.
    const NO_TRIM: &str = "\
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
        .decode(NO_TRIM.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);
    assert_eq!(prim.topology, Topology::Triangles);
    assert_eq!(prim.positions.len(), 25, "5×5 vertex lattice");
    assert_eq!(indices_as_u32(prim).len(), 96, "4×4 cells × 2 × 3");
    // The trim/hole extras must NOT appear when no clip ran.
    assert!(
        !prim.extras.contains_key("obj:surface_trimmed"),
        "obj:surface_trimmed must only appear when a clip actually ran"
    );
}

#[test]
fn trim_loop_clips_triangles_outside_the_square() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(10)
        .decode(TRIMMED_BILINEAR_SURF.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);
    assert_eq!(prim.topology, Topology::Triangles);
    // The full 11×11 = 121 lattice vertices are retained (we don't drop
    // vertices, only triangles — keeps the encoder filter logic simple
    // and the vertex IDs in extras like obj:surface_samples stable);
    // sub-cell re-meshing appends its synthesised boundary vertices
    // after the lattice block.
    assert!(
        prim.positions.len() >= 121,
        "11×11 vertex lattice retained (plus boundary vertices), got {}",
        prim.positions.len()
    );
    let boundary_count = prim
        .extras
        .get("obj:surface_trim_boundary_vertices")
        .and_then(|v| v.as_u64())
        .expect("boundary-vertex provenance present") as usize;
    assert_eq!(
        prim.positions.len(),
        121 + boundary_count,
        "boundary-vertex extras must match the appended vertex count"
    );

    let kept_indices = indices_as_u32(prim);
    // Without clipping there'd be 10×10 × 2 = 200 triangles → 600 indices.
    let total = 10 * 10 * 2 * 3;
    assert!(
        kept_indices.len() < total,
        "trim must drop triangles: {} kept vs {} total",
        kept_indices.len(),
        total
    );

    // Every surviving triangle vertex must lie inside or on the trim
    // square (the bilinear surface is an identity onto (u, v) in xy
    // since the four corner controls land at (0,0), (1,0), (0,1),
    // (1,1) with z = 0). Synthesised boundary vertices converge onto
    // the rasterised loop polyline, which cuts the square's corners
    // slightly (the 11-point curv2 polyline doesn't sample the corner
    // parameters exactly) but never leaves the square.
    for &i in &kept_indices {
        let [x, y, _z] = prim.positions[i as usize];
        assert!(
            (0.2 - 1e-4..=0.8 + 1e-4).contains(&x),
            "vertex x={x} fell outside the trim square"
        );
        assert!(
            (0.2 - 1e-4..=0.8 + 1e-4).contains(&y),
            "vertex y={y} fell outside the trim square"
        );
    }

    // The kept parameter-space area must approximate the loop's area.
    // The exact square covers 0.6² = 0.36; the rasterised polyline cuts
    // each corner by ~0.007, so accept [0.30, 0.37]. The conservative
    // all-corners-kept clip of earlier rounds capped out at 0.24
    // (48 whole triangles), so the lower bound also proves the
    // re-meshed rim recovered real boundary area.
    let area: f32 = kept_indices
        .chunks(3)
        .map(|t| {
            let p = prim.positions[t[0] as usize];
            let q = prim.positions[t[1] as usize];
            let r = prim.positions[t[2] as usize];
            ((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])).abs() * 0.5
        })
        .sum();
    assert!(
        (0.30..=0.37).contains(&area),
        "kept area {area} not in [0.30, 0.37]"
    );

    // Provenance extras for the trim clip.
    assert_eq!(
        prim.extras
            .get("obj:surface_trimmed")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        prim.extras
            .get("obj:surface_trim_loops")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        prim.extras
            .get("obj:surface_hole_loops")
            .and_then(|v| v.as_u64()),
        Some(0)
    );
}

/// Spec §"Example 7 — Two trimming regions with a hole" condensed to a
/// single trim with a single hole. The trim is a `[0.1, 0.9]² square`
/// outer loop and the hole is a `[0.4, 0.6]²` inner loop. Vertices
/// inside the outer loop and outside the inner loop survive.
const TRIM_WITH_HOLE_SURF: &str = "\
vp 0.1 0.1
vp 0.9 0.1
vp 0.9 0.9
vp 0.1 0.9
cstype bspline
deg 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0 5.0 6.0
end
vp 0.4 0.4
vp 0.6 0.4
vp 0.6 0.6
vp 0.4 0.6
cstype bspline
deg 1
curv2 5 6 7 8 5
parm u 0.0 1.0 2.0 3.0 4.0 5.0 6.0
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
hole 0.0 6.0 2
end
";

#[test]
fn hole_loop_punches_a_gap_inside_the_trim() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(10)
        .decode(TRIM_WITH_HOLE_SURF.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);

    let kept_indices = indices_as_u32(prim);
    // The kept parameter-space area must approximate "trim area minus
    // hole area". The exact loops cover 0.8² − 0.2² = 0.60; the
    // 11-point rasterised polylines cut each loop's corners (≈ 0.05
    // off the trim, ≈ 0.003 added back from the hole), so accept a
    // window around 0.555.
    let area: f32 = kept_indices
        .chunks(3)
        .map(|t| {
            let p = prim.positions[t[0] as usize];
            let q = prim.positions[t[1] as usize];
            let r = prim.positions[t[2] as usize];
            ((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])).abs() * 0.5
        })
        .sum();
    assert!(
        (0.50..=0.62).contains(&area),
        "kept area {area} not in [0.50, 0.62]"
    );

    // No surviving triangle should have all three vertices strictly
    // inside the hole square — every emitted (sub-)triangle keeps at
    // least one lattice corner that classified outside the hole loop,
    // and synthesised boundary vertices converge onto the loop
    // boundary itself.
    let strictly_inside =
        |x: f32, y: f32| x > 0.4 + 1e-4 && x < 0.6 - 1e-4 && y > 0.4 + 1e-4 && y < 0.6 - 1e-4;
    for tri in kept_indices.chunks(3) {
        let p0 = prim.positions[tri[0] as usize];
        let p1 = prim.positions[tri[1] as usize];
        let p2 = prim.positions[tri[2] as usize];
        let all_in = strictly_inside(p0[0], p0[1])
            && strictly_inside(p1[0], p1[1])
            && strictly_inside(p2[0], p2[1]);
        assert!(
            !all_in,
            "triangle ({p0:?}, {p1:?}, {p2:?}) fully inside the hole"
        );
    }

    assert_eq!(
        prim.extras
            .get("obj:surface_trim_loops")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        prim.extras
            .get("obj:surface_hole_loops")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
}

#[test]
fn trim_and_hole_directives_still_round_trip_through_extras() {
    // Decode without the tessellation knob ⇒ the trim/hole directives
    // ride on Scene3D::extras and the encoder must replay them.
    use oxideav_mesh3d::Mesh3DEncoder;
    use oxideav_obj::ObjEncoder;
    let scene = ObjDecoder::new()
        .decode(TRIM_WITH_HOLE_SURF.as_bytes())
        .unwrap();
    let dirs = scene
        .extras
        .get("obj:freeform_directives")
        .and_then(|v| v.as_array())
        .expect("freeform directives captured");
    let keywords: Vec<&str> = dirs
        .iter()
        .map(|d| {
            d.as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("")
        })
        .collect();
    assert!(
        keywords.contains(&"trim"),
        "trim must be in the captured directives: {keywords:?}"
    );
    assert!(
        keywords.contains(&"hole"),
        "hole must be in the captured directives: {keywords:?}"
    );

    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("\ntrim 0"),
        "trim must round-trip: {text}\n---"
    );
    assert!(
        text.contains("\nhole 0"),
        "hole must round-trip: {text}\n---"
    );
}

#[test]
fn clipped_surface_is_not_emitted_as_v_lines_by_encoder() {
    // The clipped synthetic surface mesh must remain filtered out of
    // the OBJ encoder — same as the un-clipped synthetic surface — so
    // a decode → encode cycle doesn't pollute the file with hundreds
    // of lattice `v` lines and `f` triangle faces.
    use oxideav_mesh3d::Mesh3DEncoder;
    use oxideav_obj::ObjEncoder;
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(TRIMMED_BILINEAR_SURF.as_bytes())
        .unwrap();
    // Confirm the synthetic surface mesh did get added at decode time.
    assert!(
        scene
            .meshes
            .iter()
            .any(|m| m.name.as_deref() == Some("obj:surfaces")),
        "decode should have produced a synthetic surfaces mesh"
    );

    let bytes = ObjEncoder::new().encode(&scene).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    // Only the original 4 `v` control vertices for the bilinear patch
    // should reappear — plus zero synthesised lattice vertices.
    let v_count = text
        .lines()
        .filter(|l| l.starts_with("v ") || l == &"v")
        .count();
    assert_eq!(
        v_count, 4,
        "encoder must not emit lattice vertices; got:\n{text}\n---"
    );
}

// --- Sub-cell boundary re-meshing (round 282) ---------------------------
//
// At 8 tessellation samples the deg-1 B-spline curv2 below is sampled at
// 9 points over its [1, 5] evaluation window, which lands samples exactly
// on the knots u = 2, 3, 4 — i.e. exactly on the square's corner control
// points — so the assembled loop polygon is the *exact* square
// [0.3, 0.7] × [0.3, 0.7] (plus collinear edge midpoints). Combined with
// the identity bilinear patch (x = u, y = v, z = 0), the kept mesh's
// xy-area is directly comparable against the analytic trimmed area.
const REMESH_EXACT_SQUARE: &str = "\
vp 0.3 0.3
vp 0.7 0.3
vp 0.7 0.7
vp 0.3 0.7
cstype bspline
deg 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0 5.0 6.0
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
end
";

/// Same loop used as a `hole` with no preceding `trim` — spec
/// §"Trimming loops and holes": "If the first trim statement in the
/// sequence is omitted, the enclosing outer trimming loop is taken to
/// be the parameter range of the surface."
const REMESH_EXACT_SQUARE_HOLE: &str = "\
vp 0.3 0.3
vp 0.7 0.3
vp 0.7 0.7
vp 0.3 0.7
cstype bspline
deg 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0 5.0 6.0
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
hole 0.0 6.0 1
end
";

fn kept_xy_area(prim: &oxideav_mesh3d::Primitive) -> f32 {
    indices_as_u32(prim)
        .chunks(3)
        .map(|t| {
            let p = prim.positions[t[0] as usize];
            let q = prim.positions[t[1] as usize];
            let r = prim.positions[t[2] as usize];
            ((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])).abs() * 0.5
        })
        .sum()
}

#[test]
fn remeshed_trim_recovers_the_analytic_loop_area() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(REMESH_EXACT_SQUARE.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);

    // The loop boundary u = 0.3 / 0.7 sits strictly between lattice
    // lines (multiples of 0.125), so the conservative all-corners-kept
    // clip of earlier rounds could keep at most the 2×2 fully-interior
    // cell block = 0.0625 area. The re-meshed boundary must recover
    // the analytic 0.4² = 0.16 to bisection precision (the only error
    // left is the chord across the loop corner inside the four
    // corner-straddling cells, well under 5e-3 total).
    let area = kept_xy_area(prim);
    // The only remaining deficit is the chord across each loop corner
    // inside its straddling cell (the in/out frontier bends 90° there,
    // and the two edge crossings are joined by a straight sub-triangle
    // edge): ~0.0014 per corner at this lattice, ~0.0056 total.
    assert!(
        (area - 0.16).abs() < 1e-2,
        "kept area {area} differs from the analytic 0.16"
    );

    // Boundary vertices were synthesised, appended after the 9×9
    // lattice block, and every one of them sits on the loop perimeter.
    let boundary_count = prim
        .extras
        .get("obj:surface_trim_boundary_vertices")
        .and_then(|v| v.as_u64())
        .expect("boundary-vertex provenance present") as usize;
    assert!(
        boundary_count > 0,
        "straddling cells must synthesise vertices"
    );
    assert_eq!(prim.positions.len(), 81 + boundary_count);
    for [x, y, _z] in &prim.positions[81..] {
        let on_perimeter = (x - 0.3).abs() < 1e-3
            || (x - 0.7).abs() < 1e-3
            || (y - 0.3).abs() < 1e-3
            || (y - 0.7).abs() < 1e-3;
        assert!(
            on_perimeter
                && (0.3 - 1e-3..=0.7 + 1e-3).contains(x)
                && (0.3 - 1e-3..=0.7 + 1e-3).contains(y),
            "boundary vertex ({x}, {y}) not on the loop perimeter"
        );
    }
}

#[test]
fn remeshed_hole_recovers_the_complement_area() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(REMESH_EXACT_SQUARE_HOLE.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);

    // Hole-only: kept area = parameter rectangle minus the hole,
    // 1 − 0.16 = 0.84.
    let area = kept_xy_area(prim);
    // Mirror of the trim case: the chord across each hole corner hands
    // its ~0.0014 back to the kept complement, ~0.0056 total.
    assert!(
        (area - 0.84).abs() < 1e-2,
        "kept area {area} differs from the analytic 0.84"
    );

    // No kept triangle's centroid may fall deeper than one lattice
    // cell (0.125) inside the hole square. Right at the hole's corners
    // a cell triangle whose three corners all classify outside can
    // still overlap the corner tip (the loop corner pokes into the
    // triangle's interior without flipping any corner classification —
    // a lattice-grain effect the corner-based classification shares
    // with the conservative clip), so the boundary cell ring is
    // exempt; the hole interior proper must be empty.
    for t in indices_as_u32(prim).chunks(3) {
        let p = prim.positions[t[0] as usize];
        let q = prim.positions[t[1] as usize];
        let r = prim.positions[t[2] as usize];
        let cx = (p[0] + q[0] + r[0]) / 3.0;
        let cy = (p[1] + q[1] + r[1]) / 3.0;
        let in_hole = cx > 0.3 + 0.13 && cx < 0.7 - 0.13 && cy > 0.3 + 0.13 && cy < 0.7 - 0.13;
        assert!(!in_hole, "triangle centroid ({cx}, {cy}) inside the hole");
    }

    assert_eq!(
        prim.extras
            .get("obj:surface_trim_loops")
            .and_then(|v| v.as_u64()),
        Some(0)
    );
    assert_eq!(
        prim.extras
            .get("obj:surface_hole_loops")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
}

#[test]
fn remeshed_rim_is_watertight_and_sliver_free() {
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(REMESH_EXACT_SQUARE.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);
    let indices = indices_as_u32(prim);

    // Watertight: crossings are cached per undirected lattice edge, so
    // adjacent straddling triangles share their synthesised boundary
    // vertex and no undirected edge may be referenced by more than two
    // triangles (a pinched / doubled rim would exceed 2).
    let mut edge_use: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    for t in indices.chunks(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let key = if a < b { (a, b) } else { (b, a) };
            *edge_use.entry(key).or_insert(0) += 1;
        }
    }
    for (edge, count) in &edge_use {
        assert!(
            *count <= 2,
            "edge {edge:?} referenced by {count} triangles (non-manifold rim)"
        );
    }

    // Sliver-free: every emitted (sub-)triangle carries real area.
    for t in indices.chunks(3) {
        let p = prim.positions[t[0] as usize];
        let q = prim.positions[t[1] as usize];
        let r = prim.positions[t[2] as usize];
        let area2 = ((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])).abs();
        assert!(area2 > 1e-9, "degenerate sliver triangle {t:?} emitted");
    }
}

#[test]
fn lattice_aligned_loop_suppresses_slivers() {
    // The loop edges u = 0.25 / 0.75 coincide exactly with lattice
    // lines at 8 samples (multiples of 0.125). Whichever way the
    // on-edge lattice points classify, the re-mesh must converge onto
    // the same lattice line: the kept area is the analytic 0.25 and no
    // degenerate slivers survive the area threshold.
    const ALIGNED: &str = "\
vp 0.25 0.25
vp 0.75 0.25
vp 0.75 0.75
vp 0.25 0.75
cstype bspline
deg 1
curv2 1 2 3 4 1
parm u 0.0 1.0 2.0 3.0 4.0 5.0 6.0
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
end
";
    let scene = ObjDecoder::new()
        .with_curve_tessellation(8)
        .decode(ALIGNED.as_bytes())
        .unwrap();
    let prim = surf_prim(&scene);
    let area = kept_xy_area(prim);
    assert!(
        (area - 0.25).abs() < 1e-3,
        "kept area {area} differs from the analytic 0.25"
    );
    for t in indices_as_u32(prim).chunks(3) {
        let p = prim.positions[t[0] as usize];
        let q = prim.positions[t[1] as usize];
        let r = prim.positions[t[2] as usize];
        let area2 = ((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])).abs();
        assert!(area2 > 1e-9, "degenerate sliver triangle {t:?} emitted");
    }
}
