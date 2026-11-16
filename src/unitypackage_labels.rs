//! Experimental, opt-in exporter that adds native Unity Asset Labels to a
//! *copy* of a .unitypackage. It never writes to the source or to a Unity project.
//! Each GUID's pathname/asset/payload is kept; only eligible asset.meta entries
//! gain a top-level YAML label (preserving any labels they already contain).
use anyhow::{bail, Context, Result};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use std::{collections::BTreeSet, fs::File, io::{BufReader, Read}, path::Path};
use tar::{Archive, Builder};
use tempfile::NamedTempFile;

const MAX_ENTRIES: usize = 100_000;
const MAX_PATHNAME_BYTES: u64 = 16 * 1024;
const MAX_META_BYTES: u64 = 8 * 1024 * 1024;
const MAX_UNCOMPRESSED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_TARGETS: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabeledExportReport {
    pub labeled_assets: usize,
    pub already_labeled: usize,
    pub label: String,
}

pub fn label_for_category(category: &str) -> Result<String> {
    if !crate::db::CATEGORIES.contains(&category) {
        bail!("Categoría no reconocida en SZKura");
    }
    Ok(format!("SZKura_{category}"))
}

pub fn export_labeled_unitypackage(
    source: &Path, destination: &Path, category: &str
) -> Result<LabeledExportReport> {
    let label = label_for_category(category)?;
    if source.extension().and_then(|x|x.to_str())
        .is_none_or(|x|!x.eq_ignore_ascii_case("unitypackage"))
        || destination.extension().and_then(|x|x.to_str())
        .is_none_or(|x|!x.eq_ignore_ascii_case("unitypackage")) {
        bail!("Origen y destino deben ser archivos .unitypackage");
    }
    if destination.exists() {
        bail!("El destino ya existe. Elige otro nombre: no se sobrescriben archivos");
    }
    let info = std::fs::metadata(source).context("No se pudo abrir el UnityPackage original")?;
    if !info.is_file() || info.len() > crate::package_inspect::MAX_PACKAGE_BYTES {
        bail!("UnityPackage no válido o mayor de 2 GiB");
    }

    // Pass 1: only GUIDs with valid Assets/ pathname may be labeled. This also
    // validates the input TAR before writing any proposed output.
    let eligible = eligible_guids(source)?;
    if eligible.is_empty() { bail!("El paquete no contiene rutas Assets/ etiquetables"); }

    // Same filesystem directory => atomic persist_noclobber, no partial output
    // on errors and no risk of overwriting an existing package.
    let parent = destination.parent().filter(|p|!p.as_os_str().is_empty())
        .unwrap_or_else(||Path::new("."));
    let mut staging = NamedTempFile::new_in(parent)
        .context("No se puede crear el archivo temporal de exportación")?;
    let file = File::open(source)?;
    let gzip = GzDecoder::new(BufReader::new(file)).take(MAX_UNCOMPRESSED_BYTES);
    let mut archive = Archive::new(gzip);
    let encoder = GzEncoder::new(staging.as_file_mut(), Compression::default());
    let mut writer = Builder::new(encoder);
    let mut copied = 0usize;
    let mut total = 0u64;
    let mut seen = BTreeSet::new();
    let mut labeled = 0usize;
    let mut already = 0usize;
    for entry in archive.entries().context("No se pudo leer el TAR original")? {
        let mut entry = entry.context("Entrada TAR dañada")?;
        copied += 1;
        if copied > MAX_ENTRIES { bail!("UnityPackage con demasiadas entradas"); }
        total = total.saturating_add(entry.size()).saturating_add(512);
        if total > MAX_UNCOMPRESSED_BYTES { bail!("UnityPackage supera el límite de 4 GiB de datos"); }
        let path = entry.path().context("Nombre TAR no válido")?
            .to_string_lossy().into_owned();
        let mut header = entry.header().clone();
        if entry.header().entry_type().is_file() {
            if let Some(guid) = member_guid(&path, "asset.meta") {
                if eligible.contains(guid) {
                    if !seen.insert(guid.to_owned()) {
                        bail!("Metadatos duplicados en el paquete");
                    }
                    if entry.size() > MAX_META_BYTES { bail!("asset.meta supera el límite de 8 MiB"); }
                    let mut original = Vec::with_capacity(entry.size() as usize);
                    entry.read_to_end(&mut original)?;
                    let updated = add_label(&original, &label, guid)?;
                    if updated == original {
                        already += 1;
                        writer.append(&header, updated.as_slice())?;
                    } else {
                        labeled += 1;
                        header.set_size(updated.len() as u64);
                        header.set_cksum();
                        writer.append(&header, updated.as_slice())?;
                    }
                    continue;
                }
            }
        }
        // No path, importer payload, scripts, images or GUIDs are rewritten.
        writer.append(&header, &mut entry).context("No se pudo copiar una entrada TAR")?;
    }
    if seen.len() != eligible.len() {
        bail!("Faltan archivos asset.meta correspondientes a rutas Assets/ del paquete");
    }
    writer.finish()?;
    let encoder = writer.into_inner()?;
    encoder.finish()?;
    staging.as_file_mut().sync_all()?;
    staging.persist_noclobber(destination)
        .map_err(|e|e.error).context("No se pudo guardar la copia sin sobrescribir el destino")?;
    Ok(LabeledExportReport {
        labeled_assets: labeled, already_labeled: already, label,
    })
}

