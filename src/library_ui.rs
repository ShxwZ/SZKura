//! UI for optional, manually activated LAN sharing and remote browsing.
//! Does not change the existing temporary one-product sharing feature.
use eframe::egui::{self, RichText, TextureHandle};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicBool,Ordering},mpsc::{self,Receiver}},
    thread,time::Duration,
};
use szkura::{
    db::{Asset, Attachment, Catalog, FileFilter, CATEGORIES},
    library_lan::{self,LibrarySession,ShareSelection,RemoteConnection,RemoteAsset,
        RemoteSummary,RemotePage},
};
use crate::{i18n::tr,theme::colors,ui_support::{Icon,action,icon}};

const SETTINGS_PAGE:usize=50;
struct CoverReady {
    id:u64,
    image:Option<egui::ColorImage>,
}

/// A crashed/disconnected worker must never leave the UI permanently busy.
fn poll_result<T>(rx:&mut Option<Receiver<Result<T,String>>>)->Option<Result<T,String>> {
    let result=rx.as_ref().and_then(|channel|match channel.try_recv() {
        Ok(value)=>Some(value),
        Err(mpsc::TryRecvError::Disconnected)=>
            Some(Err("La tarea en segundo plano se interrumpió".into())),
        Err(mpsc::TryRecvError::Empty)=>None,
    });
    if result.is_some(){*rx=None;}
    result
}

pub struct LibraryUi {
    pub remote_active:bool,
    host_products:Vec<Asset>,
    host_load_rx:Option<Receiver<Result<Vec<Asset>,String>>>,
    host_files:HashMap<i64,Vec<Attachment>>,
    host_selection:ShareSelection,
    host_filter:String,
    host_page:usize,
    host_expanded:BTreeSet<i64>,
    pub host_session:Option<LibrarySession>,
    host_rx:Option<Receiver<Result<LibrarySession,String>>>,
    host_cancel:Arc<AtomicBool>,
    host_status:String,
    invite_input:String,
    connection:Option<RemoteConnection>,
    connect_rx:Option<Receiver<Result<RemoteConnection,String>>>,
    search:String,
    category:String,
    avatar_filter:String,
    remote_page:RemotePage,
    offset:usize,
    browse_rx:Option<Receiver<Result<RemotePage,String>>>,
    browse_parameters:Option<(String,String,String,usize)>,
    detail:Option<RemoteAsset>,
    detail_rx:Option<Receiver<Result<RemoteAsset,String>>>,
    chosen_files:BTreeSet<u64>,
    import_rx:Option<Receiver<Result<i64,String>>>,
    import_cancel:Arc<AtomicBool>,
    cover_rx:Option<Receiver<CoverReady>>,
    cover_textures:HashMap<u64,TextureHandle>,
    status:String,
}

impl Default for LibraryUi {
    fn default()->Self{
        Self{
            remote_active:false,host_products:Vec::new(),host_load_rx:None,
            host_files:HashMap::new(),
            host_selection:ShareSelection::default(),host_filter:String::new(),
            host_page:0,host_expanded:BTreeSet::new(),host_session:None,host_rx:None,
            host_cancel:Arc::new(AtomicBool::new(false)),host_status:String::new(),
            invite_input:String::new(),connection:None,connect_rx:None,
            search:String::new(),category:String::new(),avatar_filter:String::new(),
            remote_page:RemotePage{total:0,items:Vec::new()},offset:0,
            browse_rx:None,browse_parameters:None,detail:None,detail_rx:None,
            chosen_files:BTreeSet::new(),import_rx:None,
            import_cancel:Arc::new(AtomicBool::new(false)),
            cover_rx:None,cover_textures:HashMap::new(),status:String::new(),
        }
    }
}

impl LibraryUi {
    pub fn new()->Self {Self::default()}

    pub fn open_connect(&mut self) {
        self.remote_active=true;
        self.detail=None;
        self.status.clear();
    }
    pub fn leave_remote(&mut self){
        self.remote_active=false;
        self.import_cancel.store(true,Ordering::Relaxed);
        self.detail=None;
        self.browse_rx=None;
        self.detail_rx=None;
        self.connect_rx=None;
        self.cover_rx=None;
        self.cover_textures.clear();
        self.connection=None;
        self.remote_page.items.clear();
        self.browse_parameters=None;
    }

