//! Infer compatible avatar labels from archive names and Unity pathname metadata.
//! Never executes Unity content or rewrites linked assets.
use std::{collections::HashSet, path::{Path, PathBuf}};
use crate::{import, package_inspect, unitypackage_inspect};

const MAX_FILES: usize = 256;
const MAX_PACKAGES_PER_ARCHIVE: usize = 16;
const MAX_PACKAGE_SCAN_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ScanResult {
    pub matches: Vec<String>,
    pub warnings: Vec<String>,
    pub scanned_files: usize,
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_uppercase)
        .collect()
}

/// Compare complete words (also multi-word avatar names) rather than substrings.
pub fn name_mentions_avatar(path: &str, avatar: &str) -> bool {
    let filename = tokens(path);
    let expected = tokens(avatar);
    !expected.is_empty() && filename.windows(expected.len())
        .any(|window| window == expected.as_slice())
}

fn detect_name(name: &str, known: &[String], found: &mut HashSet<String>) {
    for avatar in known {
        if !found.contains(avatar) && name_mentions_avatar(name, avatar) {
            found.insert(avatar.clone());
        }
    }
}

fn detect_unitypackage(path: &Path, known: &[String], found: &mut HashSet<String>)
    -> anyhow::Result<()> {
    unitypackage_inspect::inspect_unitypackage_paths_with(path, |name| {
        detect_name(name, known, found);
    })?;
    Ok(())
}

fn scan_file(path: &Path, original_name: &str, known: &[String],
    found: &mut HashSet<String>, warnings: &mut Vec<String>, scratch: &Path
) {
    // Managed copies can have a checksum-prefixed name. Prefer the original label.
    detect_name(original_name, known, found);
    if !path.is_file() {
        warnings.push(format!("Archivo no disponible: {original_name}"));
        return;
    }
    if path.extension().and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("unitypackage")) {
        if let Err(error) = detect_unitypackage(path, known, found) {
            warnings.push(format!("{original_name}: {error}"));
        }
        return;
    }
    if !import::supported(path) { return; }

    // Unlike the inspector's 512-row UI preview, include every safe root name.
    match package_inspect::list_archive_root_names(path) {
        Ok(names) => for name in names { detect_name(&name, known, found); },
        Err(error) => {
            warnings.push(format!("{original_name}: {error}"));
            return;
        },
    }

    // Reuse the safe nested-ZIP traversal and package metadata (also for 7z/RAR).
    let preview = match package_inspect::inspect_any_archive(path) {
        Ok(preview) => preview,
        Err(error) => {
            warnings.push(format!("{original_name}: {error}"));
            return;
        },
    };
    for entry in &preview.root_files { detect_name(&entry.name, known, found); }
    for nested in &preview.inner_archives {
        for part in &nested.chain { detect_name(&part.name, known, found); }
        for entry in &nested.files { detect_name(&entry.name, known, found); }
        if nested.hidden_files > 0 || nested.skipped_reason.is_some() {
            warnings.push(format!("{original_name}: inspección parcial de ZIP interno"));
        }
    }
    if preview.root_hidden_files > 0 && preview.nested_entries > 0 {
        warnings.push(format!("{original_name}: parte del contenido anidado está oculta"));
    }
    if preview.nested_skipped > 0 || preview.hidden_archives > 0 {
        warnings.push(format!("{original_name}: algunos ZIP internos se omitieron"));
    }

    // An explicitly requested scan may read bounded Unity metadata inside an
    // archive. Extracted packages live only in this scan's temporary directory.
    let mut bytes = 0u64;
    let mut skipped = 0usize;
    for (index, package) in preview.packages.iter().enumerate() {
        detect_name(&package.name, known, found);
        if index >= MAX_PACKAGES_PER_ARCHIVE
            || bytes.saturating_add(package.size) > MAX_PACKAGE_SCAN_BYTES {
            skipped += 1;
            continue;
        }
        bytes += package.size;
        match package_inspect::extract_unitypackage_nested(path, package, scratch) {
            Ok(extracted) => {
                if let Err(error) = detect_unitypackage(&extracted, known, found) {
                    warnings.push(format!("{}: {error}", package.name));
                }
            },
            Err(error) => warnings.push(format!("{}: {error}", package.name)),
        }
    }
    if preview.total_packages > preview.packages.len() {
        skipped += preview.total_packages - preview.packages.len();
    }
    if skipped > 0 {
        warnings.push(format!("{original_name}: {skipped} UnityPackages omitidos por límites de análisis"));
    }
}

