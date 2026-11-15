//! GitHub Releases update check and verified, in-place executable replacement.
//! No unattended installations unless the user explicitly enables them.
use anyhow::{bail,Context,Result};
use reqwest::{blocking::Client,redirect::Policy};
use serde::Deserialize;
use sha2::{Digest,Sha256};
use std::{
    fs::File,
    io::{Read,Write},
    path::Path,
    sync::atomic::{AtomicU64,Ordering},
    time::Duration,
};
use url::Url;

const RELEASE_API:&str="https://api.github.com/repos/ShxwZ/SZKura/releases?per_page=30";
const RELEASE_PAGE:&str="https://github.com/ShxwZ/SZKura/releases";
const MAX_API_BYTES:u64=3*1024*1024;
const MAX_EXE_BYTES:u64=180*1024*1024;

pub fn installed_version()->&'static str {
    option_env!("SZKURA_BUILD_TAG")
        .unwrap_or(concat!("v",env!("CARGO_PKG_VERSION"),"-local"))
}
pub fn github_release_page()->&'static str {RELEASE_PAGE}

fn alpha_number(tag:&str)->Option<u64> {
    tag.strip_prefix("v0.1.0-alpha.")?.parse().ok()
}

#[derive(Clone,Debug)]
pub struct AvailableUpdate {
    pub tag:String,
    pub sequence:u64,
    pub url:String,
    pub size:u64,
    pub sha256:Option<String>,
}
impl AvailableUpdate {
    pub fn can_install(&self)->bool{self.sha256.is_some() && cfg!(windows)}
}
#[derive(Deserialize)]
struct ReleaseApi {
    tag_name:String,
    #[serde(default)]
    draft:bool,
    assets:Vec<AssetApi>,
}
#[derive(Deserialize)]
struct AssetApi {
    name:String,
    browser_download_url:String,
    size:u64,
    digest:Option<String>,
}
fn safe_download_location(raw:&str,tag:&str)->bool {
    let Ok(url)=Url::parse(raw) else{return false};
    url.scheme()=="https" && url.host_str()==Some("github.com")
        && url.username().is_empty() && url.password().is_none()
        && url.port().is_none() && url.query().is_none() && url.fragment().is_none()
        && ["SZ_BOOTH","SZKura"].iter().any(|repository|
            ["SZKura-win-x64.exe","SZ_BOOTH-win-x64.exe","sz-booth.exe"]
                .iter().any(|name|url.path()==format!(
                    "/ShxwZ/{repository}/releases/download/{tag}/{name}"
                ))
        )
}
fn safe_redirect(url:&Url)->bool {
    if url.scheme()!="https" || !url.username().is_empty()
        || url.password().is_some() || url.port().is_some_and(|p|p!=443) {
        return false;
    }
    url.host_str().is_some_and(|host|{
        host=="github.com" || host=="api.github.com"
            || host=="githubusercontent.com"
            || host.ends_with(".githubusercontent.com")
    })
}
fn client()->Result<Client>{
    Ok(Client::builder()
        .user_agent(format!("SZKura/{} (update checker)",env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(20*60))
        .redirect(Policy::custom(|attempt|{
            if attempt.previous().len()>=6 || !safe_redirect(attempt.url()) {
                attempt.stop()
            }else{attempt.follow()}
        }))
        .build()?)
}
fn parse_releases(payload:&[u8],current_tag:&str)->Result<Option<AvailableUpdate>>{
    let releases:Vec<ReleaseApi>=serde_json::from_slice(payload)?;
    let current=alpha_number(current_tag);
    if current.is_none() {return Ok(None);}
    let mut candidates=Vec::new();
    for release in releases{
        if release.draft {continue;}
        let Some(number)=alpha_number(&release.tag_name) else{continue};
        if number<=current.unwrap(){continue;}
        let Some(asset)=release.assets.into_iter().find(|asset|{
            ["SZKura-win-x64.exe","SZ_BOOTH-win-x64.exe","sz-booth.exe"]
                .iter().any(|name|asset.name.eq_ignore_ascii_case(name))
                && asset.size>1024*1024 && asset.size<=MAX_EXE_BYTES
                && safe_download_location(&asset.browser_download_url,&release.tag_name)
        })else{continue};
        let digest=asset.digest.and_then(|text|{
            let hex=text.strip_prefix("sha256:")?;
            if hex.len()==64 && hex.bytes().all(|c|c.is_ascii_hexdigit()){
                Some(hex.to_ascii_lowercase())
            }else{None}
        });
        candidates.push(AvailableUpdate{
            tag:release.tag_name,sequence:number,url:asset.browser_download_url,
            size:asset.size,sha256:digest,
        });
    }
    candidates.sort_by_key(|x|x.sequence);
    Ok(candidates.pop())
}
pub fn check_github()->Result<Option<AvailableUpdate>>{
    let response=client()?.get(RELEASE_API).header("Accept","application/vnd.github+json")
        .send().context("No se pudo consultar GitHub Releases")?
        .error_for_status().context("GitHub rechazó la consulta de releases")?;
    if response.content_length().is_some_and(|size|size>MAX_API_BYTES){
        bail!("La respuesta de GitHub es demasiado grande");
    }
    let mut bytes=Vec::new();
    response.take(MAX_API_BYTES+1).read_to_end(&mut bytes)?;
    if bytes.len() as u64>MAX_API_BYTES{bail!("Respuesta GitHub excesiva");}
    parse_releases(&bytes,installed_version())
}
/// Download and SHA256-check first. Only then replace running executable.
/// Source is constrained to exact project release asset URL and HTTPS redirects.
#[cfg(windows)]
pub fn install_verified(update:&AvailableUpdate,downloaded:&AtomicU64)->Result<()>{
    if !safe_download_location(&update.url,&update.tag){
        bail!("La URL de actualización no pertenece a SZKura");
    }
    let hash=update.sha256.as_ref()
        .context("GitHub no ha publicado el SHA-256; no es seguro instalar automáticamente")?;
    let mut response=client()?.get(&update.url).send()
        .context("No se pudo descargar la actualización")?
        .error_for_status().context("GitHub rechazó la descarga")?;
    if response.content_length().is_some_and(|len|len>MAX_EXE_BYTES){
        bail!("La actualización supera el tamaño permitido");
    }
    let mut temporary=tempfile::NamedTempFile::new()
        .context("No se pudo crear archivo temporal de actualización")?;
    let mut sha=Sha256::new();
    let mut bytes=0u64;
    let mut magic=[0u8;2];
    let mut pos=0usize;
    let mut chunk=[0u8;128*1024];
    loop {
        let n=response.read(&mut chunk)?;
        if n==0{break;}
        if bytes.saturating_add(n as u64)>MAX_EXE_BYTES {
            bail!("El ejecutable supera el límite permitido");
        }
        for byte in chunk[..n].iter().take(2-pos) {
            magic[pos]=*byte;
            pos+=1;
        }
        bytes+=n as u64;
        temporary.write_all(&chunk[..n])?;
        sha.update(&chunk[..n]);
        downloaded.store(bytes,Ordering::Relaxed);
    }
    temporary.as_file_mut().sync_all()?;
    if bytes!=update.size || magic!=*b"MZ" {
        bail!("Tamaño o formato del ejecutable descargado inesperado");
    }
    let received=format!("{:x}",sha.finalize());
    if received!=*hash {
        bail!("SHA-256 no coincide: la actualización no se instalará");
    }
    let exe=std::env::current_exe()
        .context("No se pudo localizar la aplicación actual")?;
    self_replace::self_replace(temporary.path())
        .context("No se pudo sustituir el ejecutable (comprueba permisos de instalación)")?;
    // Restart to use the new executable; the original SQLite remains unchanged.
    std::process::Command::new(&exe).spawn()
        .context("Actualización instalada; reinicia SZKura manualmente")?;
    Ok(())
}

#[cfg(not(windows))]
pub fn install_verified(_update:&AvailableUpdate,_downloaded:&AtomicU64)->Result<()>{
    bail!("Las releases automáticas publican actualmente un EXE de Windows; utiliza GitHub Releases para Linux")
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn version_comparison_is_numeric_not_lexicographic(){
        assert_eq!(alpha_number("v0.1.0-alpha.42"),Some(42));
        assert!(alpha_number("v0.1.0-alpha.100")>alpha_number("v0.1.0-alpha.42"));
        assert_eq!(alpha_number("v0.2.0"),None);
    }
    #[test]
    fn newest_release_filters_drafts_wrong_hosts_and_invalid_sha(){
        let body=r#"[
          {"tag_name":"v0.1.0-alpha.43","draft":false,"assets":[
            {"name":"sz-booth.exe","size":10000000,
             "browser_download_url":"https://github.com/ShxwZ/SZ_BOOTH/releases/download/v0.1.0-alpha.43/sz-booth.exe",
             "digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]},
          {"tag_name":"v0.1.0-alpha.45","draft":true,"assets":[]},
          {"tag_name":"v0.1.0-alpha.44","draft":false,"assets":[
            {"name":"sz-booth.exe","size":15000000,
             "browser_download_url":"https://evil.example/release.exe",
             "digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}
        ]"#;
        let result=parse_releases(body.as_bytes(),"v0.1.0-alpha.42").unwrap().unwrap();
        assert_eq!(result.tag,"v0.1.0-alpha.43");
        assert_eq!(result.sha256.as_deref(),Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(parse_releases(body.as_bytes(),"v0.1.0-alpha.43").unwrap().is_none());
        assert!(parse_releases(body.as_bytes(),"v0.1.0-local").unwrap().is_none());
        assert!(safe_download_location("https://github.com/ShxwZ/SZKura/releases/download/v0.1.0-alpha.44/SZKura-win-x64.exe","v0.1.0-alpha.44"));
        // New releases are named for SZKura, legacy SZ_BOOTH assets still
        // remain eligible so old alpha users can upgrade without a reset.
        let new_asset=r#"[{"tag_name":"v0.1.0-alpha.99","draft":false,"assets":[
           {"name":"SZKura-win-x64.exe","size":16000000,
            "browser_download_url":"https://github.com/ShxwZ/SZ_BOOTH/releases/download/v0.1.0-alpha.99/SZKura-win-x64.exe",
            "digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}]"#;
        assert_eq!(parse_releases(new_asset.as_bytes(),"v0.1.0-alpha.98")
            .unwrap().unwrap().tag,"v0.1.0-alpha.99");
    }
    #[test]
    fn rejects_non_github_and_non_https_downloads(){
        for url in [
            "https://github.com.evil.example/ShxwZ/SZ_BOOTH/releases/download/v0.1.0-alpha.44/sz-booth.exe",
            "http://github.com/ShxwZ/SZ_BOOTH/releases/download/v0.1.0-alpha.44/sz-booth.exe",
            "https://github.com/other/repo/releases/download/v0.1.0-alpha.44/sz-booth.exe",
            "https://github.com/ShxwZ/SZ_BOOTH/releases/download/v0.1.0-alpha.44/other.exe"
        ] {assert!(!safe_download_location(url,"v0.1.0-alpha.44"));}
    }
}