    pub fn stop_share(&mut self){
        self.host_cancel.store(true,Ordering::Relaxed);
        self.host_session=None;
        self.host_status=tr("Biblioteca compartida detenida.").into();
    }
    fn load_host_products(&mut self,db_path:&Path){
        if self.host_load_rx.is_some() || self.host_rx.is_some()
            || self.host_session.is_some(){return;}
        let path=db_path.to_path_buf();
        let (tx,rx)=mpsc::channel();
        self.host_load_rx=Some(rx);
        self.host_status=tr("Cargando selección de productos...").into();
        thread::spawn(move||{
            let result=Catalog::open(&path).and_then(|db|
                db.list_filtered_visibility("","","",FileFilter::All,false,true,2_001,0))
                .map_err(|err|format!("{err:#}"));
            let _=tx.send(result);
        });
    }
    fn start_sharing(&mut self,db_path:&Path){
        if self.host_rx.is_some() || self.host_session.is_some(){return;}
        if self.host_selection.products.is_empty(){
            self.host_status=tr("Selecciona al menos una ficha.").into();
            return;
        }
        self.host_cancel=Arc::new(AtomicBool::new(false));
        let cancel=Arc::clone(&self.host_cancel);
        let selected=self.host_selection.clone();
        let path=db_path.to_owned();
        let (tx,rx)=mpsc::channel();
        self.host_rx=Some(rx);
        self.host_status=tr("Preparando catálogo y verificando adjuntos...").into();
        thread::spawn(move||{
            let response=library_lan::start_library(&path,&selected,&cancel)
                .map_err(|err|format!("{err:#}"));
            let _=tx.send(response);
        });
    }