/// Scan only local files, directories and bounded archive metadata. Per-file
/// failures are reported but do not prevent results from other attachments.
pub fn scan(sources: &[(PathBuf, String)], known: &[String]) -> ScanResult {
    let mut result = ScanResult::default();
    if known.is_empty() { return result; }
    let scratch = match tempfile::tempdir() {
        Ok(scratch) => scratch,
        Err(error) => {
            result.warnings.push(format!("No se pudo crear caché temporal: {error}"));
            return result;
        }
    };
    let mut found = HashSet::new();
    for (source, name) in sources {
        if source.is_dir() {
            for entry in walkdir::WalkDir::new(source).follow_links(false).into_iter() {
                if result.scanned_files >= MAX_FILES { break; }
                match entry {
                    Ok(item) if item.file_type().is_file() && import::supported(item.path()) => {
                        let filename = item.file_name().to_string_lossy();
                        scan_file(item.path(), &filename, known, &mut found,
                            &mut result.warnings, scratch.path());
                        result.scanned_files += 1;
                    },
                    Err(error) => result.warnings.push(format!("Carpeta: {error}")),
                    _ => {}
                }
            }
        } else {
            if result.scanned_files >= MAX_FILES { break; }
            scan_file(source, name, known, &mut found, &mut result.warnings, scratch.path());
            result.scanned_files += 1;
        }
    }
    if result.scanned_files >= MAX_FILES && sources.len() >= MAX_FILES {
        result.warnings.push("Límite de 256 archivos examinados".into());
    }
    result.matches = known.iter().filter(|name| found.contains(*name)).cloned().collect();
    // Keep the UI status small and avoid repeating the same warning.
    result.warnings.sort();
    result.warnings.dedup();
    result
}

/// Preserve text already typed in the field. Only append absent labels.
/// Persisting/uppercasing is still handled by Catalog::set_avatars on Save.
pub fn merge_avatars(current: &str, detected: &[String]) -> (String, Vec<String>) {
    let mut seen: HashSet<String> = current.split([',', '\n', ';'])
        .map(|text| text.trim().to_uppercase())
        .filter(|text| !text.is_empty())
        .collect();
    let mut added = Vec::new();
    for avatar in detected {
        let label = avatar.trim().to_uppercase();
        if !label.is_empty() && seen.insert(label.clone()) {
            added.push(label);
        }
    }
    if added.is_empty() { return (current.to_owned(), added); }
    let prefix = current.trim_end_matches(|c: char|
        c.is_whitespace() || c == ',' || c == ';');
    let joined = if prefix.is_empty() {
        added.join(", ")
    } else {
        format!("{prefix}, {}", added.join(", "))
    };
    (joined, added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, io::Write};

    #[test]
    fn matches_paths_and_whole_names_not_substrings() {
        assert!(name_mentions_avatar(
            "Manuka_CWU27P_FlightSuit/Manuka_CWU27P_FlightSuit.unitypackage", "MANUKA"));
        assert!(name_mentions_avatar("Assets/Outfit/MANUKA_XXXX.prefab", "MANUKA"));
        assert!(name_mentions_avatar("Avatar/Maya-Plus/coat.prefab", "MAYA PLUS"));
        assert!(!name_mentions_avatar("Mayako_Outfit.zip", "MAYA"));
        assert!(!name_mentions_avatar("NotManuka.prefab", "MANUKA"));
        assert!(name_mentions_avatar("Assets/しなの/hat.prefab", "しなの"));
    }

    #[test]
    fn appends_without_losing_typed_content_or_creating_duplicates() {
        let detected = vec!["MANUKA".into(), "MAYA".into()];
        let (once, added) = merge_avatars("Maya, avatar manual", &detected);
        assert_eq!(once, "Maya, avatar manual, MANUKA");
        assert_eq!(added, vec!["MANUKA"]);
        let (twice, again) = merge_avatars(&once, &detected);
        assert_eq!(twice, once);
        assert!(again.is_empty());
    }

    #[test]
    fn reads_zip_names_including_nested_unitypackages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.start_file("Manuka_CWU27P_FlightSuit/Manuka_CWU27P_FlightSuit.unitypackage",
            zip::write::FileOptions::default()).unwrap();
        zip.write_all(b"not a valid unitypackage").unwrap();
        zip.finish().unwrap();
        let report = scan(&[(path, "other.zip".into())], &["MANUKA".into()]);
        assert_eq!(report.matches, vec!["MANUKA"]);
    }

    #[test]
    fn reads_prefab_names_inside_real_unitypackage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outfit.unitypackage");
        let gz = flate2::write::GzEncoder::new(File::create(&path).unwrap(),
            flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        let contents = b"Assets/Clothing/Prefabs/MANUKA_XXXX.prefab\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "00112233/pathname", &contents[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let report = scan(&[(path, "outfit.unitypackage".into())], &["MANUKA".into()]);
        assert_eq!(report.matches, vec!["MANUKA"]);
        assert!(report.warnings.is_empty());
    }
}
