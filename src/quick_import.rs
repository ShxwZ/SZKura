//! Quick-import URL validation. No download bypasses or undocumented provider API.
//! Workupload is opened by the user's default browser, including any password form.
use anyhow::{bail, Result};
use url::Url;

/// Validate and normalize a user-provided public Workupload archive or file URL.
/// Never fetch arbitrary hosts or trust a redirect as a downloadable asset.
pub fn workupload_url(input: &str) -> Result<String> {
    let parsed=Url::parse(input.trim()).map_err(|_|anyhow::anyhow!("Enlace de Workupload no válido"))?;
    let host=parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if parsed.scheme()!="https" || (host!="workupload.com"&&host!="www.workupload.com")
       || !parsed.username().is_empty() || parsed.password().is_some()
       || !matches!(parsed.port(),None|Some(443)) {
        bail!("Solo se admiten enlaces HTTPS de workupload.com");
    }
    let parts=parsed.path_segments().ok_or_else(||anyhow::anyhow!("Enlace sin identificador"))?
        .filter(|part|!part.is_empty()).collect::<Vec<_>>();
    if parts.len()!=2 || !matches!(parts[0],"archive"|"file")
        || parts[1].len()>64 || parts[1].is_empty()
        || !parts[1].bytes().all(|x|x.is_ascii_alphanumeric()||x==b'-'||x==b'_') {
        bail!("Usa un enlace de tipo workupload.com/archive/ID o /file/ID");
    }
    Ok(format!("https://workupload.com/{}/{}",parts[0],parts[1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_valid_archive_and_file_urls(){
        assert_eq!(workupload_url("https://workupload.com/archive/ZgFma42HBH").unwrap(),
            "https://workupload.com/archive/ZgFma42HBH");
        assert_eq!(workupload_url(" https://www.workupload.com/file/AA_Bb-2?ref=share ").unwrap(),
            "https://workupload.com/file/AA_Bb-2");
    }
    #[test]
    fn rejects_external_hosts_http_and_unsafe_paths(){
        for value in [
            "http://workupload.com/archive/ZgFma42HBH",
            "https://workupload.com.evil.example/archive/ZgFma42HBH",
            "https://evil.example/archive/X",
            "https://workupload.com/archive",
            "https://workupload.com/archive/a/other",
            "https://workupload.com/archive/%2e%2e",
            "https://someone:secret@workupload.com/archive/AA",
            "https://workupload.com:444/archive/AA",
            "https://workupload.com/redirect/AA"
        ] {assert!(workupload_url(value).is_err(),"should reject {value}");}
    }
}
