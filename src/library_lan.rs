//! Manually started, ephemeral library sharing on LAN or Cloudflare HTTPS.
//!
//! Only published product METADATA is snapshotted at start. Archive bytes are
//! never read during startup: their previously verified BLAKE3 digests come
//! from SQLite. Covers and archive streams are loaded only on authorized GETs.
//! Hidden products, SQLite IDs and filesystem paths never leave the host.
use anyhow::{anyhow, bail, Context, Result};
use rand::{rngs::OsRng, RngCore};
use reqwest::{blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    net::{IpAddr, Ipv4Addr, UdpSocket},
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}},
    thread,
    time::Duration,
};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use url::Url;
use crate::{booth, db::{Catalog, CATEGORIES, FileFilter, RemoteFileState,
    RemoteImportRecord,RemoteStagedFile,RemoteCommitReport},share_transfer};

pub const PROTOCOL_VERSION: u32 = 3;
const KEY_HEADER: &str = "X-SZKura-Library-Key";
const MAX_PRODUCTS: usize = 2_000;
pub const MAX_FILES: usize = 4_096;
const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024 * 1024;
// Match the native library's cover budget: 32 MiB and 24 MP.
const MAX_COVER_BYTES: u64 = 32 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 4;
const PAGE_SIZE: usize = 32;

