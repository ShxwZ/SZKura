//! Product-centric catalog. SQLite schema upgrades preserve alpha.3 libraries.
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub const CATEGORIES: &[&str] = &["Avatar", "Ropa", "Accesorios", "Pelo", "Texturas", "Animaciones", "Mundos", "Herramientas", "Otros"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFilter { All, WithFiles, WithoutFiles }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveMode { Reference, Copy }
impl ArchiveMode {
    pub fn as_str(self) -> &'static str { match self { Self::Reference => "reference", Self::Copy => "copy" } }
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub id: i64,
    pub title: String,
    pub category: String,
    pub tags: String,
    pub avatars: String,
    pub creator: String,
    pub description: String,
    pub booth_url: String,
    pub image_path: String,
    pub added_at: i64,
    pub attachments: i64,
    pub total_bytes: i64,
    pub favorite: bool,
    pub nsfw: bool,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub id: i64,
    pub asset_id: i64,
    pub original_name: String,
    pub stored_path: String,
    pub checksum: String,
    pub bytes: i64,
    pub file_count: i64,
    pub mode: ArchiveMode,
}

pub struct NewAttachment<'a> {
    pub asset_id: i64,
    pub name: &'a str,
    pub path: &'a str,
    pub checksum: &'a str,
    pub bytes: i64,
    pub count: i64,
    pub mode: ArchiveMode,
}

pub struct Catalog { conn: Connection }

