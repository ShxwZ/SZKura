//! Opt-in peer-to-peer asset transfer through a temporary Cloudflare Quick Tunnel.
//! No component is downloaded or process started unless explicitly requested by the user.
use crate::{booth, db::{Catalog, CATEGORIES}, managed_storage};
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rand::{rngs::OsRng, RngCore};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File}, io::{Read, Write}, path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{atomic::{AtomicBool, AtomicU32, Ordering}, mpsc, Arc, Mutex},
    thread, time::{Duration, Instant},
};
use tiny_http::{Header, Response, Server, StatusCode};
use url::Url;

pub const CLOUDFLARED_VERSION: &str = "2026.10.7";
const MAX_COMPONENT_BYTES: u64 = 100_000_000;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_FILES: usize = 32;

// Pinned SHA-256 values published in the official Cloudflare release notes.
// Updates require a deliberate source-code change and a new user consent.
#[cfg(target_os = "windows")]
const COMPONENT_NAME: &str = "cloudflared-windows-amd64.exe";
#[cfg(target_os = "windows")]
const COMPONENT_SHA256: &str = "2493dbc168923bb3e59e5e7716f71deced92992eea9acd6e9d74f84ac7751343";
#[cfg(target_os = "linux")]
const COMPONENT_NAME: &str = "cloudflared-linux-amd64";
#[cfg(target_os = "linux")]
const COMPONENT_SHA256: &str = "1f99fb518b83dec2ba8d870a7cc991d82a77e6196295a3d2754ba17ab9f91872";
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
const COMPONENT_NAME: &str = "";
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
const COMPONENT_SHA256: &str = "";

pub fn component_path(data_dir: &Path) -> PathBuf {
    data_dir.join("components").join(if cfg!(windows) { "cloudflared.exe" } else { "cloudflared" })
}
pub fn component_present(data_dir: &Path) -> bool { component_path(data_dir).is_file() }

fn digest(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0_u8; 131_072];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 { break; }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn component_verified(data_dir: &Path) -> Result<PathBuf> {
    if COMPONENT_NAME.is_empty() || !cfg!(target_arch = "x86_64") {
        bail!("Cloudflare Tunnel integrado solo está disponible en Windows/Linux x64");
    }
    let path = component_path(data_dir);
    if !path.is_file() { bail!("Cloudflare Tunnel aún no está descargado"); }
    if digest(&path)? != COMPONENT_SHA256 {
        bail!("Cloudflare Tunnel no supera la verificación SHA-256. Elimina el componente desde Ajustes e instálalo de nuevo");
    }
    Ok(path)
}