#[derive(Clone, Default)]
pub struct ShareSelection {
    // The host selects WHOLE product records. Attached files are included as
    // part of the same product, never controlled by a second file picker.
    pub products: BTreeSet<i64>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    pub id: u64,
    pub name: String,
    pub size: u64,
    /// BLAKE3 originally computed on import and stored in SQLite.
    pub blake3: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct RemoteAsset {
    pub id: u64,
    pub origin_key:String,
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
pub struct RemoteFileSignature {
    pub name:String,
    pub blake3:String,
    pub size:u64,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct RemoteSummary {
    pub id: u64,
    pub origin_key:String,
    pub booth_url:String,
    pub signatures:Vec<RemoteFileSignature>,
    pub title: String,
    pub category: String,
    pub creator: String,
    pub tags: String,
    pub avatars: Vec<String>,
    pub nsfw: bool,
    pub has_cover: bool,
    pub files: usize,
    pub total_bytes: u64,
    #[serde(skip)]
    pub local_state:RemoteLocalState,
}
impl From<&RemoteAsset> for RemoteSummary {
    fn from(asset: &RemoteAsset) -> Self {
        Self {
            id: asset.id, origin_key:asset.origin_key.clone(),
            booth_url:asset.booth_url.clone(),
            signatures:asset.files.iter().map(|f|RemoteFileSignature{
                name:f.name.clone(),blake3:f.blake3.clone(),size:f.size,
            }).collect(),
            title: asset.title.clone(), category: asset.category.clone(),
            creator: asset.creator.clone(),
            tags: asset.tags.chars().take(240).collect(),
            avatars: asset.avatars.clone(), nsfw: asset.nsfw,
            has_cover: asset.has_cover, files: asset.files.len(),
            total_bytes:asset.files.iter().map(|f|f.size).sum(),
            local_state:RemoteLocalState::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RemoteLocalState {
    #[default]
    Unknown,
    New,
    Complete,
    NeedsUpdate(usize),
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
    /// Filter choices come from EXCLUSIVELY published products.
    pub avatars: Vec<String>,
}

#[derive(Clone)]
struct ServedFile {
    path: PathBuf,
    size: u64,
    /// Host file metadata version when this publication session began.
    modified:Option<std::time::SystemTime>,
}

struct Snapshot {
    info: LibraryInfo,
    assets: Vec<RemoteAsset>,
    covers: HashMap<u64, PathBuf>,
    files: HashMap<u64, ServedFile>,
}

/// Used ONLY to verify files that already exist on the receiving computer.
/// The hosting startup never calls this function on shared attachments.
fn file_digest(path: &Path, cancel: &AtomicBool) -> Result<String> {
    let mut file = File::open(path)?;
    let mut native=blake3::Hasher::new();
    let mut bytes = [0u8; 128 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) { bail!("Operación cancelada"); }
        let count = file.read(&mut bytes)?;
        if count == 0 { break; }
        native.update(&bytes[..count]);
    }
    Ok(native.finalize().to_hex().to_string())
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
    // BOOTH saves the original bytes as v2-<hash>.img when recompressing
    // them would not help. The native library reads these images by signature.
    matches!(path.extension().and_then(|s|s.to_str())
        .map(str::to_ascii_lowercase).as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "img"))
        && regular_file(path, MAX_COVER_BYTES).is_ok()
}

/// Never trust the suffix (especially .img) when serving bytes to a peer.
/// Sniff only an image signature on demand, then reset the streaming cursor.
fn cover_mime(file:&mut File)->Option<&'static str>{
    let mut signature=[0u8;32];
    let length=file.read(&mut signature).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    match image::guess_format(&signature[..length]).ok()? {
        image::ImageFormat::Jpeg=>Some("image/jpeg"),
        image::ImageFormat::Png=>Some("image/png"),
        image::ImageFormat::WebP=>Some("image/webp"),
        _=>None,
    }
}

/// Immutable, authorization-safe snapshot of SQLite metadata only.
/// Opening a 50+ GiB library does not open, read or hash archive contents.
/// On an archive request the host checks metadata, and the receiving client
/// verifies actual BLAKE3 bytes before committing its all-or-nothing import.
fn snapshot(db_path:&Path,choice:&ShareSelection,name:&str,cancel:&AtomicBool)
    ->Result<Snapshot>{
    if name.trim().is_empty() || name.len()>80
        || name.chars().any(char::is_control){
        bail!("Nombre de biblioteca no válido");
    }
    if choice.products.is_empty() || choice.products.len() > MAX_PRODUCTS {
        bail!("Selecciona entre 1 y 2000 fichas antes de compartir");
    }
    let mut db = Catalog::open(db_path)?;
    let identity=db.shared_library_secret()?;
    let mut assets = Vec::new();
    let mut covers = HashMap::new();
    let mut files = HashMap::new();
    let mut total_size = 0u64;
    let mut next_file_id = 1u64;
    for &local_id in &choice.products {
        if cancel.load(Ordering::Relaxed) { bail!("Operación cancelada"); }
        let asset = db.get(local_id)?.context("Una ficha seleccionada ya no existe")?;
        if asset.title.len()>512 || asset.creator.len()>512
            || asset.tags.len()>8_000 || asset.description.len()>32_000
            || asset.booth_url.len()>2_048 {
            bail!("Una ficha seleccionada supera los límites de metadatos publicados");
        }
        let id = assets.len() as u64 + 1;
        let origin_key=blake3::keyed_hash(&identity,&local_id.to_le_bytes())
            .to_hex().to_string();
        let cover = PathBuf::from(&asset.image_path);
        let has_cover = !asset.image_path.is_empty() && permitted_cover(&cover);
        if has_cover { covers.insert(id, cover); }
        let mut visible_files=Vec::new();
        for file in db.attachments_for(local_id)? {
            if files.len()>=MAX_FILES { bail!("Máximo 4096 archivos compartidos por sesión"); }
            if !approved_name(&file.original_name){
                bail!("Un adjunto tiene un nombre o extensión no compatible");
            }
            let path=PathBuf::from(&file.stored_path);
            let size=regular_file(&path,MAX_FILE_BYTES)
                .with_context(||format!("Adjunto no disponible: {}",file.original_name))?;
            total_size=total_size.checked_add(size).context("Tamaño total no válido")?;
            if total_size>MAX_TOTAL_BYTES { bail!("Máximo 256 GiB de adjuntos"); }
            // The importer already stores BLAKE3 checksums in attachments.
            // Never trust malformed/legacy checksums or fabricate a digest.
            if file.checksum.len()!=64
                || !file.checksum.bytes().all(|c|c.is_ascii_hexdigit()){
                bail!("El adjunto {} no tiene un BLAKE3 válido en SQLite; vuelve a indexarlo",
                    file.original_name);
            }
            let modified=fs::symlink_metadata(&path)?.modified().ok();
            let public_id=next_file_id;
            next_file_id+=1;
            files.insert(public_id,ServedFile{path,size,modified});
            visible_files.push(RemoteFile{
                id:public_id,name:file.original_name,size,blake3:file.checksum,
            });
        }
        assets.push(RemoteAsset {
            id, origin_key,title:asset.title,creator:asset.creator,category:asset.category,
            tags:asset.tags,description:asset.description,avatars:db.avatars_for(local_id)?,
            booth_url:asset.booth_url,nsfw:asset.nsfw,has_cover,files:visible_files,
        });
    }
    let mut avatars:BTreeSet<String>=BTreeSet::new();
    for product in &assets {
        for label in &product.avatars {
            if label.len()<=70 {avatars.insert(label.clone());}
        }
    }
    Ok(Snapshot {
        info:LibraryInfo {
            version:PROTOCOL_VERSION,name:name.trim().into(),
            products:assets.len(),avatars:avatars.into_iter().take(512).collect(),
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

/// Stop already-open file responses too, not only future HTTP requests.
/// The worker checks this flag before every streaming chunk.
struct RevocableReader {
    file:File,
    stop:Arc<AtomicBool>,
}
impl Read for RevocableReader {
    fn read(&mut self, bytes:&mut [u8])->io::Result<usize>{
        if self.stop.load(Ordering::Relaxed){
            return Err(io::Error::new(io::ErrorKind::Interrupted,"La sesión ha terminado"));
        }
        self.file.read(bytes)
    }
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
    let nsfw=params.get("nsfw").is_some_and(|value|value=="1");
    let files_filter=params.get("files").map(String::as_str).unwrap_or("all");
    let offset=params.get("offset").and_then(|s|s.parse::<usize>().ok())
        .unwrap_or(0).min(MAX_PRODUCTS);
    let limit=params.get("limit").and_then(|s|s.parse::<usize>().ok())
        .unwrap_or(PAGE_SIZE).clamp(1,PAGE_SIZE);
    let filtered:Vec<_>=snapshot.assets.iter().filter(|asset| {
        (nsfw || !asset.nsfw)
            && (files_filter!="with" || !asset.files.is_empty())
            && (files_filter!="without" || asset.files.is_empty())
            && (category.is_empty() || asset.category==category)
            && (avatar.is_empty() || asset.avatars.iter().any(|s|s==&avatar))
            && (filter.is_empty() || [&asset.title,&asset.creator,&asset.tags]
                .iter().any(|text|text.to_lowercase().contains(&filter)))
    }).collect();
    RemotePage {
        total:filtered.len(),
        items:filtered.into_iter().skip(offset).take(limit).map(RemoteSummary::from).collect(),
    }
}
fn handle_request(request:Request,snapshot:&Snapshot,prefix:&str,key:&str,
    stop:Arc<AtomicBool>) {
    let uri=request.url().to_owned();
    if uri.len()>4_096 || stop.load(Ordering::Relaxed)
        || unauthorized(&request,key,prefix) {
        let _=request.respond(Response::empty(StatusCode(404)));
        return;
    }
    let (path,query)=uri.split_once('?').unwrap_or((&uri,""));
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
                    Ok(mut file)=>{
                        let Some(mime)=cover_mime(&mut file) else {
                            let _=request.respond(Response::empty(StatusCode(404)));
                            return;
                        };
                        let size=file.metadata().map(|meta|meta.len()).unwrap_or(0);
                        let reader=RevocableReader{file,stop:Arc::clone(&stop)};
                        let _=request.respond(Response::new(StatusCode(200),vec![],reader,
                            usize::try_from(size).ok(),None)
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
            Some(selected) if regular_file(&selected.path,MAX_FILE_BYTES).ok()==Some(selected.size)
                && fs::symlink_metadata(&selected.path).ok().is_some_and(|meta|
                    meta.modified().ok()==selected.modified) => {
                match File::open(&selected.path) {
                    Ok(file) if file.metadata().ok().is_some_and(|meta|
                        meta.is_file() && meta.len()==selected.size) => {
                        let reader=RevocableReader{file,stop:Arc::clone(&stop)};
                        let _=request.respond(Response::new(StatusCode(200),vec![],reader,
                            usize::try_from(selected.size).ok(),None)
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
    pub name:String,
    pub visible:usize,
    pub downloadable:usize,
    /// SQLite estimate for published attached files, without reading their bytes.
    pub total_bytes:u64,
    stop:Arc<AtomicBool>,
    tunnel:Option<share_transfer::QuickTunnel>,
}
impl LibrarySession {
    pub fn stop(&self) {
        self.stop.store(true,Ordering::Relaxed);
        if let Some(tunnel)=self.tunnel.as_ref(){tunnel.stop();}
    }
}
impl Drop for LibrarySession {
    fn drop(&mut self) { self.stop(); }
}

/// Manually start a private LAN session, or a loopback-only session published
/// through the existing verified Cloudflare component after separate consent.
pub fn start_library(db_path:&Path,selection:&ShareSelection,data_dir:&Path,
    internet:bool,name:&str,cancel:&AtomicBool)->Result<LibrarySession> {
    let snapshot=snapshot(db_path,selection,name,cancel)?;
    if cancel.load(Ordering::Relaxed) { bail!("Operación cancelada"); }
    let ip=if internet{Ipv4Addr::LOCALHOST}else{selected_lan_ip()};
    let server=Server::http(format!("{ip}:0"))
        .map_err(|e|anyhow!("No se pudo abrir el puerto LAN: {e}"))?;
    let address=server.server_addr().to_ip().context("Puerto de servidor no disponible")?;
    let token=random_secret();
    let key=random_secret();
    let prefix=format!("/l/{token}");
    let local_url=format!("http://{ip}:{}{}#key={key}",address.port(),prefix);
    let stop=Arc::new(AtomicBool::new(false));
    let stop_thread=Arc::clone(&stop);
    let active=Arc::new(AtomicUsize::new(0));
    let snapshot=Arc::new(snapshot);
    let files=snapshot.files.len();
    let visible=snapshot.assets.len();
    let total_bytes=snapshot.assets.iter().flat_map(|asset|&asset.files)
        .fold(0u64,|total,file|total.saturating_add(file.size));
    let name=snapshot.info.name.clone();
    let thread_key=key.clone();
    let thread_prefix=prefix.clone();
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
            let endpoint=thread_prefix.clone();
            let secret=thread_key.clone();
            thread::spawn(move||{
                handle_request(request,&published,&endpoint,&secret,stop);
                current.fetch_sub(1,Ordering::SeqCst);
            });
        }
    });
    let tunnel=if internet{
        let result=share_transfer::start_quick_tunnel(data_dir,address.port(),cancel);
        match result {
            Ok(tunnel)=>Some(tunnel),
            Err(err)=>{
                stop.store(true,Ordering::Relaxed);
                return Err(err);
            },
        }
    }else{None};
    if cancel.load(Ordering::Relaxed){
        stop.store(true,Ordering::Relaxed);
        bail!("Operación cancelada");
    }
    let invitation=if let Some(ref tunnel)=tunnel{
        format!("{}{}#key={key}",tunnel.base_url,prefix)
    }else{local_url};
    Ok(LibrarySession {invitation,name,visible,downloadable:files,total_bytes,stop,tunnel})
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
    if url.username()!="" || url.password().is_some() || url.query().is_some() {
        bail!("Formato de invitación no válido");
    }
    match url.scheme(){
        "http"=>{
            let ip=url.host_str().context("Invitación sin dirección")?
                .parse::<IpAddr>().context("Solo IP privada para conexiones HTTP")?;
            match ip{
                IpAddr::V4(ip) if ip.is_private() || ip.is_loopback()=>{},
                _=>bail!("HTTP únicamente en IPv4 de LAN privada"),
            }
            if url.port().is_none(){bail!("Falta el puerto de la LAN");}
        },
        "https"=>{
            let hostname=url.host_str().unwrap_or("");
            if !hostname.ends_with(".trycloudflare.com")
                || hostname==".trycloudflare.com" || url.port().is_some() {
                bail!("La invitación HTTPS debe ser de Cloudflare Quick Tunnel");
            }
        },
        _=>bail!("Se requiere HTTP privado o HTTPS de Cloudflare"),
    }
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
    // Never route private LAN access tokens through environment HTTP proxies.
    Ok(Client::builder().redirect(Policy::none()).no_proxy()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(40)).build()?)
}
fn response(conn:&RemoteConnection,path:&str)->Result<reqwest::blocking::Response> {
    let mut url=conn.base.clone();
    url.set_path(&format!("{}{}",conn.base.path(),path));
    url.set_query(None);
    let timeout=if path.starts_with("/v1/files/") {
        // Large verified LAN downloads can exceed a regular API timeout.
        Duration::from_secs(24 * 60 * 60)
    }else{Duration::from_secs(40)};
    let response=conn.client.get(url)
        .header(KEY_HEADER,&conn.key)
        .header("Accept","application/octet-stream")
        .timeout(timeout)
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
            version:0,name:String::new(),products:0,avatars:Vec::new(),
        },invitation:invitation.trim().into(),
    };
    let info:LibraryInfo=limited_json(response(&result,"/v1/library")?)?;
    if info.version!=PROTOCOL_VERSION || info.products>MAX_PRODUCTS
        || info.name.len()>120 || info.avatars.len()>512
        || info.avatars.iter().any(|avatar|avatar.len()>70) {
        bail!("Versión incompatible o biblioteca remota no válida");
    }
    result.info=info;
    Ok(result)
}
pub fn browse(conn:&RemoteConnection,query:&str,category:&str,avatar:&str,
    files_filter:FileFilter,show_nsfw:bool,offset:usize)->Result<RemotePage> {
    if query.len()>120 || category.len()>70 || avatar.len()>70 {bail!("Filtro demasiado largo");}
    let mut url=conn.base.clone();
    url.set_path(&format!("{}/v1/assets",conn.base.path()));
    url.query_pairs_mut().append_pair("q",query).append_pair("category",category)
        .append_pair("avatar",avatar).append_pair("offset",&offset.to_string())
        .append_pair("files",match files_filter{
            FileFilter::All=>"all",FileFilter::WithFiles=>"with",
            FileFilter::WithoutFiles=>"without"
        })
        .append_pair("nsfw",if show_nsfw{"1"}else{"0"})
        .append_pair("limit",&PAGE_SIZE.to_string());
    let response=conn.client.get(url).header(KEY_HEADER,&conn.key).send()?
        .error_for_status()?;
    let page:RemotePage=limited_json(response)?;
    if page.items.len()>PAGE_SIZE || page.total>MAX_PRODUCTS {bail!("Página remota no válida");}
    Ok(page)
}
/// Local status is evaluated by the receiving application's background worker.
/// It is never sent by the host or included in public catalog metadata.
pub fn local_state(db:&Catalog,origin_key:&str,booth_url:&str,
    signatures:&[RemoteFileSignature])->Result<RemoteLocalState>{
    let Some(id)=db.resolve_remote_product(origin_key,booth_url)? else {
        return Ok(RemoteLocalState::New);
    };
    let expected=signatures.iter().map(|entry|
        (entry.name.clone(),entry.blake3.clone(),entry.size)).collect::<Vec<_>>();
    Ok(match db.remote_file_state(id,&expected)? {
        RemoteFileState::Complete=>RemoteLocalState::Complete,
        RemoteFileState::NeedsUpdate(count)=>RemoteLocalState::NeedsUpdate(count),
    })
}
pub fn decorate_local_states(db_path:&Path, page:&mut RemotePage)->Result<()>{
    let db=Catalog::open(db_path)?;
    for item in &mut page.items {
        item.local_state=local_state(&db,&item.origin_key,&item.booth_url,
            &item.signatures)?;
    }
    Ok(())
}

pub fn get_asset(conn:&RemoteConnection,id:u64)->Result<RemoteAsset>{
    let asset:RemoteAsset=limited_json(response(conn,&format!("/v1/assets/{id}"))?)?;
    if asset.id!=id || asset.origin_key.len()!=64
        || !asset.origin_key.bytes().all(|c|c.is_ascii_hexdigit())
        || asset.files.len()>MAX_FILES || !CATEGORIES.contains(&asset.category.as_str())
        || asset.avatars.len()>64 || asset.title.len()>512
        || asset.files.iter().any(|f|f.blake3.len()!=64
            || !f.blake3.bytes().all(|c|c.is_ascii_hexdigit())) {
        bail!("Ficha remota no válida");
    }
    Ok(asset)
}
/// Decode untrusted covers only after checking header dimensions. Prevents
/// decompressing a tiny compressed image into an enormous pixel allocation.
/// Same validation, resize filter and 64px blurred backdrop as native covers.
/// Decode only after header dimension checks. This work runs off the UI thread.
pub fn decode_cover_prepared(bytes:&[u8],max_side:u32)
    ->Option<(image::RgbaImage,image::RgbaImage)>{
    if bytes.len() as u64>MAX_COVER_BYTES || max_side==0 {return None;}
    let header=image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format().ok()?;
    let (width,height)=header.into_dimensions().ok()?;
    if width==0 || height==0 || width>8_192 || height>8_192
        || width as u64*height as u64>24_000_000 {return None;}
    let original=image::load_from_memory(bytes).ok()?;
    let resized=if width>max_side || height>max_side{
        original.resize(max_side,max_side,image::imageops::FilterType::Triangle)
    }else{original};
    let small=resized.resize_to_fill(64,64,image::imageops::FilterType::Triangle)
        .to_rgba8();
    let backdrop=image::imageops::blur(&small,5.0);
    Some((resized.to_rgba8(),backdrop))
}
pub fn decode_cover_small(bytes:&[u8],max_side:u32)->Option<image::RgbaImage>{
    decode_cover_prepared(bytes,max_side).map(|(main,_)|main)
}

pub fn fetch_cover(conn:&RemoteConnection,id:u64)->Result<Option<Vec<u8>>>{
    let mut url=conn.base.clone();
    url.set_path(&format!("{}/v1/covers/{id}",conn.base.path()));
    // Cloudflare or a busy host can return a temporary 503/429 or disconnect.
    // Retry transient failures, but never retry missing or oversized images.
    for attempt in 0..3 {
        let received=conn.client.get(url.clone()).header(KEY_HEADER,&conn.key)
            .timeout(Duration::from_secs(25)).send();
        let reply=match received {
            Ok(reply)=>reply,
            Err(err) if attempt<2=>{
                let _=err;
                thread::sleep(Duration::from_millis(250*(attempt+1)));
                continue;
            },
            Err(err)=>return Err(err.into()),
        };
        let status=reply.status();
        if status==reqwest::StatusCode::NOT_FOUND {return Ok(None);}
        if !status.is_success(){
            if attempt<2 && (status==reqwest::StatusCode::TOO_MANY_REQUESTS
                || status==reqwest::StatusCode::REQUEST_TIMEOUT || status.is_server_error()) {
                thread::sleep(Duration::from_millis(250*(attempt+1)));
                continue;
            }
            bail!("Error HTTP al descargar portada: {status}");
        }
        if reply.content_length().is_some_and(|n|n>MAX_COVER_BYTES) {
            bail!("Portada remota demasiado grande");
        }
        let mut bytes=Vec::new();
        match reply.take(MAX_COVER_BYTES+1).read_to_end(&mut bytes){
            Ok(_)=>{},
            Err(_) if attempt<2=>{
                thread::sleep(Duration::from_millis(250*(attempt+1)));
                continue;
            },
            Err(err)=>return Err(err.into()),
        }
        if bytes.len() as u64>MAX_COVER_BYTES {bail!("Portada remota demasiado grande");}
        return Ok(Some(bytes));
    }
    bail!("No se pudo descargar la portada tras varios intentos")
}

fn download_file(conn:&RemoteConnection,file:&RemoteFile,dest:&Path,cancel:&AtomicBool)
    ->Result<()>{
    if !approved_name(&file.name) || file.size>MAX_FILE_BYTES ||
        file.blake3.len()!=64 || !file.blake3.bytes().all(|c|c.is_ascii_hexdigit()) {
        bail!("Archivo remoto no válido");
    }
    let mut source=response(conn,&format!("/v1/files/{}",file.id))?;
    let mut target=File::create(dest)?;
    let mut native=blake3::Hasher::new();
    let mut remaining=file.size;
    let mut buf=[0u8;64*1024];
    loop{
        if cancel.load(Ordering::Relaxed){bail!("Descarga cancelada");}
        let count=source.read(&mut buf)?;
        if count==0{break;}
        if count as u64>remaining{bail!("Archivo remoto demasiado grande");}
        target.write_all(&buf[..count])?;
        native.update(&buf[..count]);
        remaining-=count as u64;
    }
    target.flush()?;
    target.sync_all()?;
    if remaining!=0 || native.finalize().to_hex().as_str()!=file.blake3{
        bail!("Archivo incompleto o hash BLAKE3 incorrecto");
    }
    Ok(())
}
/// Validate downloaded containers before changing the destination library.
fn archive_entry_count(path:&Path)->Result<i64>{
    let ext=path.extension().and_then(|ext|ext.to_str()).unwrap_or("");
    let count:i64=if ext.eq_ignore_ascii_case("zip"){
        i64::try_from(zip::ZipArchive::new(File::open(path)?)
            .context("ZIP remoto inválido")?.len())?
    }else if ext.eq_ignore_ascii_case("7z"){
        crate::package_inspect::sevenzip_count(path)?
    }else if ext.eq_ignore_ascii_case("rar"){
        crate::package_inspect::rar_count(path)?
    }else{0};
    Ok(count)
}

/// Verify registered local content before trusting the SQLite cache. Never
/// skip a download based only on a filename, timestamp or stale stored hash.
fn local_copy_is_valid(attachments:&[crate::db::Attachment],file:&RemoteFile,
    cancel:&AtomicBool)->bool{
    attachments.iter().any(|entry|{
        if entry.checksum!=file.blake3
            || u64::try_from(entry.bytes).ok()!=Some(file.size){return false;}
        let path=Path::new(&entry.stored_path);
        if regular_file(path,MAX_FILE_BYTES).ok()!=Some(file.size){return false;}
        file_digest(path,cancel).ok().is_some_and(|native|
            native==file.blake3)
    })
}

/// Accurate detail check, including hash verification on disk. Called only
/// AFTER the remote detail becomes visible, never in the manifest fetch.
pub fn verified_local_state_with_cancel(db_path:&Path,asset:&RemoteAsset,
    cancel:&AtomicBool)->Result<RemoteLocalState>{
    if cancel.load(Ordering::Relaxed){bail!("Comprobación cancelada");}
    let db=Catalog::open(db_path)?;
    let Some(id)=db.resolve_remote_product(&asset.origin_key,&asset.booth_url)?
        else{return Ok(RemoteLocalState::New)};
    let attachments=db.attachments_for(id)?;
    let mut pending=0;
    for file in &asset.files {
        if cancel.load(Ordering::Relaxed){bail!("Comprobación cancelada");}
        if !local_copy_is_valid(&attachments,file,cancel){pending+=1;}
    }
    if cancel.load(Ordering::Relaxed){bail!("Comprobación cancelada");}
    Ok(if pending==0{RemoteLocalState::Complete}
        else{RemoteLocalState::NeedsUpdate(pending)})
}
/// Convenience for callers not requiring cancellation.
pub fn verified_local_state(db_path:&Path,asset:&RemoteAsset)->Result<RemoteLocalState>{
    verified_local_state_with_cancel(db_path,asset,&AtomicBool::new(false))
}

/// Import or update the complete product, never a subset of its files.
/// Downloads are fully verified before one atomic SQLite transaction.
pub fn import_asset(conn:&RemoteConnection,id:u64,db_path:&Path,
    library:&Path,covers_dir:&Path,cancel:&AtomicBool)->Result<RemoteCommitReport>{
    let asset=get_asset(conn,id)?;
    if asset.description.len()>32_000 || asset.tags.len()>8_000
        || asset.creator.len()>512 || asset.avatars.iter().any(|n|n.len()>70)
        || asset.files.len()>MAX_FILES {
        bail!("Metadatos remotos demasiado grandes");
    }
    if !asset.booth_url.is_empty(){booth::validate_booth_url(&asset.booth_url)?;}
    let total=asset.files.iter().try_fold(0u64,|sum,file|
        sum.checked_add(file.size)).context("Tamaño inválido")?;
    if total>MAX_TOTAL_BYTES{bail!("Ficha demasiado grande");}
    let mut db=Catalog::open(db_path)?;
    let configured=db.storage_root()?.context("Configura la carpeta de assets")?;
    fs::create_dir_all(library)?;
    if fs::canonicalize(library)?!=configured{
        bail!("La carpeta principal de assets ha cambiado");
    }
    let existing=db.resolve_remote_product(&asset.origin_key,&asset.booth_url)?;
    let local=if let Some(id)=existing{db.attachments_for(id)?}else{Vec::new()};
    let staging=tempfile::tempdir_in(library)?;
    let mut downloaded=Vec::new();
    for file in &asset.files{
        if cancel.load(Ordering::Relaxed){bail!("Importación cancelada");}
        if local_copy_is_valid(&local,file,cancel){continue;}
        let folder=staging.path().join(file.id.to_string());
        fs::create_dir(&folder)?;
        let dest=folder.join(&file.name);
        download_file(conn,file,&dest,cancel)?;
        let file_count=archive_entry_count(&dest)?;
        downloaded.push(RemoteStagedFile{
            original_name:file.name.clone(),source:dest,
            blake3:file.blake3.clone(),bytes:file.size,file_count,
        });
    }
    let cover=if existing.is_none() && asset.has_cover{
        let bytes=fetch_cover(conn,id)?.context("La portada publicada no está disponible")?;
        if decode_cover_small(&bytes,320).is_none(){bail!("Portada remota dañada");}
        let ext=match image::guess_format(&bytes)?{
            image::ImageFormat::Png=>"png",
            image::ImageFormat::Jpeg=>"jpg",
            image::ImageFormat::WebP=>"webp",
            _=>bail!("Formato de portada no permitido"),
        };
        let dest=staging.path().join(format!("cover.{ext}"));
        fs::write(&dest,bytes)?;
        Some((dest,ext.to_owned()))
    }else{None};
    if cancel.load(Ordering::Relaxed){bail!("Importación cancelada");}
    let record=RemoteImportRecord{
        origin_key:asset.origin_key,booth_url:asset.booth_url,
        title:asset.title,category:asset.category,creator:asset.creator,
        tags:asset.tags,description:asset.description,
        avatars:asset.avatars,nsfw:asset.nsfw,cover,
    };
    db.commit_remote_import(library,covers_dir,&record,&downloaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_stream_reader_stops_as_soon_as_session_is_revoked() {
        let tmp=tempfile::tempdir().unwrap();
        let path=tmp.path().join("file.bin");
        fs::write(&path,b"abcdef").unwrap();
        let stop=Arc::new(AtomicBool::new(false));
        let mut reader=RevocableReader{file:File::open(&path).unwrap(),
            stop:Arc::clone(&stop)};
        let mut bytes=[0u8;3];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes,b"abc");
        stop.store(true,Ordering::Relaxed);
        assert!(reader.read(&mut bytes).is_err());
    }

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
        let cloudflare=format!("https://library-demo.trycloudflare.com/l/{token}#key={secret}");
        assert!(lan_url(&cloudflare).is_ok());
        assert!(lan_url(&format!("https://library-demo.trycloudflare.com.evil.org/l/{token}#key={secret}")).is_err());
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
        selected.products.insert(public);
        let snapshot=snapshot(&db_path,&selected,"Biblioteca de prueba",&AtomicBool::new(false)).unwrap();
        assert_eq!(snapshot.info.products,1);
        assert_eq!(snapshot.assets.len(),1);
        assert_eq!(snapshot.assets[0].id,1);
        assert_eq!(snapshot.assets[0].title,"VISIBLE");
        assert!(snapshot.assets[0].files.is_empty());
        assert!(snapshot.files.is_empty());
        assert!(!serde_json::to_string(&snapshot.assets).unwrap().contains("PRIVADO"));
        assert!(db.get(hidden).unwrap().is_some());
    }

    #[test]
    fn private_lan_api_never_reveals_unselected_products_or_their_files() {
        use crate::db::{ArchiveMode,NewAttachment};
        let tmp=tempfile::tempdir().unwrap();
        let db_path=tmp.path().join("catalog.sqlite");
        let mut db=Catalog::open(&db_path).unwrap();
        let hidden=db.create_product("HIDDEN_ONLY","Ropa").unwrap();
        let visible=db.create_product("VISIBLE_ONLY","Ropa").unwrap();
        let public_path=tmp.path().join("public.zip");
        let secret_path=tmp.path().join("private.zip");
        let hidden_path=tmp.path().join("hidden_only.zip");
        fs::write(&public_path,b"public content").unwrap();
        fs::write(&secret_path,b"selected product attachment").unwrap();
        fs::write(&hidden_path,b"unselected product attachment").unwrap();
        for (product,name,path) in [
            (visible,"public.zip",&public_path),
            (visible,"private.zip",&secret_path),
            (hidden,"hidden_only.zip",&hidden_path),
        ] {
            let checksum=blake3::hash(&fs::read(path).unwrap()).to_hex().to_string();
            db.add_attachment(&NewAttachment {
                asset_id:product,name,path:&path.to_string_lossy(),checksum:&checksum,
                bytes:fs::metadata(path).unwrap().len() as i64,
                count:0,mode:ArchiveMode::Reference,
            }).unwrap();
        }
        let attachments=db.attachments_for(visible).unwrap();
        let public_id=attachments.iter().find(|f|f.original_name=="public.zip").unwrap().id;
        assert!(public_id>0);
        let mut selection=ShareSelection::default();
        selection.products.insert(visible);
        let session=start_library(&db_path,&selection,tmp.path(),false,"Biblioteca de prueba",&AtomicBool::new(false)).unwrap();
        let receiver=connect(&session.invitation).unwrap();
        assert_eq!(receiver.info.products,1);
        let page=browse(&receiver,"","","",FileFilter::All,true,0).unwrap();
        assert_eq!(page.total,1);
        assert_eq!(page.items[0].id,1);
        assert_eq!(page.items[0].title,"VISIBLE_ONLY");
        assert_eq!(page.items[0].files,2);
        let manifest=get_asset(&receiver,1).unwrap();
        assert_eq!(manifest.files.len(),2);
        assert!(manifest.files.iter().any(|file|file.name=="public.zip"));
        assert!(get_asset(&receiver,2).is_err());
        let serialized=serde_json::to_string(&manifest).unwrap();
        assert!(serialized.contains("private.zip"));
        assert!(!serialized.contains("HIDDEN_ONLY"));
        assert!(!serialized.contains("hidden_only.zip"));
        assert!(!serialized.contains(&public_path.to_string_lossy().to_string()));
        assert!(!serialized.contains(&secret_path.to_string_lossy().to_string()));
        let local_id_token=format!("{}/v1/library",receiver.base.as_str());
        let client=http_client().unwrap();
        assert_eq!(client.get(&local_id_token).send().unwrap().status(),
            reqwest::StatusCode::NOT_FOUND);
        assert_eq!(client.post(&local_id_token).header(KEY_HEADER,&receiver.key)
            .send().unwrap().status(),reqwest::StatusCode::NOT_FOUND);
        let missing=format!("{}/v1/files/99",receiver.base.as_str());
        assert_eq!(client.get(missing).header(KEY_HEADER,&receiver.key)
            .send().unwrap().status(),reqwest::StatusCode::NOT_FOUND);
        let allowed=response(&receiver,&format!("/v1/files/{}",manifest.files.iter().find(|file|file.name=="public.zip").unwrap().id)).unwrap();
        assert_eq!(allowed.bytes().unwrap().as_ref(),b"public content");
        session.stop();
        thread::sleep(Duration::from_millis(250));
        assert!(browse(&receiver,"","","",FileFilter::All,true,0).is_err());
    }

    #[test]
    fn remote_cover_decode_matches_local_dimensions_and_blurred_backdrop(){
        let img=image::DynamicImage::new_rgba8(1280,960);
        let mut writer=std::io::Cursor::new(Vec::new());
        img.write_to(&mut writer,image::ImageFormat::Png).unwrap();
        let (foreground,blur)=decode_cover_prepared(&writer.into_inner(),768).unwrap();
        assert_eq!(foreground.dimensions(),(768,576));
        assert_eq!(blur.dimensions(),(64,64));
    }

    #[test]
    fn cover_cap_matches_native_catalog_and_does_not_reject_nine_mib_images(){
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("big-cover.png");
        let file=File::create(&path).unwrap();
        file.set_len(9*1024*1024).unwrap();
        assert!(permitted_cover(&path),
            "Local catalog permits covers up to 32 MiB, so the host must too");
        assert!(decode_cover_prepared(b"invalid png bytes",360).is_none());
    }

    #[test]
    fn booth_img_cover_is_published_with_real_mime_not_filename(){
        let tmp=tempfile::tempdir().unwrap();
        let db_path=tmp.path().join("catalog.sqlite");
        let mut db=Catalog::open(&db_path).unwrap();
        let product=db.create_product("BOOTH original","Ropa").unwrap();
        let cover=tmp.path().join("v2-original.img");
        let mut writer=std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(24,24)
            .write_to(&mut writer,image::ImageFormat::Png).unwrap();
        let data=writer.into_inner();
        fs::write(&cover,&data).unwrap();
        db.set_cover(product,&cover.to_string_lossy()).unwrap();
        let mut selection=ShareSelection::default();
        selection.products.insert(product);
        let session=start_library(&db_path,&selection,tmp.path(),false,"Portadas",
            &AtomicBool::new(false)).unwrap();
        let conn=connect(&session.invitation).unwrap();
        let page=browse(&conn,"","","",FileFilter::All,false,0).unwrap();
        assert!(page.items[0].has_cover,"BOOTH .img must appear in remote catalog");
        let url=format!("{}/v1/covers/1",conn.base.as_str());
        let response=conn.client.get(&url).header(KEY_HEADER,&conn.key).send().unwrap();
        assert_eq!(response.status(),reqwest::StatusCode::OK);
        assert_eq!(response.headers()[reqwest::header::CONTENT_TYPE],"image/png");
        assert_eq!(response.bytes().unwrap().as_ref(),data);
        assert!(decode_cover_prepared(&fetch_cover(&conn,1).unwrap().unwrap(),360).is_some());
        session.stop();
    }

    #[test]
    fn invalid_img_is_not_disclosed_as_image(){
        let temp=tempfile::tempdir().unwrap();
        let db_path=temp.path().join("catalog.sqlite");
        let mut db=Catalog::open(&db_path).unwrap();
        let id=db.create_product("Bad image","Ropa").unwrap();
        let bogus=temp.path().join("v2-not-an-image.img");
        fs::write(&bogus,b"private non-image content").unwrap();
        db.set_cover(id,&bogus.to_string_lossy()).unwrap();
        let mut choice=ShareSelection::default();
        choice.products.insert(id);
        let session=start_library(&db_path,&choice,temp.path(),false,"Portadas",
            &AtomicBool::new(false)).unwrap();
        let conn=connect(&session.invitation).unwrap();
        assert!(fetch_cover(&conn,1).unwrap().is_none());
        session.stop();
    }

    #[test]
    fn remote_cover_retries_temporary_503_errors(){
        let server=Server::http("127.0.0.1:0").unwrap();
        let address=server.server_addr().to_ip().unwrap();
        let endpoint=format!("http://{address}/l/test");
        let worker=thread::spawn(move||{
            for attempt in 0..3{
                let request=server.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
                assert_eq!(request.url(),"/l/test/v1/covers/1");
                let reply=if attempt<2{
                    Response::from_data(b"busy".to_vec()).with_status_code(StatusCode(503))
                }else{Response::from_data(b"cover".to_vec())};
                request.respond(reply).unwrap();
            }
        });
        let conn=RemoteConnection{
            base:Url::parse(&endpoint).unwrap(),key:"test".into(),
            client:http_client().unwrap(),info:LibraryInfo{
                version:PROTOCOL_VERSION,name:String::new(),products:1,avatars:Vec::new(),
            },invitation:String::new(),
        };
        assert_eq!(fetch_cover(&conn,1).unwrap(),Some(b"cover".to_vec()));
        worker.join().unwrap();
    }

    #[test]
    fn hosted_manifest_uses_sqlite_blake3_without_reading_or_rehashing_file() {
        use crate::db::{ArchiveMode,NewAttachment};
        let tmp=tempfile::tempdir().unwrap();
        let host_db=tmp.path().join("host.sqlite");
        let client_db=tmp.path().join("client.sqlite");
        let local_root=tmp.path().join("client-files");
        let cover_root=tmp.path().join("covers");
        let archive=tmp.path().join("example.zip");
        valid_zip(&archive,b"original content");
        let mut db=Catalog::open(&host_db).unwrap();
        let id=db.create_product("Unchanged metadata","Ropa").unwrap();
        // Simulate stale external metadata: the host must not scan all files
        // before it can publish a catalog, even when the checksum is stale.
        let stale="a".repeat(64);
        db.add_attachment(&NewAttachment{
            asset_id:id,name:"example.zip",path:&archive.to_string_lossy(),
            checksum:&stale,bytes:fs::metadata(&archive).unwrap().len() as i64,
            count:1,mode:ArchiveMode::Reference,
        }).unwrap();
        let mut allowed=ShareSelection::default();
        allowed.products.insert(id);
        let session=start_library(&host_db,&allowed,tmp.path(),false,
            "Lazy catalog",&AtomicBool::new(false)).unwrap();
        let conn=connect(&session.invitation).unwrap();
        assert_eq!(conn.info.version,PROTOCOL_VERSION);
        let remote=get_asset(&conn,1).unwrap();
        assert_eq!(remote.files.len(),1);
        assert_eq!(remote.files[0].blake3,stale);
        let mut local=Catalog::open(&client_db).unwrap();
        local.configure_storage(&local_root).unwrap();
        drop(local);
        assert!(import_asset(&conn,1,&client_db,&local_root,&cover_root,
            &AtomicBool::new(false)).is_err());
        assert_eq!(Catalog::open(&client_db).unwrap().count().unwrap(),0);
    }

    #[cfg(target_os="linux")]
    #[test]
    fn fifty_gib_library_opens_without_reading_archive_bytes() {
        // Linux ftruncate creates a sparse file: this test does not allocate
        // fifty GiB or download anything. A startup hash would be prohibitively
        // slow and would fail the CI timeout.
        use crate::db::{ArchiveMode,NewAttachment};
        let tmp=tempfile::tempdir().unwrap();
        let db_path=tmp.path().join("host.sqlite");
        let file_path=tmp.path().join("sparse.zip");
        let bytes=50u64*1024*1024*1024;
        File::create(&file_path).unwrap().set_len(bytes).unwrap();
        let mut db=Catalog::open(&db_path).unwrap();
        let id=db.create_product("Large library","Ropa").unwrap();
        let digest="b".repeat(64);
        db.add_attachment(&NewAttachment{
            asset_id:id,name:"sparse.zip",path:&file_path.to_string_lossy(),
            checksum:&digest,bytes:bytes as i64,count:0,mode:ArchiveMode::Reference,
        }).unwrap();
        let mut selected=ShareSelection::default();
        selected.products.insert(id);
        let session=start_library(&db_path,&selected,tmp.path(),false,
            "50 GiB",&AtomicBool::new(false)).unwrap();
        let conn=connect(&session.invitation).unwrap();
        let remote=get_asset(&conn,1).unwrap();
        assert_eq!(remote.files[0].size,bytes);
        assert_eq!(remote.files[0].blake3,digest);
        assert_eq!(browse(&conn,"","","",FileFilter::All,true,0)
            .unwrap().items[0].total_bytes,bytes);
    }

    #[test]
    fn whole_product_without_files_imports_all_available_metadata() {
        let tmp=tempfile::tempdir().unwrap();
        let host=tmp.path().join("host.sqlite");
        let own=tmp.path().join("my-library.sqlite");
        let mut db=Catalog::open(&host).unwrap();
        let public=db.create_product("Remote product","Ropa").unwrap();
        db.set_avatars(public,"MANUKA").unwrap();
        db.update_details(public,"Remote product","Ropa","Creator","test","Description").unwrap();
        drop(db);
        let mut selected=ShareSelection::default();
        selected.products.insert(public);
        let session=start_library(&host,&selected,tmp.path(),false,"Biblioteca de prueba",&AtomicBool::new(false)).unwrap();
        let conn=connect(&session.invitation).unwrap();
        let destination=tmp.path().join("assets");
        let covers=tmp.path().join("covers");
        let mut local_db=Catalog::open(&own).unwrap();
        local_db.configure_storage(&destination).unwrap();
        drop(local_db);
        let imported=import_asset(&conn,1,&own,&destination,&covers,
            &AtomicBool::new(false)).unwrap().asset_id;
        let ours=Catalog::open(&own).unwrap();
        let saved=ours.get(imported).unwrap().unwrap();
        assert_eq!(saved.title,"Remote product");
        assert_eq!(saved.avatars,"MANUKA");
        assert!(ours.attachments_for(imported).unwrap().is_empty());
        assert_eq!(Catalog::open(&host).unwrap().count().unwrap(),1);
    }

    fn valid_zip(path:&Path,body:&[u8]){
        use std::io::Write;
        let target=File::create(path).unwrap();
        let mut zip=zip::ZipWriter::new(target);
        zip.start_file("Assets/costume.txt",zip::write::FileOptions::default()).unwrap();
        zip.write_all(body).unwrap();
        zip.finish().unwrap();
    }
    fn attach(db:&mut Catalog,asset_id:i64,path:&Path,name:&str){
        use crate::db::{NewAttachment,ArchiveMode};
        let bytes=fs::read(path).unwrap();
        let hash=blake3::hash(&bytes).to_hex().to_string();
        db.add_attachment(&NewAttachment{
            asset_id,name,path:&path.to_string_lossy(),checksum:&hash,
            bytes:bytes.len() as i64,count:1,mode:ArchiveMode::Reference,
        }).unwrap();
    }
    #[test]
    fn remote_import_round_trip_detects_existing_files_and_new_versions(){
        let temp=tempfile::tempdir().unwrap();
        let host_db=temp.path().join("host.sqlite");
        let client_db=temp.path().join("client.sqlite");
        let root=temp.path().join("my-assets");
        let covers=temp.path().join("my-covers");
        let v1=temp.path().join("v1.zip");
        let v2=temp.path().join("v2.zip");
        valid_zip(&v1,b"version-1");
        valid_zip(&v2,b"version-2");
        let mut host=Catalog::open(&host_db).unwrap();
        let id=host.create_product("Costume","Ropa").unwrap();
        attach(&mut host,id,&v1,"costume.zip");
        let mut own=Catalog::open(&client_db).unwrap();
        own.configure_storage(&root).unwrap();
        drop(own);
        let mut selection=ShareSelection::default();
        selection.products.insert(id);
        let first=start_library(&host_db,&selection,temp.path(),false,
            "Colección", &AtomicBool::new(false)).unwrap();
        let connection=connect(&first.invitation).unwrap();
        let remote=get_asset(&connection,1).unwrap();
        let first_key=remote.origin_key.clone();
        let done=import_asset(&connection,1,&client_db,&root,&covers,
            &AtomicBool::new(false)).unwrap();
        assert!(done.is_new);
        assert_eq!(done.new_files,1);
        let same=import_asset(&connection,1,&client_db,&root,&covers,
            &AtomicBool::new(false)).unwrap();
        assert_eq!(same.asset_id,done.asset_id);
        assert_eq!(same.new_files,0);
        let mut client=Catalog::open(&client_db).unwrap();
        client.update_details(done.asset_id,"Mi título manual","Ropa","", "", "").unwrap();
        let old_bytes=fs::read(&client.attachments_for(done.asset_id).unwrap()[0].stored_path)
            .unwrap();
        let old_hash=blake3::hash(&old_bytes);
        drop(client);
        first.stop();
        drop(first);
        let old=host.attachments_for(id).unwrap()[0].id;
        host.detach(old,id).unwrap();
        attach(&mut host,id,&v2,"costume.zip");
        drop(host);
        let next=start_library(&host_db,&selection,temp.path(),false,
            "Colección", &AtomicBool::new(false)).unwrap();
        let connection2=connect(&next.invitation).unwrap();
        let latest=get_asset(&connection2,1).unwrap();
        assert_eq!(latest.origin_key,first_key);
        let pending=verified_local_state(&client_db,&latest).unwrap();
        assert_eq!(pending,RemoteLocalState::NeedsUpdate(1));
        let update=import_asset(&connection2,1,&client_db,&root,&covers,
            &AtomicBool::new(false)).unwrap();
        assert_eq!(update.asset_id,done.asset_id);
        assert_eq!(update.new_files,1);
        assert!(!update.is_new);
        let client=Catalog::open(&client_db).unwrap();
        assert_eq!(client.get(done.asset_id).unwrap().unwrap().title,"Mi título manual");
        let local_files=client.attachments_for(done.asset_id).unwrap();
        assert_eq!(local_files.len(),2);
        assert!(local_files.iter().any(|file|
            file.checksum==old_hash.to_hex().to_string()));
        assert_eq!(verified_local_state(&client_db,&latest).unwrap(),
            RemoteLocalState::Complete);
    }
    #[test]
    fn invalid_second_remote_archive_never_creates_partial_product(){
        let temp=tempfile::tempdir().unwrap();
        let host_db=temp.path().join("host.sqlite");
        let client_db=temp.path().join("client.sqlite");
        let root=temp.path().join("assets");
        let covers=temp.path().join("covers");
        let first=temp.path().join("first.zip");
        let bad=temp.path().join("bad.zip");
        valid_zip(&first,b"ok");
        fs::write(&bad,b"not a ZIP").unwrap();
        let mut host=Catalog::open(&host_db).unwrap();
        let id=host.create_product("Remote","Ropa").unwrap();
        attach(&mut host,id,&first,"first.zip");
        attach(&mut host,id,&bad,"bad.zip");
        drop(host);
        let mut own=Catalog::open(&client_db).unwrap();
        own.configure_storage(&root).unwrap();
        drop(own);
        let mut selection=ShareSelection::default();
        selection.products.insert(id);
        let server=start_library(&host_db,&selection,temp.path(),false,
            "Test",&AtomicBool::new(false)).unwrap();
        let connection=connect(&server.invitation).unwrap();
        assert!(import_asset(&connection,1,&client_db,&root,&covers,
            &AtomicBool::new(false)).is_err());
        let client=Catalog::open(&client_db).unwrap();
        assert_eq!(client.count().unwrap(),0);
    }

}
