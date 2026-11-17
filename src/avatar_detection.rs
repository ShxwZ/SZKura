//! Infer compatible avatars from archive filenames and Unity pathname metadata.
//! No user files are modified, executed or permanently extracted.
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use crate::{import, package_inspect, unitypackage_inspect};

const MAX_FILES: usize = 256;
const MAX_WALK_ENTRIES: usize = 4_096;
const MAX_WALK_DEPTH: usize = 12;
const MAX_PACKAGES_PER_ARCHIVE: usize = 16;
// Resource budgets apply to the ENTIRE button click, not each attachment.
const MAX_UNITY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_UNITY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DEEP_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_DEEP_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ScanResult {
    pub matches: Vec<String>,
    pub warnings: Vec<String>,
    pub scanned_files: usize,
}

#[derive(Default)]
struct Budget {
    unity_bytes: u64,
    deep_archive_bytes: u64,
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_uppercase)
        .collect()
}

/// Compare complete words, including multi-word avatar names, not substrings.
pub fn name_mentions_avatar(path: &str, avatar: &str) -> bool {
    let filename = tokens(path);
    let expected = tokens(avatar);
    !expected.is_empty() && filename.windows(expected.len())
        .any(|window| window == expected.as_slice())
}

/// Compile the avatar vocabulary once. Previously each filename was tokenized
/// once per avatar, causing quadratic allocations in large archives.
struct Matcher {
    names: Vec<String>,
    words: Vec<Vec<String>>,
    by_first_word: HashMap<String, Vec<usize>>,
}
impl Matcher {
    fn new(known: &[String]) -> Self {
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        let mut words = Vec::new();
        let mut by_first_word:HashMap<String,Vec<usize>> = HashMap::new();
        for avatar in known {
            let name = avatar.trim().to_uppercase();
            let expected = tokens(&name);
            if expected.is_empty() || !seen.insert(name.clone()) { continue; }
            let index = names.len();
            by_first_word.entry(expected[0].clone()).or_default().push(index);
            names.push(name);
            words.push(expected);
        }
        Self { names, words, by_first_word }
    }

    fn detect(&self, name: &str, found: &mut HashSet<String>) {
        // Untrusted archive filenames may be long; do not allocate or tokenize
        // arbitrary megabytes of textual metadata per entry.
        if name.len() > 16 * 1024 { return; }
        let words = tokens(name);
        for (position, word) in words.iter().enumerate() {
            if let Some(candidates) = self.by_first_word.get(word) {
                for &index in candidates {
                    if !found.contains(&self.names[index])
                        && words[position..].starts_with(&self.words[index]) {
                        found.insert(self.names[index].clone());
                    }
                }
            }
        }
    }

    fn complete(&self, found: &HashSet<String>) -> bool {
        found.len() == self.names.len()
    }
}

fn canceled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

fn detect_unitypackage(
    path: &Path, matcher: &Matcher, found: &mut HashSet<String>, cancel: &AtomicBool,
) -> anyhow::Result<()> {
    // Stop reading the gzip/TAR stream as soon as all names are detected or the
    // user cancels. No Unity assets/scripts are extracted or executed.
    unitypackage_inspect::inspect_unitypackage_paths_with(path, |name| {
        matcher.detect(name, found);
        !matcher.complete(found) && !canceled(cancel)
    })?;
    Ok(())
}