/// Must be called only after the user accepted the explicit download dialog.
pub fn install_component_with_consent(data_dir: &Path) -> Result<()> {
    if COMPONENT_NAME.is_empty() || !cfg!(target_arch = "x86_64") {
        bail!("Plataforma no compatible con Cloudflare Tunnel integrado");
    }
    if component_path(data_dir).is_file() { return component_verified(data_dir).map(|_| ()); }
    let url = format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{CLOUDFLARED_VERSION}/{COMPONENT_NAME}"
    );
    let client = Client::builder().timeout(Duration::from_secs(180))
        .user_agent("SZKura/0.2 opt-in tunnel installer").build()?;
    let mut response = client.get(&url).send()?.error_for_status()?;
    if response.content_length().is_some_and(|len| len > MAX_COMPONENT_BYTES) {
        bail!("El componente supera el tamaño máximo permitido");
    }
    let parent = component_path(data_dir).parent().context("Ruta de componentes inválida")?.to_owned();
    fs::create_dir_all(&parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(&parent)?;
    let bytes = std::io::copy(&mut response.by_ref().take(MAX_COMPONENT_BYTES + 1), &mut temporary)?;
    if bytes == 0 || bytes > MAX_COMPONENT_BYTES { bail!("Descarga vacía o demasiado grande"); }
    temporary.as_file_mut().sync_all()?;
    if digest(temporary.path())? != COMPONENT_SHA256 {
        bail!("La descarga no coincide con el SHA-256 oficial; no se ejecutará");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary.as_file().set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    let target = component_path(data_dir);
    temporary.persist_noclobber(&target).map_err(|e| e.error)
        .context("El componente ya existe; verifica o elimínalo antes de reintentar")?;
    Ok(())
}

pub fn remove_component(data_dir: &Path) -> Result<()> {
    let path = component_path(data_dir);
    if path.exists() { fs::remove_file(path)?; }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransferFile {
    pub index: usize,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransferCover {
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Manifest {
    pub version: u32,
    pub title: String,
    pub creator: String,
    pub category: String,
    pub tags: String,
    pub description: String,
    pub avatars: Vec<String>,
    pub booth_url: String,
    pub nsfw: bool,
    pub cover: Option<TransferCover>,
    pub files: Vec<TransferFile>,
}
struct Offer {
    manifest: Manifest,
    paths: Vec<PathBuf>,
    cover: Option<PathBuf>,
}

fn valid_filename(s: &str) -> bool {
    !s.is_empty() && s.len() <= 240 && s != "." && s != ".." &&
        !s.chars().any(|c| matches!(c,'/'|'\\'|'\0'|'\r'|'\n'|':')) &&
        !s.chars().any(char::is_control) &&
        Path::new(s).file_name().is_some_and(|part| part.to_string_lossy() == s)
}
fn validate_manifest(m: &Manifest) -> Result<()> {
    if m.version != 1 || m.title.trim().is_empty() || m.title.len() > 250 ||
       m.creator.len() > 250 || m.tags.len() > 3_000 || m.description.len() > 20_000 ||
       !CATEGORIES.contains(&m.category.as_str()) || m.avatars.len() > 64 ||
       m.avatars.iter().any(|v| v.len() > 140 || v.is_empty()) ||
       m.files.len() > MAX_FILES {
        bail!("El manifiesto no es compatible o excede los límites de seguridad");
    }
    if !m.booth_url.is_empty() { booth::validate_booth_url(&m.booth_url)?; }
    let mut total = 0_u64;
    for (i, file) in m.files.iter().enumerate() {
        if file.index != i || !valid_filename(&file.name) ||
           !crate::import::supported(Path::new(&file.name)) ||
           file.size > MAX_FILE_BYTES || file.sha256.len() != 64 ||
           !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("Archivo remoto inválido en el manifiesto");
        }
        total = total.checked_add(file.size).context("Tamaño total inválido")?;
    }
    if total > MAX_TOTAL_BYTES { bail!("La transferencia supera 16 GiB"); }
    if let Some(cover) = &m.cover {
        if cover.size > 8_000_000 || cover.sha256.len() != 64 ||
           !cover.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("Portada remota inválida");
        }
    }
    Ok(())
}

fn offer(db_path: &Path, asset_id: i64) -> Result<Offer> {
    let db = Catalog::open(db_path)?;
    let item = db.get(asset_id)?.context("No se encontró el producto")?;
    let attachments = db.attachments_for(asset_id)?;
    if attachments.is_empty() { bail!("El producto no tiene archivos para compartir"); }
    let mut files = Vec::new();
    let mut paths = Vec::new();
    for attachment in attachments {
        let path = PathBuf::from(&attachment.stored_path);
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            bail!("Adjunto no disponible o enlace simbólico: {}", attachment.original_name);
        }
        let name = attachment.original_name;
        if !valid_filename(&name) { bail!("Nombre de adjunto no permitido: {name}"); }
        files.push(TransferFile {
            index: files.len(), name, size: meta.len(), sha256: digest(&path)?,
        });
        paths.push(path);
    }
    let (cover, cover_info) = if !item.image_path.is_empty() {
        let path = PathBuf::from(&item.image_path);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 8_000_000 =>
                (Some(path.clone()), Some(TransferCover {size:meta.len(),sha256:digest(&path)?})),
            _ => (None, None),
        }
    } else { (None, None) };
    let manifest = Manifest {
        version:1,title:item.title,creator:item.creator,category:item.category,
        tags:item.tags,description:item.description,avatars:db.avatars_for(asset_id)?,
        booth_url:item.booth_url,nsfw:item.nsfw,cover:cover_info,files,
    };
    validate_manifest(&manifest)?;
    if (serde_json::to_vec(&manifest)?.len() as u64) > MAX_MANIFEST_BYTES {
        bail!("El manifiesto es demasiado grande");
    }
    Ok(Offer {manifest,paths,cover})
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("Header constante válida")
}
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;")
     .replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}
