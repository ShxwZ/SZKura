//! Early startup diagnostics for GUI builds. Windows binaries hide stderr, so
//! errors in the eframe bootstrap used to terminate silently.
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::Once,
    time::{SystemTime, UNIX_EPOCH},
};

static INSTALL_HOOK: Once = Once::new();

pub fn log_path() -> PathBuf {
    directories::ProjectDirs::from("dev", "ShxwZ", "SZKura")
        .map(|dirs| dirs.data_local_dir().join("logs").join("startup.log"))
        .unwrap_or_else(|| std::env::temp_dir().join("SZKura-startup.log"))
}

fn write_error(message: &str) -> PathBuf {
    let path = log_path();
    if let Some(folder) = path.parent() {
        let _ = fs::create_dir_all(folder);
    }
    if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(&path) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let _ = writeln!(file, "\n[{timestamp}] {message}");
        let _ = file.flush();
    }
    path
}

#[cfg(windows)]
fn alert(message: &str) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK, MB_TOPMOST};
    let title = std::ffi::OsStr::new("SZKura: error al iniciar")
        .encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let body = std::ffi::OsStr::new(message)
        .encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    unsafe {
        MessageBoxW(std::ptr::null_mut(), body.as_ptr(), title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_TOPMOST);
    }
}
#[cfg(not(windows))]
fn alert(message: &str) {
    eprintln!("{message}");
}

/// Log and display an actionable error without printing user files or library data.
pub fn report(error: &str) {
    let location = write_error(error);
    let detail = format!(
        "{error}\n\nRegistro de diagnóstico:\n{}\n\nNo borres la carpeta de datos de SZKura.",
        location.display()
    );
    alert(&detail);
}

/// Register as the first action in main, before creating eframe/egui.
pub fn install() {
    // This marker distinguishes a Windows block (no entry into main) from an
    // application startup crash, even when the GUI has no terminal.
    let _ = write_error("Arranque de SZKura: entrada en main().");
    INSTALL_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let message = format!("Error inesperado durante el inicio o ejecución:\n{info}");
            let location = write_error(&message);
            // Show a dialog on the main thread. Worker panics are logged without
            // blocking the renderer on a modal spawned by another thread.
            if std::thread::current().name() == Some("main") {
                alert(&format!(
                    "{message}\n\nRegistro:\n{}",
                    location.display()
                ));
            }
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_log_location_is_stable() {
        assert_eq!(log_path(), log_path());
        assert_eq!(log_path().file_name().unwrap(), "startup.log");
    }
}
