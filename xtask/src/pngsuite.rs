//! Verify the fixed PngSuite release before extraction and every time a cache is reused.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use crate::corpus_pins::{PNGSUITE_SHA256, PNGSUITE_URL};
use crate::pinned::USER_AGENT;
use crate::{root, run, sha256};

const MANIFEST: &str = include_str!("../pngsuite.sha256");
// The archive is 68 KB and its largest file is 4 KB. Bound reads of a corrupt cache/download.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

fn digest(path: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(format!("{}: expected a regular file under {MAX_BYTES} bytes", path.display()));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{}: exceeds {MAX_BYTES} bytes", path.display()));
    }
    Ok(sha256::hex(&bytes))
}

fn entries(manifest: &str) -> Result<BTreeMap<&str, &str>, String> {
    let mut files = BTreeMap::new();
    for line in manifest.lines() {
        let (hash, name) = line.split_once("  ").ok_or("pngsuite: malformed manifest entry")?;
        if hash.len() != 64
            || !hash.bytes().all(|c| c.is_ascii_hexdigit())
            || name.is_empty()
            || name == "."
            || name == ".."
            || !name.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            || files.insert(name, hash).is_some()
        {
            return Err(format!("pngsuite: invalid manifest entry {name:?}"));
        }
    }
    if files.is_empty() {
        return Err("pngsuite: empty manifest".into());
    }
    Ok(files)
}

fn verify(dir: &Path, files: &BTreeMap<&str, &str>) -> Result<(), String> {
    for (name, expected) in files {
        if digest(&dir.join(name))? != *expected {
            return Err(format!("pngsuite: {name}: sha256 mismatch"));
        }
    }
    // Extra PNGs must not reach the corpus decoder, even if all expected files still match.
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        if !entry.file_name().to_str().is_some_and(|name| files.contains_key(name)) {
            return Err(format!("pngsuite: unexpected file {}", entry.path().display()));
        }
    }
    Ok(())
}

pub fn is_current() -> bool {
    entries(MANIFEST).is_ok_and(|files| verify(&root().join("corpus/pngsuite"), &files).is_ok())
}

pub fn fetch() -> Result<(), String> {
    fetch_with(
        &root().join("corpus"),
        PNGSUITE_SHA256,
        MANIFEST,
        |archive| {
            let mut curl = Command::new("curl");
            curl.args(["-fsSL", "--retry", "3", "-A", USER_AGENT, "-o"]).arg(archive).arg(PNGSUITE_URL);
            run(curl, &format!("curl {PNGSUITE_URL}"))
        },
        |archive, dest| {
            let mut tar = Command::new("tar");
            tar.arg("-xzf").arg(archive).arg("-C").arg(dest);
            run(tar, "tar -xzf PngSuite-2017jul19.tgz")
        },
    )
}