fn valid_token(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn parse_public_url(raw: &str) -> Result<Url> {
    let u = Url::parse(raw.trim()).context("Enlace no válido")?;
    let host = u.host_str().unwrap_or("");
    if u.scheme() != "https" || !host.ends_with(".trycloudflare.com") ||
       u.port().is_some_and(|p| p != 443) || !u.username().is_empty() ||
       u.password().is_some() || u.query().is_some() || u.fragment().is_some() {
        bail!("Solo se admiten enlaces HTTPS de SZKura en trycloudflare.com");
    }
    let parts = u.path().split('/').collect::<Vec<_>>();
    if parts.len() != 3 || parts[1] != "s" || !valid_token(parts[2]) {
        bail!("El enlace no tiene el formato de transferencia SZKura");
    }
    Ok(u)
}
fn client() -> Result<Client> {
    Ok(Client::builder().redirect(Policy::none())
        .timeout(Duration::from_secs(60 * 30))
        .user_agent("SZKura/0.2 asset transfer").build()?)
}
fn get(client: &Client, url: Url, password: &str) -> Result<reqwest::blocking::Response> {
    let mut req = client.get(url).header("Accept", "application/octet-stream");
    if !password.is_empty() { req = req.basic_auth("szkura", Some(password)); }
    Ok(req.send()?.error_for_status()?)
}
pub fn fetch_manifest(link: &str, password: &str) -> Result<Manifest> {
    let mut url = parse_public_url(link)?;
    url.set_path(&format!("{}/manifest", url.path()));
    let mut result = get(&client()?, url, password)?;
    if result.content_length().is_some_and(|n| n > MAX_MANIFEST_BYTES) {
        bail!("Manifiesto remoto demasiado grande");
    }
    let mut data = Vec::new();
    result.by_ref().take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_MANIFEST_BYTES { bail!("Manifiesto remoto demasiado grande"); }
    let m: Manifest = serde_json::from_slice(&data)?;
    validate_manifest(&m)?;
    Ok(m)
}

fn download_verified(client: &Client, url: Url, password: &str, target: &Path,
    expected_size: u64, expected_hash: &str) -> Result<()> {
    let mut response = get(client, url, password)?;
    if response.content_length().is_some_and(|n| n != expected_size) {
        bail!("Tamaño remoto distinto del manifiesto");
    }
    let mut file = File::create(target)?;
    let mut hasher = Sha256::new();
    let mut count = 0_u64;
    let mut buf = [0_u8; 131_072];
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 { break; }
        count = count.checked_add(n as u64).context("Archivo demasiado grande")?;
        if count > expected_size { bail!("Archivo remoto mayor de lo anunciado"); }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
    }
    file.sync_all()?;
    if count != expected_size || format!("{:x}", hasher.finalize()) != expected_hash {
        bail!("El archivo descargado no coincide con su tamaño o SHA-256");
    }
    Ok(())
}