fn member_guid<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    let (guid, member) = path.split_once('/')?;
    if member != name || guid.len() != 32 || !guid.bytes().all(|c|c.is_ascii_hexdigit()) {
        return None;
    }
    Some(guid)
}

fn eligible_guids(source: &Path) -> Result<BTreeSet<String>> {
    let gzip = GzDecoder::new(BufReader::new(File::open(source)?))
        .take(MAX_UNCOMPRESSED_BYTES);
    let mut archive = Archive::new(gzip);
    let mut selected = BTreeSet::new();
    let mut count = 0usize;
    let mut total = 0u64;
    for entry in archive.entries().context("UnityPackage TAR no válido")? {
        let mut entry = entry.context("Entrada TAR dañada")?;
        count += 1;
        if count > MAX_ENTRIES { bail!("UnityPackage con demasiadas entradas"); }
        total = total.saturating_add(entry.size()).saturating_add(512);
        if total > MAX_UNCOMPRESSED_BYTES { bail!("UnityPackage supera el límite de 4 GiB de datos"); }
        if !entry.header().entry_type().is_file() { continue; }
        let name = entry.path()?.to_string_lossy().into_owned();
        let Some(guid) = member_guid(&name, "pathname") else { continue; };
        if entry.size() > MAX_PATHNAME_BYTES { bail!("pathname supera el límite de 16 KiB"); }
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        let path = std::str::from_utf8(&data).context("pathname no es UTF-8")?;
        let first_line = path.lines().next().unwrap_or("")
            .trim_end_matches('\0').replace('\\', "/");
        let parts = first_line.split('/').collect::<Vec<_>>();
        if parts.len() < 2 || parts[0] != "Assets" || parts.iter().any(|p|
            p.is_empty() || *p=="." || *p==".." || p.contains(':') ||
            p.chars().any(char::is_control)) { continue; }
        selected.insert(guid.to_owned());
        if selected.len() > MAX_TARGETS { bail!("Demasiados assets para etiquetar"); }
    }
    Ok(selected)
}

