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
    db::{Asset, Catalog, FileFilter, CATEGORIES},
    share_transfer,
    library_lan::{self,LibrarySession,ShareSelection,RemoteConnection,RemoteAsset,
        RemoteSummary,RemotePage},
};
use crate::{i18n::tr,theme::colors,ui_support::{
    Icon,action,icon,nav_choice,nav_sensitive_choice,
    confirmation_dialog,dialog_subject,dialog_text,ConfirmChoice,ConfirmTone,
}};

const MAX_SELECTION:usize=2_000;
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
    host_selection:ShareSelection,
    pub picker_open:bool,
    pub picker_grid:bool,
    pub host_filter:String,
    host_category:String,
    host_avatar_filter:String,
    host_files_filter:FileFilter,
    host_show_nsfw:bool,
    host_selected_only:bool,
    pub host_session:Option<LibrarySession>,
    host_rx:Option<Receiver<Result<LibrarySession,String>>>,
    host_cancel:Arc<AtomicBool>,
    host_status:String,
    internet_access:bool,
    product_share_busy:bool,
    pending_tunnel_consent:bool,
    invite_input:String,
    connection:Option<RemoteConnection>,
    connect_rx:Option<Receiver<Result<RemoteConnection,String>>>,
    search:String,
    category:String,
    avatar_filter:String,
    files_filter:FileFilter,
    show_nsfw:bool,
    pub remote_grid:bool,
    remote_page:RemotePage,
    offset:usize,
    browse_rx:Option<Receiver<Result<RemotePage,String>>>,
    browse_parameters:Option<(String,String,String,FileFilter,bool,usize)>,
    detail:Option<RemoteAsset>,
    detail_rx:Option<Receiver<Result<RemoteAsset,String>>>,
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
            host_selection:ShareSelection::default(),picker_open:false,picker_grid:true,
            host_filter:String::new(),host_category:String::new(),
            host_avatar_filter:String::new(),host_files_filter:FileFilter::All,
            host_show_nsfw:false,host_selected_only:false,host_session:None,host_rx:None,
            host_cancel:Arc::new(AtomicBool::new(false)),host_status:String::new(),
            internet_access:false,product_share_busy:false,pending_tunnel_consent:false,
            invite_input:String::new(),connection:None,connect_rx:None,
            search:String::new(),category:String::new(),avatar_filter:String::new(),
            files_filter:FileFilter::All,show_nsfw:false,remote_grid:true,
            remote_page:RemotePage{total:0,items:Vec::new()},offset:0,
            browse_rx:None,browse_parameters:None,detail:None,detail_rx:None,
            import_rx:None,
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

    /// Always-visible host status and direct Stop control, even while editing.
    pub fn active_header(&mut self,ui:&mut egui::Ui){
        let Some(session)=self.host_session.as_ref() else{return;};
        let invitation=session.invitation.clone();
        let visible=session.visible;
        ui.horizontal_wrapped(|ui|{
            ui.label(RichText::new(format!("{}: {visible}",
                tr("Compartiendo biblioteca (fichas)")))
                .strong().size(12.5).color(colors().accent));
            if action(ui,Icon::Link,"Copiar invitación",false){
                ui.ctx().copy_text(invitation.clone());
            }
            if action(ui,Icon::Trash,"Detener biblioteca compartida",false){
                self.stop_share();
            }
        });
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
    /// All choices are local UI state, never published until the host explicitly
    /// presses Start. Files start unselected even when a product is selected.
    /// One sharing session at a time, including a pending Cloudflare install.
    pub fn host_busy(&self)->bool{
        self.host_session.is_some() || self.host_rx.is_some()
            || self.pending_tunnel_consent
    }
    pub fn set_product_share_busy(&mut self,busy:bool){self.product_share_busy=busy;}
    pub fn selection_count(&self)->usize{self.host_selection.products.len()}
    pub fn selected(&self,id:i64)->bool{self.host_selection.products.contains(&id)}
    pub fn toggle_selected(&mut self,id:i64){
        if !self.host_selection.products.insert(id){
            self.host_selection.products.remove(&id);
        }
    }
    pub fn picker_indices(&self)->Vec<usize>{
        let filter=self.host_filter.trim().to_lowercase();
        self.host_products.iter().enumerate().filter(|(_,asset)|{
            (!self.host_selected_only || self.selected(asset.id))
            && (self.host_show_nsfw || !asset.nsfw)
            && (self.host_category.is_empty() || self.host_category==asset.category)
            && (self.host_avatar_filter.is_empty() || asset.avatars.split(", ")
                .any(|name|name.eq_ignore_ascii_case(&self.host_avatar_filter)))
            && match self.host_files_filter{
                FileFilter::All=>true,
                FileFilter::WithFiles=>asset.attachments>0,
                FileFilter::WithoutFiles=>asset.attachments==0
            }
            && (filter.is_empty() ||
                [&asset.title,&asset.creator,&asset.category,&asset.avatars,&asset.tags]
                    .iter().any(|text|text.to_lowercase().contains(&filter)))
        }).map(|(index,_)|index).collect()
    }
    pub fn picker_asset(&self,index:usize)->Option<&Asset>{
        self.host_products.get(index)
    }
    pub fn picker_search(&mut self,ui:&mut egui::Ui){
        icon(ui,Icon::Search,colors().muted,19.0);
        ui.add_sized([ui.available_width().min(610.0).max(180.0),36.0],
            egui::TextEdit::singleline(&mut self.host_filter)
                .hint_text(tr("Buscar productos, tiendas y etiquetas...")));
        if !self.host_filter.is_empty()
            && ui.button(tr("Limpiar")).clicked(){self.host_filter.clear();}
    }
    pub fn picker_sidebar(&mut self,ui:&mut egui::Ui){
        ui.label(RichText::new(tr("SELECCIÓN PARA COMPARTIR"))
            .strong().color(colors().accent).size(11.0));
        ui.add_space(8.0);
        let show=self.host_selected_only;
        if nav_choice(ui,"Solo seleccionadas",show){self.host_selected_only=!show;}
        ui.add_space(6.0);
        if nav_sensitive_choice(ui,"Mostrar NSFW",self.host_show_nsfw){
            self.host_show_nsfw=!self.host_show_nsfw;
        }
        ui.add_space(10.0);
        ui.label(RichText::new(tr("CONTENIDO")).strong().color(colors().accent).size(11.0));
        if nav_choice(ui,"Todos los productos",self.host_category.is_empty()){
            self.host_category.clear();
        }
        for &cat in CATEGORIES{
            if nav_choice(ui,cat,self.host_category==cat){self.host_category=cat.into();}
        }
        ui.add_space(12.0);ui.separator();ui.add_space(8.0);
        ui.label(RichText::new(tr("AVATAR COMPATIBLE"))
            .strong().color(colors().accent).size(11.0));
        let mut avatars=BTreeSet::new();
        for asset in &self.host_products {
            for label in asset.avatars.split([',',';']) {
                let label=label.trim();
                if !label.is_empty(){avatars.insert(label.to_owned());}
            }
        }
        egui::ComboBox::from_id_salt("picker-avatar")
            .selected_text(if self.host_avatar_filter.is_empty(){
                tr("Todos los avatares")
            }else{&self.host_avatar_filter}).width(184.0)
            .show_ui(ui,|ui|{
                ui.selectable_value(&mut self.host_avatar_filter,String::new(),
                    tr("Todos los avatares"));
                for name in avatars.iter().take(512){
                    ui.selectable_value(&mut self.host_avatar_filter,name.clone(),name);
                }
            });
        ui.add_space(12.0);ui.separator();ui.add_space(8.0);
        ui.label(RichText::new(tr("ARCHIVOS"))
            .strong().color(colors().accent).size(11.0));
        if nav_choice(ui,"Todos",self.host_files_filter==FileFilter::All){
            self.host_files_filter=FileFilter::All;
        }
        if nav_choice(ui,"Con archivos",self.host_files_filter==FileFilter::WithFiles){
            self.host_files_filter=FileFilter::WithFiles;
        }
        if nav_choice(ui,"Sin archivos",self.host_files_filter==FileFilter::WithoutFiles){
            self.host_files_filter=FileFilter::WithoutFiles;
        }
        ui.add_space(12.0);
        if nav_choice(ui,"Limpiar filtros",false){
            self.host_filter.clear();self.host_category.clear();
            self.host_avatar_filter.clear();self.host_files_filter=FileFilter::All;
            self.host_show_nsfw=false;self.host_selected_only=false;
        }
    }
    pub fn settings_ui(&mut self,ui:&mut egui::Ui,db_path:&Path,data_dir:&Path){
        ui.label(RichText::new(tr("Compartir biblioteca"))
            .size(17.0).strong().color(colors().text));
        ui.label(RichText::new(tr(
            "Elige fichas completas en la misma cuadrícula que tu biblioteca. Nada se comparte hasta que lo inicies."))
            .size(12.5).color(colors().muted));
        ui.label(RichText::new(tr(
            "Al elegir una ficha se incluyen sus adjuntos. Respeta las licencias de redistribución de BOOTH."))
            .size(12.0).color(colors().muted));
        if let Some(session)=&self.host_session{
            let invitation=session.invitation.clone();
            ui.label(RichText::new(format!("{}: {}",tr("Fichas visibles"),session.visible))
                .color(colors().accent).strong());
            ui.label(RichText::new(format!("{}: {}",tr("Adjuntos descargables"),session.downloadable))
                .color(colors().muted));
            let mut view=invitation.clone();
            ui.add(egui::TextEdit::singleline(&mut view)
                .desired_width(ui.available_width()).interactive(false));
            ui.horizontal_wrapped(|ui|{
                if action(ui,Icon::Link,"Copiar invitación",true){
                    ui.ctx().copy_text(invitation.clone());
                }
                if action(ui,Icon::Trash,"Detener biblioteca compartida",false){
                    self.stop_share();
                }
            });
            ui.label(RichText::new(tr("Para cambiar qué fichas se comparten, detén la sesión."))
                .size(12.0).color(colors().muted));
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
        if self.host_load_rx.is_some(){
            ui.horizontal(|ui|{ui.spinner();ui.label(tr("Cargando selección de productos..."));});
        }
        ui.add_space(9.0);
        if ui.add_enabled(self.host_load_rx.is_none(),
            egui::Button::new(tr("Elegir fichas para compartir"))
                .min_size(egui::vec2(200.0,36.0))).clicked(){
            self.picker_open=true;
            if self.host_products.is_empty(){self.load_host_products(db_path);}
        }
        ui.label(RichText::new(format!("{}: {}",tr("Fichas seleccionadas"),
            self.selection_count())).color(colors().accent).size(13.0));
        if ui.checkbox(&mut self.internet_access,
            tr("Compartir también por Internet (Cloudflare Tunnel)")).changed(){
            self.host_status=if self.internet_access{
                tr("Se creará un enlace HTTPS temporal. Requiere Cloudflare Tunnel.").into()
            }else{tr("Compartir solo en la red local.").into()};
        }
        ui.label(RichText::new(tr(if self.internet_access{
            "La URL HTTPS usa Cloudflare Quick Tunnel; no es un servicio permanente."
        }else{
            "El acceso LAN utiliza HTTP sin cifrar: solo redes privadas de confianza."
        })).size(12.0).color(colors().muted));
        if self.product_share_busy{
            ui.label(RichText::new(tr(
                "Detén Compartir ficha antes de publicar una biblioteca. Solo puede haber un túnel activo."))
                .color(colors().accent).size(12.0));
        }
        ui.add_space(9.0);
        if ui.add_enabled(!self.product_share_busy && !self.host_selection.products.is_empty(),
            egui::Button::new(tr("Iniciar compartir biblioteca"))
                .fill(colors().primary).min_size(egui::vec2(220.0,36.0))).clicked(){
            self.request_start(db_path,data_dir);
        }
        if !self.host_status.is_empty(){
            ui.label(RichText::new(tr(&self.host_status))
                .size(12.0).color(colors().muted));
        }
    }
    fn request_start(&mut self,db_path:&Path,data_dir:&Path){
        if self.product_share_busy || self.host_busy(){return;}
        if self.host_selection.products.is_empty(){
            self.host_status=tr("Selecciona al menos una ficha.").into();
            return;
        }
        if self.internet_access && !share_transfer::component_present(data_dir){
            self.pending_tunnel_consent=true;
            return;
        }
        self.start_sharing(db_path,data_dir,false);
    }
    fn start_sharing(&mut self,db_path:&Path,data_dir:&Path,install:bool){
        if self.product_share_busy || self.host_session.is_some() || self.host_rx.is_some(){return;}
        self.pending_tunnel_consent=false;
        self.picker_open=false;
        self.host_cancel=Arc::new(AtomicBool::new(false));
        let cancel=Arc::clone(&self.host_cancel);
        let choice=self.host_selection.clone();
        let db=db_path.to_path_buf();
        let data=data_dir.to_path_buf();
        let internet=self.internet_access;
        let (tx,rx)=mpsc::channel();
        self.host_rx=Some(rx);
        self.host_status=tr("Preparando catálogo y verificando adjuntos...").into();
        thread::spawn(move||{
            let result=(||{
                if install{share_transfer::install_component_with_consent(&data)?;}
                library_lan::start_library(&db,&choice,&data,internet,&cancel)
            })().map_err(|err:anyhow::Error|format!("{err:#}"));
            let _=tx.send(result);
        });
    }
    pub fn render_consent(&mut self,ctx:&egui::Context,db_path:&Path,data_dir:&Path){
        if !self.pending_tunnel_consent{return;}
        let choice=confirmation_dialog(ctx,"shared-library-tunnel-consent",
            "Descargar Cloudflare Tunnel","Descargar y compartir",
            ConfirmTone::Primary,!self.product_share_busy,|ui|{
                dialog_subject(ui,&format!("cloudflared {}",share_transfer::CLOUDFLARED_VERSION));
                dialog_text(ui,tr("Se descargará desde GitHub oficial de Cloudflare solo con tu permiso."));
                dialog_text(ui,tr("Se comprobará el SHA-256 antes de ejecutarlo. Puedes eliminar el componente desde Ajustes."));
            });
        match choice {
            ConfirmChoice::Accept=>self.start_sharing(db_path,data_dir,true),
            ConfirmChoice::Cancel=>self.pending_tunnel_consent=false,
            ConfirmChoice::Pending=>{},
        }
    }


    pub fn poll(&mut self,ctx:&egui::Context)->Option<i64>{
        if let Some(answer)=poll_result(&mut self.host_load_rx){
            match answer {
                Ok(products) if products.len()<=MAX_SELECTION=>{
                    let valid:BTreeSet<_>=products.iter().map(|p|p.id).collect();
                    self.host_selection.products.retain(|id|valid.contains(id));
                    self.host_products=products;
                    self.host_status=tr("Selecciona las fichas que deseas publicar.").into();
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
                Ok(asset)=>{self.detail=Some(asset);},
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