/// Downloads all selected files to a temporary directory, verifies them, then
/// attaches managed copies. Existing BOOTH records are never overwritten.
pub fn import_shared(link: &str, password: &str, db_path: &Path,
    library: &Path, covers_dir: &Path) -> Result<i64> {
    let base = parse_public_url(link)?;
    let manifest = fetch_manifest(link, password)?;
    let client = client()?;
    let temp = tempfile::tempdir_in(library).context("No se pudo preparar descarga temporal")?;
    let mut paths = Vec::new();
    for file in &manifest.files {
        let folder = temp.path().join(file.index.to_string());
        fs::create_dir(&folder)?;
        let target = folder.join(&file.name);
        let mut url = base.clone();
        url.set_path(&format!("{}/file/{}", base.path(),file.index));
        download_verified(&client,url,password,&target,file.size,&file.sha256)?;
        // Reject damaged archives before adding any product to the local catalog.
        let extension = target.extension().and_then(|s|s.to_str()).unwrap_or("").to_ascii_lowercase();
        match extension.as_str() {
            "zip" => {
                let archive = zip::ZipArchive::new(File::open(&target)?)
                    .context("ZIP remoto dañado")?;
                if archive.len() > 100_000 { bail!("ZIP remoto con demasiadas entradas"); }
            },
            "7z" => { crate::package_inspect::sevenzip_count(&target)?; },
            "rar" => { crate::package_inspect::rar_count(&target)?; },
            "unitypackage" => {},
            _ => bail!("Extensión no permitida en la transferencia"),
        }
        paths.push(target);
    }
    let mut cover_bytes = None;
    if let Some(info) = &manifest.cover {
        let mut url = base.clone();
        url.set_path(&format!("{}/cover",base.path()));
        let target = temp.path().join("cover.bin");
        download_verified(&client,url,password,&target,info.size,&info.sha256)?;
        let bytes = fs::read(&target)?;
        let decoded = image::load_from_memory(&bytes)?;
        if decoded.width() == 0 || decoded.height() == 0 ||
           decoded.width() > 6000 || decoded.height() > 6000 {
            bail!("Portada remota con dimensiones inválidas");
        }
        cover_bytes = Some(bytes);
    }
    let mut db = Catalog::open(db_path)?;
    let existing = if manifest.booth_url.is_empty() {
        None
    } else {
        db.existing_booth_link(&manifest.booth_url)?
    };
    let id = match existing {
        Some(id) => id,
        None => {
            let id = db.create_product(&manifest.title,&manifest.category)?;
            db.update_details(id,&manifest.title,&manifest.category,&manifest.creator,
                &manifest.tags,&manifest.description)?;
            db.set_avatars(id,&manifest.avatars.join(", "))?;
            db.set_nsfw(id,manifest.nsfw)?;
            if !manifest.booth_url.is_empty() {
                db.associate_booth(id,&manifest.booth_url)?;
            }
            if let Some(bytes) = cover_bytes {
                fs::create_dir_all(covers_dir)?;
                let target = covers_dir.join(format!("shared-{}.img",Sha256::digest(&bytes)
                    .iter().map(|b|format!("{b:02x}")).collect::<String>()));
                if !target.exists() {fs::write(&target,bytes)?;}
                db.set_cover(id,&target.to_string_lossy())?;
            }
            id
        }
    };
    drop(db);
    if !paths.is_empty() {
        let report = managed_storage::import_paths(db_path,library,&paths,id,false)?;
        if report.rejected > 0 {
            bail!("Importación parcial: {} archivos rechazados: {}",
                report.rejected,report.warnings.join("; "));
        }
    }
    Ok(id)
}

