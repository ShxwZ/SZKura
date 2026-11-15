//! User-initiated, public BOOTH item metadata lookup. No account access or bulk crawler.
use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use std::{fs, io::{Read,Write}, path::{Path, PathBuf}, time::Duration};
use url::Url;

pub struct BoothItem {
    pub title: String,
    pub description: String,
    pub cover_path: String,
    /// Public display name of the BOOTH shop (not the shop slug).
    pub creator: String,
    pub shop_subdomain: Option<String>,
    /// Local category matched from a BOOTH item subcategory, when recognized.
    pub category: Option<&'static str>,
}

fn is_booth_domain(host: &str) -> bool {
    host.eq_ignore_ascii_case("booth.pm") || host.to_ascii_lowercase().ends_with(".booth.pm")
}

pub fn validate_booth_url(input: &str) -> Result<Url> {
    let url = Url::parse(input.trim()).context("El enlace no tiene un formato válido")?;
    if url.scheme() != "https" || !url.host_str().is_some_and(is_booth_domain) {
        bail!("Utiliza un enlace HTTPS de booth.pm");
    }
    let segments = url.path_segments().ok_or_else(|| anyhow!("Enlace sin ruta"))?.collect::<Vec<_>>();
    if !segments.windows(2).any(|pair| pair[0] == "items" && pair[1].chars().all(|c| c.is_ascii_digit()) && !pair[1].is_empty()) {
        bail!("El enlace debe apuntar a un producto: /items/12345");
    }
    Ok(url)
}

/// Normalize the item path while *preserving the shop host*. This is the
/// visitor-facing URL; product-id deduplication is handled separately by SQLite.
pub fn canonical_url(input: &str) -> Result<String> {
    let url = validate_booth_url(input)?;
    let segments = url.path_segments().ok_or_else(|| anyhow!("Ruta de producto inválida"))?.collect::<Vec<_>>();
    let id = segments.windows(2)
        .find(|pair| pair[0] == "items" && !pair[1].is_empty() && pair[1].chars().all(|c| c.is_ascii_digit()))
        .map(|pair| pair[1]).context("No se encontró el ID del producto")?;
    let host=url.host_str().context("Host de BOOTH no válido")?.to_ascii_lowercase();
    Ok(format!("https://{host}/items/{id}"))
}

fn valid_shop_slug(slug:&str)->bool{
    !slug.is_empty() && slug.len()<=63 &&
    !slug.starts_with('-') && !slug.ends_with('-') &&
    slug.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-')
}

/// Parse BOOTH's shop metadata without relying on the HTML page structure.
/// The item's JSON often includes shop.name and shop.subdomain.
pub fn parse_json_shop(body:&str)->Result<(String,Option<String>)>{
    let data:serde_json::Value=serde_json::from_str(body)?;
    let item=data.get("item").unwrap_or(&data);
    let shop=item.get("shop");
    let display=shop.and_then(|s|s.get("name"))
        .or_else(||shop.and_then(|s|s.get("shop_name")))
        .or_else(||item.get("shop_name"))
        .and_then(|v|v.as_str()).unwrap_or("").trim().to_string();
    let slug=shop.and_then(|s|s.get("subdomain"))
        .or_else(||item.get("shop_subdomain"))
        .and_then(|v|v.as_str())
        .map(str::to_owned)
        .or_else(||shop.and_then(|s|s.get("url"))
            .and_then(|v|v.as_str())
            .and_then(|u|Url::parse(u).ok())
            .and_then(|u|u.host_str().map(str::to_owned))
            .and_then(|host|host.strip_suffix(".booth.pm").map(str::to_owned)))
        .filter(|s|valid_shop_slug(s));
    Ok((display,slug))
}

/// Use the shop's own URL if BOOTH supplies it, without replacing a shop URL
/// that the user explicitly provided.
pub fn product_url_with_shop(input:&str,subdomain:Option<&str>)->Result<String>{
    let normalized=canonical_url(input)?;
    let current=Url::parse(&normalized)?;
    if current.host_str()!=Some("booth.pm"){return Ok(normalized);}
    if let Some(slug)=subdomain.filter(|s|valid_shop_slug(s)){
        let id=current.path().trim_start_matches("/items/");
        return Ok(format!("https://{}.booth.pm/items/{id}",slug.to_ascii_lowercase()));
    }
    Ok(normalized)
}

