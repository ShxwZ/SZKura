//! Non-destructive first-run migration from the legacy SZ_BOOTH data directory.
//! The old directory stays intact as a recovery copy, including any SQLite WAL.
use anyhow::{bail,Context,Result};
use rusqlite::{backup::Backup,Connection,OpenFlags,params};
use std::{fs,path::{Path,PathBuf},time::Duration};
use walkdir::WalkDir;

/// Copy a consistent SQLite snapshot and local resources to SZKura before
/// opening the new catalog. External linked assets are never copied or moved.
pub fn migrate_if_needed(legacy:&Path,destination:&Path)->Result<bool>{
    let old_database=legacy.join("catalog.sqlite");
    if destination.join("catalog.sqlite").exists() || !old_database.is_file(){
        return Ok(false);
    }
    if destination.exists() && fs::read_dir(destination)?.next().is_some(){
        bail!("La carpeta de SZKura ya contiene archivos: no se sobrescribieron datos existentes");
    }
    let parent=destination.parent().context("Destino SZKura sin carpeta superior")?;
    fs::create_dir_all(parent)?;
    let scratch=tempfile::Builder::new().prefix(".szkura-migrate-").tempdir_in(parent)?;
    let stage=scratch.path().join("data");
    fs::create_dir(&stage)?;
    // Copy without following symlinks and never duplicate live SQLite WAL/SHM:
    // the online backup below includes all committed transactions.
    for entry in WalkDir::new(legacy).follow_links(false) {
        let entry=entry?;
        let relative=entry.path().strip_prefix(legacy)?;
        if relative.as_os_str().is_empty(){continue;}
        if entry.file_type().is_symlink(){
            bail!("La carpeta anterior contiene un enlace simbólico: {}",entry.path().display());
        }
        if relative==Path::new("catalog.sqlite")
            ||relative==Path::new("catalog.sqlite-wal")
            ||relative==Path::new("catalog.sqlite-shm")
            ||relative==Path::new("catalog.sqlite-journal"){continue;}
        let target=stage.join(relative);
        if entry.file_type().is_dir(){
            fs::create_dir_all(&target)?;
        }else if entry.file_type().is_file(){
            if let Some(dir)=target.parent(){fs::create_dir_all(dir)?;}
            fs::copy(entry.path(),&target).with_context(||format!(
                "No se pudo copiar {}",entry.path().display()))?;
        }else{
            bail!("Tipo de archivo no admitido durante la migración");
        }
    }
    let source=Connection::open_with_flags(&old_database,OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut target=Connection::open(stage.join("catalog.sqlite"))?;
    {
        let backup=Backup::new(&source,&mut target)?;
        backup.run_to_completion(128,Duration::from_millis(10),None)?;
    }
    drop(source);
    relocate_internal_paths(&mut target,legacy,destination)?;
    let check:String=target.query_row("PRAGMA integrity_check",[],|r|r.get(0))?;
    if check!="ok"{bail!("La base de datos migrada no pasó la prueba de integridad: {check}");}
    target.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    drop(target);
    // Publish only once the full tree and rewritten catalog are valid. Keep
    // SZ_BOOTH untouched so users can recover if anything goes wrong later.
    if destination.exists(){fs::remove_dir(destination)?;}
    fs::rename(&stage,destination).with_context(||format!(
        "No se pudo activar la carpeta nueva {}",destination.display()))?;
    Ok(true)
}

fn remap_path(value:&str,old:&Path,old_canonical:&Path,new:&Path)->Option<PathBuf>{
    let path=Path::new(value);
    for base in [old_canonical,old]{
        if let Ok(relative)=path.strip_prefix(base){
            return Some(new.join(relative));
        }
    }
    None
}
fn has_column(conn:&Connection,table:&str,column:&str)->Result<bool>{
    // Only hard-coded, trusted identifiers are passed in by the caller.
    let mut stmt=conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names=stmt.query_map([],|r|r.get::<_,String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names.iter().any(|name|name==column))
}
fn rewrite_column(
    conn:&Connection,table:&str,column:&str,old:&Path,canonical:&Path,new:&Path,
)->Result<()>{
    if !has_column(conn,table,column)?{return Ok(());}
    let rows={
        let mut stmt=conn.prepare(&format!("SELECT rowid,{column} FROM {table}"))?;
        let rows=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id,value) in rows{
        if let Some(replacement)=remap_path(&value,old,canonical,new){
            let path=replacement.to_str().context("Ruta de migración no válida en UTF-8")?;
            conn.execute(&format!("UPDATE {table} SET {column}=?1 WHERE rowid=?2"),
                params![path,id])?;
        }
    }
    Ok(())
}
fn relocate_internal_paths(db:&mut Connection,old:&Path,new:&Path)->Result<()>{
    let canonical=fs::canonicalize(old)?;
    let tx=db.transaction()?;
    for (table,column) in [
        ("assets","image_path"),("assets","archive_path"),
        ("attachments","stored_path"),("managed_folders","folder_path"),
        ("zip_inspection_cache","stored_path"),("library_settings","value")
    ]{
        rewrite_column(&tx,table,column,old,&canonical,new)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::db::Catalog;
    #[test]
    fn first_run_migrates_covers_and_internal_paths_without_touching_originals(){
        let temp=tempfile::tempdir().unwrap();
        let old=temp.path().join("SZ_BOOTH");
        let new=temp.path().join("SZKura");
        fs::create_dir_all(old.join("covers")).unwrap();
        fs::create_dir_all(old.join("library")).unwrap();
        let cover=old.join("covers/example.webp");
        let archive=old.join("library/test.zip");
        fs::write(&cover,b"cover").unwrap();
        fs::write(&archive,b"not a zip").unwrap();
        let mut db=Catalog::open(&old.join("catalog.sqlite")).unwrap();
        let id=db.create_product("Example","Otros").unwrap();
        db.set_cover(id,cover.to_str().unwrap()).unwrap();
        db.configure_storage(&old.join("library")).unwrap();
        db.set_language_id("en").unwrap();
        drop(db);
        let absolute=fs::canonicalize(&archive).unwrap();
        let conn=Connection::open(old.join("catalog.sqlite")).unwrap();
        conn.execute("INSERT INTO attachments(asset_id,original_name,stored_path,checksum,bytes,file_count,storage_mode)
          VALUES(?1,'test.zip',?2,'test-checksum',10,0,'copy')",
          params![id,absolute.to_str().unwrap()]).unwrap();
        drop(conn);
        assert!(migrate_if_needed(&old,&new).unwrap());
        let catalog=Catalog::open(&new.join("catalog.sqlite")).unwrap();
        assert_eq!(catalog.language_id().unwrap(),"en");
        assert_eq!(catalog.get(id).unwrap().unwrap().image_path,
            new.join("covers/example.webp").to_str().unwrap());
        assert_eq!(catalog.storage_root().unwrap().unwrap(),new.join("library"));
        let attachments=catalog.attachments_for(id).unwrap();
        assert_eq!(Path::new(&attachments[0].stored_path),new.join("library/test.zip").as_path());
        assert!(new.join("library/test.zip").exists());
        assert!(old.join("library/test.zip").exists());
        assert!(!migrate_if_needed(&old,&new).unwrap());
    }
    #[test]
    fn existing_szkura_catalog_is_never_overwritten(){
        let dir=tempfile::tempdir().unwrap();
        let old=dir.path().join("SZ_BOOTH");
        let new=dir.path().join("SZKura");
        let mut original=Catalog::open(&old.join("catalog.sqlite")).unwrap();
        original.create_product("Old","Otros").unwrap();
        let mut existing=Catalog::open(&new.join("catalog.sqlite")).unwrap();
        existing.create_product("New","Ropa").unwrap();
        drop(original);drop(existing);
        assert!(!migrate_if_needed(&old,&new).unwrap());
        assert_eq!(Catalog::open(&new.join("catalog.sqlite")).unwrap().count().unwrap(),1);
    }
    #[test]
    fn unexpected_destination_content_blocks_migration(){
        let dir=tempfile::tempdir().unwrap();
        let old=dir.path().join("SZ_BOOTH");
        let new=dir.path().join("SZKura");
        Catalog::open(&old.join("catalog.sqlite")).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("keep.txt"),b"keep").unwrap();
        assert!(migrate_if_needed(&old,&new).is_err());
        assert!(old.join("catalog.sqlite").exists());
        assert_eq!(fs::read(new.join("keep.txt")).unwrap(),b"keep");
    }
}
