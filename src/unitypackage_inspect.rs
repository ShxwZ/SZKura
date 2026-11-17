//! Read intended Unity destinations without extracting assets or running scripts.
//! Unity packages are gzip TAR archives with <GUID>/pathname entries.
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use std::{collections::BTreeSet, fs::File, io::{BufReader, Read}, path::Path};

const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_PATHNAMES: usize = 50_000;
const MAX_PATHNAME_BYTES: u64 = 16 * 1024;
const MAX_FOLDERS: usize = 4_096;
const MAX_UNCOMPRESSED_TAR_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnityPathsPreview {
    pub main_folders: Vec<String>,
    pub folders: Vec<String>,
    pub asset_count: usize,
    pub skipped_paths: usize,
}

pub fn inspect_unitypackage_paths(path: &Path) -> Result<UnityPathsPreview> {
    inspect_unitypackage_paths_with(path, |_| {})
}

/// Visit each validated Unity asset pathname without retaining potentially
/// thousands of lengthy paths in the regular UI preview.
pub fn inspect_unitypackage_paths_with(
    path: &Path, mut on_path: impl FnMut(&str),
) -> Result<UnityPathsPreview> {
    let file = File::open(path)
        .with_context(|| format!("No se pudo abrir el UnityPackage: {}", path.display()))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("No es un archivo UnityPackage regular");
    }
    if metadata.len() > crate::package_inspect::MAX_PACKAGE_BYTES {
        bail!("El UnityPackage supera el límite de inspección de 2 GiB");
    }
    inspect_reader_with(BufReader::new(file), &mut on_path)
}

fn inspect_reader(source: impl Read) -> Result<UnityPathsPreview> {
    inspect_reader_with(source, |_| {})
}

