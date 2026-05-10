//! `parse_obj_from_path` resolves `mtllib` references against the
//! OBJ file's parent directory; multiple MTL files on the same
//! `mtllib` line all resolve relative to that directory.

use std::fs;
use std::io::Write;

use oxideav_obj::obj;

fn write_temp(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    let mut f = fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

#[test]
fn obj_resolves_mtllib_against_parent_dir() {
    // Build an isolated temp directory with one OBJ + one MTL.
    let tmp = std::env::temp_dir().join(format!("oxideav-obj-r2-pathres-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    write_temp(&tmp, "test.mtl", "newmtl Wood\nKd 0.6 0.4 0.2\n");
    let obj_path = write_temp(
        &tmp,
        "test.obj",
        "mtllib test.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nusemtl Wood\nf 1 2 3\n",
    );

    let scene = obj::parse_obj_from_path(&obj_path).unwrap();
    // The MTL was found and merged.
    assert_eq!(scene.materials.len(), 1);
    assert_eq!(scene.materials[0].name.as_deref(), Some("Wood"));
    // The face primitive picked up the material binding.
    let prim = &scene.meshes[0].primitives[0];
    assert!(prim.material.is_some());

    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn obj_with_multiple_mtl_files_loads_each_one() {
    let tmp = std::env::temp_dir().join(format!("oxideav-obj-r2-multimtl-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    write_temp(&tmp, "a.mtl", "newmtl Red\nKd 1 0 0\n");
    write_temp(&tmp, "b.mtl", "newmtl Blue\nKd 0 0 1\n");
    let obj_path = write_temp(
        &tmp,
        "scene.obj",
        "mtllib a.mtl b.mtl
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
usemtl Red
f 1 2 3
usemtl Blue
f 1 3 4
",
    );

    let scene = obj::parse_obj_from_path(&obj_path).unwrap();
    assert_eq!(scene.materials.len(), 2);
    let mut names: Vec<&str> = scene
        .materials
        .iter()
        .filter_map(|m| m.name.as_deref())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Blue", "Red"]);

    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn missing_mtl_surfaces_a_clean_error() {
    let tmp = std::env::temp_dir().join(format!("oxideav-obj-r2-missing-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    let obj_path = write_temp(
        &tmp,
        "lonely.obj",
        "mtllib nonexistent.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nf 1 2 3\n",
    );

    let err = obj::parse_obj_from_path(&obj_path).unwrap_err();
    let s = format!("{err}");
    assert!(
        s.contains("nonexistent.mtl"),
        "expected mtllib path in error, got {s}",
    );

    fs::remove_dir_all(&tmp).ok();
}