pub struct ShareSession {
    pub url: String,
    expires: Instant,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Child>>,
}
impl ShareSession {
    pub fn expired(&self) -> bool { Instant::now() >= self.expires }
    pub fn stop(&self) {
        self.stop.store(true,Ordering::Relaxed);
        if let Ok(mut process) = self.child.lock() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}
impl Drop for ShareSession { fn drop(&mut self) { self.stop(); } }

fn find_cloudflare_url(line: &str) -> Option<String> {
    for part in line.split_whitespace() {
        let candidate = part.trim_matches(|c:char| !c.is_ascii_alphanumeric()
            && c != ':' && c != '/' && c != '.' && c != '-');
        if let Ok(url) = Url::parse(candidate) {
            if url.scheme() == "https" && url.host_str().is_some_and(|h|
                h.ends_with(".trycloudflare.com")) {
                return Some(format!("https://{}",url.host_str()?));
            }
        }
    }
    None
}
fn serve_offer(server: Server, offer: Offer, token: String,
    password: String, until: Instant, max_downloads: u32, stop: Arc<AtomicBool>) {
    let counts = (0..offer.paths.len()).map(|_| AtomicU32::new(0)).collect::<Vec<_>>();
    let prefix = format!("/s/{token}");
    while !stop.load(Ordering::Relaxed) && Instant::now() < until {
        let request = match server.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(request)) => request,
            Ok(None) => continue,
            Err(_) => break,
        };
        let path = request.url().split('?').next().unwrap_or("");
        if request.method() != &tiny_http::Method::Get || !path.starts_with(&prefix) {
            let _ = request.respond(Response::empty(StatusCode(404)));
            continue;
        }
        if !password.is_empty() {
            let expected = format!("Basic {}",BASE64.encode(format!("szkura:{password}")));
            let authorized = request.headers().iter().any(|h|
                h.field.equiv("Authorization") && h.value.as_str() == expected);
            if !authorized {
                let _ = request.respond(Response::empty(StatusCode(401)).with_header(
                    header("WWW-Authenticate", "Basic realm=\"SZKura Transfer\"")));
                continue;
            }
        }
        let suffix = &path[prefix.len()..];
        if suffix == "/manifest" {
            let data = serde_json::to_vec(&offer.manifest).unwrap_or_default();
            let _ = request.respond(Response::from_data(data)
                .with_header(header("Content-Type","application/json; charset=utf-8"))
                .with_header(header("Cache-Control","no-store")));
        } else if suffix == "" || suffix == "/" {
            let items = offer.manifest.files.iter().map(|f|format!(
                "<li><a href=\"{prefix}/file/{}\">{}</a> ({} bytes)</li>",
                f.index,html_escape(&f.name),f.size)).collect::<Vec<_>>().join("");
            let page = format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>SZKura</title></head><body><h1>{}</h1><p>Archivos compartidos temporalmente con SZKura</p><ul>{items}</ul></body></html>",
                html_escape(&offer.manifest.title));
            let _ = request.respond(Response::from_string(page)
                .with_header(header("Content-Type","text/html; charset=utf-8"))
                .with_header(header("Cache-Control","no-store")));
        } else if suffix == "/cover" {
            if let Some(path) = &offer.cover {
                match File::open(path) {
                    Ok(file) => {let _ = request.respond(Response::from_file(file)
                        .with_header(header("Content-Type","application/octet-stream"))
                        .with_header(header("Cache-Control","no-store")));},
                    Err(_) => {let _ = request.respond(Response::empty(StatusCode(404)));},
                }
            } else {let _=request.respond(Response::empty(StatusCode(404)));}
        } else if let Some(index) = suffix.strip_prefix("/file/")
            .and_then(|n|n.parse::<usize>().ok()) {
            if let Some(path) = offer.paths.get(index) {
                if max_downloads != 0 && counts[index].try_update(
                    Ordering::SeqCst,Ordering::SeqCst,
                    |n| (n < max_downloads).then_some(n+1)).is_err() {
                    let _=request.respond(Response::empty(StatusCode(429)));
                    continue;
                }
                match File::open(path) {
                    Ok(file) => {
                        let name = offer.manifest.files[index].name.replace('"', "_");
                        let disposition = format!("attachment; filename=\"{}\"",
                            name.chars().filter(|c|c.is_ascii_graphic() && *c!='\\').collect::<String>());
                        let _=request.respond(Response::from_file(file)
                            .with_header(header("Content-Disposition",&disposition))
                            .with_header(header("Cache-Control","no-store"))
                            .with_header(header("X-Content-Type-Options","nosniff")));
                    },
                    Err(_) => {let _=request.respond(Response::empty(StatusCode(404)));},
                }
            } else {let _=request.respond(Response::empty(StatusCode(404)));}
        } else {let _=request.respond(Response::empty(StatusCode(404)));}
    }
    stop.store(true, Ordering::Relaxed);
}

