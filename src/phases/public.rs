//! `public "./public"`: a folder beside the program copied verbatim to the
//! built site's root (logos, favicons, downloadable installers).

use std::path::{Path, PathBuf};

/// The public folder on disk, or why it cannot be used: it must exist and
/// stay inside the program's own folder.
pub fn resolve_public_dir(source_path: &Path, public_dir: &str) -> Result<PathBuf, String> {
    let base = source_path.parent().unwrap_or(Path::new("."));
    let dir = base.join(public_dir);
    let (Ok(base), Ok(dir)) = (base.canonicalize(), dir.canonicalize()) else {
        return Err(format!("public folder `{public_dir}` does not exist beside the program"));
    };
    if !dir.is_dir() {
        return Err(format!("public `{public_dir}` is not a folder"));
    }
    if !dir.starts_with(&base) {
        return Err(format!("public folder `{public_dir}` is outside the program's folder"));
    }
    Ok(dir)
}

/// Copies `from` into `to`, recursively, byte for byte.
pub fn copy_public(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_public(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("public/img")).unwrap();
        std::fs::write(dir.path().join("public/install.sh"), "#!/bin/sh\necho orb\n").unwrap();
        std::fs::write(dir.path().join("public/img/logo.svg"), "<svg/>").unwrap();
        std::fs::write(dir.path().join("site.lolo"), "").unwrap();
        dir
    }

    #[test]
    fn copies_the_folder_byte_for_byte() {
        let s = site();
        let from = resolve_public_dir(&s.path().join("site.lolo"), "./public").unwrap();
        let out = tempfile::TempDir::new().unwrap();
        copy_public(&from, out.path()).unwrap();
        assert_eq!(std::fs::read(out.path().join("install.sh")).unwrap(), b"#!/bin/sh\necho orb\n");
        assert_eq!(std::fs::read(out.path().join("img/logo.svg")).unwrap(), b"<svg/>");
    }

    #[test]
    fn a_missing_folder_is_rejected() {
        let s = site();
        assert!(resolve_public_dir(&s.path().join("site.lolo"), "./static").unwrap_err().contains("does not exist"));
    }

    #[test]
    fn edge_a_folder_outside_the_program_is_rejected() {
        let s = site();
        std::fs::create_dir_all(s.path().join("nested")).unwrap();
        std::fs::write(s.path().join("nested/site.lolo"), "").unwrap();
        assert!(resolve_public_dir(&s.path().join("nested/site.lolo"), "../public").unwrap_err().contains("outside"));
    }
}