fn html_shop_name(html:&str,shop_host:Option<&str>)->String {
    let Some(host)=shop_host.filter(|h|*h!="booth.pm")else{return String::new();};
    let doc=Html::parse_document(html);
    if let Ok(links)=Selector::parse("a[href]"){
        for link in doc.select(&links) {
            let href=link.value().attr("href").unwrap_or("");
            // Prefer the shop homepage, not a product link or a site navigation label.
            let is_shop=href==format!("https://{host}/")
                || href==format!("https://{host}")
                || href=="/";
            if is_shop {
                let value=link.text().collect::<String>().trim().to_string();
                if !value.is_empty() && value.chars().count()<=100 {return value;}
            }
        }
    }
    String::new()
}

fn meta(doc: &Html, key: &str) -> Option<String> {
    for attribute in ["property", "name"] {
        let selector = Selector::parse(&format!("meta[{attribute}=\"{key}\"]")).ok()?;
        if let Some(value) = doc.select(&selector).next().and_then(|n| n.value().attr("content")) {
            if !value.trim().is_empty() { return Some(value.trim().to_string()); }
        }
    }
    None
}

pub fn parse_metadata(html: &str) -> (String, String, Option<String>) {
    let document = Html::parse_document(html);
    let title = meta(&document, "og:title").or_else(|| meta(&document, "twitter:title"))
        .or_else(|| Selector::parse("title").ok().and_then(|s| document.select(&s).next().map(|n| n.text().collect::<String>())))
        .unwrap_or_else(|| "Producto BOOTH".to_string());
    let description = meta(&document, "og:description").or_else(|| meta(&document, "description")).unwrap_or_default();
    let cover = meta(&document, "og:image").or_else(|| meta(&document, "twitter:image"));
    (title, description, cover)
}

/// BOOTH does not offer a stable public API. Try its public item JSON representation,
/// then the ordinary HTML page. Both requests are user-initiated and may be blocked.
pub fn parse_json_metadata(body: &str) -> Result<(String, String, Option<String>)> {
    let raw: serde_json::Value = serde_json::from_str(body)?;
    let item = raw.get("item").unwrap_or(&raw);
    let title = item.get("name").or_else(|| item.get("title"))
        .and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if title.is_empty() { bail!("El JSON no contiene el nombre del producto"); }
    let description = item.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cover = item.get("images").and_then(|v| v.as_array()).and_then(|a| a.first())
        .and_then(|im| {
            // Always prefer the original-resolution BOOTH image.
            im.get("original").or_else(|| im.get("url"))
                .or_else(|| im.get("resized")).and_then(|v| v.as_str())
        }).or_else(|| item.get("image").and_then(|v| v.as_str()))
        .map(str::to_owned);
    Ok((title, description, cover))
}

/// Only exact BOOTH 3D subcategories are mapped. Do not guess from a title,
/// description or tags; those may refer to compatible products instead.
fn map_booth_category(name:&str)->Option<&'static str>{
    let category=name.trim().to_lowercase();
    match category.as_str(){
        "3d characters"|"3d character"|"3dキャラクター"=>Some("Avatar"),
        "3d clothing"|"3d衣装"|"3d shoes"|"3d靴"=>Some("Ropa"),
        "3d accessories"|"3d装飾品"|"3d props"|"3d小道具"=>Some("Accesorios"),
        "3d hair"|"3d髪型"=>Some("Pelo"),
        "3d textures"|"3dテクスチャ"=>Some("Texturas"),
        "3d motion & animation"|"3dモーション・アニメーション"=>Some("Animaciones"),
        "3d environments / world"|"3d environments / worlds"|"3d環境・ワールド"=>Some("Mundos"),
        "3d tools & systems"|"3dツール・システム"=>Some("Herramientas"),
        // Generic or unmapped categories, including VRoid, should be left as-is.
        _=>None,
    }
}

