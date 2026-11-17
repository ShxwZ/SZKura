//! Browse ZIP contents without unpacking them, and extract only a selected
//! .unitypackage into a bounded, separate cache upon explicit user action.
//! No archive paths are ever used to build an output path.
use anyhow::{bail, Context, Result};
use std::{fs::{self, File, OpenOptions}, io::{self, BufReader, Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}, time::UNIX_EPOCH};
use crate::db::Catalog;
use zip::ZipArchive;
use sevenz_rust2::{ArchiveReader as SevenZReader, Password as SevenZPassword,
    EntryDecision as SevenZDecision, Error as SevenZError};
use rars::{ArchiveReader as RarReader, ArchiveReadOptions as RarOptions,
    ExtractionDecision as RarDecision};

pub const MAX_ZIP_ENTRIES: usize = 75_000;
pub const MAX_PREVIEW_PACKAGES: usize = 128;
pub const MAX_PACKAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_CACHE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
// Never expand arbitrary nested archives without bounded disk and CPU work.
pub const MAX_NESTED_DEPTH: usize = 2;
pub const MAX_NESTED_ARCHIVES: usize = 32;
pub const MAX_NESTED_FILE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_NESTED_SCAN_BYTES: u64 = 3 * 1024 * 1024 * 1024;
const MAX_COMPRESSION_RATIO: u64 = 200;
/// Bounded list for each opened ZIP, including plain files, not just Unity packages.
pub const MAX_PREVIEW_FILES_PER_ARCHIVE: usize = 512;
pub const MAX_VISIBLE_NESTED_ARCHIVES: usize = 512;
const INSPECTOR_CACHE_VERSION: i64 = 3;
const MAX_SEVENZ_MEMORY_KB:usize=128*1024;
const MAX_SEVENZ_READ_BYTES:u64=4*1024*1024*1024;
const MAX_SERIALIZED_PREVIEW_BYTES: usize = 8*1024*1024;
const MAX_RAR_WORKSPACE:u64=256*1024*1024;
const MAX_RAR_DICTIONARY:u64=128*1024*1024;
const MAX_RAR_DECODE:u64=4*1024*1024*1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NestedZip {
    pub index: usize,
    pub name: String,
    pub size: u64,
    pub crc32: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UnityPackage {
    /// Index in the innermost ZIP (or in the root archive if chain is empty).
    pub index: usize,
    pub name: String,
    pub size: u64,
    pub crc32: u32,
    pub zip_chain: Vec<NestedZip>,
}
impl UnityPackage {
    pub fn display_path(&self)->String {
        let mut labels:Vec<&str>=self.zip_chain.iter().map(|zip|zip.name.as_str()).collect();
        labels.push(&self.name);
        labels.join(" → ")
    }
}

/// A file inside one ZIP level. Unity packages retain their extraction metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreviewFile {
    pub name: String,
    pub size: u64,
    pub package: Option<UnityPackage>,
}

/// One inner ZIP, even when unreadable or skipped. The chain gives it a unique
/// path and lets the UI display a real expandable hierarchy.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NestedArchivePreview {
    pub chain: Vec<NestedZip>,
    pub total_entries: usize,
    pub files: Vec<PreviewFile>,
    pub hidden_files: usize,
    pub skipped_reason: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZipPreview {
    /// Number of entries in the top-level ZIP.
    pub total_entries: usize,
    /// Number of additional entries found inside nested ZIPs.
    pub nested_entries: usize,
    pub nested_archives: usize,
    pub nested_scanned: usize,
    pub nested_skipped: usize,
    pub total_packages: usize,
    pub packages: Vec<UnityPackage>,
    pub sample_other_files: Vec<String>,
    pub root_files: Vec<PreviewFile>,
    pub root_hidden_files: usize,
    pub inner_archives: Vec<NestedArchivePreview>,
    pub hidden_archives: usize,
}

fn is_unitypackage(name:&str)->bool {
    name.rsplit('.').next().is_some_and(|extension|extension.eq_ignore_ascii_case("unitypackage"))
}
fn is_nested_zip(name:&str)->bool {
    name.rsplit('.').next().is_some_and(|extension|extension.eq_ignore_ascii_case("zip"))
}
fn safe_regular_file(file:&zip::read::ZipFile<'_>)->bool{
    !file.is_dir() && file.enclosed_name().is_some() &&
    !file.unix_mode().is_some_and(|mode|mode & 0o170000==0o120000)
}

/// Never extract ZIP entry paths into the filesystem. Only temporary,
/// anonymous files are used to read a nested ZIP central directory.
#[derive(Default)]
struct ScanBudget {
    bytes: u64,
    archives: usize,
}

/// Keep only a bounded snapshot of each archive's filenames for the tree.
fn record_file(preview:&mut ZipPreview, target:Option<usize>, file:PreviewFile) {
    if let Some(index)=target {
        if let Some(node)=preview.inner_archives.get_mut(index) {
            if node.files.len()<MAX_PREVIEW_FILES_PER_ARCHIVE {
                node.files.push(file);
            }else{
                node.hidden_files+=1;
            }
        }
    } else if preview.root_files.len()<MAX_PREVIEW_FILES_PER_ARCHIVE {
        preview.root_files.push(file);
    } else {
        preview.root_hidden_files+=1;
    }
}

fn scan_archive(
    archive:&mut ZipArchive<File>, preview:&mut ZipPreview,chain:&[NestedZip],
    depth:usize,budget:&mut ScanBudget,target:Option<usize>,
)->Result<()> {
    if archive.len()>MAX_ZIP_ENTRIES {
        bail!("ZIP con demasiadas entradas para examinar");
    }
    if depth>0 {preview.nested_entries+=archive.len();}
    if let Some(index)=target {
        preview.inner_archives[index].total_entries=archive.len();
    }
    let mut nested=Vec::new();
    for index in 0..archive.len() {
        let entry=archive.by_index(index)?;
        if !safe_regular_file(&entry){continue;}
        let name=entry.name().to_owned();
        if is_unitypackage(&name) {
            preview.total_packages+=1;
            let package=UnityPackage {
                index,name:name.clone(),size:entry.size(),crc32:entry.crc32(),
                zip_chain:chain.to_vec(),
            };
            if preview.packages.len()<MAX_PREVIEW_PACKAGES {
                preview.packages.push(package.clone());
            }
            record_file(preview,target,PreviewFile{
                name,size:entry.size(),package:Some(package),
            });
        } else if is_nested_zip(&name) {
            preview.nested_archives+=1;
            if depth==0 && preview.sample_other_files.len()<16 {
                preview.sample_other_files.push(name.clone());
            }
            nested.push(NestedZip{
                index,name,size:entry.size(),crc32:entry.crc32(),
            });
        } else {
            if depth==0 && preview.sample_other_files.len()<16 {
                preview.sample_other_files.push(name.clone());
            }
            record_file(preview,target,PreviewFile{
                name,size:entry.size(),package:None,
            });
        }
    }

    for zip in nested {
        // Every displayed ZIP retains its status, including an unreadable ZIP.
        if preview.inner_archives.len()>=MAX_VISIBLE_NESTED_ARCHIVES {
            preview.hidden_archives+=1;
            preview.nested_skipped+=1;
            continue;
        }
        let mut next=chain.to_vec();
        next.push(zip.clone());
        let node_index=preview.inner_archives.len();
        preview.inner_archives.push(NestedArchivePreview{
            chain:next.clone(),total_entries:0,
            files:Vec::new(),hidden_files:0,
            skipped_reason:Some("No se ha podido examinar este ZIP".into()),
        });
        let why=if depth>=MAX_NESTED_DEPTH {
            Some("Límite de profundidad (dos niveles de ZIP internos)".to_string())
        }else if budget.archives>=MAX_NESTED_ARCHIVES {
            Some("Límite de 32 ZIP internos examinados".to_string())
        }else if zip.size>MAX_NESTED_FILE_BYTES {
            Some("ZIP interno de más de 1 GiB".to_string())
        }else if budget.bytes.saturating_add(zip.size)>MAX_NESTED_SCAN_BYTES {
            Some("Presupuesto de exploración de 3 GiB alcanzado".to_string())
        }else{None};
        if let Some(reason)=why {
            preview.inner_archives[node_index].skipped_reason=Some(reason);
            preview.nested_skipped+=1;
            continue;
        }
        let mut file=match archive.by_index(zip.index){
            Ok(entry)=>entry,
            Err(_)=>{
                preview.inner_archives[node_index].skipped_reason=
                    Some("Entrada interna no disponible".into());
                preview.nested_skipped+=1;
                continue;
            }
        };
        if !safe_regular_file(&file)
            || file.name()!=zip.name || file.size()!=zip.size || file.crc32()!=zip.crc32
            || (file.compressed_size()>0
                && file.size()/file.compressed_size()>MAX_COMPRESSION_RATIO)
        {
            preview.inner_archives[node_index].skipped_reason=
                Some("Entrada insegura o compresión sospechosa".into());
            preview.nested_skipped+=1;
            continue;
        }
        budget.archives+=1;
        budget.bytes=budget.bytes.saturating_add(zip.size);
        // Temporary disk space is released after this ZIP has been inspected.
        let mut temporary=match tempfile::tempfile(){
            Ok(file)=>file,
            Err(_)=>{
                preview.inner_archives[node_index].skipped_reason=
                    Some("No hay espacio temporal disponible".into());
                preview.nested_skipped+=1;
                continue;
            }
        };
        let copied=io::copy(&mut (&mut file).take(zip.size.saturating_add(1)),
            &mut temporary);
        drop(file);
        if !matches!(copied,Ok(n) if n==zip.size)
            || temporary.seek(SeekFrom::Start(0)).is_err()
        {
            preview.inner_archives[node_index].skipped_reason=
                Some("No se pudo leer el ZIP interno completo".into());
            preview.nested_skipped+=1;
            continue;
        }
        let Ok(mut inner)=ZipArchive::new(temporary) else {
            preview.inner_archives[node_index].skipped_reason=
                Some("No es un ZIP interno válido o está dañado".into());
            preview.nested_skipped+=1;
            continue;
        };
        if inner.len()>MAX_ZIP_ENTRIES {
            preview.inner_archives[node_index].skipped_reason=
                Some("ZIP interno con más de 75.000 entradas".into());
            preview.nested_skipped+=1;
            continue;
        }
        preview.nested_scanned+=1;
        match scan_archive(&mut inner,preview,&next,depth+1,budget,Some(node_index)) {
            Ok(())=>preview.inner_archives[node_index].skipped_reason=None,
            Err(error)=>{
                preview.inner_archives[node_index].skipped_reason=
                    Some(format!("Inspección parcial: {error}"));
                preview.nested_skipped+=1;
            },
        }
    }
    Ok(())
}