fn scan_file(
    path: &Path, original_name: &str, matcher: &Matcher, found: &mut HashSet<String>,
    warnings: &mut Vec<String>, scratch: &Path, budget: &mut Budget, cancel: &AtomicBool,
) {
    // Managed files may use a checksum-prefixed on-disk name.
    matcher.detect(original_name, found);
    if canceled(cancel) || matcher.complete(found) { return; }
    if !path.is_file() {
        warnings.push(format!("Archivo no disponible: {original_name}"));
        return;
    }

    let size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            warnings.push(format!("{original_name}: {error}"));
            return;
        }
    };
    if path.extension().and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("unitypackage")) {
        if size > MAX_UNITY_BYTES
            || budget.unity_bytes.saturating_add(size) > MAX_TOTAL_UNITY_BYTES {
            warnings.push(format!("{original_name}: UnityPackage omitido por límite de lectura"));
            return;
        }
        budget.unity_bytes += size;
        if let Err(error) = detect_unitypackage(path, matcher, found, cancel) {
            warnings.push(format!("{original_name}: {error}"));
        }
        return;
    }
    if !import::supported(path) { return; }

    // This visits ALL safe root entry names, unlike the inspector's capped
    // preview, and avoids allocating an extra list of thousands of paths.
    if let Err(error) = package_inspect::visit_archive_root_names(path, |name| {
        if canceled(cancel) || matcher.complete(found) { return false; }
        matcher.detect(name, found);
        !matcher.complete(found)
    }) {
        warnings.push(format!("{original_name}: {error}"));
        return;
    }
    if canceled(cancel) || matcher.complete(found) { return; }

    // The nested inspector can decompress ZIPs inside ZIP/7z/RAR. Limit the
    // amount of outer archive data considered for *deep* inspections across
    // the entire operation; root filenames above were still examined.
    if size > MAX_DEEP_ARCHIVE_BYTES
        || budget.deep_archive_bytes.saturating_add(size) > MAX_TOTAL_DEEP_ARCHIVE_BYTES {
        warnings.push(format!("{original_name}: inspección profunda omitida por límite de lectura"));
        return;
    }
    budget.deep_archive_bytes += size;
    let preview = match package_inspect::inspect_any_archive(path) {
        Ok(preview) => preview,
        Err(error) => {
            warnings.push(format!("{original_name}: {error}"));
            return;
        },
    };
    // All outer names are already visited; handle the nested ZIP hierarchy.
    for nested in &preview.inner_archives {
        if canceled(cancel) || matcher.complete(found) { break; }
        for part in &nested.chain { matcher.detect(&part.name, found); }
        for entry in &nested.files { matcher.detect(&entry.name, found); }
        if nested.hidden_files > 0 || nested.skipped_reason.is_some() {
            warnings.push(format!("{original_name}: inspección parcial de ZIP interno"));
        }
    }
    if preview.nested_skipped > 0 || preview.hidden_archives > 0 {
        warnings.push(format!("{original_name}: algunos ZIP internos se omitieron"));
    }
    if canceled(cancel) || matcher.complete(found) { return; }

    // Bounded temporary extraction for nested Unity packages only. Budget
    // applies globally (a set of 256 attachments must not use 256*512 MiB).
    let mut skipped_packages = 0usize;
    for (index, package) in preview.packages.iter().enumerate() {
        if canceled(cancel) || matcher.complete(found) { break; }
        matcher.detect(&package.name, found);
        if matcher.complete(found) { break; }
        if index >= MAX_PACKAGES_PER_ARCHIVE
            || package.size > MAX_UNITY_BYTES
            || budget.unity_bytes.saturating_add(package.size) > MAX_TOTAL_UNITY_BYTES {
            skipped_packages += 1;
            continue;
        }
        budget.unity_bytes += package.size;
        match package_inspect::extract_unitypackage_nested(path, package, scratch) {
            Ok(extracted) => if let Err(error) = detect_unitypackage(&extracted, matcher, found, cancel) {
                warnings.push(format!("{}: {error}", package.name));
            },
            Err(error) => warnings.push(format!("{}: {error}", package.name)),
        }
    }
    if preview.total_packages > preview.packages.len() {
        skipped_packages += preview.total_packages - preview.packages.len();
    }
    if skipped_packages > 0 && !canceled(cancel) {
        warnings.push(format!(
            "{original_name}: {skipped_packages} UnityPackages omitidos por límites de análisis"
        ));
    }
}

/// Public convenience wrapper for non-interactive use and unit tests.
pub fn scan(sources: &[(PathBuf, String)], known: &[String]) -> ScanResult {
    scan_with_cancel(sources, known, &AtomicBool::new(false))
}