    /// All choices are local UI state, never published until the host explicitly
    /// presses Start. Files start unselected even when a product is selected.
    pub fn settings_ui(&mut self,ui:&mut egui::Ui,db:&Catalog,db_path:&Path){
        ui.label(RichText::new(tr("Compartir biblioteca"))
            .size(17.0).strong().color(colors().text));
        ui.label(RichText::new(tr(
            "Se inicia manualmente y se detiene al cerrar SZKura. Solo se publica lo seleccionado."))
            .size(12.5).color(colors().muted));
        ui.label(RichText::new(tr(
            "Solo para LAN privada de confianza; el transporte HTTP no cifra la conexión. No abrir puertos a Internet."))
            .size(12.0).color(colors().accent));
        ui.label(RichText::new(tr(
            "Publica archivos únicamente si su licencia permite redistribuirlos."))
            .size(12.0).color(colors().muted));
        ui.add_space(8.0);
        if let Some(session)=&self.host_session {
            // Snapshot avoids borrowing the active session inside UI actions.
            let invitation=session.invitation.clone();
            let visible=session.visible;
            let downloadable=session.downloadable;
            ui.label(RichText::new(format!("{}: {}",
                tr("Fichas visibles"),visible))
                .color(colors().accent).strong());
            ui.label(RichText::new(format!("{}: {}",
                tr("Adjuntos descargables"),downloadable))
                .color(colors().muted));
            let mut invite_display=invitation.clone();
            ui.add(egui::TextEdit::singleline(&mut invite_display)
                .desired_width(ui.available_width()).interactive(false));
            ui.horizontal_wrapped(|ui|{
                if action(ui,Icon::Link,"Copiar invitación",true){
                    ui.ctx().copy_text(invitation.clone());
                }
                if action(ui,Icon::Trash,"Detener biblioteca compartida",false){
                    self.stop_share();
                }
            });
            ui.label(RichText::new(tr("Para cambiar los permisos, detén la sesión y vuelve a compartir."))
                .color(colors().muted).size(12.0));
            return;
        }
        if self.host_rx.is_some(){
            ui.horizontal(|ui|{
                ui.spinner();
                ui.label(tr("Preparando catálogo y verificando adjuntos..."));
                if ui.button(tr("Cancelar")).clicked(){
                    self.host_cancel.store(true,Ordering::Relaxed);
                }
            });
            return;
        }
        ui.horizontal_wrapped(|ui|{
            if ui.button(tr("Cargar productos para seleccionar")).clicked(){
                self.load_host_products(db_path);
            }
            if !self.host_products.is_empty(){
                ui.label(format!("{}: {}",tr("Fichas seleccionadas"),
                    self.host_selection.products.len()));
                if ui.button(tr("Limpiar selección")).clicked(){
                    self.host_selection.products.clear();
                    self.host_expanded.clear();
                }
            }
        });
        if self.host_load_rx.is_some(){ui.spinner();}
        if self.host_products.is_empty() {
            ui.label(RichText::new(tr("Ninguna ficha se publicará hasta que la selecciones y pulses Compartir biblioteca."))
                .color(colors().muted).size(12.0));
            if !self.host_status.is_empty(){ui.label(tr(&self.host_status));}
            return;
        }
        ui.add_sized([ui.available_width().min(520.0),36.0],
            egui::TextEdit::singleline(&mut self.host_filter)
                .hint_text(tr("Filtrar fichas por nombre, creador o categoría...")));
        let query=self.host_filter.to_lowercase();
        let matching:Vec<Asset>=self.host_products.iter()
            .filter(|asset| query.is_empty() ||
                [&asset.title,&asset.creator,&asset.category,&asset.avatars]
                    .iter().any(|s|s.to_lowercase().contains(&query)))
            .cloned().collect();
        let pages=matching.len().div_ceil(SETTINGS_PAGE).max(1);
        self.host_page=self.host_page.min(pages-1);
        let start=self.host_page*SETTINGS_PAGE;
        ui.horizontal(|ui|{
            ui.label(format!("{}: {}/{}",tr("Página"),self.host_page+1,pages));
            if ui.add_enabled(self.host_page>0,
                egui::Button::new(tr("Anterior"))).clicked(){self.host_page-=1;}
            if ui.add_enabled(self.host_page+1<pages,
                egui::Button::new(tr("Siguiente"))).clicked(){self.host_page+=1;}
            if ui.button(tr("Seleccionar las fichas de esta página")).clicked(){
                for asset in matching.iter().skip(start).take(SETTINGS_PAGE){
                    self.host_selection.products.entry(asset.id).or_default();
                }
            }
        });
        for asset in matching.iter().skip(start).take(SETTINGS_PAGE) {
            let mut checked=self.host_selection.products.contains_key(&asset.id);
            ui.horizontal_wrapped(|ui|{
                if ui.checkbox(&mut checked,"").changed() {
                    if checked {
                        self.host_selection.products.entry(asset.id).or_default();
                    }else{
                        self.host_selection.products.remove(&asset.id);
                    }
                }
                ui.add(egui::Label::new(RichText::new(&asset.title).strong().size(13.0))
                    .truncate()).on_hover_text(&asset.title);
                ui.label(RichText::new(tr(&asset.category)).color(colors().muted).size(12.0));
                if checked {
                    if ui.small_button(tr(if self.host_expanded.contains(&asset.id) {
                        "Ocultar archivos"
                    } else { "Elegir adjuntos" })).clicked() {
                        if self.host_expanded.remove(&asset.id) {
                            // Keep previously selected files in their whitelist.
                        } else {
                            self.host_expanded.insert(asset.id);
                            if !self.host_files.contains_key(&asset.id) {
                                match db.attachments_for(asset.id){
                                    Ok(files)=>{self.host_files.insert(asset.id,files);},
                                    Err(err)=>self.host_status=format!("{}: {err}",
                                        tr("Error leyendo archivos")),
                                }
                            }
                        }
                    }
                }
            });
            if checked && self.host_expanded.contains(&asset.id){
                ui.indent(format!("share-files-{}",asset.id),|ui|{
                    ui.label(RichText::new(tr("Por defecto, ningún archivo se puede descargar."))
                        .color(colors().muted).size(11.5));
                    if let Some(files)=self.host_files.get(&asset.id){
                        for file in files {
                            let selection=self.host_selection.products.entry(asset.id).or_default();
                            let mut published=selection.contains(&file.id);
                            if ui.checkbox(&mut published,format!("{} ({})",
                                file.original_name, file.bytes)).changed(){
                                if published{selection.insert(file.id);}
                                else{selection.remove(&file.id);}
                            }
                        }
                    }
                });
            }
        }
        ui.add_space(8.0);
        if !self.host_status.is_empty(){
            ui.label(RichText::new(tr(&self.host_status)).size(12.0).color(colors().muted));
        }
        if ui.add_enabled(!self.host_selection.products.is_empty(),
            egui::Button::new(tr("Iniciar compartir biblioteca"))
                .fill(colors().primary).min_size(egui::vec2(180.0,36.0)))
            .clicked(){self.start_sharing(db_path);}
    }