/// .7z listing reads the header without unpacking its payload. The memory
/// ceiling is passed into the decoder, not assumed from OS conditions.
fn sevenzip_reader(path:&Path)->Result<SevenZReader<File>> {
    SevenZReader::new_with_memory_limit(
        File::open(path).with_context(||format!("No se pudo abrir {}",path.display()))?,
        SevenZPassword::empty(),
        MAX_SEVENZ_MEMORY_KB,
    ).context("7z dañado, cifrado o con compresión incompatible")
}
pub fn sevenzip_count(path:&Path)->Result<i64> {
    let reader=sevenzip_reader(path)?;
    let length=reader.archive().files.len();
    if length>MAX_ZIP_ENTRIES {bail!("7z con demasiadas entradas");}
    Ok(i64::try_from(length)?)
}
fn is_sevenzip(path:&Path)->bool {
    path.extension().and_then(|s|s.to_str())
        .is_some_and(|ext|ext.eq_ignore_ascii_case("7z"))
}
fn is_rar(path:&Path)->bool {
    path.extension().and_then(|s|s.to_str())
        .is_some_and(|ext|ext.eq_ignore_ascii_case("rar"))
}
pub fn rar_count(path:&Path)->Result<i64> {
    let archive=rar_reader(path)?;
    let count=archive.members().take(MAX_ZIP_ENTRIES+1).count();
    if count>MAX_ZIP_ENTRIES{bail!("RAR con demasiadas entradas");}
    Ok(i64::try_from(count)?)
}

fn safe_member_name(raw:&str)->bool {
    if raw.is_empty() || raw.starts_with('/') || raw.starts_with('\\') ||
        raw.contains(':') || raw.contains('\0') {return false;}
    let normalized=raw.replace('\\',"/");
    !normalized.split('/').any(|piece|piece==".." || piece==".")
}
fn sevenzip_crc(entry:&sevenz_rust2::ArchiveEntry)->u32 {
    if entry.has_crc{entry.crc as u32}else{0}
}
fn copy_7z_bounded(
    input:&mut dyn Read,output:&mut impl Write,
    expected:u64,limit:u64,crc:Option<u32>,
)->Result<()> {
    if expected>limit {bail!("La entrada 7z supera el límite de tamaño");}
    let mut count=0u64;
    let mut hash=crc32fast::Hasher::new();
    let mut chunk=[0u8;128*1024];
    loop {
        let size=input.read(&mut chunk)?;
        if size==0{break;}
        count=count.saturating_add(size as u64);
        if count>expected || count>limit {bail!("La salida 7z excede el tamaño declarado");}
        hash.update(&chunk[..size]);
        output.write_all(&chunk[..size])?;
    }
    if count!=expected {bail!("7z incompleto: tamaño incorrecto");}
    if crc.is_some_and(|expected|hash.finalize()!=expected) {
        bail!("El CRC del archivo 7z no coincide");
    }
    Ok(())
}
/// Extract exactly one 7z entry, to a caller-owned temporary file or bounded
/// cache destination. The entry index, name, size and available CRC are checked
/// again against the live header (cached previews may be stale).
fn extract_sevenz_entry(
    path:&Path,index:usize,name:&str,size:u64,crc:u32,max:u64,output:&mut impl Write
)->Result<()> {
    let mut reader=sevenzip_reader(path)?;
    if reader.archive().files.len()>MAX_ZIP_ENTRIES {bail!("7z demasiado grande");}
    let entry=reader.archive().files.get(index).context("Entrada 7z inexistente")?;
    if !safe_member_name(&entry.name) || entry.is_directory || entry.is_anti_item
        || entry.name!=name || entry.size!=size || sevenzip_crc(entry)!=crc
        || size>max {
        bail!("La entrada 7z ha cambiado o excede los límites; reexamínala");
    }
    // Decoding preceding entries in solid blocks may be required. Refuse huge
    // declared prefixes instead of decoding an unbounded number of bytes.
    let prefix:u64=reader.archive().files.iter().take(index+1)
        .try_fold(0u64,|sum,item|sum.checked_add(item.size))
        .context("Tamaños 7z inválidos")?;
    if prefix>MAX_SEVENZ_READ_BYTES {
        bail!("7z sólido: más de 4 GiB antes de la entrada solicitada");
    }
    let mut visited=0usize;
    let mut found=false;
    reader.for_each_entries_with_decision(|entry,stream|{
        let current=visited;visited+=1;
        if current!=index {return Ok(SevenZDecision::Continue);}
        let expected_crc=if entry.has_crc{Some(entry.crc as u32)}else{None};
        copy_7z_bounded(stream,output,size,max,expected_crc)
            .map_err(|e|SevenZError::Other(e.to_string().into()))?;
        found=true;
        Ok(SevenZDecision::Stop)
    }).context("No se pudo descomprimir la entrada 7z")?;
    if !found {bail!("No se encontró la entrada 7z seleccionada");}
    Ok(())
}

/// RAR4/RAR5 decoder: pure Rust, read-only, with explicit parsing, dictionary,
/// workspace, decompression and output ceilings. No shell/WinRAR is used.
fn rar_options()->RarOptions<'static> {
    RarOptions::new()
        .with_max_header_count((MAX_ZIP_ENTRIES*3) as u64)
        .with_max_header_bytes(64*1024*1024)
        .with_max_member_output_bytes(MAX_PACKAGE_BYTES)
        .with_max_total_output_bytes(MAX_RAR_DECODE)
        .with_max_reader_workspace_bytes(MAX_RAR_WORKSPACE)
        .with_rar50_dictionary_size_limit(MAX_RAR_DICTIONARY)
        .with_rar50_buffered_decode_limit(MAX_RAR_WORKSPACE)
}
fn rar_reader(path:&Path)->Result<rars::Archive> {
    RarReader::read_path_with_options(path,rar_options())
        .context("RAR no válido, protegido, multipartido o no compatible")
}
fn rar_ordinary_member(member:&rars::ArchiveMember)->bool {
    !member.meta.is_directory && !member.meta.is_redirection
        // UNIX symlink mode (0120000) is rejected even on old RAR4 headers.
        && !(member.meta.host_os==Some(3)
            && member.meta.file_attr & 0o170000 == 0o120000)
        && !member.meta.is_split_before && !member.meta.is_split_after
        && !member.meta.is_encrypted
        && safe_member_name(&member.meta.name_lossy())
}
/// Decode one selected entry into an already-created temporary file. When the
/// RAR is solid, required earlier entries are verified and discarded, subject
/// to a strict decoded-prefix budget. Never use archive paths for destinations.
fn extract_rar_entry(
    path:&Path,index:usize,name:&str,size:u64,limit:u64,output:&mut File
)->Result<()> {
    if size>limit{bail!("La entrada RAR supera el tamaño admitido");}
    let archive=rar_reader(path)?;
    let members:Vec<_>=archive.members().take(MAX_ZIP_ENTRIES+1).collect();
    if members.len()>MAX_ZIP_ENTRIES{bail!("RAR con demasiadas entradas");}
    let selected=members.get(index).context("Entrada RAR inexistente")?;
    if !rar_ordinary_member(selected) || selected.meta.name_lossy()!=name
        || selected.meta.unpacked_size!=size {
        bail!("El RAR ha cambiado o contiene una entrada protegida; reexamínalo");
    }
    let solid=archive.is_solid();
    if solid {
        let decoded=members.iter().take(index+1).try_fold(0u64,|bytes,member|{
            bytes.checked_add(member.meta.unpacked_size)
        }).context("RAR sólido con tamaños inválidos")?;
        if decoded>MAX_RAR_DECODE{bail!("RAR sólido: más de 4 GiB hasta el archivo seleccionado");}
    }
    let mut stream=Some(output.try_clone()?);
    let mut cursor=0usize;
    archive.extract_with_control(rar_options(),|member|{
        let current=cursor;cursor+=1;
        if current>index{return Ok(RarDecision::Stop);}
        if current==index{
            return Ok(RarDecision::Extract(Box::new(
                stream.take().expect("RAR seleccionado una sola vez"))));
        }
        if solid && !member.meta.is_directory && !member.meta.is_redirection {
            Ok(RarDecision::Extract(Box::new(io::sink())))
        }else{
            Ok(RarDecision::Skip)
        }
    }).context("No se pudo descomprimir la entrada RAR")?;
    output.flush()?;
    if output.metadata()?.len()!=size{
        bail!("RAR incompleto: el tamaño extraído no coincide");
    }
    Ok(())
}

