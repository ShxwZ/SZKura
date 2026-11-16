//! Opt-in transfer UI using SZKura's shared visual language and ES/EN i18n.
//! Consent uses the same confirmation_dialog as other high-impact actions.
//! Multistep workflows use themed egui::Modal, never unstyled floating windows.
use crate::{
    i18n::{tr, tr_message},
    theme,
    ui_support::{action, confirmation_dialog, dialog_subject, dialog_text, ConfirmChoice, ConfirmTone, Icon},
};
use eframe::egui::{self, RichText};
use std::{
    path::Path,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};
use szkura::{
    db::Catalog,
    share_transfer::{self, Manifest, ShareSession},
};

fn modal_button(ui: &mut egui::Ui, title: &str, primary: bool, enabled: bool) -> bool {
    let p = theme::colors();
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(tr(title)).color(p.text).size(13.0))
            .min_size(egui::vec2(0.0, 36.0))
            .corner_radius(8.0)
            .fill(if primary { p.primary } else { p.button_bg })
            .stroke(egui::Stroke::new(1.0, if primary { p.primary_border } else { p.stroke })),
    ).clicked()
}

/// Matches the application's modal frame and typography, but supports a form
/// instead of forcing the two-button confirmation helper onto a multi-step flow.
fn workflow_modal(
    ctx: &egui::Context,
    id: &'static str,
    heading: &str,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let p = theme::colors();
    let width = (ctx.content_rect().width() - 80.0).clamp(300.0, 440.0);
    let modal = egui::Modal::new(egui::Id::new(id))
        .frame(egui::Frame::new()
            .fill(p.panel)
            .stroke(egui::Stroke::new(1.0, p.stroke))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(20, 18)))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.label(RichText::new(tr(heading)).strong().size(19.0).color(p.text));
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(12.0);
            body(ui);
        });
    modal.should_close()
}

