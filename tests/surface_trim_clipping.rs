//! Surface trim/hole clipping — under the
//! `ObjDecoder::with_curve_tessellation(N)` knob, every `surf` element
//! whose enclosing `cstype … end` block also carries one or more
//! `trim` / `hole` directives now has its tessellated triangle grid
//! conservatively clipped against the parameter-space loop(s) those
//! directives assemble from `curv2` segments. Spec §"Trimming Loops",
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
//! polygon-tested against the assembled loops, and a triangle survives
//! only when **all three** of its vertices pass. This is a conservative
//! clip (the boundary stays jagged at the lattice grain); sub-cell
//! boundary re-meshing is left for a later round.
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
    // The lattice stays the full 11×11 = 121 vertices (we don't drop
    // vertices, only triangles — keeps the encoder filter logic simple
    // and the vertex IDs in extras like obj:surface_samples stable).
    assert_eq!(prim.positions.len(), 121, "11×11 vertex lattice retained");

    let kept_indices = indices_as_u32(prim);
    // Without clipping there'd be 10×10 × 2 = 200 triangles → 600 indices.
    let total = 10 * 10 * 2 * 3;
    assert!(
        kept_indices.len() < total,
        "trim must drop triangles: {} kept vs {} total",
        kept_indices.len(),
        total
    );
    // The square covers [0.2, 0.8] × [0.2, 0.8]. Strictly-interior
    // lattice values are u ∈ {0.3, …, 0.7} (5 values, giving a 4×4 = 16
    // fully-interior cell block = 32 triangles = 96 indices); including
    // boundary-aligned values u ∈ {0.2, …, 0.8} expands to a 6×6 = 36
    // cell block = 72 triangles = 216 indices. The actual kept count
    // depends on whether the rasterised polygon boundary classifies
    // boundary-aligned lattice points as inside, so we accept anything
    // in that range.
    assert!(
        (96..=216).contains(&kept_indices.len()),
        "kept index count {} not in [96, 216]",
        kept_indices.len()
    );
    // ≥ 50 % of the parameter area got dropped.
    assert!(kept_indices.len() < total / 2 + 1);

    // Every surviving triangle vertex must lie inside or on the trim
    // square (the bilinear surface is an identity onto (u, v) in xy
    // since the four corner controls land at (0,0), (1,0), (0,1),
    // (1,1) with z = 0).
    for &i in &kept_indices {
        let [x, y, _z] = prim.positions[i as usize];
        assert!(
            (0.2 - 1e-5..=0.8 + 1e-5).contains(&x),
            "vertex x={x} fell outside the trim square"
        );
        assert!(
            (0.2 - 1e-5..=0.8 + 1e-5).contains(&y),
            "vertex y={y} fell outside the trim square"
        );
    }

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
    // Trim alone (no hole) would keep an 8×8 inner cell block; the
    // hole punches out a smaller block. Strictly fewer triangles than
    // the trim-only case.
    let trim_only_index_count = 8 * 8 * 2 * 3; // 384
    assert!(
        kept_indices.len() < trim_only_index_count,
        "hole must drop triangles below the trim-only count: {} vs {}",
        kept_indices.len(),
        trim_only_index_count
    );

    // No surviving triangle should have all three vertices strictly
    // inside the hole square — the conservative clip drops a triangle
    // when **any** vertex falls inside a hole loop (i.e. would be
    // excluded from the kept-vertex mask), so no fully-interior
    // triangle should remain.
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