/// List RAR members without extracting them. Nested ZIP files may be inspected
/// within the same bounded resource policy used by ZIP and 7z.
fn inspect_rar(path:&Path)->Result<ZipPreview>{
    let archive=rar_reader(path)?;
    let entries:Vec<_>=archive.members().take(MAX_ZIP_ENTRIES+1).collect();
    if entries.len()>MAX_ZIP_ENTRIES{bail!("RAR con demasiadas entradas");}
    let mut preview=ZipPreview{
        total_entries:entries.len(),nested_entries:0,nested_archives:0,
        nested_scanned:0,nested_skipped:0,total_packages:0,
        packages:Vec::new(),sample_other_files:Vec::new(),
        root_files:Vec::new(),root_hidden_files:0,
        inner_archives:Vec::new(),hidden_archives:0,
    };
    let mut nested=Vec::<(usize,NestedZip)>::new();
    let mut budget=ScanBudget::default();
    for (index,entry) in entries.iter().enumerate() {
        let name=entry.meta.name_lossy().to_string();
        if !rar_ordinary_member(entry){continue;}
        let size=entry.meta.unpacked_size;
        if is_unitypackage(&name) {
            preview.total_packages+=1;
            let pkg=UnityPackage{index,name:name.clone(),size,crc32:0,
                zip_chain:Vec::new()};
            if preview.packages.len()<MAX_PREVIEW_PACKAGES {
                preview.packages.push(pkg.clone());
            }
            record_file(&mut preview,None,PreviewFile{name,size,package:Some(pkg)});
        }else if is_nested_zip(&name){
            preview.nested_archives+=1;
            let entry_info=NestedZip{index,name:name.clone(),size,crc32:0};
            if preview.inner_archives.len()>=MAX_VISIBLE_NESTED_ARCHIVES{
                preview.hidden_archives+=1;
                preview.nested_skipped+=1;
                continue;
            }
            let reason=if nested.len()>=MAX_NESTED_ARCHIVES{
                Some("Límite de ZIP internos examinados".to_string())
            }else if size>MAX_NESTED_FILE_BYTES{
                Some("ZIP interno de más de 1 GiB".to_string())
            }else if entry.meta.packed_size>0
                && size/entry.meta.packed_size>MAX_COMPRESSION_RATIO {
                Some("Relación de compresión sospechosa".to_string())
            }else if budget.bytes.saturating_add(size)>MAX_NESTED_SCAN_BYTES{
                Some("Límite de 3 GiB de ZIP internos alcanzado".to_string())
            }else{None};
            let node=preview.inner_archives.len();
            preview.inner_archives.push(NestedArchivePreview{
                chain:vec![entry_info.clone()],total_entries:0,
                files:Vec::new(),hidden_files:0,skipped_reason:reason.clone(),
            });
            if reason.is_some(){preview.nested_skipped+=1;}
            else{
                budget.bytes=budget.bytes.saturating_add(size);
                budget.archives+=1;
                nested.push((node,entry_info));
            }
        }else{
            if preview.sample_other_files.len()<16 {
                preview.sample_other_files.push(name.clone());
            }
            record_file(&mut preview,None,PreviewFile{name,size,package:None});
        }
    }
    for (node,zip) in nested {
        let inspected=(||->Result<()>{
            let mut stage=tempfile::tempfile()?;
            extract_rar_entry(path,zip.index,&zip.name,zip.size,
                MAX_NESTED_FILE_BYTES,&mut stage)?;
            stage.seek(SeekFrom::Start(0))?;
            let mut inner=ZipArchive::new(stage).context("RAR contiene un ZIP inválido")?;
            preview.nested_scanned+=1;
            scan_archive(&mut inner,&mut preview,&[zip],1,&mut budget,Some(node))?;
            Ok(())
        })();
        if let Err(err)=inspected {
            preview.inner_archives[node].skipped_reason=Some(format!("{err:#}"));
            preview.nested_skipped+=1;
        }else{
            preview.inner_archives[node].skipped_reason=None;
        }
    }
    Ok(preview)
}

/// Browse 7z members using header metadata; inspect ZIPs nested in 7z with a
/// single bounded decode pass. .7z inside .7z is listed as an ordinary file.
fn inspect_sevenzip(path:&Path)->Result<ZipPreview>{
    let mut reader=sevenzip_reader(path)?;
    let files=reader.archive().files.clone();
    if files.len()>MAX_ZIP_ENTRIES{bail!("7z con demasiadas entradas");}
    let mut preview=ZipPreview{
        total_entries:files.len(),nested_entries:0,nested_archives:0,
        nested_scanned:0,nested_skipped:0,total_packages:0,
        packages:Vec::new(),sample_other_files:Vec::new(),root_files:Vec::new(),
        root_hidden_files:0,inner_archives:Vec::new(),hidden_archives:0,
    };
    let mut selected=std::collections::HashMap::<usize,(usize,NestedZip)>::new();
    let mut budget=ScanBudget::default();
    let total_unpacked=files.iter().try_fold(0u64,|total,file|{
        total.checked_add(file.size)
    }).context("El 7z declara un tamaño de salida inválido")?;
    for (index,entry) in files.iter().enumerate(){
        if entry.is_directory || entry.is_anti_item || !safe_member_name(&entry.name) {
            continue;
        }
        let name=entry.name.clone();
        if is_unitypackage(&name){
            preview.total_packages+=1;
            let pkg=UnityPackage{index,name:name.clone(),size:entry.size,
                crc32:sevenzip_crc(entry),zip_chain:Vec::new()};
            if preview.packages.len()<MAX_PREVIEW_PACKAGES {
                preview.packages.push(pkg.clone());
            }
            record_file(&mut preview,None,PreviewFile{
                name,size:entry.size,package:Some(pkg),
            });
        }else if is_nested_zip(&name){
            preview.nested_archives+=1;
            let nested=NestedZip{index,name:name.clone(),size:entry.size,
                crc32:sevenzip_crc(entry)};
            if preview.inner_archives.len()>=MAX_VISIBLE_NESTED_ARCHIVES {
                preview.hidden_archives+=1;
                preview.nested_skipped+=1;
                continue;
            }
            let node=preview.inner_archives.len();
            let reason=if selected.len()>=MAX_NESTED_ARCHIVES {
                Some("Límite de 32 ZIP internos examinados".to_string())
            }else if total_unpacked>MAX_NESTED_SCAN_BYTES {
                Some("7z sólido demasiado grande para examinar ZIP internos (3 GiB)".to_string())
            }else if entry.size>MAX_NESTED_FILE_BYTES {
                Some("ZIP interno de más de 1 GiB".to_string())
            }else if budget.bytes.saturating_add(entry.size)>MAX_NESTED_SCAN_BYTES {
                Some("Presupuesto de exploración de 3 GiB alcanzado".to_string())
            }else{None};
            preview.inner_archives.push(NestedArchivePreview{
                chain:vec![nested.clone()],total_entries:0,
                files:Vec::new(),hidden_files:0,
                skipped_reason:Some(reason.clone().unwrap_or_else(||"Pendiente de lectura".into())),
            });
            if reason.is_some(){
                preview.nested_skipped+=1;
            }else{
                budget.bytes=budget.bytes.saturating_add(entry.size);
                budget.archives+=1;
                selected.insert(index,(node,nested));
            }
        }else{
            if preview.sample_other_files.len()<16{
                preview.sample_other_files.push(name.clone());
            }
            record_file(&mut preview,None,PreviewFile{name,size:entry.size,package:None});
        }
    }
    if !selected.is_empty(){
        let mut pos=0usize;
        let mut scan_error=None;
        let result=reader.for_each_entries_with_decision(|entry,stream|{
            let index=pos;pos+=1;
            let Some((node,nested))=selected.get(&index).cloned() else{
                return Ok(SevenZDecision::Continue);
            };
            let scan=(||->Result<()>{
                if entry.name!=nested.name || entry.size!=nested.size ||
                    sevenzip_crc(entry)!=nested.crc32 {
                    bail!("El ZIP interior cambió durante el análisis");
                }
                let mut temp=tempfile::tempfile()?;
                let expected_crc=if entry.has_crc{Some(entry.crc as u32)}else{None};
                copy_7z_bounded(stream,&mut temp,entry.size,
                    MAX_NESTED_FILE_BYTES,expected_crc)?;
                temp.seek(SeekFrom::Start(0))?;
                let mut zip=ZipArchive::new(temp).context("No es un ZIP interno válido")?;
                preview.nested_scanned+=1;
                scan_archive(&mut zip,&mut preview,&[nested],1,
                    &mut budget,Some(node))?;
                Ok(())
            })();
            match scan{
                Ok(())=>preview.inner_archives[node].skipped_reason=None,
                Err(e)=>{
                    preview.inner_archives[node].skipped_reason=Some(format!("{e:#}"));
                    preview.nested_skipped+=1;
                }
            }
            Ok(SevenZDecision::Continue)
        });
        if let Err(error)=result{
            scan_error=Some(error.to_string());
        }
        if let Some(reason)=scan_error {
            for (node,_) in selected.values(){
                if preview.inner_archives[*node].skipped_reason.as_deref()
                    ==Some("Pendiente de lectura"){
                    preview.inner_archives[*node].skipped_reason=
                        Some(format!("Error decodificando 7z: {reason}"));
                    preview.nested_skipped+=1;
                }
            }
        }
    }
    Ok(preview)
}