/// Analyze on a background worker, with bounded work and cooperative cancel.
pub fn scan_with_cancel(
    sources: &[(PathBuf, String)], known: &[String], cancel: &AtomicBool,
) -> ScanResult {
    let mut result = ScanResult::default();
    let matcher = Matcher::new(known);
    if matcher.names.is_empty() || canceled(cancel) { return result; }
    let scratch = match tempfile::tempdir() {
        Ok(scratch) => scratch,
        Err(error) => {
            result.warnings.push(format!("No se pudo crear caché temporal: {error}"));
            return result;
        }
    };
    let mut budget = Budget::default();
    let mut found = HashSet::new();
    let mut limit_reached = false;
    for (source, original) in sources {
        if canceled(cancel) || matcher.complete(&found) { break; }
        if result.scanned_files >= MAX_FILES { limit_reached = true; break; }
        if source.is_dir() {
            let mut walked = 0usize;
            for entry in walkdir::WalkDir::new(source)
                .follow_links(false).max_depth(MAX_WALK_DEPTH).into_iter() {
                if canceled(cancel) || matcher.complete(&found) { break; }
                walked += 1;
                if walked > MAX_WALK_ENTRIES || result.scanned_files >= MAX_FILES {
                    limit_reached = true;
                    break;
                }
                match entry {
                    Ok(item) if item.file_type().is_file() && import::supported(item.path()) => {
                        let filename = item.file_name().to_string_lossy();
                        scan_file(item.path(), &filename, &matcher, &mut found,
                            &mut result.warnings, scratch.path(), &mut budget, cancel);
                        result.scanned_files += 1;
                    },
                    Err(error) => result.warnings.push(format!("Carpeta: {error}")),
                    _ => {}
                }
            }
        } else {
            scan_file(source, original, &matcher, &mut found,
                &mut result.warnings, scratch.path(), &mut budget, cancel);
            result.scanned_files += 1;
        }
    }
    if limit_reached && !canceled(cancel) {
        result.warnings.push("Límite de archivos o entradas de carpeta alcanzado".into());
    }
    result.matches = matcher.names.into_iter().filter(|name| found.contains(name)).collect();
    result.warnings.sort();
    result.warnings.dedup();
    result
}

/// Number of slots left before the catalog's 64-avatar limit per product.
pub fn remaining_slots(current: &str) -> usize {
    let unique: HashSet<String> = current.split([',','\n',';'])
        .map(|name| name.trim().to_uppercase())
        .filter(|name| !name.is_empty()).collect();
    64usize.saturating_sub(unique.len())
}

/// Preserve all typed values and their formatting, append only absent names,
/// and respect the existing 64-avatar limit in Catalog::set_avatars.
pub fn merge_avatars(current: &str, detected: &[String]) -> (String, Vec<String>) {
    let mut seen: HashSet<String> = current.split([',', '\n', ';'])
        .map(|name| name.trim().to_uppercase())
        .filter(|name| !name.is_empty()).collect();
    let mut added = Vec::new();
    for avatar in detected {
        let label = avatar.trim().to_uppercase();
        if !label.is_empty() && !seen.contains(&label) && seen.len() < 64 {
            seen.insert(label.clone());
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
    #[test]
    fn scans_all_root_entries_after_the_preview_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other.zip");
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        for index in 0..600 {
            writer.start_file(format!("ordinary/file_{index}.txt"),
                zip::write::FileOptions::default()).unwrap();
            writer.write_all(b"content").unwrap();
        }
        writer.start_file("outfits/MANUKA_XXXX.prefab",
            zip::write::FileOptions::default()).unwrap();
        writer.write_all(b"content").unwrap();
        writer.finish().unwrap();
        let report = scan(&[(path, "other.zip".into())], &["MANUKA".into()]);
        assert_eq!(report.matches, vec!["MANUKA"]);
    }

    #[test]
    fn respects_existing_avatar_cap_and_preserves_original_text() {
        let current = (0..64).map(|i| format!("AVATAR{i}"))
            .collect::<Vec<_>>().join(", ");
        let (merged, added) = merge_avatars(&current, &["MANUKA".into()]);
        assert_eq!(merged, current);
        assert!(added.is_empty());
        assert_eq!(remaining_slots(&current), 0);
    }

    #[test]
    fn cancelled_scan_does_no_file_work() {
        let cancel = AtomicBool::new(true);
        let report = scan_with_cancel(
            &[(PathBuf::from("missing.zip"), "MANUKA.zip".into())],
            &["MANUKA".into()], &cancel,
        );
        assert_eq!(report.scanned_files, 0);
        assert!(report.matches.is_empty());
    }

    #[test]
    fn many_candidates_and_compound_names_match_without_substrings() {
        let known:Vec<String> = (0..1500).map(|i| format!("AV{i}")).collect();
        let matcher = Matcher::new(&known);
        let mut found = HashSet::new();
        matcher.detect("Assets/Outfits/AV1499_test.prefab", &mut found);
        assert!(found.contains("AV1499"));
        assert!(!found.contains("AV149"));
        assert!(!matcher.complete(&found));
    }

}
