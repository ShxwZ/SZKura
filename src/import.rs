//! ZIP/7z/RAR/.unitypackage references or opt-in managed copies, attached to products.
use crate::db::{ArchiveMode, Catalog, NewAttachment};
use anyhow::{bail, Context, Result};
use std::{fs::{self, File}, io::{Read, BufReader}, path::{Path, PathBuf}};
use walkdir::WalkDir;

#[derive(Default, Debug)]
pub struct ImportReport {
    pub created_products: usize,
    pub linked_files: usize,
    pub duplicates: usize,
    pub rejected: usize,
    pub errors: Vec<String>,
}

pub fn supported(path: &Path) -> bool {
    path.extension().and_then(|e|e.to_str()).is_some_and(|e|e.eq_ignore_ascii_case("zip")||e.eq_ignore_ascii_case("unitypackage")||e.eq_ignore_ascii_case("7z")||e.eq_ignore_ascii_case("rar"))
}
fn hash_file(file: &Path) -> Result<String> {
    let mut reader=BufReader::new(File::open(file)?);
    let mut hasher=blake3::Hasher::new();
    let mut buffer=[0u8; 128*1024];
    loop { let n=reader.read(&mut buffer)?; if n==0 {break;} hasher.update(&buffer[..n]); }
    Ok(hasher.finalize().to_hex().to_string())
}
fn zip_count(path: &Path) -> Result<i64> {
    let archive=zip::ZipArchive::new(File::open(path)?).context("ZIP dañado o no válido")?;
    if archive.len()>100_000 { bail!("El ZIP tiene demasiadas entradas"); }
    Ok(archive.len() as i64)
}

/// `asset_id=None` means one new manual catalog product per archive.
/// `Reference` saves only the absolute path, without duplicating file bytes.
pub fn import_paths(db_path: &Path, library: &Path, paths: &[PathBuf], category: &str,
    asset_id: Option<i64>, mode: ArchiveMode) -> Result<ImportReport> {
    let mut db=Catalog::open(db_path)?;
    fs::create_dir_all(library)?;
    let mut report=ImportReport::default();
    for path in paths {
        if path.is_dir() {
            for entry in WalkDir::new(path).follow_links(false).into_iter() {
                match entry {
                    Ok(ent) if ent.file_type().is_file() && supported(ent.path()) => accumulate(&mut db,library,ent.path(),category,asset_id,mode,&mut report),
                    Err(err) => { report.rejected+=1; if report.errors.len()<8 {report.errors.push(format!("No se pudo leer una carpeta: {err}"));} },
                    _=>{}
                }
            }
        } else if supported(path) {
            accumulate(&mut db,library,path,category,asset_id,mode,&mut report);
        } else {
            report.rejected+=1;
            if report.errors.len()<8 {report.errors.push(format!("No es ZIP/7z/RAR/unitypackage: {}",path.display()));}
        }
    }
    Ok(report)
}
fn accumulate(db:&mut Catalog,library:&Path,path:&Path,category:&str,asset_id:Option<i64>,mode:ArchiveMode, report:&mut ImportReport) {
    match import_one(db,library,path,category,asset_id,mode) {
        Ok(Some(created)) => {report.linked_files+=1; if created {report.created_products+=1;} },
        Ok(None) => report.duplicates+=1,
        Err(err) => {report.rejected+=1; if report.errors.len()<8 {report.errors.push(format!("{}: {err:#}",path.display()));} }
    }
}