pub struct SharingUi {
    pub enabled: bool,
    share_target: Option<i64>,
    share_target_title: Option<String>,
    pending_asset: Option<i64>,
    active_asset: Option<i64>,
    active_title: Option<String>,
    navigate_to_shared: Option<i64>,
    active: Option<ShareSession>,
    share_rx: Option<Receiver<Result<ShareSession, String>>>,
    consent: bool,
    minutes: u64,
    max_downloads: u32,
    password: String,
    pub status: String,
    import_open: bool,
    confirm_import: bool,
    import_link: String,
    import_password: String,
    manifest: Option<Manifest>,
    preview_rx: Option<Receiver<Result<Manifest, String>>>,
    import_rx: Option<Receiver<Result<i64, String>>>,
}
impl SharingUi {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            share_target: None,
            share_target_title: None,
            pending_asset: None,
            active_asset: None,
            active_title: None,
            navigate_to_shared: None,
            active: None,
            share_rx: None,
            consent: false,
            minutes: 30,
            max_downloads: 1,
            password: String::new(),
            status: String::new(),
            import_open: false,
            confirm_import: false,
            import_link: String::new(),
            import_password: String::new(),
            manifest: None,
            preview_rx: None,
            import_rx: None,
        }
    }
    fn can_start_share(&self) -> bool {
        self.enabled && self.active.is_none() && self.active_asset.is_none()
            && self.share_rx.is_none()
    }
    pub fn open_share(&mut self, id: i64, title: &str) {
        if !self.can_start_share() { return; }
        self.share_target = Some(id);
        self.share_target_title = Some(title.to_owned());
        self.status.clear();
    }
    pub fn take_navigation(&mut self) -> Option<i64> {
        self.navigate_to_shared.take()
    }
    fn navigate_to_active(&mut self) {
        if self.active_asset.is_none() { return; }
        self.navigate_to_shared = self.active_asset;
        // Product navigation should not leave an old preview dialog floating
        // over the destination's copy/revoke controls.
        self.import_open = false;
        self.confirm_import = false;
        self.manifest = None;
    }
    /// A persistent, contextual indicator: the active product is visible
    /// from the library, settings, and any other product detail.
    pub fn active_header(&mut self, ui: &mut egui::Ui) {
        if !self.enabled || self.active.as_ref().is_none_or(ShareSession::expired) {
            return;
        }
        let Some(title) = self.active_title.clone() else { return; };
        let display = if title.chars().count() > 54 {
            format!("{}…",title.chars().take(54).collect::<String>())
        } else {
            title.to_owned()
        };
        ui.add_space(5.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("Compartiendo ahora:"))
                .strong().size(12.5).color(theme::colors().accent));
            ui.label(RichText::new(display).size(12.5).color(theme::colors().text))
                .on_hover_text(&title);
            if action(ui, Icon::Link, "Ver ficha", false) {
                self.navigate_to_active();
            }
            if action(ui, Icon::Trash, "Dejar de compartir", false) {
                self.stop_share();
            }
        });
    }
    pub fn open_import(&mut self) {
        if self.enabled {
            self.import_open = true;
            self.status.clear();
        }
    }
    fn active_url_for(&self, id: i64) -> Option<&str> {
        if !self.enabled || self.active_asset != Some(id) {
            return None;
        }
        self.active.as_ref().filter(|s| !s.expired()).map(|s| s.url.as_str())
    }
    fn stop_share(&mut self) {
        self.active = None; // Drop kills cloudflared and revokes the link.
        self.active_asset = None;
        self.active_title = None;
        self.navigate_to_shared = None;
        self.share_target = None;
        self.share_target_title = None;
        self.consent = false;
        self.status = "Enlace revocado.".into();
    }

    /// Product-specific actions belong above the product scroll, not in an
    /// attachment list or a floating window.
    pub fn detail_actions(&mut self, ui: &mut egui::Ui, id: i64,
        title: &str, has_files: bool, busy: bool) {
        if !self.enabled { return; }
        let active_url = self.active_url_for(id).map(str::to_owned);
        ui.horizontal_wrapped(|ui| {
            if let Some(url) = active_url {
                ui.label(RichText::new(tr("Enlace activo"))
                    .size(12.5).color(theme::colors().accent))
                    .on_hover_text(&url);
                ui.label(RichText::new(tr("Solo para importar en SZKura; no hay descarga desde el navegador."))
                    .size(11.5).color(theme::colors().muted));
                if action(ui, Icon::Link, "Copiar enlace", true) {
                    ui.ctx().copy_text(url);
                    self.status = "Enlace copiado al portapapeles.".into();
                }
                if action(ui, Icon::Trash, "Dejar de compartir", false) {
                    self.stop_share();
                } else if self.status == "Enlace copiado al portapapeles." {
                    ui.label(RichText::new(tr("Enlace copiado al portapapeles."))
                        .size(12.0).color(theme::colors().muted));
                }
            } else if has_files {
                // A second tunnel cannot be started. Keep the same button in
                // place, but disabled; the header identifies the active product
                // and offers a direct stop action.
                if ui.add_enabled_ui(!busy && self.can_start_share(), |ui| {
                    action(ui, Icon::Link, "Compartir ficha", false)
                }).inner {
                    self.open_share(id,title);
                }
            }
        });
    }

    pub fn settings_ui(&mut self, ui: &mut egui::Ui, db: &mut Catalog, data_dir: &Path) {
        let p = theme::colors();
        ui.separator();
        ui.add_space(10.0);
        ui.label(RichText::new(tr("Compartir temporalmente (opcional)"))
            .strong().color(p.text).size(17.0));
        ui.label(RichText::new(tr("Transferencia HTTPS entre equipos. Cloudflare Tunnel no se descarga al instalar ni al abrir SZKura."))
            .color(p.muted).size(13.0));
        let working = self.share_rx.is_some() || self.import_rx.is_some() || self.preview_rx.is_some();
        let old = self.enabled;
        if ui.add_enabled_ui(!working, |ui| {
            ui.checkbox(&mut self.enabled, tr("Habilitar compartir archivos"))
        }).inner.changed() {
            if let Err(err) = db.update_preference("sharing_enabled", self.enabled) {
                self.enabled = old;
                self.status = format!("No se pudo guardar la preferencia: {err}");
            } else if !self.enabled {
                if self.active.is_some() {
                    self.stop_share();
                } else {
                    self.share_target = None;
                    self.share_target_title = None;
                    self.navigate_to_shared = None;
                    self.consent = false;
                    self.status = "Compartir archivos desactivado.".into();
                }
                self.import_open = false;
                self.confirm_import = false;
                self.manifest = None;
            }
        }
        if working {
            ui.label(RichText::new(tr("Espera a que termine la transferencia para cambiar esta opción."))
                .color(p.muted).size(12.0));
        }
        let present = share_transfer::component_present(data_dir);
        ui.label(RichText::new(tr(if present {
            "Componente opcional: instalado (se comprueba SHA-256 antes de ejecutarlo)."
        } else {
            "Componente opcional: no descargado. Se solicitará permiso en el primer uso."
        })).color(p.muted).size(12.0));
        if present && ui.add_enabled_ui(self.active.is_none() && !working, |ui| {
            action(ui, Icon::Trash, "Eliminar Cloudflare Tunnel descargado", false)
        }).inner {
            match share_transfer::remove_component(data_dir) {
                Ok(()) => self.status = "Componente eliminado. Se volverá a solicitar permiso para descargarlo.".into(),
                Err(err) => self.status = format!("No se pudo eliminar el componente: {err}"),
            }
        }
        if !self.status.is_empty() {
            ui.label(RichText::new(tr_message(&self.status).as_ref()).color(p.muted).size(12.0));
        }
    }

    fn begin_share(&mut self, data_dir: &Path, db_path: &Path, install: bool) {
        let Some(asset_id) = self.share_target else { return; };
        if self.share_rx.is_some() || self.active.is_some() || !self.enabled { return; }
        let (tx, rx) = mpsc::channel();
        let data = data_dir.to_path_buf();
        let db = db_path.to_path_buf();
        let minutes = self.minutes;
        let limit = self.max_downloads;
        let password = self.password.clone();
        self.share_rx = Some(rx);
        self.pending_asset = Some(asset_id);
        self.consent = false;
        self.status = if install { "Descargando y verificando Cloudflare Tunnel..." }
            else { "Creando enlace temporal..." }.into();
        thread::spawn(move || {
            let result = (|| {
                if install {
                    share_transfer::install_component_with_consent(&data)?;
                }
                share_transfer::start_share(&data, &db, asset_id, minutes, limit, password)
            })().map_err(|err: anyhow::Error| format!("{err:#}"));
            let _ = tx.send(result);
        });
    }
    fn inspect_link(&mut self) {
        if !self.enabled || self.preview_rx.is_some() || self.import_rx.is_some() { return; }
        self.manifest = None;
        let (tx, rx) = mpsc::channel();
        let url = self.import_link.clone();
        let password = self.import_password.clone();
        self.preview_rx = Some(rx);
        self.status = "Comprobando el manifiesto del enlace...".into();
        thread::spawn(move || {
            let result = share_transfer::fetch_manifest(&url, &password)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(result);
        });
    }
    fn begin_import(&mut self, db_path: &Path, library: &Path, covers: &Path) {
        if !self.enabled || self.import_rx.is_some() || self.manifest.is_none() { return; }
        let (tx, rx) = mpsc::channel();
        let db = db_path.to_path_buf();
        let library = library.to_path_buf();
        let covers = covers.to_path_buf();
        let url = self.import_link.clone();
        let password = self.import_password.clone();
        self.import_rx = Some(rx);
        self.confirm_import = false;
        self.status = "Descargando, verificando e importando la ficha...".into();
        thread::spawn(move || {
            let result = share_transfer::import_shared(&url, &password, &db, &library, &covers)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(result);
        });
    }

    /// Returns imported product ID to navigate to the new/updated catalog entry.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Result<i64, String>> {
        if let Some(result) = self.share_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.share_rx = None;
            let asset_id = self.pending_asset.take();
            match result {
                Ok(session) => {
                    if self.enabled && self.share_target.is_some() && self.share_target == asset_id {
                        self.status = "Enlace creado. Puedes copiarlo desde la cabecera de la ficha.".into();
                        self.active_asset = asset_id;
                        self.active_title = self.share_target_title.take();
                        self.active = Some(session);
                        self.share_target = None;
                    } else {
                        drop(session);
                        self.share_target_title = None;
                        self.status = "Enlace cancelado.".into();
                    }
                }
                Err(err) => {
                    // Keep the selected product title when creation fails:
                    // the user can retry without reopening the share dialog.
                    self.status = format!("Error al compartir: {err}");
                },
            }
        }
        if let Some(result) = self.preview_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.preview_rx = None;
            match result {
                Ok(m) => {
                    self.manifest = Some(m);
                    self.status = "Ficha comprobada. Revisa antes de importar.".into();
                }
                Err(err) => {
                    self.manifest = None;
                    self.status = format!("Error al abrir enlace: {err}");
                }
            }
        }
        if let Some(result) = self.import_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.import_rx = None;
            match result {
                Ok(id) => {
                    self.status = "Ficha y archivos importados correctamente.".into();
                    self.import_open = false;
                    self.confirm_import = false;
                    self.manifest = None;
                    return Some(Ok(id));
                }
                Err(err) => {
                    self.status = format!("Error al importar: {err}");
                    return Some(Err(err));
                }
            }
        }
        if self.active.as_ref().is_some_and(ShareSession::expired) {
            self.stop_share();
            self.status = "El enlace ha caducado y el túnel se ha detenido.".into();
        }
        if self.share_rx.is_some() || self.preview_rx.is_some() || self.import_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(140));
        } else if self.active.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        None
    }

    pub fn render(
        &mut self,
        ctx: &egui::Context,
        data_dir: &Path,
        db_path: &Path,
        library: &Path,
        covers: &Path,
    ) {
        if !self.enabled { return; }
        // The product link is copied from the detail header. The modal only
        // configures a temporary share and then closes after successful startup.
        if self.share_target.is_some() && !self.consent {
            let mut create = false;
            let mut cancel = false;
            let close = workflow_modal(ctx, "asset-share-setup", "Compartir ficha temporalmente", |ui| {
                dialog_text(ui, "Se compartirán esta ficha y todos sus archivos vinculados. Comparte solo archivos cuya licencia permita redistribuirlos.");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(tr("Caducidad:"));
                    egui::ComboBox::from_id_salt("share-expiry")
                        .selected_text(format!("{} {}", self.minutes, tr("min")))
                        .show_ui(ui, |ui| {
                            for minutes in [15_u64, 30, 60, 180, 360] {
                                ui.selectable_value(&mut self.minutes, minutes, format!("{minutes} {}", tr("min")));
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Descargas por archivo:"));
                    egui::ComboBox::from_id_salt("share-limit")
                        .selected_text(if self.max_downloads == 0 {
                            tr("Sin límite").to_owned()
                        } else { self.max_downloads.to_string() })
                        .show_ui(ui, |ui| {
                            for count in [1, 3, 10, 0] {
                                let label = if count == 0 { tr("Sin límite").to_owned() } else { count.to_string() };
                                ui.selectable_value(&mut self.max_downloads, count, label);
                            }
                        });
                });
                ui.label(tr("Contraseña adicional (opcional, se entrega por separado):"));
                ui.add(egui::TextEdit::singleline(&mut self.password)
                    .desired_width(ui.available_width()).password(true));
                if self.share_rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr_message(&self.status).as_ref());
                    });
                } else if self.status.starts_with("Error al compartir: ") {
                    dialog_text(ui, &self.status);
                }
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if modal_button(ui, "Crear enlace temporal", true, self.share_rx.is_none()) {
                            create = true;
                        }
                        ui.add_space(8.0);
                        if modal_button(ui, "Cancelar", false, true) {
                            cancel = true;
                        }
                    });
                });
            });
            if close || cancel {
                self.share_target = None; // In-flight result is discarded and tunnel stopped.
                self.share_target_title = None;
            } else if create {
                if share_transfer::component_present(data_dir) {
                    self.begin_share(data_dir, db_path, false);
                } else {
                    self.consent = true;
                }
            }
        }
        if self.consent {
            let choice = confirmation_dialog(
                ctx,
                "szkura-tunnel-consent",
                "Descargar Cloudflare Tunnel",
                "Descargar y compartir",
                ConfirmTone::Primary,
                self.share_rx.is_none(),
                |ui| {
                    dialog_subject(ui, &format!("cloudflared {}", share_transfer::CLOUDFLARED_VERSION));
                    ui.add_space(8.0);
                    dialog_text(ui, "Descarga opcional desde los lanzamientos oficiales de Cloudflare en GitHub.");
                    ui.add_space(7.0);
                    dialog_text(ui, "SZKura verificará SHA-256, guardará el ejecutable localmente y no necesitará permisos de administrador.");
                    ui.add_space(7.0);
                    dialog_text(ui, "Sin aceptar no se descarga nada. Puedes eliminar el componente desde Configuración.");
                },
            );
            match choice {
                ConfirmChoice::Accept => self.begin_share(data_dir, db_path, true),
                ConfirmChoice::Cancel => self.consent = false,
                ConfirmChoice::Pending => {}
            }
        }
        if self.import_open && !self.confirm_import {
            let mut preview = false;
            let mut request_confirm = false;
            let mut cancel = false;
            let working = self.preview_rx.is_some() || self.import_rx.is_some();
            let close = workflow_modal(ctx, "asset-import-flow", "Importar ficha desde enlace SZKura", |ui| {
                dialog_text(ui, "Pega el enlace HTTPS que te ha enviado otra instalación de SZKura:");
                if ui.add_enabled(!working, egui::TextEdit::singleline(&mut self.import_link)
                    .desired_width(ui.available_width())
                    .hint_text(tr("Enlace HTTPS de SZKura"))).changed() {
                    self.manifest = None;
                }
                ui.label(tr("Contraseña, si el remitente estableció una:"));
                if ui.add_enabled(!working, egui::TextEdit::singleline(&mut self.import_password)
                    .desired_width(ui.available_width()).password(true)).changed() {
                    self.manifest = None;
                }
                if let Some(m) = &self.manifest {
                    ui.add_space(9.0);
                    ui.separator();
                    ui.add_space(8.0);
                    dialog_subject(ui, &m.title);
                    ui.label(format!("{}: {} · {}: {}",
                        tr("Creador"), m.creator, tr("Categoría"), tr(&m.category)));
                    ui.label(format!("{}: {}", tr("Avatares"), m.avatars.join(", ")));
                    ui.label(format!("{} {} · {} {}",
                        m.files.len(), tr("archivos"), m.files.iter().map(|f| f.size).sum::<u64>(), tr("bytes")));
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for file in &m.files {
                            ui.label(format!("{} ({})", file.name, file.size));
                        }
                    });
                    dialog_text(ui, "Si la ficha ya existe, se conservarán sus metadatos locales. Los archivos recibidos no se ejecutarán.");
                }
                if working {
                    ui.horizontal(|ui| { ui.spinner(); ui.label(tr_message(&self.status).as_ref()); });
                } else if !self.status.is_empty() {
                    dialog_text(ui, &self.status);
                }
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.manifest.is_some() {
                            if modal_button(ui, "Continuar con la importación", true, !working) {
                                request_confirm = true;
                            }
                        } else if modal_button(ui, "Examinar enlace", true, !working && !self.import_link.trim().is_empty()) {
                            preview = true;
                        }
                        ui.add_space(8.0);
                        if modal_button(ui, "Cancelar", false, !working) {
                            cancel = true;
                        }
                    });
                });
            });
            if (cancel || close) && !working {
                self.import_open = false;
                self.manifest = None;
            } else if preview {
                self.inspect_link();
            } else if request_confirm {
                self.confirm_import = true;
            }
        }
        if self.confirm_import && self.import_open {
            let choice = confirmation_dialog(
                ctx,
                "szkura-import-confirm",
                "Confirmar importación",
                "Importar ficha y archivos",
                ConfirmTone::Primary,
                self.import_rx.is_none(),
                |ui| {
                    if let Some(manifest) = &self.manifest {
                        dialog_subject(ui, &manifest.title);
                    }
                    ui.add_space(8.0);
                    dialog_text(ui, "Se descargarán y verificarán los archivos antes de añadirlos a tu biblioteca.");
                    ui.add_space(8.0);
                    dialog_text(ui, "Las fichas BOOTH existentes conservarán sus metadatos. No se ejecutarán los archivos.");
                },
            );
            match choice {
                ConfirmChoice::Accept => self.begin_import(db_path, library, covers),
                ConfirmChoice::Cancel => self.confirm_import = false,
                ConfirmChoice::Pending => {}
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_tunnel_start_keeps_product_title_for_retry() {
        let mut ui = SharingUi::new(true);
        ui.open_share(21,"Original title");
        let (tx, rx) = mpsc::channel();
        ui.share_rx = Some(rx);
        ui.pending_asset = Some(21);
        tx.send(Err("offline".to_owned())).unwrap();
        assert!(ui.poll(&egui::Context::default()).is_none());
        assert_eq!(ui.share_target,Some(21));
        assert_eq!(ui.share_target_title.as_deref(),Some("Original title"));
        assert!(ui.status.starts_with("Error al compartir:"));
    }

    #[test]
    fn second_share_is_disabled_and_header_actions_use_active_product() {
        let mut ui = SharingUi::new(true);
        assert!(ui.can_start_share());
        ui.open_share(17,"First product");
        assert_eq!(ui.share_target,Some(17));
        ui.share_target = None;
        ui.share_target_title = None;
        // Model the existing product's active session without starting a tunnel.
        ui.active_asset = Some(17);
        ui.active_title = Some("First product".into());
        assert!(!ui.can_start_share());
        ui.open_share(35,"Other product");
        assert!(ui.share_target.is_none());
        assert_eq!(ui.active_title.as_deref(),Some("First product"));
        ui.import_open = true;
        ui.navigate_to_active();
        assert!(!ui.import_open);
        assert_eq!(ui.take_navigation(),Some(17));
        assert_eq!(ui.take_navigation(),None);
        ui.stop_share();
        assert!(ui.active_title.is_none());
        assert!(ui.take_navigation().is_none());
        assert!(ui.can_start_share());
        ui.open_share(35,"Other product");
        assert_eq!(ui.share_target,Some(35));
    }

    #[test]
    fn disabled_feature_has_no_import_or_share_entry_point() {
        let mut ui = SharingUi::new(false);
        ui.open_import();
        ui.open_share(42,"A product");
        assert!(!ui.import_open);
        assert!(ui.share_target.is_none());
        assert!(ui.active_url_for(42).is_none());
        ui.enabled = true;
        ui.open_import();
        ui.open_share(42,"A product");
        assert!(ui.import_open);
        assert_eq!(ui.share_target, Some(42));
        assert_eq!(ui.share_target_title.as_deref(),Some("A product"));
    }
}