/// Read central directory metadata from the outer ZIP and recursively inspect
/// bounded nested ZIPs on a worker thread. Never unpack entire assets.
pub fn inspect_zip(path:&Path)->Result<ZipPreview>{
    let mut archive=ZipArchive::new(File::open(path).with_context(||format!(
        "No se puede abrir el ZIP: {}",path.display()))?)
        .context("No es un ZIP válido o está dañado")?;
    if archive.len()>MAX_ZIP_ENTRIES {bail!("ZIP con demasiadas entradas para examinar");}
    let mut preview=ZipPreview{
        total_entries:archive.len(),nested_entries:0,nested_archives:0,
        nested_scanned:0,nested_skipped:0,total_packages:0,
        packages:Vec::new(),sample_other_files:Vec::new(),
        root_files:Vec::new(),root_hidden_files:0,
        inner_archives:Vec::new(),hidden_archives:0,
    };
    scan_archive(&mut archive,&mut preview,&[],0,&mut ScanBudget::default(),None)?;
    Ok(preview)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheUse {
    /// ZIP unchanged since the last hashed, cached scan: no heavy file IO.
    Unchanged,
    /// File size or mtime changed, but a fresh BLAKE3 hash matches a cached ZIP.
    MatchingHash,
    /// The ZIP was scanned and a new preview was persisted.
    Fresh,
    /// The user explicitly bypassed the cache with Reexaminar.
    Forced,
    /// Reading the ZIP worked but writing the optional cache did not.
    NoCache,
}
impl CacheUse {
    pub fn label(self)->&'static str {
        match self {
            Self::Unchanged=>"Vista cargada desde SQLite (archivo sin cambios)",
            Self::MatchingHash=>"Vista reutilizada: hash BLAKE3 idéntico",
            Self::Fresh=>"Archivo examinado y vista guardada en SQLite",
            Self::Forced=>"Archivo reexaminado y caché actualizada",
            Self::NoCache=>"Archivo examinado; caché no disponible para esta vista",
        }
    }
}

fn signature(path:&Path)->Result<(u64,String)> {
    let meta=fs::metadata(path).with_context(||format!(
        "No se pudo consultar el ZIP: {}",path.display()
    ))?;
    if !meta.is_file(){bail!("No es un archivo ZIP regular");}
    let modified=meta.modified().context("No se pudo conocer la fecha del ZIP")?
        .duration_since(UNIX_EPOCH)
        .context("Fecha de modificación del ZIP no admitida")?;
    Ok((meta.len(), modified.as_nanos().to_string()))
}

fn content_hash(path:&Path)->Result<String> {
    // Stream the file in bounded chunks rather than allocating a multi-GB ZIP.
    let mut reader=BufReader::with_capacity(512*1024, File::open(path)?);
    let mut digest=blake3::Hasher::new();
    let mut buf=[0u8; 128*1024];
    loop {
        let count=reader.read(&mut buf)?;
        if count==0{break}
        digest.update(&buf[..count]);
    }
    Ok(digest.finalize().to_hex().to_string())
}
fn valid_cached_json(data:&str)->Option<ZipPreview>{
    if data.len()>MAX_SERIALIZED_PREVIEW_BYTES{return None;}
    serde_json::from_str::<ZipPreview>(data).ok()
}

/// Persistent, non-destructive ZIP metadata cache. On ordinary openings,
/// identical path/length/mtime returns instantly. On a changed file the
/// actual BLAKE3 is recomputed; a matching digest reuses its prior scan.
/// Explicit Reexaminar bypasses both reuse paths.
///
/// The linked attachment checksum is NOT assumed current: users may replace a
/// referenced ZIP after linking it. Never trust it without revalidation.
/// Visit all safe root entry names, including entries hidden by the UI's
/// bounded preview. Returning false stops early, without allocating a list
/// of thousands of filenames. Nested archive inspection remains bounded.
pub fn visit_archive_root_names(path: &Path, mut visit: impl FnMut(&str) -> bool) -> Result<()> {
    if is_sevenzip(path) {
        let reader = sevenzip_reader(path)?;
        let entries = &reader.archive().files;
        if entries.len() > MAX_ZIP_ENTRIES { bail!("7z con demasiadas entradas"); }
        for entry in entries {
            if !entry.is_directory && !entry.is_anti_item
                && safe_member_name(&entry.name) && !visit(&entry.name) {
                break;
            }
        }
    } else if is_rar(path) {
        let archive = rar_reader(path)?;
        // Enforce the cap without storing the full member list again.
        for (index, entry) in archive.members().enumerate() {
            if index >= MAX_ZIP_ENTRIES { bail!("RAR con demasiadas entradas"); }
            if rar_ordinary_member(&entry) {
                let name = entry.meta.name_lossy();
                if !visit(&name) { break; }
            }
        }
    } else {
        let mut archive = ZipArchive::new(File::open(path)?)?;
        if archive.len() > MAX_ZIP_ENTRIES { bail!("ZIP con demasiadas entradas"); }
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            if safe_regular_file(&entry) && !visit(entry.name()) { break; }
        }
    }
    Ok(())
}

pub fn inspect_any_archive(path:&Path)->Result<ZipPreview> {
    if is_sevenzip(path){inspect_sevenzip(path)}
    else if is_rar(path){inspect_rar(path)}
    else{inspect_zip(path)}
}

pub fn inspect_zip_cached(
    path:&Path,db_path:&Path,force:bool
)->Result<(ZipPreview,CacheUse)>{
    let initial=signature(path)?;
    let mut catalog=match Catalog::open(db_path){
        Ok(db)=>db,
        Err(_)=>return Ok((inspect_any_archive(path)?,CacheUse::NoCache)),
    };
    let key=path.to_string_lossy();
    if !force {
        if let Ok(Some(data))=catalog.cached_zip_preview_by_file(
            &key,initial.0,&initial.1,INSPECTOR_CACHE_VERSION
        ) {
            if let Some(preview)=valid_cached_json(&data){
                // Recheck after retrieval so a concurrent rewrite isn't reused.
                if signature(path)?==initial {
                    return Ok((preview,CacheUse::Unchanged));
                }
                bail!("El ZIP cambió mientras se consultaba la caché; vuelve a examinarlo");
            }
        }
    }

    let hash=content_hash(path)?;
    if signature(path)?!=initial {
        bail!("El ZIP cambió mientras se calculaba su hash; vuelve a examinarlo");
    }
    if !force {
        if let Ok(Some(data))=catalog.cached_zip_preview_by_hash(
            &hash,INSPECTOR_CACHE_VERSION
        ) {
            if let Some(preview)=valid_cached_json(&data) {
                if catalog.store_zip_preview(
                    &key,initial.0,&initial.1,&hash,INSPECTOR_CACHE_VERSION,&data
                ).is_ok() {
                    return Ok((preview,CacheUse::MatchingHash));
                }
                return Ok((preview,CacheUse::NoCache));
            }
        }
    }
    let preview=inspect_any_archive(path)?;
    if signature(path)?!=initial {
        bail!("El ZIP cambió durante el análisis; vuelve a examinarlo");
    }
    let Ok(snapshot)=serde_json::to_string(&preview) else {
        return Ok((preview,CacheUse::NoCache));
    };
    if snapshot.len()>MAX_SERIALIZED_PREVIEW_BYTES
        || catalog.store_zip_preview(
            &key,initial.0,&initial.1,&hash,INSPECTOR_CACHE_VERSION,&snapshot
        ).is_err()
    {
        return Ok((preview,CacheUse::NoCache));
    }
    Ok((preview,if force{CacheUse::Forced}else{CacheUse::Fresh}))
}