fn fetch_with(
    corpus: &Path,
    archive_hash: &str,
    manifest: &str,
    download: impl FnOnce(&Path) -> Result<(), String>,
    extract: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    let files = entries(manifest)?;
    let dest = corpus.join("pngsuite");
    if verify(&dest, &files).is_ok() {
        println!("pngsuite: {} already matches xtask/pngsuite.sha256", dest.display());
        return Ok(());
    }
    let staging = corpus.join(".pngsuite-download");
    if staging.join("previous").exists() && !dest.exists() {
        return Err(format!("pngsuite: recover the previous corpus from {} before retrying", staging.join("previous").display()));
    }
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| format!("remove {}: {e}", staging.display()))?;
    }
    let unpacked = staging.join("files");
    std::fs::create_dir_all(&unpacked).map_err(|e| format!("create {}: {e}", unpacked.display()))?;
    let previous = staging.join("previous");
    let result = (|| {
        let archive = staging.join("PngSuite-2017jul19.tgz");
        download(&archive)?;
        if digest(&archive)? != archive_hash {
            return Err("pngsuite: archive sha256 mismatch; refusing to extract".into());
        }
        extract(&archive, &unpacked)?;
        verify(&unpacked, &files)?;
        // Keep the old corpus until both checks pass; restore it if installation fails.
        if dest.exists() {
            std::fs::rename(&dest, &previous).map_err(|e| format!("move {}: {e}", dest.display()))?;
        }
        if let Err(e) = std::fs::rename(&unpacked, &dest) {
            if previous.exists() {
                std::fs::rename(&previous, &dest)
                    .map_err(|restore| format!("install pngsuite: {e}; restore failed: {restore}; previous corpus is in {}", previous.display()))?;
            }
            return Err(format!("install pngsuite: {e}"));
        }
        println!("pngsuite: {} verified files in {}", files.len(), dest.display());
        Ok(())
    })();
    // Retain a backup if restoring it failed, so the error's recovery path remains usable.
    if result.is_ok() || !previous.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!("photocraft-pngsuite-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn manifest(&self) -> String {
            format!("{}  a.png\n{}  PngSuite.LICENSE\n", sha256::hex(b"image"), sha256::hex(b"license"))
        }
        fn write_files(dir: &Path) {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("a.png"), b"image").unwrap();
            std::fs::write(dir.join("PngSuite.LICENSE"), b"license").unwrap();
        }
        fn fetch(&self, extract: impl FnOnce(&Path, &Path) -> Result<(), String>) -> Result<(), String> {
            fetch_with(&self.0, &sha256::hex(b"archive"), &self.manifest(), |p| std::fs::write(p, b"archive").map_err(|e| e.to_string()), extract)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn wrong_archive_is_rejected_before_extraction_and_keeps_existing_corpus() {
        let f = Fixture::new();
        let dest = f.0.join("pngsuite");
        Fixture::write_files(&dest);
        std::fs::write(dest.join("a.png"), b"old cache").unwrap();
        let result = fetch_with(
            &f.0,
            &sha256::hex(b"archive"),
            &f.manifest(),
            |p| std::fs::write(p, b"tampered").map_err(|e| e.to_string()),
            |_, _| panic!("unverified archive reached tar"),
        );
        assert!(result.unwrap_err().contains("archive sha256 mismatch"));
        assert_eq!(std::fs::read(dest.join("a.png")).unwrap(), b"old cache");
        assert!(!f.0.join(".pngsuite-download").exists());
    }

    #[test]
    fn verified_cache_needs_no_network_or_extraction() {
        let f = Fixture::new();
        Fixture::write_files(&f.0.join("pngsuite"));
        fetch_with(&f.0, &sha256::hex(b"archive"), &f.manifest(), |_| panic!("cache downloaded"), |_, _| panic!("cache extracted")).unwrap();
    }

    #[test]
    fn missing_modified_and_extra_cached_files_are_refetched() {
        for corruption in ["missing", "modified", "extra"] {
            let f = Fixture::new();
            let dest = f.0.join("pngsuite");
            Fixture::write_files(&dest);
            match corruption {
                "missing" => std::fs::remove_file(dest.join("a.png")).unwrap(),
                "modified" => std::fs::write(dest.join("a.png"), b"tampered").unwrap(),
                _ => std::fs::write(dest.join("untrusted.png"), b"extra").unwrap(),
            }
            let mut extracted = false;
            f.fetch(|_, dir| {
                extracted = true;
                Fixture::write_files(dir);
                Ok(())
            })
            .unwrap();
            assert!(extracted, "{corruption}");
            assert_eq!(std::fs::read(dest.join("a.png")).unwrap(), b"image");
            assert!(!dest.join("untrusted.png").exists());
            assert!(!f.0.join(".pngsuite-download").exists());
        }
    }

    #[test]
    fn extracted_file_mismatch_does_not_replace_existing_corpus() {
        let f = Fixture::new();
        let dest = f.0.join("pngsuite");
        Fixture::write_files(&dest);
        std::fs::write(dest.join("a.png"), b"old cache").unwrap();
        let result = f.fetch(|_, dir| {
            Fixture::write_files(dir);
            std::fs::write(dir.join("a.png"), b"wrong").map_err(|e| e.to_string())
        });
        assert!(result.unwrap_err().contains("a.png: sha256 mismatch"));
        assert_eq!(std::fs::read(dest.join("a.png")).unwrap(), b"old cache");
    }

    #[test]
    fn extracted_extra_files_and_unsafe_manifest_paths_are_rejected() {
        let f = Fixture::new();
        let result = f.fetch(|_, dir| {
            Fixture::write_files(dir);
            std::fs::write(dir.join("extra.png"), b"extra").map_err(|e| e.to_string())
        });
        assert!(result.unwrap_err().contains("unexpected file"));
        assert!(!f.0.join("pngsuite").exists());
        assert!(entries(&format!("{}  ../escape.png", sha256::hex(b"image"))).is_err());
    }
}