fn inspect_reader_with(
    source: impl Read, mut on_path: impl FnMut(&str),
) -> Result<UnityPathsPreview> {
    // The cap applies to the entire decompressed stream, including TAR headers
    // and asset payloads skipped by the iterator. No files are written to disk.
    let gzip = GzDecoder::new(source);
    let mut archive = tar::Archive::new(gzip.take(MAX_UNCOMPRESSED_TAR_BYTES));
    let mut roots = BTreeSet::new();
    let mut folders = BTreeSet::new();
    let mut total_entries = 0usize;
    let mut asset_count = 0usize;
    let mut skipped_paths = 0usize;

    for entry in archive.entries().context("No se pudo leer el TAR del UnityPackage")? {
        let mut entry = entry.context("Entrada TAR no válida en el UnityPackage")?;
        total_entries += 1;
        if total_entries > MAX_ARCHIVE_ENTRIES {
            bail!("El UnityPackage contiene demasiadas entradas");
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let entry_path = entry.path().context("Ruta TAR no válida")?;
        let entry_name = entry_path.to_string_lossy().replace('\\', "/");
        // Unity metadata is GUID/pathname; ignore asset and asset.meta payloads.
        if entry_name.rsplit('/').next() != Some("pathname") {
            continue;
        }
        if entry.size() > MAX_PATHNAME_BYTES {
            bail!("Un pathname de UnityPackage excede el límite de 16 KiB");
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes).context("No se pudo leer un pathname")?;
        let raw = std::str::from_utf8(&bytes).context("Un pathname no es UTF-8 válido")?;
        if let Some((root, folder)) = asset_folders(raw) {
            on_path(raw.trim_end_matches(['\0', '\r', '\n']));
            asset_count += 1;
            if asset_count > MAX_PATHNAMES {
                bail!("El UnityPackage contiene demasiados pathnames");
            }
            roots.insert(root);
            folders.insert(folder);
            if folders.len() > MAX_FOLDERS {
                bail!("El UnityPackage contiene demasiadas carpetas distintas");
            }
        } else {
            skipped_paths += 1;
        }
    }

    Ok(UnityPathsPreview {
        main_folders: roots.into_iter().collect(),
        folders: folders.into_iter().collect(),
        asset_count,
        skipped_paths,
    })
}

fn asset_folders(raw: &str) -> Option<(String, String)> {
    let path = raw.trim_end_matches(['\0', '\r', '\n']).replace('\\', "/");
    if path.contains('\0') || path.starts_with('/') {
        return None;
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() < 2 || segments[0] != "Assets"
        || segments.iter().any(|part| part.is_empty() || *part == "." || *part == ".."
            || part.contains(':') || part.chars().any(char::is_control)) {
        return None;
    }
    let main = if segments.len() == 2 {
        "Assets".to_owned()
    } else {
        format!("Assets/{}", segments[1])
    };
    let parent = segments[..segments.len() - 1].join("/");
    Some((main, parent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    fn fixture(paths: &[&str]) -> Vec<u8> {
        let gzip = GzEncoder::new(Vec::new(), Compression::default());
        let mut tar = tar::Builder::new(gzip);
        for (index, path) in paths.iter().enumerate() {
            let mut header = tar::Header::new_gnu();
            header.set_size(path.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, format!("{index:032x}/pathname"), path.as_bytes())
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn lists_multiple_destinations_without_extracting_assets() {
        let input = fixture(&[
            "Assets/Outfit/Prefabs/Top.prefab\n",
            "Assets/Outfit/Textures/Top.png",
            "Assets/Poiyomi/Materials/Toon.mat",
            "Assets/Root.prefab",
            "Assets/Outfit/Prefabs/Top.prefab",
        ]);
        let result = inspect_reader(input.as_slice()).unwrap();
        assert_eq!(result.asset_count, 5);
        assert_eq!(result.main_folders, vec!["Assets", "Assets/Outfit", "Assets/Poiyomi"]);
        assert_eq!(result.folders, vec![
            "Assets", "Assets/Outfit/Prefabs", "Assets/Outfit/Textures",
            "Assets/Poiyomi/Materials"
        ]);
        assert_eq!(result.skipped_paths, 0);
    }

    #[test]
    fn ignores_unsafe_and_outside_assets_paths() {
        let input = fixture(&[
            "../../System/file", "/Assets/Absolute.asset", "Assets/../Secret.asset",
            "Packages/com.example/plugin.asset", "Assets//invalid.asset",
            "Assets\\Valid\\Prefab.prefab", "Assets/Ok.asset",
        ]);
        let result = inspect_reader(input.as_slice()).unwrap();
        assert_eq!(result.asset_count, 2);
        assert_eq!(result.skipped_paths, 5);
        assert_eq!(result.main_folders, vec!["Assets", "Assets/Valid"]);
        assert_eq!(result.folders, vec!["Assets", "Assets/Valid"]);
    }

    #[test]
    fn rejects_corrupt_gzip() {
        assert!(inspect_reader(&b"not a gzip unitypackage"[..]).is_err());
    }

    #[test]
    fn no_assets_is_distinguished_from_malformed_archive() {
        let input = fixture(&["Packages/com.example/tool.prefab"]);
        let result = inspect_reader(input.as_slice()).unwrap();
        assert!(result.folders.is_empty());
        assert_eq!(result.skipped_paths, 1);
    }

    #[test]
    fn oversized_pathname_is_rejected() {
        let oversized = "A".repeat(MAX_PATHNAME_BYTES as usize + 1);
        let input = fixture(&[&oversized]);
        assert!(inspect_reader(input.as_slice()).is_err());
    }

    #[test]
    fn public_reader_works_on_a_real_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.unitypackage");
        let mut file = File::create(&file_path).unwrap();
        file.write_all(&fixture(&["Assets/Demo/mesh.fbx"])).unwrap();
        assert_eq!(inspect_unitypackage_paths(&file_path).unwrap().main_folders,
            vec!["Assets/Demo"]);
    }
}