    pub fn poll(&mut self,ctx:&egui::Context)->Option<i64>{
        if let Some(answer)=poll_result(&mut self.host_load_rx){
            match answer {
                Ok(products) if products.len()<=2_000=>{
                    self.host_products=products;
                    self.host_files.clear();
                    self.host_expanded.clear();
                    self.host_selection.products.clear();
                    self.host_page=0;
                    self.host_status=tr("Selecciona las fichas y los archivos que deseas publicar.").into();
                },
                Ok(_)=>self.host_status=tr("La biblioteca supera el límite de 2000 productos.").into(),
                Err(err)=>self.host_status=format!("{}: {err}",tr("Error leyendo biblioteca")),
            }
        }
        if let Some(answer)=poll_result(&mut self.host_rx){
            match answer {
                Ok(session)=>{
                    if self.host_cancel.load(Ordering::Relaxed) {
                        session.stop();
                    }else{
                        self.host_status=tr("Biblioteca compartida activa.").into();
                        self.host_session=Some(session);
                    }
                },
                Err(error)=>self.host_status=format!("{}: {error}",tr("Error al compartir")),
            }
        }
        if let Some(answer)=poll_result(&mut self.connect_rx){
            match answer {
                Ok(connection)=>{
                    self.connection=Some(connection);
                    self.remote_page=RemotePage{total:0,items:Vec::new()};
                    self.detail=None;
                    self.offset=0;
                    self.browse_parameters=None;
                    self.cover_textures.clear();
                    self.cover_rx=None;
                    self.request_browse();
                    self.status=tr("Biblioteca remota conectada.").into();
                },
                Err(err)=>self.status=format!("{}: {err}",tr("Error al conectar")),
            }
        }
        if let Some(answer)=poll_result(&mut self.browse_rx){
            match answer{
                Ok(page)=>{
                    self.remote_page=page;
                    self.cover_textures.clear();
                    self.start_covers();
                },
                Err(err)=>self.status=format!("{}: {err}",tr("Error al cargar catálogo")),
            }
        }
        if let Some(answer)=poll_result(&mut self.detail_rx){
            match answer{
                Ok(asset)=>{self.chosen_files.clear();self.detail=Some(asset);},
                Err(err)=>self.status=format!("{}: {err}",tr("Error al cargar ficha")),
            }
        }
        if let Some(answer)=poll_result(&mut self.import_rx){
            match answer{
                Ok(local_id)=>{
                    self.status=tr("Ficha añadida a mi biblioteca.").into();
                    self.leave_remote();
                    return Some(local_id);
                },
                Err(err)=>self.status=format!("{}: {err}",tr("Error al importar")),
            }
        }
        if let Some(rx)=&self.cover_rx {
            loop {
                match rx.try_recv() {
                    Ok(cover)=>{
                        if let Some(image)=cover.image {
                            let tex=ctx.load_texture(
                                format!("szkura-remote-cover-{}",cover.id),image,
                                egui::TextureOptions::LINEAR);
                            self.cover_textures.insert(cover.id,tex);
                        }
                    },
                    Err(mpsc::TryRecvError::Empty)=>break,
                    Err(mpsc::TryRecvError::Disconnected)=>{
                        self.cover_rx=None;
                        break;
                    }
                }
            }
        }
        if self.remote_active && self.connection.is_some() && self.detail.is_none()
            && self.browse_rx.is_none() {
            if let Some((previous_search,previous_cat,previous_avatar,_))=&self.browse_parameters {
                if previous_search!=&self.search || previous_cat!=&self.category
                    || previous_avatar!=&self.avatar_filter {self.offset=0;}
            }
            let request=(self.search.clone(),self.category.clone(),
                self.avatar_filter.clone(),self.offset);
            if self.browse_parameters.as_ref()!=Some(&request){
                self.request_browse();
            }
        }
        if self.host_load_rx.is_some() || self.host_rx.is_some() || self.connect_rx.is_some()
            || self.browse_rx.is_some() || self.detail_rx.is_some()
            || self.import_rx.is_some() || self.cover_rx.is_some(){
            ctx.request_repaint_after(Duration::from_millis(120));
        }
        None
    }