/// Selective extraction: the ZIP is read, but only one entry is decompressed.
/// The destination is a hash-based filename in app cache, never an untrusted ZIP path.
/// Existing cached files are reused when their length matches.
pub fn extract_unitypackage(
    zip_path:&Path,index:usize,expected_name:&str,expected_size:u64,cache:&Path
)->Result<PathBuf>{
    let mut archive=ZipArchive::new(File::open(zip_path)?)?;
    if archive.len()>MAX_ZIP_ENTRIES {bail!("ZIP con demasiadas entradas");}
    if index>=archive.len(){bail!("El paquete ya no está en el ZIP");}
    let mut entry=archive.by_index(index).context("Entrada ZIP no disponible")?;
    if !safe_regular_file(&entry) || !is_unitypackage(entry.name())
        || entry.name()!=expected_name || entry.size()!=expected_size {
        bail!("El contenido del ZIP ha cambiado; examínalo de nuevo");
    }
    if entry.size()>MAX_PACKAGE_BYTES {
        bail!("El unitypackage supera el límite de extracción de 2 GiB");
    }
    let source_meta=fs::metadata(zip_path)?;
    let mut hasher=blake3::Hasher::new();
    hasher.update(zip_path.to_string_lossy().as_bytes());
    hasher.update(&source_meta.len().to_le_bytes());
    if let Ok(modified)=source_meta.modified().and_then(|v|v.duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)) {
        hasher.update(&modified.as_nanos().to_le_bytes());
    }
    hasher.update(&(index as u64).to_le_bytes());
    hasher.update(expected_name.as_bytes());
    hasher.update(&entry.crc32().to_le_bytes());
    let hash=hasher.finalize().to_hex();
    let out=cache.join(format!("pkg-{}.unitypackage",&hash[..24]));
    if out.is_file() && fs::metadata(&out)?.len()==expected_size{
        return Ok(out);
    }
    fs::create_dir_all(cache)?;
    let used=fs::read_dir(cache)?.filter_map(|entry|entry.ok())
        .filter_map(|entry|entry.metadata().ok())
        .filter(|meta|meta.is_file())
        .map(|meta|meta.len()).sum::<u64>();
    if used.saturating_add(expected_size)>MAX_CACHE_BYTES {
        bail!("La caché Unity está llena (máximo 4 GiB). Vacíala desde la ficha");
    }

    let partial=cache.join(format!("pkg-{}.partial",&hash[..24]));
    // Create new rather than following an existing partial path.
    let _=fs::remove_file(&partial);
    let mut output=OpenOptions::new().write(true).create_new(true).open(&partial)?;
    let result=(||->Result<()>{
        let mut limited=(&mut entry).take(MAX_PACKAGE_BYTES+1);
        let copied=io::copy(&mut limited,&mut output)?;
        output.flush()?;
        if copied!=expected_size{bail!("La longitud del unitypackage no coincide");}
        Ok(())
    })();
    drop(output);
    if let Err(error)=result{let _=fs::remove_file(&partial);return Err(error);}
    if let Err(error)=fs::rename(&partial,&out){
        let _=fs::remove_file(&partial);
        return Err(error.into());
    }
    Ok(out)
}

/// Selective 7z extraction: only the requested entry is materialized into
/// the dedicated Unity cache; no untrusted member name becomes a filesystem path.
fn extract_sevenz_unitypackage(root:&Path,package:&UnityPackage,cache:&Path)
    ->Result<PathBuf> {
    if package.size>MAX_PACKAGE_BYTES {
        bail!("El unitypackage excede el límite de 2 GiB");
    }
    let meta=fs::metadata(root)?;
    let mut hasher=blake3::Hasher::new();
    hasher.update(root.to_string_lossy().as_bytes());
    hasher.update(&meta.len().to_le_bytes());
    if let Ok(time)=meta.modified().and_then(|v|v.duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)) {
        hasher.update(&time.as_nanos().to_le_bytes());
    }
    for hop in &package.zip_chain {
        hasher.update(&(hop.index as u64).to_le_bytes());
        hasher.update(hop.name.as_bytes());
        hasher.update(&hop.size.to_le_bytes());
        hasher.update(&hop.crc32.to_le_bytes());
    }
    hasher.update(&(package.index as u64).to_le_bytes());
    hasher.update(package.name.as_bytes());
    hasher.update(&package.size.to_le_bytes());
    hasher.update(&package.crc32.to_le_bytes());
    let hash=hasher.finalize().to_hex();
    let out=cache.join(format!("pkg-{}.unitypackage",&hash[..24]));
    if out.is_file() && fs::metadata(&out)?.len()==package.size{return Ok(out);}
    fs::create_dir_all(cache)?;
    let used=fs::read_dir(cache)?.filter_map(|e|e.ok())
        .filter_map(|e|e.metadata().ok())
        .filter(|meta|meta.is_file()).map(|meta|meta.len()).sum::<u64>();
    if used.saturating_add(package.size)>MAX_CACHE_BYTES {
        bail!("La caché Unity supera 4 GiB; vacíala antes de extraer");
    }
    if package.zip_chain.is_empty(){
        if !is_unitypackage(&package.name){bail!("El archivo no es un unitypackage");}
        let mut stage=tempfile::NamedTempFile::new_in(cache)?;
        extract_sevenz_entry(root,package.index,&package.name,
            package.size,package.crc32,MAX_PACKAGE_BYTES,stage.as_file_mut())?;
        stage.as_file_mut().sync_all()?;
        match stage.persist_noclobber(&out){
            Ok(_)=>{},
            Err(e)=>{
                if !out.is_file() || fs::metadata(&out)?.len()!=package.size {
                    return Err(e.error.into());
                }
            }
        }
        return Ok(out);
    }
    if package.zip_chain.len()>MAX_NESTED_DEPTH {
        bail!("Demasiados ZIP internos");
    }
    let stage=tempfile::tempdir()?;
    let first=&package.zip_chain[0];
    if !is_nested_zip(&first.name){bail!("No es un ZIP interior admitido");}
    let temp_zip=stage.path().join("inner.zip");
    {
        let mut target=File::create(&temp_zip)?;
        extract_sevenz_entry(root,first.index,&first.name,first.size,
            first.crc32,MAX_NESTED_FILE_BYTES,&mut target)?;
        target.sync_all()?;
    }
    let mut inner_pkg=package.clone();
    inner_pkg.zip_chain.remove(0);
    let temporary_result=extract_unitypackage_nested(&temp_zip,&inner_pkg,cache)?;
    if out.is_file() && fs::metadata(&out)?.len()==package.size {
        if temporary_result!=out {let _=fs::remove_file(temporary_result);}
        return Ok(out);
    }
    if temporary_result!=out {
        fs::rename(&temporary_result,&out)
            .context("No se pudo guardar el unitypackage extraído del 7z")?;
    }
    Ok(out)
}

/// Extract only the selected Unity package from a RAR or from a ZIP nested
/// in the RAR. All output names are generated from validated metadata, never
/// archive paths, and staging is removed automatically on errors.
fn extract_rar_unitypackage(root:&Path,package:&UnityPackage,cache:&Path)
    ->Result<PathBuf>{
    if package.size>MAX_PACKAGE_BYTES || !is_unitypackage(&package.name){
        bail!("El paquete RAR no es un UnityPackage válido o supera 2 GiB");
    }
    if package.zip_chain.len()>MAX_NESTED_DEPTH{
        bail!("RAR con demasiados ZIP internos");
    }
    let meta=fs::metadata(root)?;
    let mut hasher=blake3::Hasher::new();
    hasher.update(root.to_string_lossy().as_bytes());
    hasher.update(&meta.len().to_le_bytes());
    if let Ok(time)=meta.modified().and_then(|v|v.duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)){
        hasher.update(&time.as_nanos().to_le_bytes());
    }
    for hop in &package.zip_chain{
        hasher.update(&(hop.index as u64).to_le_bytes());
        hasher.update(hop.name.as_bytes());
        hasher.update(&hop.size.to_le_bytes());
    }
    hasher.update(&(package.index as u64).to_le_bytes());
    hasher.update(package.name.as_bytes());
    hasher.update(&package.size.to_le_bytes());
    let key=hasher.finalize().to_hex();
    let out=cache.join(format!("pkg-{}.unitypackage",&key[..24]));
    if out.is_file() && fs::metadata(&out)?.len()==package.size{
        return Ok(out);
    }
    fs::create_dir_all(cache)?;
    let used=fs::read_dir(cache)?.filter_map(|file|file.ok())
        .filter_map(|file|file.metadata().ok())
        .filter(|meta|meta.is_file()).map(|meta|meta.len()).sum::<u64>();
    if used.saturating_add(package.size)>MAX_CACHE_BYTES{
        bail!("La caché Unity supera 4 GiB. Vacíala antes de extraer");
    }
    if package.zip_chain.is_empty(){
        let mut stage=tempfile::NamedTempFile::new_in(cache)?;
        extract_rar_entry(root,package.index,&package.name,package.size,
            MAX_PACKAGE_BYTES,stage.as_file_mut())?;
        stage.as_file_mut().sync_all()?;
        if let Err(err)=stage.persist_noclobber(&out){
            if !out.is_file() || fs::metadata(&out)?.len()!=package.size{
                return Err(err.error.into());
            }
        }
        return Ok(out);
    }
    // Nested archives are ZIPs only, respecting the existing depth & disk caps.
    let stage=tempfile::tempdir()?;
    let first=&package.zip_chain[0];
    if !is_nested_zip(&first.name) || first.size>MAX_NESTED_FILE_BYTES{
        bail!("ZIP interno RAR no admitido");
    }
    let temp_zip=stage.path().join("inner.zip");
    let mut output=OpenOptions::new().write(true).create_new(true).open(&temp_zip)?;
    extract_rar_entry(root,first.index,&first.name,first.size,
        MAX_NESTED_FILE_BYTES,&mut output)?;
    output.sync_all()?;
    drop(output);
    let mut next=package.clone();
    next.zip_chain.remove(0);
    let extracted=extract_unitypackage_nested(&temp_zip,&next,cache)?;
    if extracted!=out{
        if out.is_file(){
            if fs::metadata(&out)?.len()==package.size {
                let _=fs::remove_file(extracted);
                return Ok(out);
            }
            bail!("La caché RAR contiene un archivo incompatible");
        }
        fs::rename(&extracted,&out).context("No se pudo registrar el paquete RAR extraído")?;
    }
    Ok(out)
}

