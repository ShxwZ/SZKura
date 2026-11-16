//! egui controller for optional, explicitly consented sharing and import.
use eframe::egui::{self, RichText};
use std::{
    path::{Path,PathBuf},
    sync::mpsc::{self,Receiver},
    thread,
    time::Duration,
};
use szkura::{
    db::Catalog,
    share_transfer::{self, Manifest, ShareSession},
};

pub struct SharingUi {
    pub enabled: bool,
    share_target: Option<i64>,
    active_asset: Option<i64>,
    active: Option<ShareSession>,
    share_rx: Option<Receiver<Result<ShareSession,String>>>,
    consent: bool,
    minutes: u64,
    max_downloads: u32,
    password: String,
    pub status: String,
    import_open: bool,
    import_link: String,
    import_password: String,
    manifest: Option<Manifest>,
    preview_rx: Option<Receiver<Result<Manifest,String>>>,
    import_rx: Option<Receiver<Result<i64,String>>>,
}
impl SharingUi {
    pub fn new(enabled:bool) -> Self {
        Self {
            enabled,share_target:None,active_asset:None,active:None,share_rx:None,
            consent:false,minutes:30,max_downloads:1,password:String::new(),
            status:String::new(),import_open:false,import_link:String::new(),
            import_password:String::new(),manifest:None,preview_rx:None,import_rx:None,
        }
    }
    pub fn open_share(&mut self, id:i64) {
        if self.enabled { self.share_target=Some(id); }
    }
    pub fn open_import(&mut self) {
        self.import_open=true;
    }
    pub fn settings_ui(&mut self,ui:&mut egui::Ui,db:&mut Catalog,data_dir:&Path) {
        ui.separator();
        ui.add_space(10.0);
        ui.label(RichText::new("Compartir temporalmente (opcional)").strong().size(17.0));
        ui.label("Transferencia HTTPS entre equipos. Cloudflare Tunnel no se descarga al instalar ni al abrir SZKura.");
        let old=self.enabled;
        if ui.checkbox(&mut self.enabled,"Habilitar compartir archivos").changed() {
            if let Err(err)=db.update_preference("sharing_enabled",self.enabled) {
                self.enabled=old;
                self.status=format!("No se pudo guardar la preferencia: {err}");
            } else if !self.enabled {
                self.active=None;
                self.share_target=None;
                self.consent=false;
            }
        }
        let present=share_transfer::component_present(data_dir);
        ui.label(if present {
            "Componente opcional: instalado (se comprueba SHA-256 antes de ejecutarlo)."
        } else {
            "Componente opcional: no descargado. Se solicitará permiso en el primer uso."
        });
        if present && ui.add_enabled(self.active.is_none() && self.share_rx.is_none(),
            egui::Button::new("Eliminar Cloudflare Tunnel descargado")).clicked() {
            match share_transfer::remove_component(data_dir) {
                Ok(())=>self.status="Componente eliminado. Se volverá a solicitar permiso para descargarlo.".into(),
                Err(e)=>self.status=format!("No se pudo eliminar el componente: {e}"),
            }
        }
        if !self.status.is_empty(){ui.label(&self.status);}
    }
    fn begin_share(&mut self, data_dir:&Path, db_path:&Path, install:bool) {
        let Some(asset_id)=self.share_target else{return};
        if self.share_rx.is_some() || self.active.is_some(){return;}
        let (tx,rx)=mpsc::channel();
        let data=data_dir.to_path_buf();
        let db=db_path.to_path_buf();
        let minutes=self.minutes;
        let limit=self.max_downloads;
        let password=self.password.clone();
        self.share_rx=Some(rx);
        self.consent=false;
        self.status=if install {"Descargando y verificando Cloudflare Tunnel..."}
            else {"Creando enlace temporal..."}.into();
        thread::spawn(move||{
            let result=(||{
                if install {share_transfer::install_component_with_consent(&data)?;}
                share_transfer::start_share(&data,&db,asset_id,minutes,limit,password)
            })().map_err(|e:anyhow::Error|format!("{e:#}"));
            let _=tx.send(result);
        });
    }
    fn inspect_link(&mut self) {
        if self.preview_rx.is_some() || self.import_rx.is_some(){return;}
        self.manifest=None;
        let (tx,rx)=mpsc::channel();
        let url=self.import_link.clone();
        let password=self.import_password.clone();
        self.preview_rx=Some(rx);
        self.status="Comprobando el manifiesto del enlace...".into();
        thread::spawn(move||{
            let result=share_transfer::fetch_manifest(&url,&password).map_err(|e|format!("{e:#}"));
            let _=tx.send(result);
        });
    }
    fn import_link(&mut self, db_path:&Path, library:&Path, covers:&Path) {
        if self.import_rx.is_some() || self.manifest.is_none(){return;}
        let (tx,rx)=mpsc::channel();
        let db=db_path.to_path_buf();
        let library=library.to_path_buf();
        let covers=covers.to_path_buf();
        let url=self.import_link.clone();
        let password=self.import_password.clone();
        self.import_rx=Some(rx);
        self.status="Descargando, verificando e importando la ficha...".into();
        thread::spawn(move||{
            let result=share_transfer::import_shared(&url,&password,&db,&library,&covers)
                .map_err(|e|format!("{e:#}"));
            let _=tx.send(result);
        });
    }
    /// Call once per frame; returns an imported product ID for the main catalog.
    pub fn poll(&mut self,ctx:&egui::Context) -> Option<Result<i64,String>> {
        if let Some(result)=self.share_rx.as_ref().and_then(|rx|rx.try_recv().ok()){
            self.share_rx=None;
            match result {
                Ok(session)=>{
                    if self.enabled {
                        self.status="Enlace creado. Solo funcionará mientras la sesión esté activa.".into();
                        self.active_asset=self.share_target;
                        self.active=Some(session);
                    } else {
                        // Revoking the opt-in while startup ran must stop the new tunnel.
                        drop(session);
                        self.status="Compartición cancelada: función desactivada.".into();
                    }
                },
                Err(e)=>self.status=format!("Error al compartir: {e}"),
            }
        }
        if let Some(result)=self.preview_rx.as_ref().and_then(|rx|rx.try_recv().ok()){
            self.preview_rx=None;
            match result {
                Ok(m)=>{self.manifest=Some(m);self.status="Ficha comprobada. Revisa antes de importar.".into();},
                Err(e)=>{self.manifest=None;self.status=format!("Error al abrir enlace: {e}");},
            }
        }
        if let Some(result)=self.import_rx.as_ref().and_then(|rx|rx.try_recv().ok()){
            self.import_rx=None;
            match result {
                Ok(id)=>{
                    self.status="Ficha y archivos importados correctamente.".into();
                    self.import_open=false;
                    self.manifest=None;
                    return Some(Ok(id));
                },
                Err(e)=>{
                    self.status=format!("Error al importar: {e}");
                    return Some(Err(e));
                },
            }
        }
        if self.active.as_ref().is_some_and(ShareSession::expired) {
            self.active=None;
            self.active_asset=None;
            self.share_target=None;
            self.status="El enlace ha caducado y el túnel se ha detenido.".into();
        }
        if self.share_rx.is_some() || self.preview_rx.is_some() || self.import_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(140));
        } else if self.active.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        None
    }
    pub fn render(&mut self,ctx:&egui::Context,data_dir:&Path,db_path:&Path,
        library:&Path,covers:&Path) {
        if self.share_target.is_some() || self.active.is_some() {
            let mut visible=true;
            let mut share=false;
            let mut stop=false;
            egui::Window::new("Compartir asset temporalmente")
                .id(egui::Id::new("asset-sharing-dialog"))
                .open(&mut visible).resizable(false).default_width(460.0)
                .show(ctx,|ui|{
                    if let Some(active)=self.active.as_ref(){
                        ui.label("Enlace HTTPS activo (cualquiera con el enlace y la contraseña puede acceder):");
                        ui.horizontal_wrapped(|ui|{
                            ui.label(RichText::new(&active.url).monospace());
                            if ui.button("Copiar enlace").clicked(){ui.ctx().copy_text(active.url.clone());}
                        });
                        if self.active_asset!=self.share_target {
                            ui.label("Primero detén la sesión actual para compartir otro producto.");
                        }
                        if ui.button("Detener y revocar enlace").clicked(){stop=true;}
                    } else {
                        ui.label("Se compartirán la ficha y todos sus archivos vinculados. Asegúrate de tener permiso para redistribuirlos.");
                        ui.horizontal(|ui|{
                            ui.label("Caducidad:");
                            egui::ComboBox::from_id_salt("share-expiry")
                                .selected_text(format!("{} min",self.minutes))
                                .show_ui(ui,|ui|{
                                    for time in [15_u64,30,60,180,360] {
                                        ui.selectable_value(&mut self.minutes,time,format!("{time} min"));
                                    }
                                });
                        });
                        ui.horizontal(|ui|{
                            ui.label("Descargas por archivo:");
                            egui::ComboBox::from_id_salt("share-limit")
                                .selected_text(if self.max_downloads==0{"Sin límite".into()}
                                    else{self.max_downloads.to_string()})
                                .show_ui(ui,|ui|{
                                    for (count,label) in [(1,"1"),(3,"3"),(10,"10"),(0,"Sin límite")] {
                                        ui.selectable_value(&mut self.max_downloads,count,label);
                                    }
                                });
                        });
                        ui.label("Contraseña adicional (opcional, se entrega por separado):");
                        ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
                        if self.share_rx.is_some(){ui.horizontal(|ui|{ui.spinner();ui.label("Preparando enlace...");});}
                        if ui.add_enabled(self.share_rx.is_none(),
                            egui::Button::new("Crear enlace temporal")).clicked(){share=true;}
                    }
                    if !self.status.is_empty(){ui.label(&self.status);}
                });
            if stop {
                self.active=None;
                self.active_asset=None;
                self.share_target=None;
                self.status="Enlace revocado.".into();
            }
            if share {
                if share_transfer::component_present(data_dir) {
                    self.begin_share(data_dir,db_path,false);
                } else {
                    self.consent=true;
                }
            }
            if !visible {self.share_target=None;}
        }
        if self.consent {
            let mut visible=true;
            let mut accepted=false;
            let mut cancelled=false;
            egui::Window::new("Permiso para descargar Cloudflare Tunnel")
                .collapsible(false).resizable(false)
                .open(&mut visible).default_width(490.0)
                .show(ctx,|ui|{
                    ui.label("Esta función es opcional y está desactivada por defecto.");
                    ui.label(format!("Para compartir, SZKura necesita descargar cloudflared {} desde los lanzamientos oficiales de Cloudflare en GitHub.",share_transfer::CLOUDFLARED_VERSION));
                    ui.label("La app comprobará su SHA-256 oficial antes de ejecutarlo y lo guardará localmente. No necesita permisos de administrador.");
                    ui.label("Sin tu confirmación no se descargará nada. Podrás eliminarlo desde Ajustes.");
                    ui.horizontal(|ui|{
                        if ui.button("Cancelar").clicked(){cancelled=true;}
                        if ui.button("Acepto: descargar y compartir").clicked(){accepted=true;}
                    });
                });
            if accepted {self.begin_share(data_dir,db_path,true);}
            if cancelled || !visible {self.consent=false;}
        }
        if self.import_open {
            let mut visible=true;
            let mut preview=false;
            let mut receive=false;
            egui::Window::new("Importar ficha desde enlace SZKura")
                .open(&mut visible).default_width(530.0)
                .show(ctx,|ui|{
                    ui.label("Pega el enlace HTTPS que te ha enviado otra instalación de SZKura:");
                    if ui.text_edit_singleline(&mut self.import_link).changed(){self.manifest=None;}
                    ui.label("Contraseña, si el remitente estableció una:");
                    if ui.add(egui::TextEdit::singleline(&mut self.import_password)
                        .password(true)).changed(){self.manifest=None;}
                    if ui.add_enabled(self.preview_rx.is_none() && self.import_rx.is_none(),
                        egui::Button::new("Examinar enlace")).clicked(){preview=true;}
                    if let Some(m)=&self.manifest {
                        ui.separator();
                        ui.label(RichText::new(&m.title).strong());
                        ui.label(format!("Creador: {} · Categoría: {}",m.creator,m.category));
                        ui.label(format!("Avatares: {}",m.avatars.join(", ")));
                        ui.label(format!("{} archivos · {} bytes",m.files.len(),
                            m.files.iter().map(|f|f.size).sum::<u64>()));
                        egui::ScrollArea::vertical().max_height(115.0).show(ui,|ui|{
                            for f in &m.files {ui.label(format!("{} ({})",f.name,f.size));}
                        });
                        ui.label("La importación conservará los metadatos existentes si ya tienes el producto de BOOTH. No se ejecutarán los archivos recibidos.");
                        if ui.add_enabled(self.import_rx.is_none(),
                            egui::Button::new("Confirmar e importar ficha y archivos")).clicked(){
                            receive=true;
                        }
                    }
                    if self.preview_rx.is_some() || self.import_rx.is_some(){
                        ui.horizontal(|ui|{ui.spinner();ui.label("Procesando transferencia...");});
                    }
                    if !self.status.is_empty(){ui.label(&self.status);}
                });
            if preview {self.inspect_link();}
            if receive {self.import_link(db_path,library,covers);}
            if !visible && self.import_rx.is_none() {self.import_open=false;}
        }
    }
}
