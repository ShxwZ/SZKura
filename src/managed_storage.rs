//! Managed storage: copy+verify+register, then (only with explicit opt-in) trash source.
use crate::{
    db::{ArchiveMode, Catalog, NewAttachment},
    import,
};
use anyhow::{bail, Context, Result};
use std::{
    fs::{self,File},
    io::{Read,Write},
    path::{Path,PathBuf},
};
use walkdir::WalkDir;

#[derive(Default, Debug)]
pub struct ManagedReport {
    pub linked:usize,
    pub duplicates:usize,
    pub rejected:usize,
    pub moved_to_trash:usize,
    pub warnings:Vec<String>,
}
impl ManagedReport {
    fn failure(&mut self,path:&Path,error:&anyhow::Error) {
        self.rejected+=1;
        if self.warnings.len()<8 {
            self.warnings.push(format!("{}: {error:#}",path.display()));
        }
    }
}
fn digest(path:&Path)->Result<(String,u64)> {
    let mut f=File::open(path)?;
    let mut h=blake3::Hasher::new();
    let mut bytes=0u64;
    let mut buf=[0u8;128*1024];
    loop {
        let n=f.read(&mut buf)?;
        if n==0 {break;}
        h.update(&buf[..n]);
        bytes+=n as u64;
    }
    Ok((h.finalize().to_hex().to_string(),bytes))
}
fn safe_name(raw:&str,max:usize)->String {
    let cleaned=raw.chars().map(|c|{
        if c.is_alphanumeric() || matches!(c,'_'|'-'|'.') {c}else{'_'}
    }).take(max).collect::<String>();
    if cleaned.is_empty() {"asset".into()}else{cleaned}
}
fn folder_for(db:&mut Catalog,root:&Path,id:i64)->Result<PathBuf> {
    let asset=db.get(id)?.context("No existe el producto")?;
    let category=safe_name(&asset.category,36);
    let title=safe_name(&asset.title,44);
    let candidate=root.join(category).join(format!("{id:08}-{title}"));
    fs::create_dir_all(&candidate)?;
    // Existing symlinks must never redirect writes outside the chosen root.
    let folder=fs::canonicalize(&candidate)?;
    if !folder.starts_with(root){bail!("La carpeta de producto sale del almacén configurado");}
    db.register_managed_folder(id,&folder)?;
    Ok(folder)
}
fn verified_copy(source:&Path,folder:&Path,hash:&str,bytes:u64)->Result<PathBuf> {
    let name=source.file_name().context("Archivo sin nombre")?.to_string_lossy();
    let suffix=safe_name(&name,85);
    let target=folder.join(format!("{}-{suffix}",&hash[..16]));
    if target.exists() {
        let actual=digest(&target)?;
        if actual==(hash.to_owned(),bytes) {return Ok(target);}
        bail!("Colisión o archivo de destino modificado: {}",target.display());
    }
    let mut temp=tempfile::NamedTempFile::new_in(folder)
        .context("No se pudo crear el archivo temporal en la biblioteca")?;
    let mut input=File::open(source)?;
    std::io::copy(&mut input,&mut temp)?;
    temp.as_file_mut().flush()?;
    temp.as_file_mut().sync_all()?;
    if digest(temp.path())? != (hash.to_owned(),bytes) {
        bail!("La copia no coincide con el hash y tamaño de origen");
    }
    match temp.persist_noclobber(&target) {
        Ok(_)=>{},
        Err(error)=>{
            if !target.is_file() || digest(&target)? != (hash.to_owned(),bytes) {
                return Err(error.error.into());
            }
        }
    }
    Ok(target)
}
fn source_can_be_removed(db:&Catalog,source:&Path,root:&Path,expected:&(String,u64))->Result<bool>{
    if source.starts_with(root) || db.is_file_referenced(source)? {
        return Ok(false);
    }
    // Check against any concurrent modifications before moving to trash.
    Ok(&digest(source)?==expected)
}
fn import_one(
    db:&mut Catalog,root:&Path,source:&Path,id:i64,delete_source:bool,
    report:&mut ManagedReport
)->Result<()> {
    if !import::supported(source) {bail!("Solo ZIP, 7z, RAR y UnityPackage");}
    let meta=fs::symlink_metadata(source)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        bail!("No se importan enlaces simbólicos ni archivos especiales");
    }
    let canonical=fs::canonicalize(source)?;
    if !canonical.is_file(){bail!("El archivo original no está disponible");}
    let (hash,bytes)=digest(&canonical)?;
    if db.asset_has_checksum(id,&hash)? {
        report.duplicates+=1;
        return Ok(());
    }
    let ext=canonical.extension().and_then(|e|e.to_str()).unwrap_or("");
    let count=if ext.eq_ignore_ascii_case("zip") {
        let archive=zip::ZipArchive::new(File::open(&canonical)?)
            .context("El ZIP está dañado o no es válido")?;
        archive.len().try_into()?
    }else if ext.eq_ignore_ascii_case("7z") {
        crate::package_inspect::sevenzip_count(&canonical)?
    }else if ext.eq_ignore_ascii_case("rar") {
        crate::package_inspect::rar_count(&canonical)?
    }else{0};
    let folder=folder_for(db,root,id)?;
    let target=verified_copy(&canonical,&folder,&hash,bytes)?;
    // The source still must match the hash computed before copying.
    if digest(&canonical)? != (hash.clone(),bytes) {
        bail!("El original cambió mientras se copiaba; se conserva el original");
    }
    let name=canonical.file_name().unwrap().to_string_lossy().into_owned();
    let added=db.add_attachment(&NewAttachment{
        asset_id:id,name:&name,path:&target.to_string_lossy(),checksum:&hash,
        bytes:bytes.try_into()?,count,mode:ArchiveMode::Copy,
    })?;
    if !added {
        report.duplicates+=1;
        return Ok(());
    }
    report.linked+=1;
    if delete_source {
        if source_can_be_removed(db,&canonical,root,&(hash,bytes))? {
            match trash::delete(&canonical) {
                Ok(())=>report.moved_to_trash+=1,
                Err(error)=>{
                    if report.warnings.len()<8 {
                        report.warnings.push(format!(
                            "Archivo copiado correctamente, pero no se pudo enviar el original a la papelera: {} ({error})",
                            canonical.display()
                        ));
                    }
                }
            }
        }else if report.warnings.len()<8 {
            report.warnings.push(format!(
                "Se conservó el original por seguridad (ya vinculado, situado en el almacén o modificado): {}",
                canonical.display()
            ));
        }
    }
    Ok(())
}
/// All entries are verified managed copies; never create unmanaged references.
pub fn import_paths(
    db_path:&Path,library:&Path,paths:&[PathBuf],id:i64,delete_sources:bool
)->Result<ManagedReport> {
    let mut db=Catalog::open(db_path)?;
    let expected=db.storage_root()?.context("Configura primero la carpeta de assets")?;
    let root=fs::canonicalize(library)?;
    if root!=expected {bail!("La carpeta configurada ha cambiado; vuelve a abrir Configuración");}
    let mut report=ManagedReport::default();
    for path in paths {
        if path.is_dir() {
            let directory=fs::canonicalize(path)?;
            if root.starts_with(&directory) {
                report.rejected+=1;
                report.warnings.push("No se puede importar el propio almacén ni una carpeta que lo contenga".into());
                continue;
            }
            for entry in WalkDir::new(path).follow_links(false).into_iter() {
                match entry {
                    Ok(ent) if ent.file_type().is_file() && import::supported(ent.path())=>{
                        if let Err(e)=import_one(&mut db,&root,ent.path(),id,delete_sources,&mut report){
                            report.failure(ent.path(),&e);
                        }
                    },
                    Err(e)=>{report.rejected+=1;if report.warnings.len()<8 {report.warnings.push(e.to_string());}},
                    _=>{}
                }
            }
        }else if let Err(e)=import_one(&mut db,&root,path,id,delete_sources,&mut report) {
            report.failure(path,&e);
        }
    }
    Ok(report)
}