/// BOOTH's item JSON commonly includes a category object containing a name.
/// Prefer a subcategory if explicitly present; never read unrelated shop data.
pub fn parse_json_category(body:&str)->Result<Option<&'static str>>{
    let data:serde_json::Value=serde_json::from_str(body)?;
    let item=data.get("item").unwrap_or(&data);
    for key in ["sub_category","subcategory","category"]{
        if let Some(name)=item.get(key).and_then(|v|v.get("name"))
            .and_then(|v|v.as_str()).or_else(||item.get(key).and_then(|v|v.as_str())){
            if let Some(category)=map_booth_category(name){return Ok(Some(category));}
        }
    }
    Ok(None)
}

fn read_limited(response: reqwest::blocking::Response, max: usize) -> Result<String> {
    let response = response.error_for_status()?;
    if response.content_length().is_some_and(|n| n > max as u64) {
        bail!("Respuesta de BOOTH demasiado grande");
    }
    let mut bytes = Vec::new();
    response.take((max + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > max { bail!("Respuesta demasiado grande"); }
    Ok(String::from_utf8(bytes).context("BOOTH devolvió texto no UTF-8")?)
}

pub fn lookup(url_input: &str, cache: &Path) -> Result<BoothItem> {
    let canonical = canonical_url(url_input)?;
    let id = canonical.rsplit('/').next().context("Falta el ID del producto")?;
    let json_url = format!("https://booth.pm/ja/items/{id}.json");
    let html_url = format!("https://booth.pm/ja/items/{id}");
    let client = Client::builder()
        .user_agent("SZKura/0.2 (local catalog; request initiated by user)")
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?;

    let json_result=(||->Result<(String,String,Option<String>,String,Option<String>,Option<&'static str>)>{
        let response=client.get(&json_url)
            .header("Accept","application/json")
            .header("Referer",&html_url)
            .header("Accept-Language","ja,en;q=0.8")
            .send().context("No se pudo solicitar el JSON de BOOTH")?;
        let body=read_limited(response,2_000_000)?;
        let (title,description,cover)=parse_json_metadata(&body)?;
        let (creator,shop_subdomain)=parse_json_shop(&body)?;
        let category=parse_json_category(&body)?;
        Ok((title,description,cover,creator,shop_subdomain,category))
    })();
    let (title,description,cover,creator,shop_subdomain,category)=match json_result {
        Ok(data)=>data,
        Err(json_err)=>{
            let html_result=(||->Result<(String,String,Option<String>,String,Option<String>,Option<&'static str>)>{
                let response=client.get(&html_url)
                    .header("Accept","text/html")
                    .header("Accept-Language","ja,en;q=0.8")
                    .send().context("No se pudo solicitar la página BOOTH")?;
                let body=read_limited(response,2_000_000)?;
                let (title,description,cover)=parse_metadata(&body);
                if title=="Producto BOOTH" && cover.is_none(){
                    bail!("La página no contiene metadatos públicos");
                }
                let original_host=Url::parse(&canonical)?.host_str()
                    .unwrap_or("booth.pm").to_string();
                let creator=html_shop_name(&body,Some(&original_host));
                let slug=original_host.strip_suffix(".booth.pm")
                    .filter(|s|valid_shop_slug(s)).map(str::to_owned);
                // Related-product HTML links are not reliable item categories.
                Ok((title,description,cover,creator,slug,None))
            })();
            html_result.with_context(||format!(
                "BOOTH no ha permitido recuperar los datos por JSON ({json_err}). \
                 Puedes completar nombre, tienda e imagen manualmente."
            ))?
        }
    };
    let slug_from_url=Url::parse(&canonical)?.host_str()
        .unwrap_or("booth.pm").strip_suffix(".booth.pm")
        .filter(|s|valid_shop_slug(s)).map(str::to_owned);
    let shop_subdomain=shop_subdomain.or(slug_from_url);

    let cover_path = if let Some(source) = cover {
        let image_url = Url::parse(&html_url)?.join(&source)?;
        download_image(&client, &image_url, cache).unwrap_or_default()
    } else { String::new() };
    Ok(BoothItem { title, description, cover_path, creator, shop_subdomain, category })
}

fn permitted_image_host(host: &str) -> bool {
    is_booth_domain(host) || host.eq_ignore_ascii_case("pximg.net") || host.to_ascii_lowercase().ends_with(".pximg.net")
}

fn download_image(client: &Client, url: &Url, cache: &Path) -> Result<String> {
    if url.scheme() != "https" || !url.host_str().is_some_and(permitted_image_host) {
        bail!("El proveedor de la imagen no está permitido");
    }
    fs::create_dir_all(cache)?;
    let hash = blake3::hash(url.as_str().as_bytes()).to_hex().to_string();
    // Versioned cache avoids replacing existing paths referenced by SQLite.
    // Only future BOOTH fetches use optimized WebP; manual covers are untouched.
    let webp_target=cache.join(format!("v2-{hash}.webp"));
    let original_target=cache.join(format!("v2-{hash}.img"));
    if webp_target.is_file() {
        return Ok(webp_target.to_string_lossy().into_owned());
    }
    if original_target.is_file() {
        return Ok(original_target.to_string_lossy().into_owned());
    }
    let response = client.get(url.clone())
        .header("Referer", "https://booth.pm/")
        .header("Accept", "image/avif,image/webp,image/png,image/jpeg,*/*")
        .send()?.error_for_status()?;
    if response.content_length().is_some_and(|n| n > 8_000_000) { bail!("Imagen demasiado grande"); }
    let mut data = Vec::new();
    response.take(8_000_001).read_to_end(&mut data)?;
    if data.len() > 8_000_000 { bail!("Imagen demasiado grande"); }
    let reader = image::ImageReader::new(std::io::Cursor::new(&data)).with_guessed_format()?;
    let (w,h) = reader.into_dimensions()?;
    if w == 0 || h == 0 || w > 6_000 || h > 6_000
        || u64::from(w)*u64::from(h)>24_000_000 {
        bail!("Dimensiones de imagen no aceptadas");
    }
    // Compress only when doing so reduces disk usage. Existing already-small
    // JPEG/WebP images are left byte-for-byte intact (at the new cache path).
    let optimized=optimize_booth_cover(&data).unwrap_or(None);
    let (target,stored)=if let Some(webp)=optimized {
        (webp_target,webp)
    }else{
        (original_target,data)
    };
    let mut temporary=tempfile::NamedTempFile::new_in(cache)?;
    temporary.write_all(&stored)?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(&target){
        Ok(_)=>{},
        Err(err) if err.error.kind()==std::io::ErrorKind::AlreadyExists=>{},
        Err(err)=>return Err(err.error.into()),
    }
    Ok(target.to_string_lossy().into_owned())
}

/// Encode a downloaded BOOTH cover as WebP at quality 83 (lossy). Cap
/// resolution because the UI uses at most 768px; retaining 1280px leaves
/// headroom for DPI scaling. Keep the original bytes if WebP isn't smaller.
/// Called on the existing BOOTH lookup worker, never from egui's paint pass.
fn optimize_booth_cover(original:&[u8])->Result<Option<Vec<u8>>>{
    const WEBP_MAX_SIDE:u32=1280;
    const WEBP_QUALITY:f32=83.0;
    let decoded=image::load_from_memory(original)
        .context("No se pudo decodificar la portada de BOOTH")?;
    let scaled=if decoded.width()>WEBP_MAX_SIDE || decoded.height()>WEBP_MAX_SIDE{
        decoded.resize(WEBP_MAX_SIDE,WEBP_MAX_SIDE,
            image::imageops::FilterType::Triangle)
    }else{decoded};
    let rgba=scaled.to_rgba8();
    let encoded=webp::Encoder::from_rgba(
        rgba.as_raw(),rgba.width(),rgba.height()).encode(WEBP_QUALITY);
    let optimized=encoded.to_vec();
    if optimized.len()>=original.len()
        || optimized.len()<12
        || &optimized[..4]!=b"RIFF"
        || &optimized[8..12]!=b"WEBP" {
        return Ok(None);
    }
    Ok(Some(optimized))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn webp_optimization_limits_resolution_and_saves_bytes(){
        use image::ImageFormat;
        use std::io::Cursor;
        let raw=image::RgbImage::from_fn(1600,1000,|x,y|{
            // Nontrivial colors avoid a degenerate all-black test image.
            image::Rgb([(x%256) as u8,(y%256) as u8,((x*y)%256) as u8])
        });
        let mut source=Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(raw)
            .write_to(&mut source,ImageFormat::Png).unwrap();
        let original=source.into_inner();
        let optimized=optimize_booth_cover(&original).unwrap()
            .expect("Synthetic PNG should benefit from WebP");
        assert!(optimized.len()<original.len());
        assert_eq!(&optimized[..4],b"RIFF");
        assert_eq!(&optimized[8..12],b"WEBP");
        let decoded=image::load_from_memory(&optimized).unwrap();
        assert_eq!((decoded.width(),decoded.height()),(1280,800));
    }
    #[test]
    fn optimization_does_not_increase_disk_bytes(){
        // Tiny already-compressed image may not benefit from recompression.
        let mut png=std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1,1)
            .write_to(&mut png,image::ImageFormat::Png).unwrap();
        let original=png.into_inner();
        let data=optimize_booth_cover(&original).unwrap();
        assert!(data.as_ref().is_none_or(|optimized|optimized.len()<original.len()));
    }

    #[test]
    fn metadata_extracts_title_and_image() {
        let html = r#"<html><head><meta property="og:title" content="Ropa de prueba"/><meta property="og:image" content="https://booth.pximg.net/image.jpg"/><meta name="description" content="Detalle"/></head></html>"#;
        let (title, detail, url) = parse_metadata(html);
        assert_eq!(title,"Ropa de prueba");
        assert_eq!(detail,"Detalle");
        assert_eq!(url.as_deref(), Some("https://booth.pximg.net/image.jpg"));
    }
    #[test]
    fn chooses_full_resolution_image_for_product() {
        let json = r#"{"name":"Ropa","images":[{"resized":"https://booth.pximg.net/small.jpg","original":"https://booth.pximg.net/full.jpg"}]}"#;
        let (_, _, cover) = parse_json_metadata(json).unwrap();
        assert_eq!(cover.as_deref(),Some("https://booth.pximg.net/full.jpg"));
    }
    #[test]
    fn reads_json_product_name_description_and_cover() {
        let payload = r#"{"name":"Ropa de BOOTH","description":"Descripción","images":[{"resized":"https://booth.pximg.net/items/cover.jpg"}]}"#;
        let (name, desc, cover) = parse_json_metadata(payload).unwrap();
        assert_eq!(name, "Ropa de BOOTH");
        assert_eq!(desc, "Descripción");
        assert_eq!(cover.as_deref(), Some("https://booth.pximg.net/items/cover.jpg"));
        assert!(parse_json_metadata(r#"{"images":[]}"#).is_err());
    }
    #[test]
    fn preserves_shop_url_from_chocolate_rice() {
        let input="https://chocolaterice.booth.pm/items/5141327?from=shop";
        assert_eq!(canonical_url(input).unwrap(),
            "https://chocolaterice.booth.pm/items/5141327");
        assert_eq!(product_url_with_shop(input,Some("anotherstore")).unwrap(),
            "https://chocolaterice.booth.pm/items/5141327");
        assert_eq!(product_url_with_shop("https://booth.pm/ja/items/5141327",
            Some("chocolaterice")).unwrap(),
            "https://chocolaterice.booth.pm/items/5141327");
        assert!(product_url_with_shop("https://booth.pm/items/1",Some("bad.evil.com"))
            .unwrap().starts_with("https://booth.pm/"));
    }
    #[test]
    fn reads_shop_name_and_subdomain_from_json() {
        let data=r#"{"name":"【21アバター対応】 Classical chic","description":"衣装",
            "shop":{"name":"Chocolate rice","subdomain":"chocolaterice","url":"https://chocolaterice.booth.pm/"}}"#;
        let (name,slug)=parse_json_shop(data).unwrap();
        assert_eq!(name,"Chocolate rice");
        assert_eq!(slug.as_deref(),Some("chocolaterice"));
        assert_eq!(parse_json_shop(r#"{"name":"Dress"}"#).unwrap(),(String::new(),None));
    }
    #[test]
    fn reads_shop_from_html_fallback() {
        let html=r#"<html><a href="https://chocolaterice.booth.pm/">Chocolate rice</a>
            <a href="https://chocolaterice.booth.pm/items/5141327">Product</a></html>"#;
        assert_eq!(html_shop_name(html,Some("chocolaterice.booth.pm")),"Chocolate rice");
    }
    #[test]
    fn maps_category_from_item_json_not_from_names_or_shop(){
        let item=r#"{"name":"Consolatoria","category":{"id":123,"name":"3D衣装"},"description":"Hair avatar world","shop":{"name":"3D Hair"}}"#;
        assert_eq!(parse_json_category(item).unwrap(),Some("Ropa"));
        assert_eq!(parse_json_category(r#"{"item":{"category":{"name":"3D Clothing"}}}"#).unwrap(),Some("Ropa"));
        assert_eq!(parse_json_category(r#"{"category":{"name":"3D Models"},"sub_category":{"name":"3D Hair"}}"#).unwrap(),Some("Pelo"));
        let unrelated=r#"{"name":"3D Clothing","description":"3D衣装","tags":["3D Clothing"],"category":{"name":"Books"}}"#;
        assert_eq!(parse_json_category(unrelated).unwrap(),None);
        assert_eq!(parse_json_category(r#"{"category":{"name":"3D Models"}}"#).unwrap(),None);
        assert_eq!(parse_json_category("{}").unwrap(),None);
        assert!(parse_json_category("invalid").is_err());
    }

    #[test]
    fn booth_category_aliases_cover_the_3d_sections(){
        for (name,local) in [
            ("3D Characters","Avatar"),("3D Clothing","Ropa"),
            ("3D Hair","Pelo"),("3D Accessories","Accesorios"),
            ("3D Props","Accesorios"),("3D Shoes","Ropa"),
            ("3D Textures","Texturas"),("3D Motion & Animation","Animaciones"),
            ("3D Environments / World","Mundos"),
            ("3D Tools & Systems","Herramientas"),
            ("3Dキャラクター","Avatar"),("3D衣装","Ropa"),
            ("3D髪型","Pelo"),("3D装飾品","Accesorios"),
            ("3D小道具","Accesorios"),("3D靴","Ropa"),
            ("3Dテクスチャ","Texturas"),
            ("3Dモーション・アニメーション","Animaciones"),
            ("3D環境・ワールド","Mundos"),("3Dツール・システム","Herramientas"),
        ]{
            assert_eq!(map_booth_category(name),Some(local),"{name}");
        }
        assert_eq!(map_booth_category("VRoid"),None);
        assert_eq!(map_booth_category("3D Models (Other)"),None);
        assert_eq!(map_booth_category(""),None);
    }

    #[test]
    fn booth_domains_and_item_paths_are_restricted() {
        assert!(validate_booth_url("https://booth.pm/en/items/12345").is_ok());
        assert!(validate_booth_url("https://example.booth.pm/items/12345").is_ok());
        assert!(validate_booth_url("http://booth.pm/items/12345").is_err());
        assert!(validate_booth_url("https://booth.pm.evil.com/items/12345").is_err());
        assert!(validate_booth_url("https://booth.pm/settings").is_err());
        assert!(!permitted_image_host("localhost"));
        assert_eq!(canonical_url("https://store.booth.pm/en/items/12345?ref=test").unwrap(),
            "https://store.booth.pm/items/12345");
    }
}
