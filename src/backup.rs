//! Portable database snapshot plus local covers. Asset ZIPs are not duplicated.
//! Uses SQLite online backup so WAL changes are included consistently.
use anyhow::{bail,Context,Result};
use rusqlite::{backup::Backup,Connection,OpenFlags};
use std::{
    collections::HashSet,
    fs::{self,File},
    io::{Read,Write},
    path::{Path,PathBuf},
    time::{Duration,SystemTime,UNIX_EPOCH},
};
use zip::{write::FileOptions,ZipArchive,ZipWriter};

const MANIFEST:&str="SZKURA_BACKUP_V2";
const LEGACY_MANIFEST:&str="SZ_BOOTH_BACKUP_V1";
const MAX_COVER_BYTES:u64=16*1024*1024;
const MAX_DB_BYTES:u64=200*1024*1024;
const MAX_TOTAL_BYTES:u64=500*1024*1024;
const MAX_COVERS:usize=4096;

#[derive(Debug)]
pub struct BackupInfo {
    pub covers:usize,
    pub recovery:Option<PathBuf>,
}
fn snapshot(from:&Path,to:&Path)->Result<()> {
    let source=Connection::open(from)?;
    let mut destination=Connection::open(to)?;
    {
        let copy=Backup::new(&source,&mut destination)?;
        copy.run_to_completion(128,Duration::from_millis(10),None)?;
    }
    let result:String=destination.query_row("PRAGMA integrity_check",[],|r|r.get(0))?;
    if result!="ok"{bail!("La instantánea SQLite no pasó la comprobación de integridad: {result}");}
    Ok(())
}
fn validate_sqlite(path:&Path)->Result<()>{
    let db=Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("No se puede abrir la base de datos de la copia")?;
    let integrity:String=db.query_row("PRAGMA integrity_check",[],|r|r.get(0))?;
    if integrity!="ok"{bail!("La base de datos exportada está dañada");}
    for table in ["assets","attachments"] {
        let exists:i64=db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],|r|r.get(0))?;
        if exists!=1 {bail!("No es una base de datos SZKura: falta {table}");}
    }
    Ok(())
}
fn allowed_cover(name:&str)->bool {
    name.len()<=160
        && !name.is_empty()
        && !name.starts_with('.')
        && !name.contains('/') && !name.contains('\\') && !name.contains("..")
        && name.chars().all(|c|c.is_alphanumeric() || matches!(c,'-'|'_'|'.'))
        && name.rsplit_once('.').is_some_and(|(_,extension)|
            ["png","jpg","jpeg","webp"].contains(&extension.to_ascii_lowercase().as_str()))
}
fn copy_limited<R:Read,W:Write>(input:&mut R,output:&mut W,max:u64)->Result<u64>{
    let mut n=0u64;
    let mut buffer=[0u8;128*1024];
    loop{
        let amount=(&mut *input).take((max+1-n).min(buffer.len() as u64)).read(&mut buffer)?;
        if amount==0{break;}
        n+=amount as u64;
        if n>max{bail!("El archivo de la copia supera el límite de tamaño");}
        output.write_all(&buffer[..amount])?;
    }
    Ok(n)
}
/// Export a portable .szkura file with SQLite snapshot and locally managed covers.
/// Destination is created atomically and never silently overwritten.
pub fn export_bundle(db_path:&Path,covers_path:&Path,out:&Path)->Result<BackupInfo>{
    if out.exists(){bail!("Ya existe ese archivo: elige otro nombre para la copia");}
    let parent=out.parent().context("Destino sin carpeta")?;
    fs::create_dir_all(parent)?;
    let stage=tempfile::tempdir_in(parent)?;
    let sqlite=stage.path().join("catalog.sqlite");
    snapshot(db_path,&sqlite)?;
    if fs::metadata(&sqlite)?.len()>MAX_DB_BYTES {
        bail!("La base SQLite es demasiado grande para exportación portátil");
    }
    let mut tmp=tempfile::NamedTempFile::new_in(parent)?;
    let output=tmp.reopen()?;
    let mut zip=ZipWriter::new(output);
    let opts=FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("format.txt",opts)?;
    zip.write_all(MANIFEST.as_bytes())?;
    zip.start_file("catalog.sqlite",opts)?;
    let mut source=File::open(&sqlite)?;
    copy_limited(&mut source,&mut zip,MAX_DB_BYTES)?;
    let mut names=HashSet::new();
    let mut total=fs::metadata(&sqlite)?.len();
    let mut copied=0usize;
    if covers_path.exists(){
        for entry in fs::read_dir(covers_path)?{
            let entry=entry?;
            let name=entry.file_name().to_string_lossy().into_owned();
            let path=entry.path();
            if !allowed_cover(&name) || !entry.file_type()?.is_file() {
                continue;
            }
            let len=entry.metadata()?.len();
            if len>MAX_COVER_BYTES || total.saturating_add(len)>MAX_TOTAL_BYTES || copied>=MAX_COVERS {
                bail!("Demasiadas portadas o datos para crear la copia de seguridad");
            }
            if !names.insert(name.clone()){bail!("Portada duplicada en la copia");}
            zip.start_file(format!("covers/{name}"),opts)?;
            let mut cover=File::open(&path)?;
            let read=copy_limited(&mut cover,&mut zip,MAX_COVER_BYTES)?;
            if read!=len{bail!("Una portada cambió mientras se exportaba");}
            copied+=1;total+=len;
        }
    }
    let out_file=zip.finish()?;
    out_file.sync_all()?;
    drop(out_file);
    tmp.persist_noclobber(out)?;
    Ok(BackupInfo{covers:copied,recovery:None})
}