/// Safely import legacy linked files into the managed folder; originals remain
/// untouched. Existing attachment IDs, favorites and product relationships survive.
pub fn organize_existing(db_path:&Path,library:&Path)->Result<ManagedReport>{
    let mut db=Catalog::open(db_path)?;
    let root=fs::canonicalize(library)?;
    if db.storage_root()?.as_deref()!=Some(root.as_path()) {
        bail!("El almacén configurado no coincide");
    }
    let mut report=ManagedReport::default();
    for attachment in db.all_attachments()? {
        let source=PathBuf::from(&attachment.stored_path);
        if !source.exists() {
            report.rejected+=1;
            continue;
        }
        let old=source.clone();
        let result=(||->Result<bool>{
            let source=fs::canonicalize(&source)?;
            if source.starts_with(&root) {return Ok(false);}
            let (hash,bytes)=digest(&source)?;
            if hash!=attachment.checksum || bytes!=u64::try_from(attachment.bytes)? {
                bail!("El archivo ya no coincide con el hash registrado");
            }
            let folder=folder_for(&mut db,&root,attachment.asset_id)?;
            let target=verified_copy(&source,&folder,&hash,bytes)?;
            if !db.relocate_attachment(attachment.id,attachment.asset_id,&target,&hash)? {
                bail!("No se pudo actualizar la ruta del adjunto en SQLite");
            }
            Ok(true)
        })();
        match result{
            Ok(true)=>report.linked+=1,
            Ok(false)=>report.duplicates+=1,
            Err(e)=>report.failure(&old,&e),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn archive(path:&Path,content:&str) {
        let mut zip=zip::ZipWriter::new(File::create(path).unwrap());
        zip.start_file("data.txt",zip::write::FileOptions::default()).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    #[test]
    fn managed_import_copies_verifies_and_keeps_original_by_default(){
        let tmp=tempfile::tempdir().unwrap();
        let source=tmp.path().join("source.zip");
        archive(&source,"Original");
        let db_path=tmp.path().join("catalog.sqlite");
        let root=tmp.path().join("managed");
        let mut db=Catalog::open(&db_path).unwrap();
        db.configure_storage(&root).unwrap();
        let id=db.create_product("Maya Costume","Ropa").unwrap();
        drop(db);
        let result=import_paths(&db_path,&root,&[source.clone()],id,false).unwrap();
        assert_eq!(result.linked,1);
        assert!(source.exists());
        let db=Catalog::open(&db_path).unwrap();
        let attachments=db.attachments_for(id).unwrap();
        assert_eq!(attachments.len(),1);
        assert_eq!(attachments[0].mode,ArchiveMode::Copy);
        assert_eq!(digest(Path::new(&attachments[0].stored_path)).unwrap(),digest(&source).unwrap());
        assert_eq!(db.managed_folders(id).unwrap().len(),1);
        let again=import_paths(&db_path,&root,&[source.clone()],id,false).unwrap();
        assert_eq!(again.duplicates,1);
    }
    #[test]
    fn detected_category_is_used_for_the_first_and_only_managed_copy(){
        let tmp=tempfile::tempdir().unwrap();
        let source=tmp.path().join("outfit.zip");
        archive(&source,"Outfit contents");
        let db_path=tmp.path().join("catalog.sqlite");
        let root=tmp.path().join("managed");
        let mut db=Catalog::open(&db_path).unwrap();
        db.configure_storage(&root).unwrap();
        // The BOOTH metadata was fetched before import, so the final local
        // category is known before a single byte is copied into the library.
        let id=db.add_booth_link("https://booth.pm/items/6304563").unwrap();
        db.update_details(id,"Consolatoria","Ropa","TL SHOP","","").unwrap();
        drop(db);
        let report=import_paths(&db_path,&root,&[source.clone()],id,false).unwrap();
        assert_eq!(report.linked,1);
        let db=Catalog::open(&db_path).unwrap();
        let stored=db.attachments_for(id).unwrap();
        assert_eq!(stored.len(),1);
        // On Windows the managed importer canonicalizes paths with the
        // extended-length \\?\ prefix; compare like-for-like absolute paths.
        let canonical_root=fs::canonicalize(&root).unwrap();
        assert!(Path::new(&stored[0].stored_path).starts_with(canonical_root.join("Ropa")),
            "Debe copiarse directamente en Ropa: {}",stored[0].stored_path);
        assert!(!canonical_root.join("Otros").exists(),
            "No debe crear/cambiar una carpeta temporal en Otros");
        assert!(source.is_file());
        assert_eq!(import_paths(&db_path,&root,&[source.clone()],id,false)
            .unwrap().duplicates,1);
    }

    #[test]
    fn failed_or_changed_source_never_is_removed(){
        let tmp=tempfile::tempdir().unwrap();
        let db_path=tmp.path().join("db.sqlite");
        let root=tmp.path().join("managed");
        let mut db=Catalog::open(&db_path).unwrap();
        db.configure_storage(&root).unwrap();
        let id=db.create_product("test","Otros").unwrap();
        drop(db);
        let invalid=tmp.path().join("bad.zip");
        fs::write(&invalid,b"invalid ZIP").unwrap();
        let report=import_paths(&db_path,&root,&[invalid.clone()],id,true).unwrap();
        assert_eq!(report.linked,0);
        assert_eq!(report.rejected,1);
        assert!(invalid.exists());
    }
}