/// Starts only after the caller explicitly enabled sharing and verified/installed
/// the optional component. Creates an HTTPS-only public URL after cloudflared is ready.
pub fn start_share(data_dir: &Path, db_path: &Path, asset_id: i64,
    minutes: u64, max_downloads: u32, password: String) -> Result<ShareSession> {
    if !(1..=360).contains(&minutes) || password.len() > 200 {
        bail!("Caducidad o contraseña no válidas");
    }
    let executable = component_verified(data_dir)?;
    let selected = offer(db_path, asset_id)?;
    let server = Server::http("127.0.0.1:0")
        .map_err(|e|anyhow!("No se pudo abrir el servidor local: {e}"))?;
    let addr = server.server_addr().to_ip().context("Dirección local no disponible")?;
    let mut token_bytes = [0_u8;32];
    OsRng.fill_bytes(&mut token_bytes);
    let token = token_bytes.iter().map(|b|format!("{b:02x}")).collect::<String>();
    // Isolate SZKura Quick Tunnels from the user's global ~/.cloudflared/config.yml.
    let tunnel_config = data_dir.join("components").join("quick-tunnel.yml");
    fs::write(&tunnel_config, b"")?;
    let mut command = Command::new(executable);
    command
        .arg("--config").arg(&tunnel_config)
        .arg("tunnel").arg("--no-autoupdate").arg("--url")
        .arg(format!("http://127.0.0.1:{}",addr.port()))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    // cloudflared is a console executable on Windows. Running it from an
    // eframe GUI would otherwise open a persistent, unwanted CMD window.
    // CREATE_NO_WINDOW suppresses only the console; stderr remains piped for
    // tunnel URL discovery and the child is still killed when sharing stops.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().context("No se pudo iniciar Cloudflare Tunnel")?;
    let stderr = child.stderr.take().context("Sin salida del proceso de túnel")?;
    let (tx,rx)=mpsc::channel();
    thread::spawn(move||{
        use std::io::{BufRead,BufReader};
        let mut reported = false;
        // Continue draining logs after URL discovery so cloudflared cannot block on stderr.
        for line in BufReader::new(stderr).lines().map_while(std::result::Result::ok) {
            if !reported {
                if let Some(url)=find_cloudflare_url(&line) {
                    let _=tx.send(url);
                    reported=true;
                }
            }
        }
    });
    let child = Arc::new(Mutex::new(child));
    let stop = Arc::new(AtomicBool::new(false));
    let expires = Instant::now() + Duration::from_secs(minutes * 60);
    let stop_worker = Arc::clone(&stop);
    let route_token = token.clone();
    thread::spawn(move||serve_offer(server,selected,route_token,password,
        expires,max_downloads,stop_worker));
    let mut public = None;
    let started=Instant::now();
    while started.elapsed() < Duration::from_secs(30) {
        if let Ok(url) = rx.recv_timeout(Duration::from_millis(250)) {
            public=Some(url); break;
        }
        if child.lock().map_err(|_|anyhow!("Proceso de túnel inaccesible"))?
            .try_wait()?.is_some() {break;}
    }
    let Some(base) = public else {
        stop.store(true,Ordering::Relaxed);
        let _=child.lock().map(|mut c|{let _=c.kill();let _=c.wait();});
        bail!("Cloudflare no ha proporcionado un enlace temporal. Comprueba la conexión");
    };
    let url = format!("{base}/s/{token}");
    let kill_stop = Arc::clone(&stop);
    let kill_child = Arc::clone(&child);
    thread::spawn(move || {
        while !kill_stop.load(Ordering::Relaxed) && Instant::now() < expires {
            thread::sleep(Duration::from_millis(250));
        }
        if let Ok(mut process) = kill_child.lock() {
            let _ = process.kill();
            let _ = process.wait();
        }
    });
    Ok(ShareSession {url,expires,stop,child})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_link_validation_rejects_ssrf_and_invalid_tokens() {
        let token="a".repeat(64);
        assert!(parse_public_url(&format!("https://test.trycloudflare.com/s/{token}")).is_ok());
        for url in [
            format!("http://test.trycloudflare.com/s/{token}"),
            format!("https://test.trycloudflare.com.evil.org/s/{token}"),
            format!("https://localhost/s/{token}"),
            format!("https://test.trycloudflare.com/s/{token}/../../etc"),
            "https://test.trycloudflare.com/s/123".into(),
        ] { assert!(parse_public_url(&url).is_err(),"{url}"); }
    }
    #[test]
    fn manifest_rejects_traversal_and_bad_extensions() {
        let mut m=Manifest {
            version:1,title:"Sample".into(),creator:"Shop".into(),
            category:"Ropa".into(),tags:String::new(),description:String::new(),
            avatars:vec!["MAYO".into()],booth_url:String::new(),
            nsfw:false,cover:None,files:vec![TransferFile {
                index:0,name:"outfit.zip".into(),size:123,
                sha256:"f".repeat(64),
            }],
        };
        assert!(validate_manifest(&m).is_ok());
        m.files[0].name="../../secret.zip".into();
        assert!(validate_manifest(&m).is_err());
        m.files[0].name="secret.exe".into();
        assert!(validate_manifest(&m).is_err());
        m.files[0].name="outfit.zip".into();
        m.files[0].size=MAX_FILE_BYTES+1;
        assert!(validate_manifest(&m).is_err());
    }
    #[test]
    fn finds_cloudflare_address_from_process_log() {
        assert_eq!(find_cloudflare_url(
            "INF | https://friendly-owl.trycloudflare.com |"
        ).as_deref(),Some("https://friendly-owl.trycloudflare.com"));
    }
}