/// Add a root-level YAML labels entry without touching GUID, importer data or
/// existing labels. Deliberately reject uncommon unsupported YAML forms rather
/// than risking corruption of an asset's import settings.
fn add_label(original: &[u8], label: &str, guid: &str) -> Result<Vec<u8>> {
    let src = std::str::from_utf8(original).context("asset.meta no es UTF-8")?;
    if src.contains('\0') { bail!("asset.meta contiene bytes NUL"); }
    let crlf = src.contains("\r\n");
    let eol = if crlf {"\r\n"} else {"\n"};
    let newline = src.ends_with('\n');
    let mut lines = src.lines().map(|l|l.trim_end_matches('\r').to_owned())
        .collect::<Vec<_>>();
    if !lines.iter().any(|line|line.trim()=="fileFormatVersion: 2") {
        bail!("asset.meta sin formato Unity compatible");
    }
    let guid_line = format!("guid: {guid}");
    let Some(guid_index) = lines.iter().position(|line|line==&guid_line) else {
        bail!("El GUID de asset.meta no coincide con el paquete");
    };
    let mut label_indices = lines.iter().enumerate().filter(|(_,line)|
        line.starts_with("labels:")).map(|(index,_)|index);
    match (label_indices.next(), label_indices.next()) {
        (None,None) => {
            lines.insert(guid_index+1,"labels:".into());
            lines.insert(guid_index+2,format!("- {label}"));
        },
        (Some(index),None) => {
            match lines[index].trim() {
                "labels: []" => {
                    lines[index]="labels:".into();
                    lines.insert(index+1,format!("- {label}"));
                },
                "labels:" => {
                    let mut end = index+1;
                    while end < lines.len() && lines[end].starts_with("- ") {
                        if lines[end][2..].trim() == label { return Ok(original.to_vec()); }
                        end += 1;
                    }
                    if end < lines.len() && lines[end].trim_start().starts_with("- ") {
                        bail!("Lista de etiquetas YAML no admitida");
                    }
                    lines.insert(end,format!("- {label}"));
                },
                _ => bail!("Formato inline de etiquetas no admitido para exportación"),
            }
        },
        _ => bail!("Metadatos con varias secciones labels"),
    }
    let mut output = lines.join(eol);
    if newline {output.push_str(eol);}
    Ok(output.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap,io::Write};

    const A:&str="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B:&str="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    fn add_entry<W:Write>(builder:&mut Builder<W>,path:&str,data:&[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder.append_data(&mut header,path,data).unwrap();
    }
    fn fixture(path:&Path,existing_label:bool) {
        let output = File::create(path).unwrap();
        let gz = GzEncoder::new(output,Compression::default());
        let mut tar = Builder::new(gz);
        for (guid,payload) in [(A,b"FIRST".as_slice()),(B,b"SECOND".as_slice())] {
            add_entry(&mut tar,&format!("{guid}/asset"),payload);
            let meta=if existing_label {
                format!("fileFormatVersion: 2\nguid: {guid}\nlabels:\n- CreatorTag\nDefaultImporter:\n  userData: \n")
            }else{format!("fileFormatVersion: 2\nguid: {guid}\nDefaultImporter:\n  userData: \n")};
            add_entry(&mut tar,&format!("{guid}/asset.meta"),meta.as_bytes());
            let path=if guid==A {"Assets/Outfit/top.prefab"} else {"Assets/Other/file.txt"};
            add_entry(&mut tar,&format!("{guid}/pathname"),path.as_bytes());
        }
        let gz=tar.into_inner().unwrap();
        gz.finish().unwrap();
    }
    fn contents(path:&Path)->HashMap<String,Vec<u8>> {
        let mut archive=Archive::new(GzDecoder::new(File::open(path).unwrap()));
        let mut files=HashMap::new();
        for file in archive.entries().unwrap() {
            let mut file=file.unwrap();
            let name=file.path().unwrap().to_string_lossy().into_owned();
            let mut bytes=Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            files.insert(name,bytes);
        }
        files
    }

    #[test]
    fn category_is_limited_to_catalog_values() {
        assert_eq!(label_for_category("Ropa").unwrap(),"SZKura_Ropa");
        assert_eq!(label_for_category("Pelo").unwrap(),"SZKura_Pelo");
        assert!(label_for_category("..\nmalicious").is_err());
    }
    #[test]
    fn meta_with_existing_labels_preserves_them_and_guid() {
        let input=format!("fileFormatVersion: 2\nguid: {A}\nlabels:\n- Other\nDefaultImporter:\n  userData: \n");
        let tagged=String::from_utf8(add_label(input.as_bytes(),"SZKura_Ropa",A).unwrap()).unwrap();
        assert!(tagged.contains("labels:\n- Other\n- SZKura_Ropa\nDefaultImporter:"));
        assert!(tagged.contains(&format!("guid: {A}")));
        assert_eq!(add_label(tagged.as_bytes(),"SZKura_Ropa",A).unwrap(),tagged.as_bytes());
    }
    #[test]
    fn metadata_without_labels_gets_native_root_level_label() {
        let input=format!("fileFormatVersion: 2\r\nguid: {A}\r\nDefaultImporter:\r\n  userData: \r\n");
        let tagged=String::from_utf8(add_label(input.as_bytes(),"SZKura_Ropa",A).unwrap()).unwrap();
        assert!(tagged.contains(&format!("guid: {A}\r\nlabels:\r\n- SZKura_Ropa\r\nDefaultImporter:")));
    }
    #[test]
    fn unsupported_metadata_is_rejected_safely() {
        let input=format!("fileFormatVersion: 2\nguid: {A}\nlabels: [foo,bar]\n");
        assert!(add_label(input.as_bytes(),"SZKura_Ropa",A).is_err());
        assert!(add_label(input.as_bytes(),"SZKura_Ropa",B).is_err());
    }
    #[test]
    fn exported_package_preserves_unmodified_payloads_and_existing_labels() {
        let dir=tempfile::tempdir().unwrap();
        let src=dir.path().join("input.unitypackage");
        let dst=dir.path().join("labeled.unitypackage");
        fixture(&src,true);
        let old=contents(&src);
        let report=export_labeled_unitypackage(&src,&dst,"Ropa").unwrap();
        assert_eq!(report.labeled_assets,2);
        assert_eq!(report.already_labeled,0);
        let new=contents(&dst);
        assert_eq!(old.len(),new.len());
        for (name,bytes) in old {
            if name.ends_with("/asset.meta") {
                let s=String::from_utf8(new.get(&name).unwrap().clone()).unwrap();
                assert!(s.contains("CreatorTag"));
                assert!(s.contains("SZKura_Ropa"));
                assert!(s.contains(&format!("guid: {}",name.split('/').next().unwrap())));
            } else {
                assert_eq!(new.get(&name).unwrap(),&bytes);
            }
        }
    }
    #[test]
    fn refuses_to_overwrite_package_or_leave_partials() {
        let dir=tempfile::tempdir().unwrap();
        let src=dir.path().join("input.unitypackage");
        fixture(&src,false);
        assert!(export_labeled_unitypackage(&src,&src,"Ropa").is_err());
        let dst=dir.path().join("broken.unitypackage");
        std::fs::write(&dst,b"existing").unwrap();
        assert!(export_labeled_unitypackage(&src,&dst,"Ropa").is_err());
        assert_eq!(std::fs::read(dst).unwrap(),b"existing");
    }
    #[test]
    fn corrupt_package_leaves_no_destination() {
        let dir=tempfile::tempdir().unwrap();
        let src=dir.path().join("bad.unitypackage");
        let dst=dir.path().join("output.unitypackage");
        std::fs::write(&src,b"not a gzip").unwrap();
        assert!(export_labeled_unitypackage(&src,&dst,"Ropa").is_err());
        assert!(!dst.exists());
    }
}