    fn start_covers(&mut self){
        let Some(conn)=self.connection.clone() else{return;};
        let ids:Vec<_>=self.remote_page.items.iter()
            .filter(|item|item.has_cover).map(|item|item.id).collect();
        let (tx,rx)=mpsc::channel();
        self.cover_rx=Some(rx);
        thread::spawn(move||{
            for id in ids {
                let image=library_lan::fetch_cover(&conn,id).ok().flatten()
                    .and_then(|bytes|library_lan::decode_cover_small(&bytes,360))
                    .map(|thumb|egui::ColorImage::from_rgba_unmultiplied(
                        [thumb.width() as usize,thumb.height() as usize],thumb.as_raw()));
                if tx.send(CoverReady{id,image}).is_err(){break;}
            }
        });
    }
    fn request_browse(&mut self){
        if self.browse_rx.is_some(){return;}
        let Some(connection)=self.connection.clone() else{return;};
        let params=(self.search.clone(),self.category.clone(),
            self.avatar_filter.clone(),self.offset);
        self.browse_parameters=Some(params.clone());
        let (tx,rx)=mpsc::channel();
        self.browse_rx=Some(rx);
        thread::spawn(move||{
            let result=library_lan::browse(&connection,&params.0,&params.1,&params.2,params.3)
                .map_err(|err|format!("{err:#}"));
            let _=tx.send(result);
        });
    }
    fn start_connect(&mut self){
        if self.connect_rx.is_some(){return;}
        let invite=self.invite_input.trim().to_string();
        let (tx,rx)=mpsc::channel();
        self.connect_rx=Some(rx);
        self.status=tr("Conectando con la biblioteca...").into();
        thread::spawn(move||{
            let result=library_lan::connect(&invite).map_err(|err|format!("{err:#}"));
            let _=tx.send(result);
        });
    }
    fn fetch_detail(&mut self,id:u64){
        let Some(connection)=self.connection.clone() else {return;};
        if self.detail_rx.is_some(){return;}
        let (tx,rx)=mpsc::channel();
        self.detail_rx=Some(rx);
        thread::spawn(move||{
            let result=library_lan::get_asset(&connection,id)
                .map_err(|err|format!("{err:#}"));
            let _=tx.send(result);
        });
    }
    fn start_import(&mut self,id:u64,selected:BTreeSet<u64>,
        db_path:&Path,storage:&Path,covers:&Path){
        let Some(conn)=self.connection.clone() else{return;};
        if self.import_rx.is_some(){return;}
        let db=db_path.to_path_buf();
        let storage=storage.to_path_buf();
        let covers=covers.to_path_buf();
        self.import_cancel=Arc::new(AtomicBool::new(false));
        let cancel=Arc::clone(&self.import_cancel);
        let (tx,rx)=mpsc::channel();
        self.import_rx=Some(rx);
        self.status=tr("Descargando y verificando archivos autorizados...").into();
        thread::spawn(move||{
            let result=library_lan::import_asset(&conn,id,&selected,&db,&storage,&covers,&cancel)
                .map_err(|err|format!("{err:#}"));
            let _=tx.send(result);
        });
    }

