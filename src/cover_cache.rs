//! Off-thread cover decoding. Never read or resize large images inside egui's paint pass.
use eframe::egui;
use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{mpsc::{self, Receiver, Sender}, Arc, Mutex},
};

const MAX_COVER_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SOURCE_PIXELS: u64 = 24_000_000;
const WORKER_COUNT: usize = 2;

pub struct CoverRequest {
    pub key: String,
    pub path: PathBuf,
    pub max_side: u32,
}
pub struct CoverResult {
    pub key: String,
    pub image: Option<egui::ColorImage>,
    /// 64x64 texture generated once in the decode worker for any cover size.
    pub backdrop: Option<egui::ColorImage>,
}

/// Decode and resize away from the UI thread. Preflight size and pixel count
/// before allocating the full decoded source. Missing/invalid covers yield None.
fn decode_cover_prepared(path: &Path, max_side: u32) -> Option<CoverPixels> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_COVER_FILE_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let header = image::ImageReader::new(Cursor::new(bytes.as_slice()))
        .with_guessed_format().ok()?;
    let (width, height) = header.into_dimensions().ok()?;
    if width == 0 || height == 0
        || width > 8192 || height > 8192
        || width as u64 * height as u64 > MAX_SOURCE_PIXELS
    {
        return None;
    }
    let original = image::load_from_memory(&bytes).ok()?;
    // Triangle is much cheaper than Lanczos3 and remains clear at card sizes.
    let resized = if width > max_side || height > max_side {
        original.resize(max_side, max_side, image::imageops::FilterType::Triangle)
    } else {
        original
    };
    let rgba = resized.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    // Generate the same tiny blurred texture for both cards and the larger
    // product editor; work only happens once on a decoding worker, not per frame.
    let thumbnail=resized.resize_to_fill(64,64,
        image::imageops::FilterType::Triangle).to_rgba8();
    let blurred=image::imageops::blur(&thumbnail,5.0);
    let backdrop=Some(egui::ColorImage::from_rgba_unmultiplied([64,64],blurred.as_raw()));
    Some(CoverPixels{
        image:egui::ColorImage::from_rgba_unmultiplied(size,rgba.as_raw()),
        backdrop,
    })
}

/// Decoding/downscaling also runs off the UI thread when requested for display.
pub fn decode_cover(path:&Path,max_side:u32)->Option<egui::ColorImage>{
    decode_cover_prepared(path,max_side).map(|item|item.image)
}

struct CoverPixels {
    image:egui::ColorImage,
    backdrop:Option<egui::ColorImage>,
}

/// A fixed number of workers. Calls are queued through a channel; the UI
/// limits pending requests and uploads only a few finished textures per frame.
pub fn spawn_workers(ctx: &egui::Context) -> (Sender<CoverRequest>, Receiver<CoverResult>) {
    let (requests_tx, requests_rx) = mpsc::channel::<CoverRequest>();
    let (finished_tx, finished_rx) = mpsc::channel::<CoverResult>();
    let requests_rx = Arc::new(Mutex::new(requests_rx));
    for number in 0..WORKER_COUNT {
        let pending = Arc::clone(&requests_rx);
        let finished = finished_tx.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name(format!("szkura-cover-{number}"))
            .spawn(move || loop {
                let job = match pending.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => break,
                };
                let Ok(job) = job else { break };
                let decoded=decode_cover_prepared(&job.path,job.max_side);
                let (image,backdrop)=match decoded {
                    Some(item)=>(Some(item.image),item.backdrop),
                    None=>(None,None),
                };
                if finished.send(CoverResult { key: job.key, image, backdrop }).is_err() {
                    break;
                }
                ctx.request_repaint();
            })
            .expect("No se pudo iniciar el hilo de portadas");
    }
    (requests_tx, finished_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_is_downsampled_before_texture_upload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cover.png");
        image::DynamicImage::new_rgb8(800, 600).save(&path).unwrap();
        let cover = decode_cover(&path, 320).unwrap();
        assert_eq!(cover.size, [320, 240]);
    }
    #[test]
    fn cards_and_editor_get_one_small_blurred_backdrop(){
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("cover.png");
        let source=image::RgbImage::from_fn(400,800,|x,y|{
            if (x/20+y/20)%2==0 {
                image::Rgb([245,60,30])
            }else{
                image::Rgb([20,60,220])
            }
        });
        source.save(&path).unwrap();
        let card=decode_cover_prepared(&path,320).unwrap();
        assert_eq!(card.image.size,[160,320]);
        let backdrop=card.backdrop.expect("Card background must be generated");
        assert_eq!(backdrop.size,[64,64]);
        assert_eq!(backdrop.pixels.len()*4,16*1024);
        assert_ne!(backdrop.pixels[20],card.image.pixels[20],
            "Backdrop must be softer than the sharp cover");
        let editor=decode_cover_prepared(&path,768).unwrap();
        assert_eq!(editor.backdrop.unwrap().size,[64,64]);
    }
    #[test]
    fn invalid_and_missing_covers_do_not_crash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cover.png");
        fs::write(&path, b"not an image").unwrap();
        assert!(decode_cover(&path, 320).is_none());
        assert!(decode_cover(&dir.path().join("missing.png"), 320).is_none());
    }
}
