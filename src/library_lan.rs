//! Manually started, ephemeral LAN library sharing.
//!
//! The host publishes ONLY an explicit allowlist. Remote IDs are dense,
//! session-local values; original SQLite IDs, hidden counts, local paths and
//! unapproved attachment names never appear in responses. This is separate
//! from the existing single-product HTTPS transfer protocol.
use anyhow::{anyhow, bail, Context, Result};
use rand::{rngs::OsRng, RngCore};
use reqwest::{blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, UdpSocket},
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}},
    thread,
    time::Duration,
};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use url::Url;
use crate::{booth, db::{Catalog, CATEGORIES}, managed_storage};

pub const PROTOCOL_VERSION: u32 = 1;
const KEY_HEADER: &str = "X-SZKura-Library-Key";
const MAX_PRODUCTS: usize = 2_000;
const MAX_FILES: usize = 64;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_COVER_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 256 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 4;
const PAGE_SIZE: usize = 32;

#[derive(Clone, Default)]
pub struct ShareSelection {
    // Product ID -> explicitly allowed attachment IDs (empty = metadata only).
    pub products: BTreeMap<i64, BTreeSet<i64>>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct RemoteAsset {
    pub id: u64,
    pub title: String,
    pub category: String,
    pub creator: String,
    pub tags: String,
    pub description: String,
    pub avatars: Vec<String>,
    pub booth_url: String,
    pub nsfw: bool,
    pub has_cover: bool,
    /// ONLY the files allowed by the host. No hidden file counts.
    pub files: Vec<RemoteFile>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct RemoteSummary {
    pub id: u64,
    pub title: String,
    pub category: String,
    pub creator: String,
    pub tags: String,
    pub avatars: Vec<String>,
    pub nsfw: bool,
    pub has_cover: bool,
    pub files: usize,
}
impl From<&RemoteAsset> for RemoteSummary {
    fn from(asset: &RemoteAsset) -> Self {
        Self {
            id: asset.id, title: asset.title.clone(), category: asset.category.clone(),
            creator: asset.creator.clone(), tags: asset.tags.clone(),
            avatars: asset.avatars.clone(), nsfw: asset.nsfw,
            has_cover: asset.has_cover, files: asset.files.len(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct RemotePage {
    pub total: usize,
    pub items: Vec<RemoteSummary>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct LibraryInfo {
    pub version: u32,
    pub name: String,
    /// Number of visible products only.
    pub products: usize,
}

#[derive(Clone)]
struct ServedFile {
    path: PathBuf,
    size: u64,
    sha256: String,
}

struct Snapshot {
    info: LibraryInfo,
    assets: Vec<RemoteAsset>,
    covers: HashMap<u64, PathBuf>,
    files: HashMap<u64, ServedFile>,
}

fn file_digest(path: &Path, cancel: &AtomicBool) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) { bail!("Operación cancelada"); }
        let count = file.read(&mut bytes)?;
        if count == 0 { break; }
        hasher.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn approved_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 200 && name != "." && name != ".."
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(|ch| ch.is_control())
        && matches!(
            Path::new(name).extension().and_then(|e|e.to_str())
                .map(str::to_ascii_lowercase).as_deref(),
            Some("zip" | "rar" | "7z" | "unitypackage")
        )
}
fn regular_file(path: &Path, max_size: u64) -> Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > max_size {
        bail!("Archivo no regular, enlace simbólico o supera el tamaño permitido");
    }
    Ok(meta.len())
}
fn permitted_cover(path: &Path) -> bool {
    matches!(path.extension().and_then(|s|s.to_str())
        .map(str::to_ascii_lowercase).as_deref(),Some("png" | "jpg" | "jpeg" | "webp"))
        && regular_file(path, MAX_COVER_BYTES).is_ok()
}

/// Snapshot is immutable for the duration of this manual session.
/// This guarantees visibility and file allowlists cannot accidentally expand.
fn snapshot(db_path: &Path, choice: &ShareSelection, cancel: &AtomicBool)
    -> Result<Snapshot> {
    if choice.products.is_empty() || choice.products.len() > MAX_PRODUCTS {
        bail!("Selecciona entre 1 y 2000 fichas antes de compartir");
    }
    let db = Catalog::open(db_path)?;
    let mut assets = Vec::new();
    let mut covers = HashMap::new();
    let mut files = HashMap::new();
    let mut total_size = 0u64;
    let mut next_file_id = 1u64;
    for (&local_id, allowed_file_ids) in &choice.products {
        if cancel.load(Ordering::Relaxed) { bail!("Operación cancelada"); }
        let asset = db.get(local_id)?.context("Una ficha seleccionada ya no existe")?;
        let id = assets.len() as u64 + 1;
        let cover = PathBuf::from(&asset.image_path);
        let has_cover = !asset.image_path.is_empty() && permitted_cover(&cover);
        if has_cover { covers.insert(id, cover); }
        let mut visible_files = Vec::new();
        if !allowed_file_ids.is_empty() {
            let attached = db.attachments_for(local_id)?;
            let lookup:HashMap<_,_> = attached.iter().map(|file|(file.id,file)).collect();
            for &selected_id in allowed_file_ids {
                if files.len() >= MAX_FILES { bail!("Límite de 64 adjuntos compartidos"); }
                let file = lookup.get(&selected_id)
                    .context("Un adjunto seleccionado ya no pertenece a la ficha")?;
                if !approved_name(&file.original_name) {
                    bail!("Nombre o extensión de archivo no compatible para compartir");
                }
                let path = PathBuf::from(&file.stored_path);
                let size = regular_file(&path, MAX_FILE_BYTES)
                    .with_context(|| format!("Adjunto no disponible: {}", file.original_name))?;
                total_size = total_size.checked_add(size).context("Tamaño total no válido")?;
                if total_size > MAX_TOTAL_BYTES { bail!("Máximo 16 GiB de adjuntos"); }
                let digest = file_digest(&path, cancel)?;
                let public_id = next_file_id;
                next_file_id += 1;
                files.insert(public_id, ServedFile {
                    path, size, sha256:digest.clone(),
                });
                visible_files.push(RemoteFile {
                    id:public_id, name:file.original_name.clone(),size,sha256:digest,
                });
            }
        }
        assets.push(RemoteAsset {
            id, title:asset.title,creator:asset.creator,category:asset.category,
            tags:asset.tags,description:asset.description,avatars:db.avatars_for(local_id)?,
            booth_url:asset.booth_url,nsfw:asset.nsfw,has_cover,files:visible_files,
        });
    }
    Ok(Snapshot {
        info:LibraryInfo {
            version:PROTOCOL_VERSION,name:"Biblioteca compartida".into(),products:assets.len(),
        }, assets, covers, files,
    })
}

fn random_secret() -> String {
    let mut bytes=[0u8;32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b|format!("{b:02x}")).collect()
}
fn valid_secret(value:&str)->bool {
    value.len()==64 && value.bytes().all(|b|b.is_ascii_hexdigit())
}
fn selected_lan_ip() -> Ipv4Addr {
    // UDP connect selects an interface without sending a packet.
    if let Ok(socket)=UdpSocket::bind((Ipv4Addr::UNSPECIFIED,0)) {
        if socket.connect("192.0.2.1:80").is_ok() {
            if let Ok(std::net::SocketAddr::V4(addr))=socket.local_addr() {
                if addr.ip().is_private() { return *addr.ip(); }
            }
        }
    }
    Ipv4Addr::LOCALHOST
}
fn header(name:&str,value:&str)->Header {
    Header::from_bytes(name.as_bytes(),value.as_bytes()).expect("Constante HTTP válida")
}
fn safe_json<T:Serialize>(data:&T)->Response<std::io::Cursor<Vec<u8>>> {
    let encoded=serde_json::to_vec(data).unwrap_or_default();
    Response::from_data(encoded)
        .with_header(header("Content-Type","application/json; charset=utf-8"))
        .with_header(header("Cache-Control","no-store"))
        .with_header(header("X-Content-Type-Options","nosniff"))
}
fn unauthorized(request:&Request, secret:&str, prefix:&str)->bool {
    let correct = request.headers().iter().any(|h| {
        if !h.field.equiv(KEY_HEADER) { return false; }
        let actual=h.value.as_str().as_bytes();
        let expected=secret.as_bytes();
        actual.len()==expected.len() &&
            actual.iter().zip(expected).fold(0u8, |acc,(a,b)|acc|(a^b))==0
    });
    request.method()!=&Method::Get || !correct
        || !request.url().split('?').next().unwrap_or("").starts_with(prefix)
}
fn filtered_assets(snapshot:&Snapshot, params:&HashMap<String,String>)->RemotePage {
    let filter=params.get("q").map(|s|s.trim().to_lowercase()).unwrap_or_default();
    let category=params.get("category").map(String::as_str).unwrap_or("");
    let avatar=params.get("avatar").map(|s|s.to_uppercase()).unwrap_or_default();
    let offset=params.get("offset").and_then(|s|s.parse::<usize>().ok())
        .unwrap_or(0).min(MAX_PRODUCTS);
    let limit=params.get("limit").and_then(|s|s.parse::<usize>().ok())
        .unwrap_or(PAGE_SIZE).clamp(1,PAGE_SIZE);
    let filtered:Vec<_>=snapshot.assets.iter().filter(|asset| {
        (category.is_empty() || asset.category==category)
            && (avatar.is_empty() || asset.avatars.iter().any(|s|s==&avatar))
            && (filter.is_empty() || [&asset.title,&asset.creator,&asset.tags,&asset.description]
                .iter().any(|text|text.to_lowercase().contains(&filter)))
    }).collect();
    RemotePage {
        total:filtered.len(),
        items:filtered.into_iter().skip(offset).take(limit).map(RemoteSummary::from).collect(),
    }
}
fn handle_request(request:Request,snapshot:&Snapshot,prefix:&str,key:&str,stop:&AtomicBool) {
    let uri=request.url();
    if stop.load(Ordering::Relaxed) || unauthorized(&request,key,prefix) {
        let _=request.respond(Response::empty(StatusCode(404)));
        return;
    }
    let (path,query)=uri.split_once('?').unwrap_or((uri,""));
    let suffix=path.strip_prefix(prefix).unwrap_or("");
    let params:HashMap<String,String>=url::form_urlencoded::parse(query.as_bytes())
        .take(8).map(|(k,v)|(k.into_owned(),v.into_owned())).collect();
    if suffix=="/v1/library" {
        let _=request.respond(safe_json(&snapshot.info));
    }else if suffix=="/v1/assets" {
        let _=request.respond(safe_json(&filtered_assets(snapshot,&params)));
    }else if let Some(id)=suffix.strip_prefix("/v1/assets/")
        .and_then(|value|value.parse::<u64>().ok()) {
        match snapshot.assets.iter().find(|a|a.id==id) {
            Some(asset)=>{let _=request.respond(safe_json(asset));},
            None=>{let _=request.respond(Response::empty(StatusCode(404)));},
        }
    }else if let Some(id)=suffix.strip_prefix("/v1/covers/")
        .and_then(|value|value.parse::<u64>().ok()) {
        match snapshot.covers.get(&id).filter(|p|permitted_cover(p)) {
            Some(path)=>{
                match File::open(path) {
                    Ok(file)=>{
                        let mime=match path.extension().and_then(|s|s.to_str())
                            .unwrap_or("").to_lowercase().as_str() {
                            "jpg" | "jpeg"=>"image/jpeg",
                            "png"=>"image/png",
                            _=>"image/webp",
                        };
                        let _=request.respond(Response::from_file(file)
                            .with_header(header("Content-Type",mime))
                            .with_header(header("Cache-Control","no-store"))
                            .with_header(header("X-Content-Type-Options","nosniff")));
                    },
                    Err(_)=>{let _=request.respond(Response::empty(StatusCode(404)));},
                }
            },
            None=>{let _=request.respond(Response::empty(StatusCode(404)));},
        }
    }else if let Some(id)=suffix.strip_prefix("/v1/files/")
        .and_then(|value|value.parse::<u64>().ok()) {
        match snapshot.files.get(&id) {
            Some(selected) if regular_file(&selected.path,MAX_FILE_BYTES).ok()==Some(selected.size) => {
                match File::open(&selected.path) {
                    Ok(file) if file.metadata().ok().is_some_and(|meta|
                        meta.is_file() && meta.len()==selected.size) => {
                        let _=request.respond(Response::from_file(file)
                            .with_header(header("Content-Type","application/octet-stream"))
                            .with_header(header("Cache-Control","no-store"))
                            .with_header(header("X-Content-Type-Options","nosniff")));
                    },
                    _=>{let _=request.respond(Response::empty(StatusCode(404)));},
                }
            },
            _=>{let _=request.respond(Response::empty(StatusCode(404)));},
        }
    }else {
        let _=request.respond(Response::empty(StatusCode(404)));
    }
}

pub struct LibrarySession {
    pub invitation:String,
    pub visible:usize,
    pub downloadable:usize,
    stop:Arc<AtomicBool>,
}
impl LibrarySession {
    pub fn stop(&self) { self.stop.store(true,Ordering::Relaxed); }
}
impl Drop for LibrarySession {
    fn drop(&mut self) { self.stop(); }
}

/// Binds only a private IPv4 interface (or loopback if none exists).
/// Never activates on startup; the UI must call this after explicit selection.
pub fn start_library(db_path:&Path,selection:&ShareSelection,cancel:&AtomicBool)->Result<LibrarySession> {
    let snapshot=snapshot(db_path,selection,cancel)?;
    if cancel.load(Ordering::Relaxed) { bail!("Operación cancelada"); }
    let ip=selected_lan_ip();
    let server=Server::http(format!("{ip}:0"))
        .map_err(|e|anyhow!("No se pudo abrir el puerto LAN: {e}"))?;
    let address=server.server_addr().to_ip().context("Puerto de servidor no disponible")?;
    let token=random_secret();
    let key=random_secret();
    let prefix=format!("/l/{token}");
    let invitation=format!("http://{ip}:{}{}#key={key}",address.port(),prefix);
    let stop=Arc::new(AtomicBool::new(false));
    let stop_thread=Arc::clone(&stop);
    let active=Arc::new(AtomicUsize::new(0));
    let snapshot=Arc::new(snapshot);
    let files=snapshot.files.len();
    let visible=snapshot.assets.len();
    thread::spawn(move||{
        while !stop_thread.load(Ordering::Relaxed) {
            let request=match server.recv_timeout(Duration::from_millis(200)) {
                Ok(Some(req))=>req,
                Ok(None)=>continue,
                Err(_)=>break,
            };
            if active.fetch_add(1,Ordering::SeqCst)>=MAX_CONCURRENT_REQUESTS {
                active.fetch_sub(1,Ordering::SeqCst);
                let _=request.respond(Response::empty(StatusCode(503)));
                continue;
            }
            let published=Arc::clone(&snapshot);
            let current=Arc::clone(&active);
            let stop=Arc::clone(&stop_thread);
            let endpoint=prefix.clone();
            let secret=key.clone();
            thread::spawn(move||{
                handle_request(request,&published,&endpoint,&secret,&stop);
                current.fetch_sub(1,Ordering::SeqCst);
            });
        }
    });
    Ok(LibrarySession {invitation,visible,downloadable:files,stop})
}

#[derive(Clone)]
pub struct RemoteConnection {
    base: Url,
    key: String,
    client: Client,
    pub info: LibraryInfo,
    pub invitation: String,
}
fn lan_url(invitation:&str)->Result<(Url,String)> {
    let mut url=Url::parse(invitation.trim()).context("Invitación no válida")?;
    if url.scheme()!="http" || url.username()!="" || url.password().is_some()
        || url.query().is_some() {
        bail!("La invitación debe usar HTTP en una LAN privada");
    }
    let ip=url.host_str().context("Invitación sin dirección")?
        .parse::<IpAddr>().context("Usa una dirección IP privada, no un nombre DNS")?;
    match ip {
        IpAddr::V4(ip) if ip.is_private() || ip.is_loopback() => {},
        _=>bail!("Solo se permiten direcciones IPv4 privadas o de prueba local"),
    }
    if url.port().is_none() { bail!("La invitación requiere un puerto LAN"); }
    let components:Vec<_>=url.path().split('/').collect();
    if components.len()!=3 || components[0]!="" || components[1]!="l"
        || !valid_secret(components[2]) {
        bail!("Formato de invitación no válido");
    }
    let key=url.fragment().and_then(|frag|frag.strip_prefix("key="))
        .filter(|key|valid_secret(key)).context("Falta clave de invitación")?
        .to_owned();
    url.set_fragment(None);
    Ok((url,key))
}
fn http_client()->Result<Client> {
    Ok(Client::builder().redirect(Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(40)).build()?)
}
fn response(conn:&RemoteConnection,path:&str)->Result<reqwest::blocking::Response> {
    let mut url=conn.base.clone();
    url.set_path(&format!("{}{}",conn.base.path(),path));
    url.set_query(None);
    let response=conn.client.get(url)
        .header(KEY_HEADER,&conn.key)
        .header("Accept","application/octet-stream")
        .send().context("No se pudo conectar con la biblioteca remota")?;
    if !response.status().is_success(){
        bail!("La biblioteca no está disponible o el archivo no está autorizado ({})",
            response.status().as_u16());
    }
    Ok(response)
}
fn limited_json<T:for<'a>Deserialize<'a>>(mut response:reqwest::blocking::Response)->Result<T> {
    if response.content_length().is_some_and(|n|n>MAX_JSON_BYTES) {
        bail!("Respuesta remota demasiado grande");
    }
    let mut bytes=Vec::new();
    response.by_ref().take(MAX_JSON_BYTES+1).read_to_end(&mut bytes)?;
    if bytes.len() as u64>MAX_JSON_BYTES { bail!("Respuesta remota demasiado grande"); }
    Ok(serde_json::from_slice(&bytes)?)
}
pub fn connect(invitation:&str)->Result<RemoteConnection> {
    let (base,key)=lan_url(invitation)?;
    let mut result=RemoteConnection{
        base,key,client:http_client()?,info:LibraryInfo{
            version:0,name:String::new(),products:0,
        },invitation:invitation.trim().into(),
    };
    let info:LibraryInfo=limited_json(response(&result,"/v1/library")?)?;
    if info.version!=PROTOCOL_VERSION || info.products>MAX_PRODUCTS
        || info.name.len()>120 {
        bail!("Versión incompatible o biblioteca remota no válida");
    }
    result.info=info;
    Ok(result)
}
pub fn browse(conn:&RemoteConnection,query:&str,category:&str,avatar:&str,offset:usize)
    ->Result<RemotePage> {
    if query.len()>120 || category.len()>70 || avatar.len()>70 {bail!("Filtro demasiado largo");}
    let mut url=conn.base.clone();
    url.set_path(&format!("{}/v1/assets",conn.base.path()));
    url.query_pairs_mut().append_pair("q",query).append_pair("category",category)
        .append_pair("avatar",avatar).append_pair("offset",&offset.to_string())
        .append_pair("limit",&PAGE_SIZE.to_string());
    let response=conn.client.get(url).header(KEY_HEADER,&conn.key).send()?
        .error_for_status()?;
    let page:RemotePage=limited_json(response)?;
    if page.items.len()>PAGE_SIZE || page.total>MAX_PRODUCTS {bail!("Página remota no válida");}
    Ok(page)
}
pub fn get_asset(conn:&RemoteConnection,id:u64)->Result<RemoteAsset>{
    let asset:RemoteAsset=limited_json(response(conn,&format!("/v1/assets/{id}"))?)?;
    if asset.id!=id || asset.files.len()>MAX_FILES || !CATEGORIES.contains(&asset.category.as_str())
        || asset.avatars.len()>64 || asset.title.len()>512 {
        bail!("Ficha remota no válida");
    }
    Ok(asset)
}
pub fn fetch_cover(conn:&RemoteConnection,id:u64)->Result<Option<Vec<u8>>>{
    let mut url=conn.base.clone();
    url.set_path(&format!("{}/v1/covers/{id}",conn.base.path()));
    let mut reply=conn.client.get(url).header(KEY_HEADER,&conn.key).send()?;
    if reply.status()==reqwest::StatusCode::NOT_FOUND { return Ok(None); }
    reply=reply.error_for_status()?;
    if reply.content_length().is_some_and(|n|n>MAX_COVER_BYTES) {
        bail!("Portada remota demasiado grande");
    }
    let mut bytes=Vec::new();
    reply.take(MAX_COVER_BYTES+1).read_to_end(&mut bytes)?;
    if bytes.len() as u64>MAX_COVER_BYTES {bail!("Portada remota demasiado grande");}
    Ok(Some(bytes))
}

fn download_file(conn:&RemoteConnection,file:&RemoteFile,dest:&Path,cancel:&AtomicBool)
    ->Result<()> {
    if !approved_name(&file.name) || file.size>MAX_FILE_BYTES ||
        file.sha256.len()!=64 || !file.sha256.bytes().all(|c|c.is_ascii_hexdigit()) {
        bail!("Archivo remoto no válido");
    }
    let mut source=response(conn,&format!("/v1/files/{}",file.id))?;
    let mut target=File::create(dest)?;
    let mut digest=Sha256::new();
    let mut remaining=file.size;
    let mut buf=[0u8;64*1024];
    loop {
        if cancel.load(Ordering::Relaxed) { bail!("Descarga cancelada"); }
        let n=source.read(&mut buf)?;
        if n==0 {break;}
        if n as u64>remaining {bail!("Archivo remoto excede el tamaño declarado");}
        target.write_all(&buf[..n])?;
        digest.update(&buf[..n]);
        remaining-=n as u64;
    }
    target.flush()?;
    if remaining!=0 || format!("{:x}",digest.finalize())!=file.sha256.to_lowercase() {
        bail!("Archivo remoto incompleto o SHA-256 incorrecto");
    }
    Ok(())
}

/// Import metadata without downloading, or selectively download and attach
/// only previously approved files. Never overwrite existing BOOTH metadata.
pub fn import_asset(conn:&RemoteConnection,id:u64,chosen:&BTreeSet<u64>,
    db_path:&Path,library:&Path,covers_dir:&Path,cancel:&AtomicBool)->Result<i64> {
    let asset=get_asset(conn,id)?;
    if asset.description.len()>32_000 || asset.tags.len()>8_000
        || asset.creator.len()>512 || asset.avatars.iter().any(|n|n.len()>70)
        || asset.files.len()>MAX_FILES {
        bail!("Metadatos remotos demasiado grandes");
    }
    if !asset.booth_url.is_empty(){booth::validate_booth_url(&asset.booth_url)?;}
    let allowed:HashSet<u64>=asset.files.iter().map(|f|f.id).collect();
    if !chosen.is_subset(&allowed) {bail!("Uno de los archivos no está publicado");}
    let total=asset.files.iter().filter(|f|chosen.contains(&f.id))
        .try_fold(0u64,|sum,f|sum.checked_add(f.size)).context("Tamaño inválido")?;
    if total>MAX_TOTAL_BYTES {bail!("Selección demasiado grande");}
    let temp=if chosen.is_empty(){None}else{
        fs::create_dir_all(library)?;
        Some(tempfile::tempdir_in(library)?)
    };
    let mut downloaded=Vec::new();
    if let Some(dir)=&temp {
        for file in &asset.files {
            if !chosen.contains(&file.id) {continue;}
            if cancel.load(Ordering::Relaxed) {bail!("Importación cancelada");}
            let folder=dir.path().join(file.id.to_string());
            fs::create_dir(&folder)?;
            let dest=folder.join(&file.name);
            download_file(conn,file,&dest,cancel)?;
            downloaded.push(dest);
        }
    }
    let cover=if asset.has_cover {
        match fetch_cover(conn,id)? {
            Some(bytes) if bytes.len()<=MAX_COVER_BYTES as usize => {
                match image::load_from_memory(&bytes) {
                    Ok(image) if image.width()<=6000 && image.height()<=6000 =>Some(bytes),
                    _=>None,
                }
            },
            _=>None,
        }
    }else{None};
    if cancel.load(Ordering::Relaxed){bail!("Importación cancelada");}
    let mut db=Catalog::open(db_path)?;
    let existing=if asset.booth_url.is_empty(){None}
        else{db.existing_booth_link(&asset.booth_url)?};
    let product=match existing {
        Some(existing)=>existing,
        None=>{
            let id=db.create_product(&asset.title,&asset.category)?;
            db.update_details(id,&asset.title,&asset.category,&asset.creator,
                &asset.tags,&asset.description)?;
            db.set_avatars(id,&asset.avatars.join(", "))?;
            db.set_nsfw(id,asset.nsfw)?;
            if !asset.booth_url.is_empty(){db.associate_booth(id,&asset.booth_url)?;}
            if let Some(bytes)=cover {
                fs::create_dir_all(covers_dir)?;
                let name=format!("remote-{:x}.img",Sha256::digest(&bytes));
                let path=covers_dir.join(name);
                if !path.exists(){fs::write(&path,bytes)?;}
                db.set_cover(id,&path.to_string_lossy())?;
            }
            id
        }
    };
    drop(db);
    if !downloaded.is_empty() {
        let report=managed_storage::import_paths(db_path,library,&downloaded,product,false)?;
        if report.rejected>0 {bail!("No se pudieron importar algunos adjuntos");}
    }
    Ok(product)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_private_and_off_by_default() {
        let empty=ShareSelection::default();
        assert!(empty.products.is_empty());
        assert!(valid_secret(&random_secret()));
        assert!(!approved_name("../hidden.zip"));
        assert!(!approved_name("x.exe"));
        assert!(approved_name("MANUKA_CWU27P.zip"));
    }

    #[test]
    fn invitation_must_be_private_and_include_secret() {
        let token="a".repeat(64);
        let secret="b".repeat(64);
        let link=format!("http://192.168.1.23:4400/l/{token}#key={secret}");
        let (base,key)=lan_url(&link).unwrap();
        assert_eq!(key,secret);
        assert_eq!(base.as_str(),format!("http://192.168.1.23:4400/l/{token}"));
        for invalid in [
            format!("http://1.2.3.4:4400/l/{token}#key={secret}"),
            format!("http://127.0.0.1:4400/l/{token}"),
            format!("http://example.com:4400/l/{token}#key={secret}"),
            format!("https://192.168.1.23:4400/l/{token}#key={secret}"),
            format!("http://192.168.1.23:4400/l/{token}/../a#key={secret}"),
        ] {assert!(lan_url(&invalid).is_err(), "{invalid}");}
    }

    #[test]
    fn only_published_products_and_files_enter_snapshot() {
        let tmp=tempfile::tempdir().unwrap();
        let db_path=tmp.path().join("catalog.sqlite");
        let mut db=Catalog::open(&db_path).unwrap();
        let public=db.create_product("VISIBLE","Ropa").unwrap();
        let hidden=db.create_product("PRIVADO","Ropa").unwrap();
        let source=tmp.path().join("allowed.zip");
        fs::write(&source,b"data").unwrap();
        // Attachment creation is tested in the managed-storage module; this
        // scenario verifies metadata allowlisting and public IDs independently.
        let mut selected=ShareSelection::default();
        selected.products.insert(public,BTreeSet::new());
        let snapshot=snapshot(&db_path,&selected,&AtomicBool::new(false)).unwrap();
        assert_eq!(snapshot.info.products,1);
        assert_eq!(snapshot.assets.len(),1);
        assert_eq!(snapshot.assets[0].id,1);
        assert_eq!(snapshot.assets[0].title,"VISIBLE");
        assert!(snapshot.assets[0].files.is_empty());
        assert!(snapshot.files.is_empty());
        assert!(!serde_json::to_string(&snapshot.assets).unwrap().contains("PRIVADO"));
        assert!(db.get(hidden).unwrap().is_some());
    }
}