/// Open a selected .unitypackage from ZIPs nested up to two levels deep.
/// Each level is validated by index, name, uncompressed size, and CRC, then
/// materialized only to an auto-cleaned temporary directory. The package itself
/// is extracted only on this explicit user action.
pub fn extract_unitypackage_nested(root:&Path, package:&UnityPackage, cache:&Path)->Result<PathBuf>{
    if is_sevenzip(root){
        return extract_sevenz_unitypackage(root,package,cache);
    }
    if is_rar(root){
        return extract_rar_unitypackage(root,package,cache);
    }
    // Even if the path, index and size match, do not trust a stale preview.
    // Revalidate the CRC in addition to the existing name/size checks.
    if package.zip_chain.is_empty() {
        let mut archive=ZipArchive::new(File::open(root)?)?;
        let entry=archive.by_index(package.index)?;
        if !safe_regular_file(&entry)
            || entry.name()!=package.name || entry.size()!=package.size
            || entry.crc32()!=package.crc32 {
            bail!("El paquete ha cambiado; examina el ZIP de nuevo");
        }
        return extract_unitypackage(root,package.index,&package.name,package.size,cache);
    }
    if package.zip_chain.len()>MAX_NESTED_DEPTH {
        bail!("Demasiados niveles de ZIP anidados");
    }
    let root_meta=fs::metadata(root)?;
    let mut hash=blake3::Hasher::new();
    hash.update(root.to_string_lossy().as_bytes());
    hash.update(&root_meta.len().to_le_bytes());
    if let Ok(time)=root_meta.modified().and_then(|v|v.duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)){
        hash.update(&time.as_nanos().to_le_bytes());
    }
    for hop in &package.zip_chain {
        hash.update(&(hop.index as u64).to_le_bytes());
        hash.update(hop.name.as_bytes());
        hash.update(&hop.size.to_le_bytes());
        hash.update(&hop.crc32.to_le_bytes());
    }
    hash.update(&(package.index as u64).to_le_bytes());
    hash.update(package.name.as_bytes());
    hash.update(&package.size.to_le_bytes());
    hash.update(&package.crc32.to_le_bytes());
    let digest=hash.finalize().to_hex();
    let out=cache.join(format!("pkg-{}.unitypackage",&digest[..24]));
    if out.is_file() && fs::metadata(&out)?.len()==package.size {
        return Ok(out);
    }

    // The stage directory is removed on all exit paths, including errors.
    let stage=tempfile::tempdir().context("No se pudo crear la carpeta ZIP temporal")?;
    let mut current=root.to_path_buf();
    let mut budget=0_u64;
    for (depth,hop) in package.zip_chain.iter().enumerate() {
        if hop.size>MAX_NESTED_FILE_BYTES
            || budget.saturating_add(hop.size)>MAX_NESTED_SCAN_BYTES
        {bail!("ZIP anidado demasiado grande para extraer con seguridad");}
        let mut archive=ZipArchive::new(File::open(&current)?)?;
        if archive.len()>MAX_ZIP_ENTRIES || hop.index>=archive.len() {
            bail!("El ZIP anidado ha cambiado; examínalo de nuevo");
        }
        let mut entry=archive.by_index(hop.index)?;
        if !safe_regular_file(&entry) || !is_nested_zip(entry.name())
            || entry.name()!=hop.name || entry.size()!=hop.size
            || entry.crc32()!=hop.crc32
        {bail!("El ZIP interno ha cambiado; examínalo de nuevo");}
        let next=stage.path().join(format!("nivel-{depth}.zip"));
        let mut destination=OpenOptions::new().create_new(true).write(true).open(&next)?;
        let copied=io::copy(&mut (&mut entry).take(hop.size.saturating_add(1)),
            &mut destination)?;
        if copied!=hop.size {bail!("El ZIP interno está incompleto");}
        destination.flush()?;
        budget=budget.saturating_add(copied);
        current=next;
    }
    // Validate the final index and CRC before selective extraction.
    {
        let mut archive=ZipArchive::new(File::open(&current)?)?;
        let entry=archive.by_index(package.index)?;
        if !safe_regular_file(&entry) || !is_unitypackage(entry.name())
            || entry.name()!=package.name || entry.size()!=package.size
            || entry.crc32()!=package.crc32 {
            bail!("El paquete interno ha cambiado; examina el ZIP de nuevo");
        }
    }
    // Existing selective extractor enforces 2 GiB per file / 4 GiB cache.
    let extracted=extract_unitypackage(
        &current,package.index,&package.name,package.size,cache
    )?;
    if extracted!=out {
        if out.exists() {
            if fs::metadata(&out)?.len()==package.size {
                let _=fs::remove_file(&extracted);
                return Ok(out);
            }
            bail!("La caché de destino contiene un archivo incompatible");
        }
        fs::rename(&extracted,&out).context("No se pudo registrar el paquete extraído")?;
    }
    Ok(out)
}