    fn draw_cover(&self,ui:&mut egui::Ui,id:u64,width:f32,height:f32){
        egui::Frame::new().fill(colors().faint_bg).corner_radius(8)
            .show(ui,|ui|{
                if let Some(tex)=self.cover_textures.get(&id){
                    let natural=tex.size_vec2();
                    let ratio=(width/natural.x).min(height/natural.y);
                    let size=natural*ratio;
                    ui.allocate_ui_with_layout(egui::vec2(width,height),
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),|ui|{
                            ui.add(egui::Image::new(tex).fit_to_exact_size(size));
                        });
                }else{
                    ui.allocate_ui_with_layout(egui::vec2(width,height),
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),|ui|{
                            ui.label(RichText::new(tr("Sin portada")).color(colors().muted));
                        });
                }
            });
    }

    fn remote_tile(&mut self,ui:&mut egui::Ui,asset:&RemoteSummary,width:f32){
        egui::Frame::new().fill(colors().card)
            .stroke(egui::Stroke::new(1.0,colors().stroke)).corner_radius(12)
            .inner_margin(12).show(ui,|ui|{
                ui.set_width(width-24.0);
                self.draw_cover(ui,asset.id,width-24.0,150.0);
                ui.add(egui::Label::new(
                    RichText::new(&asset.title).strong().color(colors().text).size(15.0))
                    .truncate()).on_hover_text(&asset.title);
                ui.label(RichText::new(format!("{} · {}",asset.creator,tr(&asset.category)))
                    .size(12.0).color(colors().muted));
                if !asset.avatars.is_empty(){
                    ui.label(RichText::new(format!("{}: {}",tr("Avatar"),
                        asset.avatars.join(", "))).size(11.5).color(colors().accent));
                }
                ui.horizontal(|ui|{
                    ui.label(RichText::new(format!("{} {}",
                        asset.files,tr("archivos disponibles")))
                        .color(colors().muted).size(11.5));
                    if ui.button(tr("Ver ficha")).clicked(){self.fetch_detail(asset.id);}
                });
            });
    }

    /// Remote page is rendered in the existing single CentralPanel, with no
    /// editing controls or access to the host's SQLite file.
    pub fn page(&mut self,ctx:&egui::Context,db_path:&Path,
        storage:&Path,covers:&Path){
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(colors().bg)
                .inner_margin(egui::Margin::symmetric(22,17)))
            .show(ctx,|ui|{
                ui.horizontal_wrapped(|ui|{
                    if action(ui,Icon::Back,"Volver a mi biblioteca",true){
                        self.leave_remote();
                    }
                    ui.label(RichText::new(tr("Biblioteca remota (solo lectura)"))
                        .size(22.0).strong().color(colors().text));
                });
                ui.separator();
                if self.connection.is_none(){
                    ui.label(RichText::new(tr("Pega la invitación de otro SZKura en tu red local."))
                        .color(colors().muted));
                    ui.add_sized([ui.available_width().min(680.0),36.0],
                        egui::TextEdit::singleline(&mut self.invite_input)
                            .hint_text("http://192.168.1.10:12345/l/...#key=..."));
                    if ui.add_enabled(self.connect_rx.is_none(),
                        egui::Button::new(tr("Conectar a biblioteca"))).clicked(){
                            self.start_connect();
                    }
                }else{
                    let info=self.connection.as_ref().map(|c|c.info.clone()).unwrap();
                    ui.horizontal_wrapped(|ui|{
                        ui.label(RichText::new(&info.name).color(colors().accent).strong());
                        ui.label(RichText::new(format!("{} {}",
                            info.products,tr("fichas compartidas"))).color(colors().muted));
                        if ui.button(tr("Desconectar")).clicked(){
                            self.leave_remote();
                        }
                    });
                    if let Some(asset)=self.detail.clone(){
                        ui.separator();
                        if ui.button(tr("Volver al catálogo remoto")).clicked(){
                            self.detail=None;
                            self.chosen_files.clear();
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new(&asset.title).size(22.0).strong());
                        ui.label(RichText::new(format!("{} · {}",
                            asset.creator,tr(&asset.category))).color(colors().muted));
                        ui.label(RichText::new(format!("{}: {}",
                            tr("Avatares compatibles"),asset.avatars.join(", ")))
                            .color(colors().accent));
                        if !asset.booth_url.is_empty(){
                            ui.hyperlink_to(tr("Abrir producto en BOOTH"),&asset.booth_url);
                        }
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui,|ui|{
                            if !asset.description.is_empty(){
                                ui.label(RichText::new(&asset.description).color(colors().text));
                            }
                            ui.add_space(10.0);
                            ui.label(RichText::new(tr("Adjuntos autorizados para descargar"))
                                .strong().size(17.0));
                            if asset.files.is_empty(){
                                ui.label(tr("El anfitrión no ha ofrecido adjuntos para esta ficha."));
                            }
                            for file in &asset.files{
                                let mut checked=self.chosen_files.contains(&file.id);
                                if ui.checkbox(&mut checked,format!("{} ({} bytes)",
                                    file.name,file.size)).changed(){
                                    if checked {self.chosen_files.insert(file.id);}
                                    else {self.chosen_files.remove(&file.id);}
                                }
                            }
                            ui.add_space(15.0);
                            ui.horizontal_wrapped(|ui|{
                                if ui.add_enabled(self.import_rx.is_none(),
                                    egui::Button::new(tr("Añadir ficha sin archivos"))
                                        .fill(colors().primary)).clicked(){
                                    self.start_import(asset.id,BTreeSet::new(),
                                        db_path,storage,covers);
                                }
                                if ui.add_enabled(self.import_rx.is_none()
                                    && !self.chosen_files.is_empty(),
                                    egui::Button::new(tr("Descargar e importar seleccionados")))
                                    .clicked(){
                                    self.start_import(asset.id,self.chosen_files.clone(),
                                        db_path,storage,covers);
                                }
                                if self.import_rx.is_some(){
                                    ui.spinner();
                                    if ui.button(tr("Cancelar descarga")).clicked(){
                                        self.import_cancel.store(true,Ordering::Relaxed);
                                    }
                                }
                            });
                        });
                    }else{
                        ui.horizontal_wrapped(|ui|{
                            ui.add_sized([ui.available_width().min(330.0),36.0],
                                egui::TextEdit::singleline(&mut self.search)
                                    .hint_text(tr("Buscar productos...")));
                            egui::ComboBox::from_id_salt("remote-category")
                                .selected_text(tr(if self.category.is_empty() {
                                    "Todas las categorías"
                                }else{&self.category}))
                                .show_ui(ui,|ui|{
                                    if ui.selectable_label(self.category.is_empty(),
                                        tr("Todas las categorías")).clicked(){
                                        self.category.clear();self.offset=0;
                                    }
                                    for &cat in CATEGORIES {
                                        if ui.selectable_label(self.category==cat,tr(cat))
                                            .clicked(){
                                            self.category=cat.into();self.offset=0;
                                        }
                                    }
                                });
                            ui.add_sized([165.0,36.0],
                                egui::TextEdit::singleline(&mut self.avatar_filter)
                                    .hint_text(tr("Filtrar avatar...")));
                        });
                        ui.separator();
                        if self.browse_rx.is_some() || self.detail_rx.is_some(){
                            ui.spinner();
                        }
                        ui.horizontal(|ui|{
                            ui.label(format!("{}: {}",tr("Resultados"),self.remote_page.total));
                            if ui.add_enabled(self.offset>0,
                                egui::Button::new(tr("Anterior"))).clicked(){
                                self.offset=self.offset.saturating_sub(32);
                            }
                            if ui.add_enabled(self.offset+32<self.remote_page.total,
                                egui::Button::new(tr("Siguiente"))).clicked(){
                                self.offset+=32;
                            }
                        });
                        egui::ScrollArea::vertical().show(ui,|ui|{
                            let space=ui.available_width().max(240.0);
                            let gap=ui.spacing().item_spacing.x;
                            let columns=(((space+gap)/(325.0+gap)).floor() as usize).max(1);
                            let width=((space-gap*(columns-1) as f32)/columns as f32).max(200.0);
                            for row in self.remote_page.items.clone().chunks(columns){
                                ui.horizontal_top(|ui|{
                                    for asset in row {
                                        ui.allocate_ui_with_layout(egui::vec2(width,305.0),
                                            egui::Layout::top_down(egui::Align::Min),|ui|{
                                                self.remote_tile(ui,asset,width);
                                            });
                                    }
                                });
                            }
                        });
                    }
                }
                if !self.status.is_empty(){
                    ui.add_space(8.0);
                    ui.add(egui::Label::new(
                        RichText::new(tr(&self.status)).color(colors().muted).size(12.0))
                        .truncate()).on_hover_text(tr(&self.status));
                }
            });
    }
}
