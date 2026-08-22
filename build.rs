// Build script for orbital-compiler.
//
// Rebuild whenever the committed in-crate baked registries change. The bake is
// produced by `@almadar-io/almadar-pattern-sync rust` (`make sync`) from the
// pinned npm packages.
//
// The corpus is embedded **gzipped**: `src/embedded.rs` `include_bytes!`s the
// `.gz` files this script writes into `$OUT_DIR/baked-gz/…` and inflates them
// lazily behind the same `&'static str` accessors. The raw corpus is ~137MB
// (~35MB of it retained in the wasm client glue); gzipped it is ~6MB. Pretty
// JSON gzips as well as minified, so no minification pass is needed.

use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/baked/canonical-operators.json");
    println!("cargo:rerun-if-changed=src/baked/patterns-registry.json");
    println!("cargo:rerun-if-changed=src/baked/themes-registry.json");
    println!("cargo:rerun-if-changed=src/baked/behaviors-registry-std.json");
    println!("cargo:rerun-if-changed=src/baked/behaviors-registry-io.json");
    println!("cargo:rerun-if-changed=src/baked/factory-signatures-std.json");
    println!("cargo:rerun-if-changed=src/baked/factory-signatures-io.json");
    println!("cargo:rerun-if-changed=src/baked/services-registry.json");
    // The full std+io behavior corpus is embedded from `src/baked/{std,io}/…`.
    println!("cargo:rerun-if-changed=src/baked/std");
    println!("cargo:rerun-if-changed=src/baked/io");
    println!("cargo:rerun-if-changed=src/baked/bindings.ts");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR unset"));
    let gz_root = out_dir.join("baked-gz");
    let baked_root = Path::new("src/baked");
    let mut count = 0u32;
    gzip_tree(baked_root, baked_root, &gz_root, &mut count);
    println!("cargo:warning=baked-gz: {count} files gzipped into {gz_root:?}");
}

/// Recursively gzip every `.json`/`.orb` under `dir`, mirroring the relative
/// path under `gz_root` with a `.gz` suffix. Skips files whose `.gz` is newer
/// than the source so incremental builds don't recompress 137MB.
fn gzip_tree(root: &Path, dir: &Path, gz_root: &Path, count: &mut u32) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("dir entry in {dir:?}: {e}"));
        let path = entry.path();
        if path.is_dir() {
            gzip_tree(root, &path, gz_root, count);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("json") && ext != Some("orb") {
            continue;
        }
        let rel = path.strip_prefix(root).expect("baked file under root");
        let mut gz_rel = rel.to_path_buf();
        let name = gz_rel.file_name().expect("file name").to_os_string();
        gz_rel.set_file_name(format!("{}.gz", name.to_string_lossy()));
        let gz_path = gz_root.join(gz_rel);
        if let (Ok(src_meta), Ok(gz_meta)) = (fs::metadata(&path), fs::metadata(&gz_path)) {
            if let (Ok(src_mtime), Ok(gz_mtime)) = (src_meta.modified(), gz_meta.modified()) {
                if gz_mtime >= src_mtime {
                    continue;
                }
            }
        }
        let raw = fs::read(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        fs::create_dir_all(gz_path.parent().expect("gz parent"))
            .unwrap_or_else(|e| panic!("mkdir {gz_path:?}: {e}"));
        let mut enc = GzEncoder::new(Vec::with_capacity(raw.len() / 8), Compression::best());
        enc.write_all(&raw)
            .unwrap_or_else(|e| panic!("gzip {path:?}: {e}"));
        let compressed = enc
            .finish()
            .unwrap_or_else(|e| panic!("gzip finish {path:?}: {e}"));
        fs::write(&gz_path, compressed).unwrap_or_else(|e| panic!("write {gz_path:?}: {e}"));
        *count += 1;
    }
}