/// Deletes only this application's dedicated Unity extraction cache.
/// Referenced ZIPs and user library files are never touched.
pub fn clear_cache(cache:&Path)->Result<usize>{
    if !cache.exists(){return Ok(0);}
    if !cache.is_dir(){bail!("Ruta de caché inesperada");}
    let mut removed=0;
    for item in fs::read_dir(cache)?{
        let item=item?;
        let path=item.path();
        if !path.is_file(){continue;}
        let name=item.file_name().to_string_lossy().into_owned();
        if name.starts_with("pkg-") &&
           (name.ends_with(".unitypackage")||name.ends_with(".partial")) {
            fs::remove_file(path)?;
            removed+=1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{ZipWriter, write::FileOptions};

    // Real small RAR4/RAR5 stored fixtures from bitplane/rars (Apache-2.0).
    // They test the actual extraction path, not only extension detection.
    #[test]
    fn opens_real_rar4_and_rar5_and_extracts_selected_file() {
        let dir=tempfile::tempdir().unwrap();
        let cases:[(&str,&[u8],&str,&[u8]);2]=[
            ("rar4.rar",&[0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0x00, 0xcf, 0x90, 0x73, 0x00, 0x00, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xbc, 0xe4, 0x74, 0x00, 0x80, 0x2b, 0x00, 0x15, 0x00, 0x00, 0x00, 0x15, 0x00, 0x00, 0x00, 0x03, 0xb6, 0xa8, 0x45, 0xc9, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x30, 0x0b, 0x00, 0x20, 0x00, 0x00, 0x00, 0x70, 0x61, 0x79, 0x6c, 0x6f, 0x61, 0x64, 0x2e, 0x74, 0x78, 0x74, 0x72, 0x61, 0x72, 0x31, 0x35, 0x20, 0x6f, 0x72, 0x61, 0x63, 0x6c, 0x65, 0x20, 0x70, 0x61, 0x79, 0x6c, 0x6f, 0x61, 0x64, 0x0a],"payload.txt",b"rar15 oracle payload\n"),
            ("rar5.rar",&[0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0x01, 0x00, 0x33, 0x92, 0xb5, 0xe5, 0x0a, 0x01, 0x05, 0x06, 0x00, 0x05, 0x01, 0x01, 0x80, 0x80, 0x00, 0xb5, 0xf1, 0xc3, 0x8d, 0x25, 0x02, 0x03, 0x0b, 0x9e, 0x00, 0x04, 0x9e, 0x00, 0x20, 0x27, 0x72, 0xb2, 0x83, 0x80, 0x00, 0x00, 0x09, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x2e, 0x74, 0x78, 0x74, 0x0a, 0x03, 0x02, 0xe0, 0xa6, 0xb3, 0xd7, 0x05, 0xd6, 0xdc, 0x01, 0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x2c, 0x20, 0x52, 0x41, 0x52, 0x20, 0x35, 0x2e, 0x30, 0x20, 0x66, 0x69, 0x78, 0x74, 0x75, 0x72, 0x65, 0x20, 0x77, 0x6f, 0x72, 0x6c, 0x64, 0x2e, 0x0a, 0x1d, 0x77, 0x56, 0x51, 0x03, 0x05, 0x04, 0x00],"hello.txt",b"Hello, RAR 5.0 fixture world.\n"),
        ];
        for (filename,archive_bytes,expected_name,expected_bytes) in cases {
            let path=dir.path().join(filename);
            fs::write(&path,archive_bytes).unwrap();
            assert_eq!(rar_count(&path).unwrap(),1);
            let preview=inspect_rar(&path).unwrap();
            assert_eq!(preview.total_entries,1);
            assert_eq!(preview.root_files.len(),1);
            assert_eq!(preview.root_files[0].name,expected_name);
            let mut stage=tempfile::tempfile().unwrap();
            extract_rar_entry(&path,0,expected_name,expected_bytes.len() as u64,
                MAX_NESTED_FILE_BYTES,&mut stage).unwrap();
            stage.seek(SeekFrom::Start(0)).unwrap();
            let mut bytes=Vec::new();
            stage.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes,expected_bytes);
        }
    }

    fn fixture(path:&Path){
        let mut w=ZipWriter::new(File::create(path).unwrap());
        w.start_file("Assets/Costume.unitypackage",FileOptions::default()).unwrap();
        w.write_all(b"UNITY-ONE").unwrap();
        w.start_file("Nested/Another.UNITYPACKAGE",FileOptions::default()).unwrap();
        w.write_all(b"UNITY-TWO").unwrap();
        w.start_file("Assets/readme.txt",FileOptions::default()).unwrap();
        w.write_all(b"readme").unwrap();
        w.finish().unwrap();
    }
    #[test]
    fn zip_listing_finds_nested_unitypackages_without_unpacking(){
        let temp=tempfile::tempdir().unwrap();
        let path=temp.path().join("collection.zip");
        fixture(&path);
        let preview=inspect_zip(&path).unwrap();
        assert_eq!(preview.total_entries,3);
        assert_eq!(preview.total_packages,2);
        assert_eq!(preview.packages[0].name,"Assets/Costume.unitypackage");
        assert_eq!(preview.packages[1].name,"Nested/Another.UNITYPACKAGE");
        assert_eq!(preview.sample_other_files,vec!["Assets/readme.txt"]);
    }
    #[test]
    fn extracts_only_selected_package_and_reuses_cache(){
        let temp=tempfile::tempdir().unwrap();
        let path=temp.path().join("collection.zip");fixture(&path);
        let preview=inspect_zip(&path).unwrap();
        let pkg=&preview.packages[0];
        let cache=temp.path().join("temporary");
        let extracted=extract_unitypackage(&path,pkg.index,&pkg.name,pkg.size,&cache).unwrap();
        assert_eq!(fs::read(&extracted).unwrap(),b"UNITY-ONE");
        assert_eq!(extract_unitypackage(&path,pkg.index,&pkg.name,pkg.size,&cache).unwrap(),extracted);
        assert_eq!(fs::read_dir(&cache).unwrap().count(),1);
        assert!(extract_unitypackage(&path,pkg.index,"wrong.unitypackage",pkg.size,&cache).is_err());
        assert_eq!(clear_cache(&cache).unwrap(),1);
        assert!(!extracted.exists());
        assert!(path.exists());
    }
    fn build_inner_zip(path:&Path,package_name:&str){
        let mut zip=ZipWriter::new(File::create(path).unwrap());
        zip.start_file(package_name,FileOptions::default()).unwrap();
        zip.write_all(b"INNER UNITY PACKAGE").unwrap();
        zip.finish().unwrap();
    }
    #[test]
    fn scans_compressed_zip_inside_zip_and_opens_selected_package(){
        let dir=tempfile::tempdir().unwrap();
        let inner=dir.path().join("Maya.zip");
        build_inner_zip(&inner,"Assets/Maya.UNITYPACKAGE");
        let outer=dir.path().join("bundle.zip");
        let mut writer=ZipWriter::new(File::create(&outer).unwrap());
        writer.start_file("RiderJacket/Maya.zip",
            FileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
        writer.write_all(&fs::read(&inner).unwrap()).unwrap();
        writer.finish().unwrap();
        let inspected=inspect_zip(&outer).unwrap();
        assert_eq!(inspected.total_entries,1);
        assert_eq!(inspected.nested_archives,1);
        assert_eq!(inspected.nested_scanned,1);
        assert_eq!(inspected.nested_skipped,0);
        assert_eq!(inspected.nested_entries,1);
        assert_eq!(inspected.total_packages,1);
        assert_eq!(inspected.packages[0].zip_chain.len(),1);
        assert_eq!(inspected.inner_archives.len(),1);
        assert_eq!(inspected.inner_archives[0].files.len(),1);
        assert_eq!(inspected.inner_archives[0].files[0].name,"Assets/Maya.UNITYPACKAGE");
        assert!(inspected.inner_archives[0].files[0].package.is_some());
        assert!(inspected.packages[0].display_path().contains("Maya.zip"));
        let cache=dir.path().join("cache");
        let extracted=extract_unitypackage_nested(&outer,&inspected.packages[0],&cache).unwrap();
        assert_eq!(fs::read(&extracted).unwrap(),b"INNER UNITY PACKAGE");
        let again=extract_unitypackage_nested(&outer,&inspected.packages[0],&cache).unwrap();
        assert_eq!(again,extracted);
        assert_eq!(fs::read_dir(&cache).unwrap().count(),1);
        assert!(outer.is_file());
    }

    #[test]
    fn scans_two_levels_and_reports_broken_nested_zip_without_false_empty_result(){
        let dir=tempfile::tempdir().unwrap();
        let deep=dir.path().join("deep.zip");
        build_inner_zip(&deep,"Packages/Jacket.unitypackage");
        let mid=dir.path().join("middle.zip");
        {
            let mut writer=ZipWriter::new(File::create(&mid).unwrap());
            writer.start_file("Sub/deep.zip",FileOptions::default()).unwrap();
            writer.write_all(&fs::read(&deep).unwrap()).unwrap();
            writer.finish().unwrap();
        }
        let outer=dir.path().join("outer.zip");
        {
            let mut writer=ZipWriter::new(File::create(&outer).unwrap());
            writer.start_file("middle.zip",FileOptions::default()).unwrap();
            writer.write_all(&fs::read(&mid).unwrap()).unwrap();
            writer.start_file("broken.zip",FileOptions::default()).unwrap();
            writer.write_all(b"not really a zip").unwrap();
            writer.finish().unwrap();
        }
        let preview=inspect_zip(&outer).unwrap();
        assert_eq!(preview.nested_archives,3);
        assert_eq!(preview.nested_scanned,2);
        assert_eq!(preview.nested_skipped,1);
        assert_eq!(preview.total_packages,1);
        assert_eq!(preview.packages[0].zip_chain.len(),2);
        // Both ZIP levels are independent expandable nodes in the UI.
        let middle=preview.inner_archives.iter()
            .find(|node|node.chain.len()==1 && node.chain[0].name=="middle.zip").unwrap();
        let deep=preview.inner_archives.iter()
            .find(|node|node.chain.len()==2 && node.chain[1].name=="Sub/deep.zip").unwrap();
        assert!(middle.skipped_reason.is_none());
        assert!(deep.skipped_reason.is_none());
        assert_eq!(deep.files.len(),1);
        assert!(deep.files[0].package.is_some());
        assert!(preview.inner_archives.iter().any(|node|
            node.chain.len()==1 && node.chain[0].name=="broken.zip"
                && node.skipped_reason.is_some()));
        let cache=dir.path().join("cache");
        let extracted=extract_unitypackage_nested(&outer,&preview.packages[0],&cache).unwrap();
        assert_eq!(fs::read(extracted).unwrap(),b"INNER UNITY PACKAGE");
    }

    #[test]
    fn nested_tree_lists_regular_files_even_when_no_unitypackage_exists(){
        let dir=tempfile::tempdir().unwrap();
        let inner=dir.path().join("Maya.zip");
        {
            let mut writer=ZipWriter::new(File::create(&inner).unwrap());
            writer.start_file("Textures/Albedo.png",FileOptions::default()).unwrap();
            writer.write_all(b"PNG DATA").unwrap();
            writer.start_file("Models/Jacket.fbx",FileOptions::default()).unwrap();
            writer.write_all(b"FBX DATA").unwrap();
            writer.finish().unwrap();
        }
        let outer=dir.path().join("bundle.zip");
        {
            let mut writer=ZipWriter::new(File::create(&outer).unwrap());
            writer.start_file("README.txt",FileOptions::default()).unwrap();
            writer.write_all(b"readme").unwrap();
            writer.start_file("For_Maya/Maya.zip",FileOptions::default()).unwrap();
            writer.write_all(&fs::read(&inner).unwrap()).unwrap();
            writer.finish().unwrap();
        }
        let view=inspect_zip(&outer).unwrap();
        assert_eq!(view.total_packages,0);
        assert_eq!(view.root_files.len(),1);
        assert_eq!(view.root_files[0].name,"README.txt");
        assert_eq!(view.inner_archives.len(),1);
        let maya=&view.inner_archives[0];
        assert_eq!(maya.chain[0].name,"For_Maya/Maya.zip");
        assert!(maya.skipped_reason.is_none());
        assert_eq!(maya.total_entries,2);
        assert_eq!(maya.files.len(),2);
        assert_eq!(maya.files[0].name,"Textures/Albedo.png");
        assert_eq!(maya.files[1].name,"Models/Jacket.fbx");
        assert!(maya.files.iter().all(|file|file.package.is_none()));
    }

    #[test]
    fn nested_tree_reports_broken_zip_as_skipped_instead_of_empty(){
        let dir=tempfile::tempdir().unwrap();
        let outer=dir.path().join("bundle.zip");
        let mut writer=ZipWriter::new(File::create(&outer).unwrap());
        writer.start_file("broken.zip",FileOptions::default()).unwrap();
        writer.write_all(b"not a ZIP").unwrap();
        writer.finish().unwrap();
        let preview=inspect_zip(&outer).unwrap();
        assert_eq!(preview.nested_skipped,1);
        assert_eq!(preview.inner_archives.len(),1);
        assert!(preview.inner_archives[0].skipped_reason.is_some());
        assert!(preview.inner_archives[0].files.is_empty());
    }

    #[test]
    fn preview_caps_visible_files_without_losing_total_entry_count(){
        let dir=tempfile::tempdir().unwrap();
        let inner=dir.path().join("many.zip");
        let mut writer=ZipWriter::new(File::create(&inner).unwrap());
        for i in 0..(MAX_PREVIEW_FILES_PER_ARCHIVE+3) {
            writer.start_file(format!("Assets/item-{i:04}.txt"),FileOptions::default()).unwrap();
            writer.write_all(b"x").unwrap();
        }
        writer.finish().unwrap();
        let outer=dir.path().join("outer.zip");
        let mut writer=ZipWriter::new(File::create(&outer).unwrap());
        writer.start_file("many.zip",FileOptions::default()).unwrap();
        writer.write_all(&fs::read(inner).unwrap()).unwrap();
        writer.finish().unwrap();
        let preview=inspect_zip(&outer).unwrap();
        let node=&preview.inner_archives[0];
        assert_eq!(node.total_entries,MAX_PREVIEW_FILES_PER_ARCHIVE+3);
        assert_eq!(node.files.len(),MAX_PREVIEW_FILES_PER_ARCHIVE);
        assert_eq!(node.hidden_files,3);
        assert!(node.skipped_reason.is_none());
    }

    #[test]
    fn nested_archive_selection_rejects_changed_path(){
        let dir=tempfile::tempdir().unwrap();
        let inner=dir.path().join("inner.zip");
        build_inner_zip(&inner,"Clothing.unitypackage");
        let outer=dir.path().join("outer.zip");
        let mut writer=ZipWriter::new(File::create(&outer).unwrap());
        writer.start_file("inner.zip",FileOptions::default()).unwrap();
        writer.write_all(&fs::read(&inner).unwrap()).unwrap();
        writer.finish().unwrap();
        let preview=inspect_zip(&outer).unwrap();
        let mut altered=preview.packages[0].clone();
        altered.zip_chain[0].name="../unexpected.zip".into();
        assert!(extract_unitypackage_nested(
            &outer,&altered,&dir.path().join("cache")
        ).is_err());
    }

    #[test]
    fn sqlite_cache_survives_reopen_and_forced_refresh(){
        let dir=tempfile::tempdir().unwrap();
        let zip=dir.path().join("collection.zip");
        let db=dir.path().join("db.sqlite");
        fixture(&zip);
        let (first,status)=inspect_zip_cached(&zip,&db,false).unwrap();
        assert_eq!(status,CacheUse::Fresh);
        assert_eq!(first.total_packages,2);
        assert_eq!(first.root_files.len(),3);

        // Different database connection: proves the snapshot is persistent,
        // not an in-memory cache discarded when leaving the product page.
        let (cached,status)=inspect_zip_cached(&zip,&db,false).unwrap();
        assert_eq!(status,CacheUse::Unchanged);
        assert_eq!(cached.packages[0].name,first.packages[0].name);

        let (fresh,status)=inspect_zip_cached(&zip,&db,true).unwrap();
        assert_eq!(status,CacheUse::Forced);
        assert_eq!(fresh.total_packages,2);
        let (cached_again,status)=inspect_zip_cached(&zip,&db,false).unwrap();
        assert_eq!(status,CacheUse::Unchanged);
        assert_eq!(cached_again.total_packages,2);
    }

    #[test]
    fn cache_reuses_same_hash_for_other_path_and_invalidates_changed_bytes(){
        let dir=tempfile::tempdir().unwrap();
        let zip=dir.path().join("original.zip");
        let copy=dir.path().join("moved.zip");
        let db=dir.path().join("db.sqlite");
        fixture(&zip);
        fs::copy(&zip,&copy).unwrap();
        let (_,status)=inspect_zip_cached(&zip,&db,false).unwrap();
        assert_eq!(status,CacheUse::Fresh);
        let (copied,status)=inspect_zip_cached(&copy,&db,false).unwrap();
        assert_eq!(status,CacheUse::MatchingHash);
        assert_eq!(copied.total_packages,2);
        let (_,status)=inspect_zip_cached(&copy,&db,false).unwrap();
        assert_eq!(status,CacheUse::Unchanged);

        // Replacing a linked file must not reuse the stale preview, even
        // when the attachment's original import checksum remains in SQLite.
        let mut writer=ZipWriter::new(File::create(&copy).unwrap());
        writer.start_file("Assets/Other.txt",FileOptions::default()).unwrap();
        writer.write_all(b"changed content and a different byte count").unwrap();
        writer.finish().unwrap();
        let (changed,status)=inspect_zip_cached(&copy,&db,false).unwrap();
        assert_eq!(status,CacheUse::Fresh);
        assert_eq!(changed.total_packages,0);
        assert_eq!(changed.root_files[0].name,"Assets/Other.txt");
        let (_,status)=inspect_zip_cached(&copy,&db,false).unwrap();
        assert_eq!(status,CacheUse::Unchanged);
    }

    fn fixture_sevenzip(path:&Path,with_inner:bool){
        use sevenz_rust2::{ArchiveWriter,ArchiveEntry};
        let temp=tempfile::tempdir().unwrap();
        let pkg=temp.path().join("direct.unitypackage");
        fs::write(&pkg,b"DIRECT UNITY PACKAGE").unwrap();
        let mut w=ArchiveWriter::create(path).unwrap();
        w.push_archive_entry(ArchiveEntry::from_path(&pkg,
            "Assets/MAYA.unitypackage".to_string()),
            Some(File::open(&pkg).unwrap())).unwrap();
        if with_inner {
            let zip=temp.path().join("nested.zip");
            build_inner_zip(&zip,"Assets/Manuka.UNITYPACKAGE");
            w.push_archive_entry(ArchiveEntry::from_path(&zip,
                "Archives/nested.zip".to_string()),
                Some(File::open(&zip).unwrap())).unwrap();
        }
        w.finish().unwrap();
    }
    #[test]
    fn sevenzip_lists_extracts_nested_zip_and_reuses_cache(){
        let temp=tempfile::tempdir().unwrap();
        let path=temp.path().join("source.7z");
        fixture_sevenzip(&path,true);
        assert_eq!(sevenzip_count(&path).unwrap(),2);
        assert!(crate::import::supported(&path));
        let db=temp.path().join("library.db");
        let (preview,used)=inspect_zip_cached(&path,&db,false).unwrap();
        assert_eq!(used,CacheUse::Fresh);
        assert_eq!(preview.total_entries,2);
        assert_eq!(preview.nested_archives,1);
        assert_eq!(preview.nested_scanned,1);
        assert_eq!(preview.nested_skipped,0);
        assert_eq!(preview.total_packages,2);
        let cache=temp.path().join("unity-cache");
        let direct=preview.packages.iter().find(|p|p.zip_chain.is_empty()).unwrap();
        let direct_path=extract_unitypackage_nested(&path,direct,&cache).unwrap();
        assert_eq!(fs::read(&direct_path).unwrap(),b"DIRECT UNITY PACKAGE");
        let nested=preview.packages.iter().find(|p|!p.zip_chain.is_empty()).unwrap();
        let nested_path=extract_unitypackage_nested(&path,nested,&cache).unwrap();
        assert_eq!(fs::read(&nested_path).unwrap(),b"INNER UNITY PACKAGE");
        let (cached,status)=inspect_zip_cached(&path,&db,false).unwrap();
        assert_eq!(status,CacheUse::Unchanged);
        assert_eq!(cached.total_packages,2);
        let (again,status)=inspect_zip_cached(&path,&db,true).unwrap();
        assert_eq!(status,CacheUse::Forced);
        assert_eq!(again.total_packages,2);
    }
    #[test]
    fn sevenzip_rejects_corrupted_files_and_unsafe_member_names(){
        let tmp=tempfile::tempdir().unwrap();
        let invalid=tmp.path().join("invalid.7z");
        fs::write(&invalid,b"not a valid sevenzip").unwrap();
        assert!(sevenzip_count(&invalid).is_err());
        assert!(inspect_sevenzip(&invalid).is_err());
        for name in ["../evil.unitypackage","C:\\unsafe.unitypackage",
            "/root/evil.unitypackage","a/../../bad.unitypackage"] {
            assert!(!safe_member_name(name));
        }
        assert!(safe_member_name("Assets/MAYA.unitypackage"));
    }

    #[test]
    fn rejects_zip_slip_entries_and_non_package_extraction(){
        let temp=tempfile::tempdir().unwrap();
        let path=temp.path().join("untrusted.zip");
        let mut w=ZipWriter::new(File::create(&path).unwrap());
        w.start_file("../outside.unitypackage",FileOptions::default()).unwrap();
        w.write_all(b"bad").unwrap();
        w.start_file("normal.txt",FileOptions::default()).unwrap();
        w.write_all(b"text").unwrap();
        w.finish().unwrap();
        let preview=inspect_zip(&path).unwrap();
        assert_eq!(preview.total_packages,0);
        let cache=temp.path().join("cache");
        assert!(extract_unitypackage(&path,0,"../outside.unitypackage",3,&cache).is_err());
        assert!(extract_unitypackage(&path,1,"normal.txt",4,&cache).is_err());
        assert!(!temp.path().join("outside.unitypackage").exists());
    }
}