impl Catalog {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("No se pudo crear {}", parent.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("No se pudo abrir {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS assets (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT 'Otros',
                tags TEXT NOT NULL DEFAULT '',
                creator TEXT NOT NULL DEFAULT '',
                description TEXT NOT NULL DEFAULT '',
                booth_url TEXT NOT NULL DEFAULT '',
                image_path TEXT NOT NULL DEFAULT '',
                archive_path TEXT NOT NULL DEFAULT '',
                original_name TEXT NOT NULL DEFAULT '',
                checksum TEXT NOT NULL DEFAULT '',
                archive_bytes INTEGER NOT NULL DEFAULT 0,
                file_count INTEGER NOT NULL DEFAULT 0,
                added_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX IF NOT EXISTS assets_category_added ON assets(category, added_at DESC);
             CREATE INDEX IF NOT EXISTS assets_added ON assets(added_at DESC);
             CREATE INDEX IF NOT EXISTS assets_booth_url ON assets(booth_url) WHERE booth_url <> '';
             CREATE TABLE IF NOT EXISTS attachments (
                id INTEGER PRIMARY KEY,
                asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
                original_name TEXT NOT NULL,
                stored_path TEXT NOT NULL,
                checksum TEXT NOT NULL,
                bytes INTEGER NOT NULL DEFAULT 0,
                file_count INTEGER NOT NULL DEFAULT 0,
                storage_mode TEXT NOT NULL CHECK (storage_mode IN ('reference','copy')),
                added_at INTEGER NOT NULL DEFAULT (unixepoch()),
                UNIQUE(asset_id,checksum)
             );
             CREATE INDEX IF NOT EXISTS attachments_asset ON attachments(asset_id);
             CREATE INDEX IF NOT EXISTS attachments_checksum ON attachments(checksum);
             CREATE TABLE IF NOT EXISTS favorite_assets(asset_id INTEGER PRIMARY KEY REFERENCES assets(id) ON DELETE CASCADE);
             -- Opt-in per-product content flag. Legacy products remain unflagged.
             CREATE TABLE IF NOT EXISTS asset_content_flags (
                 asset_id INTEGER PRIMARY KEY REFERENCES assets(id) ON DELETE CASCADE,
                 nsfw INTEGER NOT NULL CHECK(nsfw=1)
             );
             CREATE TABLE IF NOT EXISTS library_settings (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             -- Each product can have folders in several storage roots over time.
             CREATE TABLE IF NOT EXISTS managed_folders (
                 id INTEGER PRIMARY KEY,
                 asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
                 folder_path TEXT NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 UNIQUE(asset_id,folder_path)
             );
             CREATE INDEX IF NOT EXISTS managed_folders_asset ON managed_folders(asset_id);

             -- Persistent ZIP metadata only; no extracted bytes or ZIP contents.
             -- The inspector version invalidates stale layouts after upgrades.
             CREATE TABLE IF NOT EXISTS zip_inspection_cache (
                 stored_path TEXT PRIMARY KEY,
                 size_bytes INTEGER NOT NULL,
                 modified_ns TEXT NOT NULL,
                 content_hash TEXT NOT NULL,
                 inspector_version INTEGER NOT NULL,
                 preview_json TEXT NOT NULL,
                 updated_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX IF NOT EXISTS zip_inspection_cache_hash
                 ON zip_inspection_cache(content_hash,inspector_version);
              -- Separate compatibility tags, indexed for fast filter intersection.
              CREATE TABLE IF NOT EXISTS asset_avatars (
                  asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
                  avatar_name TEXT NOT NULL COLLATE NOCASE,
                  PRIMARY KEY(asset_id, avatar_name)
              ) WITHOUT ROWID;
              CREATE INDEX IF NOT EXISTS asset_avatars_name ON asset_avatars(avatar_name, asset_id);
             -- Optional source URL, never a password or authentication token.
             CREATE TABLE IF NOT EXISTS asset_sources (
                 asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
                 provider TEXT NOT NULL CHECK(provider IN ('workupload')),
                 url TEXT NOT NULL,
                 PRIMARY KEY(asset_id,provider)
             ) WITHOUT ROWID;
             CREATE VIRTUAL TABLE IF NOT EXISTS assets_fts USING fts5(
                 title, category, creator, tags, description,
                 content='assets', content_rowid='id', tokenize='unicode61 remove_diacritics 2'
             );
             CREATE TRIGGER IF NOT EXISTS assets_ai AFTER INSERT ON assets BEGIN
                 INSERT INTO assets_fts(rowid,title,category,creator,tags,description)
                 VALUES(new.id,new.title,new.category,new.creator,new.tags,new.description);
             END;
             CREATE TRIGGER IF NOT EXISTS assets_ad AFTER DELETE ON assets BEGIN
                 INSERT INTO assets_fts(assets_fts,rowid,title,category,creator,tags,description)
                 VALUES('delete',old.id,old.title,old.category,old.creator,old.tags,old.description);
             END;
             CREATE TRIGGER IF NOT EXISTS assets_au AFTER UPDATE ON assets BEGIN
                 INSERT INTO assets_fts(assets_fts,rowid,title,category,creator,tags,description)
                 VALUES('delete',old.id,old.title,old.category,old.creator,old.tags,old.description);
                 INSERT INTO assets_fts(rowid,title,category,creator,tags,description)
                 VALUES(new.id,new.title,new.category,new.creator,new.tags,new.description);
             END;
             -- Idempotent migration: old releases stored one ZIP directly in assets.
             INSERT OR IGNORE INTO attachments(asset_id,original_name,stored_path,checksum,bytes,file_count,storage_mode)
                 SELECT id, original_name, archive_path, checksum, archive_bytes, file_count, 'copy'
                 FROM assets WHERE archive_path <> '' AND checksum <> '';
             UPDATE assets SET archive_path='',original_name='',checksum='',archive_bytes=0,file_count=0
                 WHERE archive_path<>'' AND checksum<>'';"
        )?;
        Ok(Self { conn })
    }

    /// Persist update settings with an explicit key allowlist.
    pub fn update_preference(&mut self,key:&str,enabled:bool)->Result<()> {
        if !matches!(key,"check_updates_on_start"|"auto_install_updates"|"delete_sources_after_import"|"sharing_enabled") {
            bail!("Clave de configuración desconocida");
        }
        self.conn.execute(
            "INSERT INTO library_settings(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key,if enabled{"1"}else{"0"}])?;
        Ok(())
    }
    pub fn preference(&self,key:&str,default:bool)->Result<bool> {
        if !matches!(key,"check_updates_on_start"|"auto_install_updates"|"delete_sources_after_import"|"sharing_enabled") {
            bail!("Clave de configuración desconocida");
        }
        let value=self.conn.query_row(
            "SELECT value FROM library_settings WHERE key=?1",[key],
            |r|r.get::<_,String>(0)
        ).optional()?;
        Ok(value.map(|v|v=="1").unwrap_or(default))
    }

    /// Palette is a standalone allowlisted text preference, not a boolean.
    /// Invalid/corrupt values safely fall back to the original palette.
    pub fn palette_id(&self)->Result<String>{
        let value=self.conn.query_row(
            "SELECT value FROM library_settings WHERE key='ui_palette'",
            [],|r|r.get::<_,String>(0)
        ).optional()?;
        Ok(match value.as_deref(){
            Some("graphite")=>"graphite",Some("purple")=>"purple",
            Some("orange")=>"orange",_=>"sage"
        }.to_owned())
    }
    pub fn set_palette_id(&mut self,id:&str)->Result<()> {
        if !matches!(id,"graphite"|"sage"|"purple"|"orange"){
            bail!("Paleta desconocida");
        }
        self.conn.execute(
            "INSERT INTO library_settings(key,value) VALUES('ui_palette',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",[id]
        )?;
        Ok(())
    }

    /// User interface language is stored alongside palette and storage settings.
    /// Missing preferences keep Spanish for existing installations. Never
    /// translate stored category values, archive paths, or product data.
    pub fn language_id(&self)->Result<String> {
        let value=self.conn.query_row(
            "SELECT value FROM library_settings WHERE key='ui_language'",
            [],|row|row.get::<_,String>(0)
        ).optional()?;
        Ok(if value.as_deref()==Some("en"){"en"}else{"es"}.to_owned())
    }
    pub fn set_language_id(&mut self,id:&str)->Result<()> {
        if !matches!(id,"es"|"en"){bail!("Unsupported UI language");}
        self.conn.execute(
            "INSERT INTO library_settings(key,value) VALUES('ui_language',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",[id]
        )?;
        Ok(())
    }

    pub fn storage_root(&self)->Result<Option<std::path::PathBuf>> {
        let value=self.conn.query_row(
            "SELECT value FROM library_settings WHERE key='storage_root'",
            [],|r|r.get::<_,String>(0)
        ).optional()?;
        Ok(value.filter(|s|!s.is_empty()).map(std::path::PathBuf::from))
    }

    /// Users select this root once before any new product/attachment can be added.
    /// Existing referenced paths are deliberately left untouched.
    pub fn configure_storage(&mut self,selected:&Path)->Result<std::path::PathBuf> {
        std::fs::create_dir_all(selected)
            .with_context(||format!("No se pudo crear {}",selected.display()))?;
        let root=std::fs::canonicalize(selected)?;
        if !root.is_dir(){bail!("La ruta elegida no es una carpeta");}
        let value=root.to_str().context("La ruta debe ser válida en UTF-8")?;
        self.conn.execute(
            "INSERT INTO library_settings(key,value) VALUES('storage_root',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
             [value]
        )?;
        Ok(root)
    }

    /// Track the real product folders, even after changing the configured root.
    pub fn register_managed_folder(&mut self,id:i64,folder:&Path)->Result<()> {
        if self.get(id)?.is_none(){bail!("No existe el producto de destino");}
        self.conn.execute("INSERT OR IGNORE INTO managed_folders(asset_id,folder_path)
            VALUES(?1,?2)",params![id,folder.to_string_lossy().as_ref()])?;
        Ok(())
    }
    pub fn managed_folders(&self,id:i64)->Result<Vec<String>> {
        let mut statement=self.conn.prepare(
            "SELECT folder_path FROM managed_folders WHERE asset_id=?1 ORDER BY id"
        )?;
        let folders=statement.query_map([id],|row|row.get::<_,String>(0))?;
        Ok(folders.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn is_file_referenced(&self,path:&Path)->Result<bool> {
        let name=path.to_string_lossy();
        let exists=self.conn.query_row(
            "SELECT 1 FROM attachments WHERE stored_path=?1 LIMIT 1",
            [name.as_ref()],|_|Ok(())
        ).optional()?.is_some();
        Ok(exists)
    }
    pub fn all_attachments(&self)->Result<Vec<Attachment>> {
        let mut stmt=self.conn.prepare(
            "SELECT id,asset_id,original_name,stored_path,checksum,bytes,file_count,storage_mode
             FROM attachments ORDER BY id"
        )?;
        let values=stmt.query_map([],|r|{
            let mode:String=r.get(7)?;
            Ok(Attachment{id:r.get(0)?,asset_id:r.get(1)?,original_name:r.get(2)?,
                stored_path:r.get(3)?,checksum:r.get(4)?,bytes:r.get(5)?,file_count:r.get(6)?,
                mode:if mode=="copy"{ArchiveMode::Copy}else{ArchiveMode::Reference}})
        })?;
        Ok(values.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    /// Relink an existing attachment after copying+verifying it to managed storage.
    pub fn relocate_attachment(&mut self,attachment_id:i64,asset_id:i64,target:&Path,
        checksum:&str)->Result<bool> {
        Ok(self.conn.execute(
            "UPDATE attachments SET stored_path=?1,storage_mode='copy'
             WHERE id=?2 AND asset_id=?3 AND checksum=?4",
            params![target.to_string_lossy().as_ref(),attachment_id,asset_id,checksum],
        )?==1)
    }

    /// NSFW is an explicit user label, not automatic detection from a product title.
    pub fn set_nsfw(&mut self,id:i64,nsfw:bool)->Result<()> {
        if self.get(id)?.is_none() { bail!("Producto no encontrado"); }
        if nsfw {
            self.conn.execute(
                "INSERT OR IGNORE INTO asset_content_flags(asset_id,nsfw) VALUES(?1,1)",
                [id]
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM asset_content_flags WHERE asset_id=?1",[id]
            )?;
        }
        Ok(())
    }

    pub fn set_favorite(&mut self,id:i64,favorite:bool)->Result<()> {
        if self.get(id)?.is_none() { bail!("Producto no encontrado"); }
        if favorite {
            self.conn.execute("INSERT OR IGNORE INTO favorite_assets(asset_id) VALUES(?1)",[id])?;
        } else {
            self.conn.execute("DELETE FROM favorite_assets WHERE asset_id=?1",[id])?;
        }
        Ok(())
    }
    pub fn create_product(&mut self, title: &str, category: &str) -> Result<i64> {
        if title.trim().is_empty() { bail!("El producto necesita un título"); }
        self.conn.execute("INSERT INTO assets(title,category) VALUES(?1,?2)", params![title.trim(),category])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get(&self, id: i64) -> Result<Option<Asset>> {
        let sql = "SELECT a.id,a.title,a.category,a.tags,a.creator,a.description,a.booth_url,a.image_path,a.added_at,
                (SELECT COUNT(*) FROM attachments att WHERE att.asset_id=a.id),
                COALESCE((SELECT SUM(bytes) FROM attachments att WHERE att.asset_id=a.id),0),
                COALESCE((SELECT group_concat(avatar_name, ', ') FROM asset_avatars av WHERE av.asset_id=a.id),''),
                EXISTS(SELECT 1 FROM favorite_assets fav WHERE fav.asset_id=a.id),
                EXISTS(SELECT 1 FROM asset_content_flags sf WHERE sf.asset_id=a.id)
             FROM assets a WHERE a.id=?1";
        Ok(self.conn.query_row(sql,[id],|r| {
            Ok(Asset { id:r.get(0)?,title:r.get(1)?,category:r.get(2)?,tags:r.get(3)?,avatars:Self::normalize_avatar_summary(&r.get::<_,String>(11)?),creator:r.get(4)?,description:r.get(5)?,booth_url:r.get(6)?,image_path:r.get(7)?,added_at:r.get(8)?,attachments:r.get(9)?,total_bytes:r.get(10)?,favorite:r.get::<_,i64>(12)?!=0,nsfw:r.get::<_,i64>(13)?!=0 })
        }).optional()?)
    }

    // Resolve a numeric product ID independently of its BOOTH shop domain.
    // A shop URL and a generic BOOTH URL point at the same product, and must
    // not create two cards or replace ZIP attachments.
    fn item_suffix(url:&str)->Option<&str>{
        let (_,id)=url.rsplit_once("/items/")?;
        if !id.is_empty() && id.bytes().all(|b|b.is_ascii_digit()){Some(id)}
        else{None}
    }
    fn linked_item(&self,url:&str,ignore_id:i64)->Result<Option<(i64,String)>>{
        let Some(id)=Self::item_suffix(url)else{return Ok(None);};
        let suffix=format!("%/items/{id}");
        Ok(self.conn.query_row(
            "SELECT id, booth_url FROM assets WHERE booth_url LIKE ?1 AND id<>?2 LIMIT 1",
            params![suffix,ignore_id],
            |r|Ok((r.get(0)?,r.get(1)?))
        ).optional()?)
    }
    fn shop_specific(url:&str)->bool{
        url::Url::parse(url).ok()
            .and_then(|u|u.host_str().map(str::to_owned))
            .is_some_and(|h|h!="booth.pm" && h.ends_with(".booth.pm"))
    }

    /// Check whether the BOOTH item already exists before creating/updating
    /// it. Used to avoid replacing a manually chosen category on re-import.
    pub fn existing_booth_link(&self,url:&str)->Result<Option<i64>>{
        Ok(self.linked_item(url,-1)?.map(|(id,_)|id))
    }

    pub fn add_booth_link(&mut self,url:&str)->Result<i64>{
        if let Some((id,existing))=self.linked_item(url,-1)?{
            // Upgrade legacy generic URLs to the real shop-specific URL.
            if Self::shop_specific(url) && !Self::shop_specific(&existing){
                self.conn.execute("UPDATE assets SET booth_url=?1 WHERE id=?2",
                    params![url,id])?;
            }
            return Ok(id);
        }
        let title=url.rsplit('/').next()
            .map(|x|format!("BOOTH - {x}")).unwrap_or_else(||"Producto BOOTH".into());
        self.conn.execute("INSERT INTO assets(title,booth_url) VALUES (?1,?2)",
            params![title,url])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn associate_booth(&mut self,id:i64,url:&str)->Result<()>{
        if let Some((other,_))=self.linked_item(url,id)?{
            bail!("Ese producto BOOTH ya está en la ficha #{other}; abre esa ficha para añadir el ZIP");
        }
        let existing:Option<String>=self.conn.query_row(
            "SELECT booth_url FROM assets WHERE id=?1",[id],|r|r.get(0)
        ).optional()?;
        let Some(existing)=existing else{bail!("Producto no encontrado");};
        let chosen=if Self::item_suffix(&existing)==Self::item_suffix(url)
            && Self::shop_specific(&existing) && !Self::shop_specific(url){
            existing.as_str()
        }else{url};
        self.conn.execute("UPDATE assets SET booth_url=?1 WHERE id=?2",
            params![chosen,id])?;
        Ok(())
    }

    /// Never overwrite a creator/shop manually provided by the user.
    /// Keep the original source link for user-authorized downloads.
    /// Passwords are intentionally never stored or logged.
    pub fn set_workupload_url(&mut self,id:i64,url:&str)->Result<()> {
        if self.get(id)?.is_none(){bail!("El producto no existe");}
        if !url.is_empty(){
            let normalized=crate::quick_import::workupload_url(url)?;
            self.conn.execute(
                "INSERT INTO asset_sources(asset_id,provider,url) VALUES(?1,'workupload',?2)
                 ON CONFLICT(asset_id,provider) DO UPDATE SET url=excluded.url",
                params![id,normalized]
            )?;
        }else{
            self.conn.execute(
                "DELETE FROM asset_sources WHERE asset_id=?1 AND provider='workupload'",
                [id]
            )?;
        }
        Ok(())
    }

    pub fn workupload_url_for(&self,id:i64)->Result<String> {
        Ok(self.conn.query_row(
            "SELECT url FROM asset_sources WHERE asset_id=?1 AND provider='workupload'",
            [id],|r|r.get::<_,String>(0)
        ).optional()?.unwrap_or_default())
    }

    pub fn fill_creator_if_blank(&mut self,id:i64,creator:&str)->Result<()>{
        if creator.trim().is_empty(){return Ok(());}
        self.conn.execute(
            "UPDATE assets SET creator=?1 WHERE id=?2 AND trim(creator)=''",
            params![creator.trim(),id]
        )?;
        Ok(())
    }

    pub fn set_booth_url(&mut self, id:i64, url:&str)->Result<()> {
        if !url.is_empty() { return self.associate_booth(id,url); }
        let affected=self.conn.execute("UPDATE assets SET booth_url='' WHERE id=?1",[id])?;
        if affected==0 { bail!("Producto no encontrado"); }
        Ok(())
    }

    pub fn update_metadata(&mut self, id: i64, url: &str, title: &str, description: &str, image_path: &str) -> Result<()> {
        self.associate_booth(id,url)?;
        self.conn.execute(
            "UPDATE assets SET title=?1, description=?2,
                image_path=CASE WHEN ?3<>'' THEN ?3 ELSE image_path END WHERE id=?4",
            params![title,description,image_path,id],
        )?;
        Ok(())
    }

    /// Use a local cover when BOOTH cannot provide an image. Keeps the product and ZIPs intact.
    pub fn set_cover(&mut self, id: i64, image_path: &str) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE assets SET image_path=?1 WHERE id=?2", params![image_path, id]
        )?;
        if affected == 0 { bail!("No existe ese producto"); }
        Ok(())
    }

    pub fn update_details(&mut self, id: i64, title: &str, category: &str, creator: &str, tags: &str, description: &str) -> Result<()> {
        if title.trim().is_empty() { bail!("El nombre no puede estar vacío"); }
        if !CATEGORIES.contains(&category) { bail!("Categoría desconocida"); }
        self.conn.execute(
            "UPDATE assets SET title=?1,category=?2,creator=?3,tags=?4,description=?5 WHERE id=?6",
            params![title.trim(),category,creator.trim(),tags.trim(),description.trim(),id],
        )?;
        Ok(())
    }

    /// Canonical display form for both new and pre-existing avatar labels.
    /// Non-Latin scripts without letter case (for example Japanese) are unchanged.
    fn normalize_avatar_names(raw:&str)->Vec<String> {
        let mut out:Vec<String>=Vec::new();
        for value in raw.split([',','\n',';']) {
            let name=value.trim().to_uppercase();
            if name.is_empty(){continue;}
            if !out.contains(&name){out.push(name);}
        }
        out
    }
    fn normalize_avatar_summary(raw:&str)->String{
        Self::normalize_avatar_names(raw).join(", ")
    }

    /// Stores comma/newline-separated avatars in Unicode uppercase.
    /// All changes commit atomically.
    pub fn set_avatars(&mut self, asset_id: i64, input: &str) -> Result<()> {
        let parsed=Self::normalize_avatar_names(input);
        if parsed.len()>64 { bail!("Máximo 64 avatares por producto"); }
        if parsed.iter().any(|value|value.chars().count()>70){
            bail!("Nombre de avatar demasiado largo (máximo 70 caracteres)");
        }
        if self.get(asset_id)?.is_none() { bail!("El producto no existe"); }
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM asset_avatars WHERE asset_id=?1", [asset_id])?;
        {
            let mut statement = tx.prepare(
                "INSERT INTO asset_avatars(asset_id,avatar_name) VALUES(?1,?2)"
            )?;
            for name in parsed { statement.execute(params![asset_id, name])?; }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn avatars_for(&self, asset_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT avatar_name FROM asset_avatars WHERE asset_id=?1 ORDER BY avatar_name COLLATE NOCASE"
        )?;
        let rows = stmt.query_map([asset_id], |r| r.get::<_,String>(0))?;
        let values = rows.collect::<rusqlite::Result<Vec<String>>>()?;
        let mut normalized:Vec<String>=values.into_iter()
            .flat_map(|v|Self::normalize_avatar_names(&v)).collect();
        normalized.sort();
        normalized.dedup();
        Ok(normalized)
    }

    /// Existing names populate the filter; no hard-coded avatar list.
    pub fn avatar_options(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT avatar_name FROM asset_avatars ORDER BY avatar_name COLLATE NOCASE LIMIT 512"
        )?;
        let rows = stmt.query_map([], |r| r.get::<_,String>(0))?;
        let values = rows.collect::<rusqlite::Result<Vec<String>>>()?;
        let mut normalized:Vec<String>=values.into_iter()
            .flat_map(|v|Self::normalize_avatar_names(&v)).collect();
        normalized.sort();
        normalized.dedup();
        normalized.truncate(512);
        Ok(normalized)
    }

    pub fn exists_checksum(&self, checksum: &str) -> Result<bool> {
        Ok(self.conn.query_row("SELECT 1 FROM attachments WHERE checksum=?1 LIMIT 1", [checksum], |_| Ok(())).optional()?.is_some())
    }
    pub fn asset_has_checksum(&self, id: i64, checksum: &str) -> Result<bool> {
        Ok(self.conn.query_row("SELECT 1 FROM attachments WHERE asset_id=?1 AND checksum=?2", params![id,checksum], |_| Ok(())).optional()?.is_some())
    }
    pub fn add_attachment(&mut self, a: &NewAttachment<'_>) -> Result<bool> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO attachments(asset_id,original_name,stored_path,checksum,bytes,file_count,storage_mode)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![a.asset_id,a.name,a.path,a.checksum,a.bytes,a.count,a.mode.as_str()],
        )?;
        Ok(inserted > 0)
    }
    /// Fast path: unchanged path + file size + modification timestamp.
    /// The snapshot was originally verified with a full BLAKE3 file hash.
    pub fn cached_zip_preview_by_file(
        &self,path:&str,size:u64,modified_ns:&str,version:i64
    )->Result<Option<String>> {
        let size=i64::try_from(size)?;
        Ok(self.conn.query_row(
            "SELECT preview_json FROM zip_inspection_cache
             WHERE stored_path=?1 AND size_bytes=?2 AND modified_ns=?3
                   AND inspector_version=?4
                   AND length(preview_json)<=8388608",
            params![path,size,modified_ns,version],|row|row.get(0)
        ).optional()?)
    }
    /// Reuse the expensive nested scan if a moved/replaced file has the same hash.
    pub fn cached_zip_preview_by_hash(
        &self,hash:&str,version:i64
    )->Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT preview_json FROM zip_inspection_cache
             WHERE content_hash=?1 AND inspector_version=?2
                   AND length(preview_json)<=8388608
             ORDER BY updated_at DESC LIMIT 1",
            params![hash,version],|row|row.get(0)
        ).optional()?)
    }
    /// Cache is intentionally bounded: at most 128 snapshots / 64 MiB JSON.
    /// The original ZIPs and the product/attachment records are never modified.
    pub fn store_zip_preview(
        &mut self,path:&str,size:u64,modified_ns:&str,hash:&str,
        version:i64,preview_json:&str
    )->Result<()> {
        if preview_json.len()>8*1024*1024 {
            bail!("La vista previa excede el límite de caché de 8 MiB");
        }
        let size=i64::try_from(size)?;
        let tx=self.conn.transaction()?;
        tx.execute(
            "INSERT INTO zip_inspection_cache(
                 stored_path,size_bytes,modified_ns,content_hash,inspector_version,preview_json,updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,unixepoch())
             ON CONFLICT(stored_path) DO UPDATE SET
                size_bytes=excluded.size_bytes,
                modified_ns=excluded.modified_ns,
                content_hash=excluded.content_hash,
                inspector_version=excluded.inspector_version,
                preview_json=excluded.preview_json,
                updated_at=unixepoch()",
            params![path,size,modified_ns,hash,version,preview_json],
        )?;
        tx.execute(
            "DELETE FROM zip_inspection_cache WHERE rowid IN
             (SELECT rowid FROM zip_inspection_cache
              ORDER BY updated_at DESC,rowid DESC LIMIT -1 OFFSET 128)",
            [],
        )?;
        tx.execute(
            "DELETE FROM zip_inspection_cache WHERE rowid IN
             (SELECT rowid FROM (
                SELECT rowid,
                    SUM(length(preview_json)) OVER
                      (ORDER BY updated_at DESC,rowid DESC) AS cumulative_bytes
                FROM zip_inspection_cache
              ) WHERE cumulative_bytes > 67108864)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn attachments_for(&self, id: i64) -> Result<Vec<Attachment>> {
        let mut stmt = self.conn.prepare("SELECT id,asset_id,original_name,stored_path,checksum,bytes,file_count,storage_mode FROM attachments WHERE asset_id=?1 ORDER BY id DESC")?;
        let rows = stmt.query_map([id], |r| {
            let mode: String = r.get(7)?;
            Ok(Attachment { id:r.get(0)?,asset_id:r.get(1)?,original_name:r.get(2)?,stored_path:r.get(3)?,checksum:r.get(4)?,bytes:r.get(5)?,file_count:r.get(6)?,mode:if mode=="copy" {ArchiveMode::Copy} else {ArchiveMode::Reference} })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn detach(&mut self, id: i64, asset_id: i64) -> Result<()> {
        // Only delete database relation, never the original/managed file.
        self.conn.execute("DELETE FROM attachments WHERE id=?1 AND asset_id=?2",params![id,asset_id])?;
        Ok(())
    }
    pub fn delete(&mut self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM assets WHERE id=?1",[id])?;
        Ok(())
    }
    pub fn count(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM assets",[],|r|r.get(0))?)
    }
    fn fts_query(search: &str) -> String {
        search.split_whitespace().take(12)
            .map(|word| format!("\"{}\"*", word.replace('"', "\"\"")))
            .collect::<Vec<_>>().join(" AND ")
    }
    fn predicate(query: &str, filter: FileFilter) -> String {
        let mut pieces = vec!["(?1='' OR a.category=?1)".to_string(), "(?2 IS NOT NULL)".to_string()];
        if !query.is_empty() { pieces.push("a.id IN (SELECT rowid FROM assets_fts WHERE assets_fts MATCH ?2)".into()); }
        match filter {
            FileFilter::All => {},
            FileFilter::WithFiles => pieces.push("EXISTS(SELECT 1 FROM attachments att WHERE att.asset_id=a.id)".into()),
            FileFilter::WithoutFiles => pieces.push("NOT EXISTS(SELECT 1 FROM attachments att WHERE att.asset_id=a.id)".into()),
        }
        pieces.push("(?3='' OR EXISTS(SELECT 1 FROM asset_avatars va WHERE va.asset_id=a.id AND va.avatar_name=?3 COLLATE NOCASE))".into());
        pieces.join(" AND ")
    }
    /// Backwards-compatible query wrappers.
    pub fn count_filtered(&self, search: &str, category: &str, files: FileFilter) -> Result<i64> {
        self.count_filtered_avatar(search, category, "", files)
    }
    pub fn count_filtered_avatar(&self, search: &str, category: &str, avatar: &str, files: FileFilter) -> Result<i64> {
        self.count_filtered_favorites(search,category,avatar,files,false)
    }
    pub fn count_filtered_favorites(&self, search:&str,category:&str,avatar:&str,
        files:FileFilter,favorites_only:bool)->Result<i64> {
        self.count_filtered_visibility(search,category,avatar,files,favorites_only,false)
    }
    /// Filter inside SQL before paging, so hidden products do not leak into
    /// search/favorite counts or exhaust the visible page size.
    pub fn count_filtered_visibility(&self, search:&str,category:&str,avatar:&str,
        files:FileFilter,favorites_only:bool,show_nsfw:bool)->Result<i64> {
        let query=Self::fts_query(search);
        let pred=Self::predicate(&query,files);
        let sql=format!("SELECT COUNT(*) FROM assets a WHERE {pred}
            AND (?4=0 OR EXISTS(SELECT 1 FROM favorite_assets fav WHERE fav.asset_id=a.id))
            AND (?5=1 OR NOT EXISTS(SELECT 1 FROM asset_content_flags sf WHERE sf.asset_id=a.id))");
        Ok(self.conn.query_row(&sql,
            params![category,query,avatar,i64::from(favorites_only),i64::from(show_nsfw)],
            |r|r.get(0))?)
    }
    /// Display statistics for the complete filtered catalog, not just the
    /// currently loaded page. Count a physical archive only once when the same
    /// stored_path is linked to multiple products. This measures linked file
    /// bytes (including referenced files), not cover cache or SQLite disk usage.
    pub fn stats_filtered_visibility(&self, search:&str,category:&str,avatar:&str,
        files:FileFilter,favorites_only:bool,show_nsfw:bool)->Result<(i64,i64)> {
        let query=Self::fts_query(search);
        let pred=Self::predicate(&query,files);
        let sql=format!(
            "WITH visible AS (
                SELECT a.id FROM assets a WHERE {pred}
                  AND (?4=0 OR EXISTS(SELECT 1 FROM favorite_assets fav WHERE fav.asset_id=a.id))
                  AND (?5=1 OR NOT EXISTS(SELECT 1 FROM asset_content_flags sf WHERE sf.asset_id=a.id))
             )
             SELECT
                (SELECT COUNT(*) FROM visible),
                COALESCE((
                    SELECT SUM(file_bytes) FROM (
                        SELECT MAX(att.bytes) AS file_bytes
                        FROM attachments att
                        INNER JOIN visible v ON v.id=att.asset_id
                        GROUP BY att.stored_path
                    )
                ),0)"
        );
        Ok(self.conn.query_row(&sql,
            params![category,query,avatar,i64::from(favorites_only),i64::from(show_nsfw)],
            |r|Ok((r.get(0)?,r.get(1)?)))?)
    }

    pub fn list_filtered(&self, search: &str, category: &str, files: FileFilter, limit: i64, offset: i64) -> Result<Vec<Asset>> {
        self.list_filtered_avatar(search, category, "", files, limit, offset)
    }
    pub fn list_filtered_avatar(&self, search: &str, category: &str, avatar: &str, files: FileFilter, limit: i64, offset: i64) -> Result<Vec<Asset>> {
        self.list_filtered_favorites(search,category,avatar,files,false,limit,offset)
    }
    pub fn list_filtered_favorites(&self,search:&str,category:&str,avatar:&str,
        files:FileFilter,favorites_only:bool,limit:i64,offset:i64)->Result<Vec<Asset>> {
        self.list_filtered_visibility(search,category,avatar,files,favorites_only,false,limit,offset)
    }
    pub fn list_filtered_visibility(&self,search:&str,category:&str,avatar:&str,
        files:FileFilter,favorites_only:bool,show_nsfw:bool,
        limit:i64,offset:i64)->Result<Vec<Asset>> {
        let query = Self::fts_query(search);
        let pred = Self::predicate(&query,files);
        let sql = format!("SELECT a.id,a.title,a.category,a.tags,a.creator,a.description,a.booth_url,a.image_path,a.added_at,
                (SELECT COUNT(*) FROM attachments att WHERE att.asset_id=a.id),
                COALESCE((SELECT SUM(bytes) FROM attachments att WHERE att.asset_id=a.id),0),
                COALESCE((SELECT group_concat(avatar_name, ', ') FROM asset_avatars av WHERE av.asset_id=a.id),''),
                EXISTS(SELECT 1 FROM favorite_assets fav WHERE fav.asset_id=a.id),
                EXISTS(SELECT 1 FROM asset_content_flags sf WHERE sf.asset_id=a.id)
             FROM assets a WHERE {pred}
             AND (?4=0 OR EXISTS(SELECT 1 FROM favorite_assets fav WHERE fav.asset_id=a.id))
             AND (?5=1 OR NOT EXISTS(SELECT 1 FROM asset_content_flags sf WHERE sf.asset_id=a.id))
             ORDER BY a.added_at DESC,a.id DESC LIMIT ?6 OFFSET ?7");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![category,query,avatar,i64::from(favorites_only),i64::from(show_nsfw),
            limit.clamp(1,500),offset.max(0)],|r| {
            Ok(Asset { id:r.get(0)?,title:r.get(1)?,category:r.get(2)?,tags:r.get(3)?,avatars:Self::normalize_avatar_summary(&r.get::<_,String>(11)?),creator:r.get(4)?,description:r.get(5)?,booth_url:r.get(6)?,image_path:r.get(7)?,added_at:r.get(8)?,attachments:r.get(9)?,total_bytes:r.get(10)?,favorite:r.get::<_,i64>(12)?!=0,nsfw:r.get::<_,i64>(13)?!=0 })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_db() -> (tempfile::TempDir, Catalog) {
        let dir=tempfile::tempdir().unwrap();
        let db=Catalog::open(&dir.path().join("catalog.sqlite")).unwrap();
        (dir,db)
    }
    #[test]
    fn avatar_names_are_uppercase_deduplicated_and_visible_in_legacy_catalog() {
        let (dir,mut db)=test_db();
        let id=db.create_product("Chaqueta","Ropa").unwrap();
        db.set_avatars(id,"maya, MAYA; Manuka\nしなの,manuka").unwrap();
        assert_eq!(db.avatars_for(id).unwrap(),
            vec!["MANUKA","MAYA","しなの"]);
        let summary=db.get(id).unwrap().unwrap().avatars;
        for name in ["MAYA","MANUKA","しなの"] {
            assert!(summary.split(", ").any(|value|value==name));
        }
        assert_eq!(db.avatar_options().unwrap(),
            vec!["MANUKA","MAYA","しなの"]);
        // Test legacy rows inserted before normalization was introduced.
        db.conn.execute("INSERT OR IGNORE INTO asset_avatars(asset_id,avatar_name)
            VALUES(?1,'Shinano')",[id]).unwrap();
        assert!(db.avatar_options().unwrap().contains(&"SHINANO".to_string()));
        drop(db);
        let db=Catalog::open(&dir.path().join("catalog.sqlite")).unwrap();
        assert!(db.get(id).unwrap().unwrap().avatars.contains("SHINANO"));
    }

    #[test]
    fn nsfw_flag_filters_before_paging_and_survives_restart() {
        let (dir,mut db)=test_db();
        let hidden=db.create_product("Oculto","Ropa").unwrap();
        let visible=db.create_product("Visible","Ropa").unwrap();
        db.set_nsfw(hidden,true).unwrap();
        db.set_favorite(hidden,true).unwrap();
        assert!(db.get(hidden).unwrap().unwrap().nsfw);
        assert!(!db.get(visible).unwrap().unwrap().nsfw);
        assert_eq!(db.count_filtered_visibility("","","",FileFilter::All,false,false).unwrap(),1);
        assert_eq!(db.count_filtered_visibility("","","",FileFilter::All,false,true).unwrap(),2);
        assert_eq!(db.count_filtered_visibility("","","",FileFilter::All,true,false).unwrap(),0);
        assert_eq!(db.count_filtered_visibility("","","",FileFilter::All,true,true).unwrap(),1);
        let hidden_off=db.list_filtered_visibility("","","",FileFilter::All,false,false,1,0).unwrap();
        assert_eq!(hidden_off.len(),1);
        assert_eq!(hidden_off[0].id,visible);
        assert!(db.list_filtered_visibility("","","",FileFilter::All,true,false,10,0).unwrap().is_empty());
        assert_eq!(db.list_filtered_visibility("","","",FileFilter::All,true,true,10,0).unwrap()[0].id,hidden);
        drop(db);
        let mut db=Catalog::open(&dir.path().join("catalog.sqlite")).unwrap();
        assert!(db.get(hidden).unwrap().unwrap().nsfw);
        db.set_nsfw(hidden,false).unwrap();
        assert_eq!(db.count_filtered_visibility("","","",FileFilter::All,false,false).unwrap(),2);
        db.set_nsfw(hidden,true).unwrap();
        db.delete(hidden).unwrap();
        assert_eq!(db.count_filtered_visibility("","","",FileFilter::All,false,true).unwrap(),1);
    }

    #[test]
    fn palette_is_saved_safely_without_touching_existing_data() {
        let (dir,mut db)=test_db();
        let asset=db.create_product("Original","Ropa").unwrap();
        assert_eq!(db.palette_id().unwrap(),"sage");
        for id in ["graphite","purple","orange","sage"] {
            db.set_palette_id(id).unwrap();
            assert_eq!(db.palette_id().unwrap(),id);
        }
        assert!(db.set_palette_id("invalid").is_err());
        db.set_palette_id("purple").unwrap();
        drop(db);
        let db=Catalog::open(&dir.path().join("catalog.sqlite")).unwrap();
        assert_eq!(db.palette_id().unwrap(),"purple");
        assert_eq!(db.get(asset).unwrap().unwrap().title,"Original");
    }

    #[test]
    fn update_preferences_are_persistent_and_allowlisted() {
        let (dir,mut db)=test_db();
        assert!(db.preference("check_updates_on_start",true).unwrap());
        assert!(!db.preference("auto_install_updates",false).unwrap());
        // Sharing must be an opt-in preference that never fails at first launch.
        assert!(!db.preference("sharing_enabled",false).unwrap());
        db.update_preference("check_updates_on_start",false).unwrap();
        db.update_preference("auto_install_updates",true).unwrap();
        db.update_preference("delete_sources_after_import",true).unwrap();
        db.update_preference("sharing_enabled",true).unwrap();
        assert!(db.update_preference("unexpected",true).is_err());
        drop(db);
        let db=Catalog::open(&dir.path().join("catalog.sqlite")).unwrap();
        assert!(!db.preference("check_updates_on_start",true).unwrap());
        assert!(db.preference("auto_install_updates",false).unwrap());
        assert!(db.preference("delete_sources_after_import",false).unwrap());
        assert!(db.preference("sharing_enabled",false).unwrap());
    }
    #[test]
    fn storage_config_and_folders_are_persistent_without_destroying_legacy_paths() {
        let (tmp,mut db)=test_db();
        assert!(db.storage_root().unwrap().is_none());
        let id=db.create_product("New Outfit","Ropa").unwrap();
        let path=tmp.path().join("managed");
        let root=db.configure_storage(&path).unwrap();
        let folder=root.join("Ropa").join(id.to_string());
        std::fs::create_dir_all(&folder).unwrap();
        db.register_managed_folder(id,&folder).unwrap();
        db.register_managed_folder(id,&folder).unwrap();
        assert_eq!(db.managed_folders(id).unwrap().len(),1);
        drop(db);
        let db=Catalog::open(&tmp.path().join("catalog.sqlite")).unwrap();
        assert_eq!(db.storage_root().unwrap(),Some(root));
        assert_eq!(db.managed_folders(id).unwrap(),vec![folder.to_string_lossy()]);
    }
    #[test]
    fn favorites_persist_filter_and_cascade() {
        let (dir,mut db)=test_db();
        let id=db.create_product("Dress","Ropa").unwrap();
        let other=db.create_product("Avatar","Avatar").unwrap();
        db.set_favorite(id,true).unwrap();
        assert!(db.get(id).unwrap().unwrap().favorite);
        assert!(!db.get(other).unwrap().unwrap().favorite);
        assert_eq!(db.list_filtered_favorites("","","",FileFilter::All,true,10,0).unwrap().len(),1);
        drop(db);
        let mut db=Catalog::open(&dir.path().join("catalog.sqlite")).unwrap();
        assert_eq!(db.count_filtered_favorites("","","",FileFilter::All,true).unwrap(),1);
        db.set_favorite(id,false).unwrap();
        assert_eq!(db.count_filtered_favorites("","","",FileFilter::All,true).unwrap(),0);
        db.set_favorite(id,true).unwrap();
        db.delete(id).unwrap();
        assert_eq!(db.count_filtered_favorites("","","",FileFilter::All,true).unwrap(),0);
    }
    #[test]
    fn filtered_library_size_counts_unique_linked_paths_not_loaded_pages() {
        let (_dir,mut db)=test_db();
        let first=db.create_product("Primero","Ropa").unwrap();
        let second=db.create_product("Segundo","Avatar").unwrap();
        let third=db.create_product("Oculto","Ropa").unwrap();
        for (id,name,path,checksum,bytes) in [
            (first,"shared.zip","/test/shared.zip","sha-a",1_048_576),
            (second,"shared.zip","/test/shared.zip","sha-b",1_048_576),
            (second,"other.7z","/test/other.7z","sha-c",2_097_152),
            (third,"hidden.rar","/test/hidden.rar","sha-d",4_194_304),
        ] {
            db.add_attachment(&NewAttachment{
                asset_id:id,name,path,checksum,bytes,count:1,
                mode:ArchiveMode::Reference,
            }).unwrap();
        }
        db.set_nsfw(third,true).unwrap();
        // No duplicate count for a path attached to two different products.
        assert_eq!(db.stats_filtered_visibility("","","",FileFilter::All,false,false).unwrap(),
            (2,3_145_728));
        assert_eq!(db.stats_filtered_visibility("","","",FileFilter::All,false,true).unwrap(),
            (3,7_340_032));
        assert_eq!(db.stats_filtered_visibility("","Ropa","",FileFilter::All,false,false).unwrap(),
            (1,1_048_576));
        assert_eq!(db.stats_filtered_visibility("Segundo","","",FileFilter::All,false,false).unwrap(),
            (1,3_145_728));
        db.set_favorite(first,true).unwrap();
        assert_eq!(db.stats_filtered_visibility("","","",FileFilter::All,true,false).unwrap(),
            (1,1_048_576));
        assert_eq!(db.stats_filtered_visibility("","","",FileFilter::WithoutFiles,false,false).unwrap(),
            (0,0));
        db.detach(db.attachments_for(first).unwrap()[0].id,first).unwrap();
        assert_eq!(db.stats_filtered_visibility("","","",FileFilter::All,true,false).unwrap(),
            (1,0));
    }
    #[test]
    fn language_setting_persists_and_rejects_unknown_values(){
        let (_tmp,mut db)=test_db();
        assert_eq!(db.language_id().unwrap(),"es");
        db.set_language_id("en").unwrap();
        assert_eq!(db.language_id().unwrap(),"en");
        assert!(db.set_language_id("ja").is_err());
        assert_eq!(db.language_id().unwrap(),"en");
        db.set_language_id("es").unwrap();
        assert_eq!(db.language_id().unwrap(),"es");
    }
    #[test]
    fn products_with_multiple_files_and_filters() {
        let (_tmp,mut db)=test_db();
        let id=db.create_product("Vestido azul","Ropa").unwrap();
        let other=db.create_product("Avatar Dragón","Avatar").unwrap();
        let att=NewAttachment {asset_id:id,name:"a.zip",path:"/tmp/a.zip",checksum:"abc",bytes:15,count:2,mode:ArchiveMode::Reference};
        assert!(db.add_attachment(&att).unwrap());
        assert!(!db.add_attachment(&att).unwrap());
        assert_eq!(db.list_filtered("vest","Ropa",FileFilter::WithFiles,50,0).unwrap().len(),1);
        assert_eq!(db.count_filtered("", "",FileFilter::WithoutFiles).unwrap(),1);
        assert_eq!(db.attachments_for(id).unwrap().len(),1);
        assert_eq!(db.attachments_for(other).unwrap().len(),0);
        db.update_details(id,"Vestido azul","Ropa","Sora","ropa bonita","descripción").unwrap();
        assert_eq!(db.list_filtered("bonita","",FileFilter::All,50,0).unwrap().len(),1);
        db.detach(db.attachments_for(id).unwrap()[0].id,id).unwrap();
        assert_eq!(db.count_filtered("","",FileFilter::WithoutFiles).unwrap(),2);
    }
    #[test]
    fn quick_delete_only_removes_catalog_record_not_original_archives() {
        let dir=tempfile::tempdir().unwrap();
        let archive=dir.path().join("costume.zip");
        std::fs::write(&archive,b"unchanged original").unwrap();
        let mut db=Catalog::open(&dir.path().join("catalog.sqlite")).unwrap();
        let id=db.create_product("Ropa para Maya","Ropa").unwrap();
        db.add_attachment(&NewAttachment{
            asset_id:id,name:"costume.zip",path:archive.to_str().unwrap(),
            checksum:"original-costume",bytes:18,count:0,mode:ArchiveMode::Reference,
        }).unwrap();
        assert_eq!(db.attachments_for(id).unwrap().len(),1);
        db.delete(id).unwrap();
        assert!(db.get(id).unwrap().is_none());
        assert!(db.attachments_for(id).unwrap().is_empty());
        assert_eq!(std::fs::read(archive).unwrap(),b"unchanged original");
    }

    #[test]
    fn migration_preserves_legacy_archives_and_is_idempotent() {
        let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("catalog.sqlite");
        {
            let mut db=Catalog::open(&path).unwrap();
            let id=db.create_product("Legacy zip","Otros").unwrap();
            db.conn.execute("UPDATE assets SET archive_path='/old/a.zip', original_name='a.zip',checksum='old',archive_bytes=24,file_count=4 WHERE id=?1",[id]).unwrap();
        }
        let db=Catalog::open(&path).unwrap();
        assert_eq!(db.attachments_for(1).unwrap().len(),1);
        drop(db);
        let db=Catalog::open(&path).unwrap();
        assert_eq!(db.attachments_for(1).unwrap().len(),1);
        assert_eq!(db.attachments_for(1).unwrap()[0].mode,ArchiveMode::Copy);
    }
    #[test]
    fn external_source_link_is_stored_without_credentials_and_cascades() {
        let dir=tempfile::tempdir().unwrap();
        let file=dir.path().join("catalog.sqlite");
        let mut db=Catalog::open(&file).unwrap();
        let id=db.create_product("Costume","Ropa").unwrap();
        db.set_workupload_url(id,"https://www.workupload.com/archive/ZgFma42HBH?ref=x").unwrap();
        assert_eq!(db.workupload_url_for(id).unwrap(),
            "https://workupload.com/archive/ZgFma42HBH");
        assert!(db.set_workupload_url(id,"https://bad.example/archive/A").is_err());
        drop(db);
        let mut db=Catalog::open(&file).unwrap();
        assert_eq!(db.workupload_url_for(id).unwrap(),
            "https://workupload.com/archive/ZgFma42HBH");
        db.set_workupload_url(id,"").unwrap();
        assert_eq!(db.workupload_url_for(id).unwrap(),"");
        db.set_workupload_url(id,"https://workupload.com/file/Abc").unwrap();
        db.delete(id).unwrap();
        assert_eq!(db.workupload_url_for(id).unwrap(),"");
    }
    #[test]
    fn avatar_tags_are_independent_filterable_and_persistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let mut db = Catalog::open(&path).unwrap();
        let dress = db.create_product("Dress","Ropa").unwrap();
        let another = db.create_product("Other","Ropa").unwrap();
        db.set_avatars(dress,"Shinano, Maya; しなの,shinano").unwrap();
        db.set_avatars(another,"Manuka").unwrap();
        assert_eq!(db.avatars_for(dress).unwrap().len(),3);
        assert!(db.get(dress).unwrap().unwrap().avatars.contains("MAYA"));
        assert!(db.avatar_options().unwrap().contains(&"MAYA".to_string()));
        let att=NewAttachment {asset_id:dress,name:"dress.zip",path:"/tmp/dress.zip",
            checksum:"avatar-dress",bytes:42,count:1,mode:ArchiveMode::Reference};
        db.add_attachment(&att).unwrap();
        assert_eq!(db.count_filtered_avatar("","Ropa","Shinano",FileFilter::WithFiles).unwrap(),1);
        assert_eq!(db.count_filtered_avatar("","Ropa","Shinano",FileFilter::WithoutFiles).unwrap(),0);
        assert_eq!(db.count_filtered_avatar("","Ropa","Shinano",FileFilter::All).unwrap(),1);
        assert_eq!(db.list_filtered_avatar("","Ropa","SHINANO",FileFilter::All,20,0).unwrap()[0].id,dress);
        assert_eq!(db.count_filtered_avatar("","Avatar","Shinano",FileFilter::All).unwrap(),0);
        assert_eq!(db.count_filtered_avatar("","Ropa","",FileFilter::All).unwrap(),2);
        drop(db);
        let mut db=Catalog::open(&path).unwrap();
        assert_eq!(db.avatars_for(dress).unwrap().len(),3);
        db.set_avatars(dress,"Maya").unwrap();
        assert_eq!(db.avatars_for(dress).unwrap(),vec!["MAYA"]);
        assert!(db.set_avatars(dress,&"x".repeat(100)).is_err());
        assert_eq!(db.avatars_for(dress).unwrap(),vec!["MAYA"]);
        db.delete(dress).unwrap();
        assert!(db.avatar_options().unwrap().iter().all(|a|a!="MAYA"));
    }

    #[test]
    fn shop_urls_reuse_legacy_products_without_losing_files() {
        let (_tmp,mut db)=test_db();
        let id=db.add_booth_link("https://booth.pm/items/5141327").unwrap();
        let att=NewAttachment{asset_id:id,name:"dress.zip",path:"/local/dress.zip",
            checksum:"unique-dress",bytes:42,count:1,mode:ArchiveMode::Reference};
        db.add_attachment(&att).unwrap();
        assert_eq!(db.existing_booth_link("https://booth.pm/items/5141327").unwrap(),Some(id));
        assert_eq!(db.existing_booth_link("https://booth.pm/items/66666").unwrap(),None);
        assert_eq!(db.add_booth_link("https://chocolaterice.booth.pm/items/5141327").unwrap(),id);
        assert_eq!(db.get(id).unwrap().unwrap().booth_url,
            "https://chocolaterice.booth.pm/items/5141327");
        db.associate_booth(id,"https://booth.pm/items/5141327").unwrap();
        assert_eq!(db.get(id).unwrap().unwrap().booth_url,
            "https://chocolaterice.booth.pm/items/5141327");
        assert_eq!(db.attachments_for(id).unwrap().len(),1);
        assert_eq!(db.count().unwrap(),1);
        db.fill_creator_if_blank(id,"Chocolate rice").unwrap();
        assert_eq!(db.get(id).unwrap().unwrap().creator,"Chocolate rice");
        db.update_details(id,"Dress","Ropa","Nombre manual","","").unwrap();
        db.fill_creator_if_blank(id,"Chocolate rice").unwrap();
        assert_eq!(db.get(id).unwrap().unwrap().creator,"Nombre manual");
        let other=db.create_product("Otro producto","Ropa").unwrap();
        assert!(db.associate_booth(other,
            "https://booth.pm/items/5141327").is_err());
        assert_eq!(db.add_booth_link("https://booth.pm/items/51413278").unwrap(),3);
    }

    #[test]
    fn booth_association_checks_existing_product() {
        let (_tmp,mut db)=test_db();
        let id=db.create_product("archivo","Otros").unwrap();
        db.associate_booth(id,"https://booth.pm/items/123").unwrap();
        assert_eq!(db.add_booth_link("https://booth.pm/items/123").unwrap(),id);
        let other=db.create_product("otro","Otros").unwrap();
        assert!(db.associate_booth(other,"https://booth.pm/items/123").is_err());
        db.update_metadata(id,"https://booth.pm/items/123","Título BOOTH","Descripción","/cover.webp").unwrap();
        assert_eq!(db.get(id).unwrap().unwrap().image_path,"/cover.webp");
        db.set_booth_url(id,"").unwrap();
        assert!(db.get(id).unwrap().unwrap().booth_url.is_empty());
        db.set_booth_url(id,"https://booth.pm/items/123").unwrap();
        db.set_cover(id, "/manual-cover.png").unwrap();
        assert_eq!(db.get(id).unwrap().unwrap().image_path, "/manual-cover.png");
    }
}