/// Returns Some(true) for a new product, Some(false) for a new attachment, None for duplicate.
pub fn import_one(db:&mut Catalog,library:&Path,path:&Path,category:&str,asset_id:Option<i64>,mode:ArchiveMode) -> Result<Option<bool>> {
    if !path.is_file() || !supported(path) {bail!("Archivo no compatible");}
    let ext=path.extension().and_then(|e|e.to_str()).unwrap_or("zip").to_ascii_lowercase();
    let count=if ext=="zip"{zip_count(path)?}
        else if ext=="7z"{crate::package_inspect::sevenzip_count(path)?}
        else if ext=="rar"{crate::package_inspect::rar_count(path)?}
        else{0};
    let hash=hash_file(path)?;
    if let Some(id)=asset_id {
        if db.get(id)?.is_none() {bail!("Producto destino no encontrado");}
        if db.asset_has_checksum(id,&hash)? {return Ok(None);}
    } else if db.exists_checksum(&hash)? { return Ok(None); }
    let original=path.file_name().context("Archivo sin nombre")?.to_string_lossy().to_string();
    let title=path.file_stem().context("Archivo sin nombre")?.to_string_lossy().to_string();
    let dest=match mode {
        ArchiveMode::Reference=>fs::canonicalize(path).context("No se pudo localizar el ZIP original")?,
        ArchiveMode::Copy=>{
            let safe:String=title.chars().map(|c| if c.is_alphanumeric()||c=='-'||c=='_' {c}else{'_'}).take(62).collect();
            let partition=library.join(&hash[..2]); fs::create_dir_all(&partition)?;
            let target=partition.join(format!("{}-{safe}.{ext}",&hash[..16]));
            if !target.exists() {
                let temporary=partition.join(format!("{}.partial",&hash[..16]));
                let _=fs::remove_file(&temporary);
                if let Err(err)=fs::copy(path,&temporary) {let _=fs::remove_file(&temporary);return Err(err.into());}
                if let Err(err)=fs::rename(&temporary,&target) {let _=fs::remove_file(&temporary);return Err(err.into());}
            }
            target
        }
    };
    let new_product=asset_id.is_none();
    let id=match asset_id {Some(id)=>id,None=>db.create_product(&title,category)?};
    let inserted=db.add_attachment(&NewAttachment {asset_id:id,name:&original,path:&dest.to_string_lossy(),checksum:&hash,bytes:fs::metadata(&dest)?.len().try_into()?,count,mode})?;
    if !inserted && new_product {db.delete(id)?;}
    Ok(inserted.then_some(new_product))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn make_zip(path:&Path,text:&str) {
        let mut writer=zip::ZipWriter::new(File::create(path).unwrap());
        writer.start_file("contenido.txt",zip::write::FileOptions::default()).unwrap();
        writer.write_all(text.as_bytes()).unwrap();writer.finish().unwrap();
    }
    #[test]
    fn links_two_archives_to_one_product_without_copy() {
        let dir=tempfile::tempdir().unwrap();
        let a=dir.path().join("a.zip");let b=dir.path().join("b.zip");
        make_zip(&a,"A");make_zip(&b,"B");
        let dbpath=dir.path().join("db.sqlite"); let library=dir.path().join("library");
        let mut db=Catalog::open(&dbpath).unwrap();
        let id=db.add_booth_link("https://booth.pm/items/123").unwrap();drop(db);
        let paths=vec![a.clone(),b.clone()];
        let report=import_paths(&dbpath,&library,&paths,"Otros",Some(id),ArchiveMode::Reference).unwrap();
        assert_eq!(report.linked_files,2);
        let again=import_paths(&dbpath,&library,&paths,"Otros",Some(id),ArchiveMode::Reference).unwrap();
        assert_eq!(again.duplicates,2);
        let db=Catalog::open(&dbpath).unwrap();
        assert_eq!(db.attachments_for(id).unwrap().len(),2);
        assert!(db.attachments_for(id).unwrap().iter().all(|it|it.mode==ArchiveMode::Reference));
        assert!(!library.join("aa").exists());
    }
    #[test]
    fn bulk_copy_creates_products_without_touching_sources() {
        let dir=tempfile::tempdir().unwrap();let zip=dir.path().join("ropa.zip");make_zip(&zip,"hola");
        let dbpath=dir.path().join("db.sqlite");let lib=dir.path().join("library");
        let r=import_paths(&dbpath,&lib,std::slice::from_ref(&zip),"Otros",None,ArchiveMode::Copy).unwrap();
        assert_eq!(r.created_products,1);
        let db=Catalog::open(&dbpath).unwrap();
        let files=db.attachments_for(1).unwrap();
        assert_eq!(files.len(),1);assert_ne!(files[0].stored_path,zip.to_string_lossy());
        assert!(Path::new(&files[0].stored_path).exists());assert!(zip.exists());
    }
    #[test]
    fn recursively_link_folder_to_single_product() {
        let dir=tempfile::tempdir().unwrap();
        let folder=dir.path().join("coleccion"); let nested=folder.join("subcarpeta");
        fs::create_dir_all(&nested).unwrap();
        make_zip(&folder.join("a.zip"),"one");
        make_zip(&nested.join("b.zip"),"two");
        fs::write(folder.join("readme.txt"),"ignore").unwrap();
        let dbpath=dir.path().join("catalog.sqlite");
        let library=dir.path().join("lib");
        let mut catalog=Catalog::open(&dbpath).unwrap();
        let id=catalog.create_product("Colección","Ropa").unwrap();
        drop(catalog);
        let report=import_paths(
            &dbpath,&library,&[folder],"Ropa",Some(id),ArchiveMode::Reference
        ).unwrap();
        assert_eq!(report.linked_files,2);
        assert_eq!(report.created_products,0);
        assert_eq!(report.rejected,0);
        let catalog=Catalog::open(&dbpath).unwrap();
        assert_eq!(catalog.count().unwrap(),1);
        assert_eq!(catalog.attachments_for(id).unwrap().len(),2);
    }
    #[test]
    fn accepts_rar_extension_and_rejects_corrupt_rar(){
        let dir=tempfile::tempdir().unwrap();
        let invalid=dir.path().join("invalid.RAR");
        fs::write(&invalid,b"not a rar").unwrap();
        assert!(supported(&invalid));
        let report=import_paths(&dir.path().join("db.sqlite"),
            &dir.path().join("library"),&[invalid],"Otros",None,
            ArchiveMode::Reference).unwrap();
        assert_eq!(report.rejected,1);
        assert_eq!(report.created_products,0);
        assert!(!report.errors.is_empty());
    }
    #[test]
    fn invalid_zip_is_not_catalogued() {
        let dir=tempfile::tempdir().unwrap();let zip=dir.path().join("bad.zip");fs::write(&zip,b"broken").unwrap();
        let result=import_paths(&dir.path().join("d.sqlite"),&dir.path().join("lib"),&[zip],"Otros",None,ArchiveMode::Reference).unwrap();
        assert_eq!(result.rejected,1);assert_eq!(result.created_products,0);
    }
}
