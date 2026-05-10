//! Decode + re-encode a 12-triangle textured cube; assert structural
//! equality of the second decode (a stable fixed point — the round
//! trip preserves the typed model).

use oxideav_mesh3d::Mesh3DDecoder;
use oxideav_obj::{ObjDecoder, obj};

const CUBE_OBJ: &str = "\
# Unit cube centred at the origin.
mtllib cube.mtl
o Cube
v -0.5 -0.5  0.5
v  0.5 -0.5  0.5
v  0.5  0.5  0.5
v -0.5  0.5  0.5
v -0.5 -0.5 -0.5
v  0.5 -0.5 -0.5
v  0.5  0.5 -0.5
v -0.5  0.5 -0.5
vn  0.0  0.0  1.0
vn  0.0  0.0 -1.0
vn  0.0  1.0  0.0
vn  0.0 -1.0  0.0
vn  1.0  0.0  0.0
vn -1.0  0.0  0.0
usemtl Stone
f 1//1 2//1 3//1
f 1//1 3//1 4//1
f 6//2 5//2 8//2
f 6//2 8//2 7//2
f 4//3 3//3 7//3
f 4//3 7//3 8//3
f 5//4 6//4 2//4
f 5//4 2//4 1//4
f 2//5 6//5 7//5
f 2//5 7//5 3//5
f 5//6 1//6 4//6
f 5//6 4//6 8//6
";

const CUBE_MTL: &str = "\
newmtl Stone
Kd 0.7 0.7 0.7
Ka 0.05 0.05 0.05
Ns 32
";

#[test]
fn cube_decodes_to_one_mesh_one_primitive_with_12_triangles() {
    // Use the resolver path so the companion MTL is merged in.
    let scene = obj::parse_obj_with_resolver(CUBE_OBJ, |path| {
        assert_eq!(path, "cube.mtl");
        Ok(CUBE_MTL.as_bytes().to_vec())
    })
    .unwrap();

    assert_eq!(scene.meshes.len(), 1);
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.name.as_deref(), Some("Cube"));
    assert_eq!(mesh.primitives.len(), 1);
    let prim = &mesh.primitives[0];
    assert_eq!(prim.triangle_count(), 12);
    // Each face shares one normal (face-normal style); face-vertex
    // dedup keys on the FaceVert, so the cube's 8 distinct positions
    // expand into 24 (one per face).
    assert_eq!(prim.positions.len(), 24);
    assert!(prim.normals.is_some());

    // The "Stone" material should have been resolved.
    assert_eq!(scene.materials.len(), 1);
    assert_eq!(scene.materials[0].name.as_deref(), Some("Stone"));
    assert_eq!(
        prim.material,
        Some(
            scene.materials[0]
                .name
                .as_ref()
                .map(|_| oxideav_mesh3d::MaterialId(0))
                .unwrap()
        )
    );
}

#[test]
fn decode_encode_decode_is_stable_fixed_point() {
    let scene1 =
        obj::parse_obj_with_resolver(CUBE_OBJ, |_| Ok(CUBE_MTL.as_bytes().to_vec())).unwrap();
    let bytes = obj::serialize_obj(&scene1, None).unwrap();

    // Re-decode the encoded bytes — no MTL resolver this time (we
    // didn't ask for one). Per-primitive `usemtl` extras should still
    // remember the material name though.
    let scene2 = ObjDecoder::new().decode(&bytes).unwrap();

    assert_eq!(scene2.meshes.len(), 1);
    let m1 = &scene1.meshes[0];
    let m2 = &scene2.meshes[0];
    assert_eq!(m1.name, m2.name);
    assert_eq!(m1.primitives.len(), m2.primitives.len());
    let p1 = &m1.primitives[0];
    let p2 = &m2.primitives[0];
    assert_eq!(p1.triangle_count(), p2.triangle_count());
    assert_eq!(p1.positions.len(), p2.positions.len());
    assert_eq!(p1.normals.is_some(), p2.normals.is_some());
    // Material name preserved via `obj:usemtl` extra.
    assert_eq!(
        p2.extras.get("obj:usemtl").and_then(|v| v.as_str()),
        Some("Stone"),
    );
}