/// Import the archive after extracting and validating everything. Creates an
/// automatic pre-restore SQLite recovery file, without replacing ZIP assets.
pub fn import_bundle(db_path:&Path,covers_dir:&Path,bundle:&Path)->Result<BackupInfo>{
    let mut archive=ZipArchive::new(File::open(bundle)?)
        .context("No es una copia de seguridad SZKura válida")?;
    if archive.len()>MAX_COVERS+2 {bail!("La copia contiene demasiados archivos");}
    let stage=tempfile::tempdir()?;
    let stage_db=stage.path().join("catalog.sqlite");
    let stage_covers=stage.path().join("covers");
    fs::create_dir_all(&stage_covers)?;
    let mut total=0u64;
    let mut names=HashSet::new();
    let mut cover_names=Vec::new();
    let mut found_manifest=false;
    let mut found_sqlite=false;
    for index in 0..archive.len(){
        let mut entry=archive.by_index(index)?;
        if entry.is_dir(){bail!("La copia contiene carpetas no esperadas");}
        let name=entry.name().to_string();
        if !names.insert(name.clone()){bail!("La copia contiene nombres duplicados");}
        let (dest,max)=match name.as_str(){
            "format.txt"=>(stage.path().join("format.txt"),128),
            "catalog.sqlite"=>(stage_db.clone(),MAX_DB_BYTES),
            _ if name.starts_with("covers/") && allowed_cover(&name[7..])=>{
                cover_names.push(name[7..].to_string());
                (stage_covers.join(&name[7..]),MAX_COVER_BYTES)
            },
            _=>bail!("Entrada inesperada o ruta insegura en la copia: {name}"),
        };
        if entry.size()>max || total.saturating_add(entry.size())>MAX_TOTAL_BYTES{
            bail!("La copia supera los límites de tamaño");
        }
        let mut output=File::create(dest)?;
        let copied=copy_limited(&mut entry,&mut output,max)?;
        if copied!=entry.size(){bail!("Una entrada de la copia está incompleta");}
        total+=copied;
    }
    if names.contains("format.txt") {
        let format=fs::read_to_string(stage.path().join("format.txt"))?;
        found_manifest=format==MANIFEST || format==LEGACY_MANIFEST;
    }
    if names.contains("catalog.sqlite"){found_sqlite=true;}
    if !found_manifest || !found_sqlite{bail!("Formato de copia incompatible");}
    validate_sqlite(&stage_db)?;

    // Check every cover collision *before* changing the database. Never overwrite.
    fs::create_dir_all(covers_dir)?;
    for name in &cover_names{
        let destination=covers_dir.join(name);
        let source=stage_covers.join(name);
        if destination.exists() {
            if blake3::hash(&fs::read(&source)?)!=blake3::hash(&fs::read(&destination)?) {
                bail!("Ya existe una portada con el mismo nombre y otro contenido: {name}");
            }
        }
    }
    let current_root=crate::db::Catalog::open(db_path)?.storage_root()?;
    let timestamp=SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let backup_file=db_path.with_file_name(format!("catalog-before-restore-{timestamp}.sqlite"));
    if backup_file.exists(){bail!("La ruta de copia de recuperación ya está ocupada");}
    snapshot(db_path,&backup_file)?;
    let restored=(||->Result<()> {
        // Covers are prepared and verified before SQLite is replaced.
        for name in &cover_names{
            let source=stage_covers.join(name);
            let target=covers_dir.join(name);
            if !target.exists() {
                let mut temporary=tempfile::NamedTempFile::new_in(covers_dir)?;
                std::io::copy(&mut File::open(&source)?,&mut temporary)?;
                temporary.persist_noclobber(&target)?;
            }
        }
        snapshot(&stage_db,db_path)?;
        // Run non-destructive schema upgrades for older exported databases.
        let mut catalog=crate::db::Catalog::open(db_path)?;
        // Keep the current machine's selected asset library, not an old PC's root.
        if let Some(root)=current_root.as_ref().filter(|root|root.is_dir()){
            catalog.configure_storage(root)?;
        }
        // Remap only covers physically bundled in this archive.
        let known:HashSet<&str>=cover_names.iter().map(String::as_str).collect();
        let db=Connection::open(db_path)?;
        let assets={
            let mut stmt=db.prepare("SELECT id,image_path FROM assets")?;
            let iter=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?;
            iter.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id,path) in assets{
            if let Some(name)=Path::new(&path).file_name().and_then(|n|n.to_str()){
                if known.contains(name) {
                    catalog.set_cover(id,&covers_dir.join(name).to_string_lossy())?;
                }
            }
        }
        Ok(())
    })();
    if let Err(error)=restored {
        // Attempt rollback of DB from the recovery snapshot; keep it on disk
        // if rollback itself fails so the user can restore it manually.
        let _=snapshot(&backup_file,db_path);
        return Err(error.context(format!(
            "Falló la restauración; copia de recuperación en {}",backup_file.display()
        )));
    }
    Ok(BackupInfo{covers:cover_names.len(),recovery:Some(backup_file)})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_export_import_restores_favorites_metadata_and_covers(){
        let tmp=tempfile::tempdir().unwrap();
        let db=tmp.path().join("catalog.sqlite");
        let covers=tmp.path().join("covers");
        let bundle=tmp.path().join("library.szkura");
        fs::create_dir_all(&covers).unwrap();
        let img=covers.join("manual-test.webp");
        fs::write(&img,b"image-fixture").unwrap();
        let mut catalog=crate::db::Catalog::open(&db).unwrap();
        let root=tmp.path().join("assets");
        catalog.configure_storage(&root).unwrap();
        let id=catalog.create_product("Favorito","Ropa").unwrap();
        catalog.set_favorite(id,true).unwrap();
        catalog.set_cover(id,&img.to_string_lossy()).unwrap();
        drop(catalog);
        let output=export_bundle(&db,&covers,&bundle).unwrap();
        assert_eq!(output.covers,1);
        let mut catalog=crate::db::Catalog::open(&db).unwrap();
        catalog.delete(id).unwrap();
        drop(catalog);
        fs::remove_file(&img).unwrap();
        let imported=import_bundle(&db,&covers,&bundle).unwrap();
        assert!(imported.recovery.unwrap().is_file());
        let catalog=crate::db::Catalog::open(&db).unwrap();
        assert!(catalog.get(id).unwrap().unwrap().favorite);
        assert!(img.is_file());
        assert_eq!(catalog.storage_root().unwrap(),Some(fs::canonicalize(&root).unwrap()));
    }
    #[test]
    fn restore_works_with_catalog_connection_open_as_in_gui(){
        let dir=tempfile::tempdir().unwrap();
        let db=dir.path().join("catalog.sqlite");
        let covers=dir.path().join("covers");
        let file=dir.path().join("live.szbooth");
        let mut live=crate::db::Catalog::open(&db).unwrap();
        let id=live.create_product("Original","Ropa").unwrap();
        export_bundle(&db,&covers,&file).unwrap();
        live.create_product("Later","Otros").unwrap();
        assert_eq!(live.count().unwrap(),2);
        // The GUI retains its original connection while a background worker
        // restores using a separate SQLite online-backup connection.
        let result=import_bundle(&db,&covers,&file).unwrap();
        assert!(result.recovery.unwrap().exists());
        drop(live);
        let fresh=crate::db::Catalog::open(&db).unwrap();
        assert_eq!(fresh.count().unwrap(),1);
        assert_eq!(fresh.get(id).unwrap().unwrap().title,"Original");
    }
    #[test]
    fn legacy_manifest_is_still_importable(){
        let temp=tempfile::tempdir().unwrap();
        let db=temp.path().join("catalog.sqlite");
        let modern=temp.path().join("new.szkura");
        let legacy=temp.path().join("old.szbooth");
        let mut catalog=crate::db::Catalog::open(&db).unwrap();
        let id=catalog.create_product("Old catalog","Otros").unwrap();
        drop(catalog);
        export_bundle(&db,&temp.path().join("covers"),&modern).unwrap();
        let mut source=ZipArchive::new(File::open(&modern).unwrap()).unwrap();
        let mut output=ZipWriter::new(File::create(&legacy).unwrap());
        let options=FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for n in 0..source.len(){
            let mut file=source.by_index(n).unwrap();
            output.start_file(file.name(),options).unwrap();
            if file.name()=="format.txt" {
                output.write_all(LEGACY_MANIFEST.as_bytes()).unwrap();
            }else{
                std::io::copy(&mut file,&mut output).unwrap();
            }
        }
        output.finish().unwrap();
        import_bundle(&db,&temp.path().join("covers"),&legacy).unwrap();
        assert_eq!(crate::db::Catalog::open(&db).unwrap().get(id).unwrap().unwrap().title,"Old catalog");
    }
    #[test]
    fn bad_archive_is_rejected_without_modifying_existing_database(){
        let tmp=tempfile::tempdir().unwrap();
        let db=tmp.path().join("db.sqlite");
        let mut catalog=crate::db::Catalog::open(&db).unwrap();
        catalog.create_product("Keep","Otros").unwrap();
        drop(catalog);
        let invalid=tmp.path().join("bad.szbooth");
        fs::write(&invalid,b"not a ZIP").unwrap();
        assert!(import_bundle(&db,&tmp.path().join("covers"),&invalid).is_err());
        assert_eq!(crate::db::Catalog::open(&db).unwrap().count().unwrap(),1);
    }
}
