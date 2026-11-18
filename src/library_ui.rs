//! UI for optional, manually activated LAN sharing and remote browsing.
//! Does not change the existing temporary one-product sharing feature.
use eframe::egui::{self, RichText, TextureHandle};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicBool,Ordering},mpsc::{self,Receiver}},
    thread,time::Duration,
};
use szkura::{
    db::{Asset, Catalog, FileFilter, CATEGORIES},
    share_transfer,
    library_lan::{self,LibrarySession,ShareSelection,RemoteConnection,RemoteAsset,
        RemoteSummary,RemotePage,RemoteLocalState},
};
use crate::{i18n::tr,theme::colors,ui_support::{
    Icon,action,icon,nav_choice,nav_sensitive_choice,
    confirmation_dialog,dialog_subject,dialog_text,ConfirmChoice,ConfirmTone,
}};

const MAX_SELECTION:usize=2_000;
const MAX_COVER_TEXTURES:usize=80;
const COVER_BATCH:usize=8;
#[derive(Clone, Copy, PartialEq, Eq)]
enum PublishMode {Exclude,Include}

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

/// Retrieve all products without relying on the 500-record SQL page cap.
fn load_publishable_products(db:&Catalog)->anyhow::Result<Vec<Asset>>{
    let mut all=Vec::new();
    loop {
        let page=db.list_filtered_visibility("","","",FileFilter::All,
            false,true,500,all.len() as i64)?;
        if page.is_empty(){break;}
        let length=page.len();
        all.extend(page);
        if all.len()>MAX_SELECTION || length<500 {break;}
    }
    Ok(all)
}

pub struct LibraryUi {
    pub remote_active:bool,
    host_products:Vec<Asset>,
    host_product_ids:BTreeSet<i64>,
    host_sensitive_ids:BTreeSet<i64>,
    host_avatar_options:Vec<String>,
    host_load_rx:Option<Receiver<Result<Vec<Asset>,String>>>,
    host_included:BTreeSet<i64>,
    host_excluded:BTreeSet<i64>,
    host_mode:PublishMode,
    host_publish_nsfw:bool,
    host_loaded:bool,
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
    host_name:String,
    internet_access:bool,
    product_share_busy:bool,
    pending_tunnel_consent:bool,
    pending_share_confirm:bool,
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
    detail_rx:Option<Receiver<Result<(RemoteAsset,RemoteLocalState),String>>>,
    detail_state:RemoteLocalState,
    import_rx:Option<Receiver<Result<szkura::db::RemoteCommitReport,String>>>,
    pending_import:Option<u64>,
    local_db_path:Option<PathBuf>,
    import_cancel:Arc<AtomicBool>,
    cover_rx:Option<Receiver<CoverReady>>,
    cover_textures:HashMap<u64,TextureHandle>,
    cover_queue:VecDeque<u64>,
    cover_inflight:BTreeSet<u64>,
    cover_failed:BTreeSet<u64>,
    cover_recent:VecDeque<u64>,
    status:String,
}

impl Default for LibraryUi {
    fn default()->Self{
        Self{
            remote_active:false,host_products:Vec::new(),
            host_product_ids:BTreeSet::new(),host_sensitive_ids:BTreeSet::new(),
            host_avatar_options:Vec::new(),host_load_rx:None,
            host_included:BTreeSet::new(),host_excluded:BTreeSet::new(),
            host_mode:PublishMode::Exclude,host_publish_nsfw:false,host_loaded:false,
            picker_open:false,picker_grid:true,
            host_filter:String::new(),host_category:String::new(),
            host_avatar_filter:String::new(),host_files_filter:FileFilter::All,
            host_show_nsfw:false,host_selected_only:false,host_session:None,host_rx:None,
            host_cancel:Arc::new(AtomicBool::new(false)),host_status:String::new(),
            host_name:"Mi biblioteca compartida".into(),internet_access:false,product_share_busy:false,pending_tunnel_consent:false,
            pending_share_confirm:false,invite_input:String::new(),connection:None,connect_rx:None,
            search:String::new(),category:String::new(),avatar_filter:String::new(),
            files_filter:FileFilter::All,show_nsfw:false,remote_grid:true,
            remote_page:RemotePage{total:0,items:Vec::new()},offset:0,
            browse_rx:None,browse_parameters:None,detail:None,detail_rx:None,
            detail_state:RemoteLocalState::Unknown,import_rx:None,pending_import:None,
            local_db_path:None,
            import_cancel:Arc::new(AtomicBool::new(false)),
            cover_rx:None,cover_textures:HashMap::new(),
            cover_queue:VecDeque::new(),cover_inflight:BTreeSet::new(),
            cover_failed:BTreeSet::new(),cover_recent:VecDeque::new(),
            status:String::new(),
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
        self.pending_import=None;
        self.browse_rx=None;
        self.detail_rx=None;
        self.connect_rx=None;
        self.cover_rx=None;
        self.cover_textures.clear();
        self.cover_queue.clear();
        self.cover_inflight.clear();
        self.cover_failed.clear();
        self.cover_recent.clear();
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
        self.pending_share_confirm=false;
        self.pending_tunnel_consent=false;
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
            // Catalog::list_filtered_visibility intentionally caps each SQL
            // page at 500 records; load the publication picker page by page.
            let result=(||->anyhow::Result<Vec<Asset>>{
                let db=Catalog::open(&path)?;
                load_publishable_products(&db)
            })().map_err(|err|format!("{err:#}"));
            let _=tx.send(result);
        });
    }
    /// All choices are local UI state, never published until the host explicitly
    /// presses Start. Files start unselected even when a product is selected.
    /// One sharing session at a time, including a pending Cloudflare install.
    pub fn host_busy(&self)->bool{
        self.host_session.is_some() || self.host_rx.is_some()
            || self.pending_tunnel_consent || self.pending_share_confirm
    }
    pub fn set_product_share_busy(&mut self,busy:bool){self.product_share_busy=busy;}
    fn selection(&self)->ShareSelection{
        ShareSelection{products:self.host_products.iter().filter(|asset|
            (self.host_publish_nsfw || !asset.nsfw)
            && match self.host_mode{
                PublishMode::Exclude=>!self.host_excluded.contains(&asset.id),
                PublishMode::Include=>self.host_included.contains(&asset.id),
            }).map(|asset|asset.id).collect()}
    }
    pub fn selection_count(&self)->usize{self.selection().products.len()}
    pub fn selected(&self,id:i64)->bool{
        self.host_product_ids.contains(&id)
            && (self.host_publish_nsfw || !self.host_sensitive_ids.contains(&id))
            && match self.host_mode{
                PublishMode::Exclude=>!self.host_excluded.contains(&id),
                PublishMode::Include=>self.host_included.contains(&id),
            }
    }
    pub fn toggle_disabled(&self,id:i64)->bool{
        !self.host_product_ids.contains(&id) ||
            (self.host_sensitive_ids.contains(&id) && !self.host_publish_nsfw)
    }
    pub fn toggle_selected(&mut self,id:i64){
        if self.toggle_disabled(id){return;}
        let set=match self.host_mode{
            PublishMode::Exclude=>&mut self.host_excluded,
            PublishMode::Include=>&mut self.host_included,
        };
        if !set.insert(id){set.remove(&id);}
    }
    pub fn picker_toggle_label(&self,id:i64)->&'static str{
        match (self.host_mode,self.selected(id)){
            (PublishMode::Exclude,true)=>"Excluir ficha",
            (PublishMode::Exclude,false)=>"Volver a incluir",
            (PublishMode::Include,true)=>"Quitar selección",
            (PublishMode::Include,false)=>"Seleccionar ficha",
        }
    }
    pub fn mode_is_exclude(&self)->bool{self.host_mode==PublishMode::Exclude}
    pub fn mode_options(&mut self,ui:&mut egui::Ui){
        ui.horizontal_wrapped(|ui|{
            ui.label(RichText::new(tr("Modo de publicación:"))
                .color(colors().muted).size(12.5));
            if ui.selectable_label(self.mode_is_exclude(),tr("Compartir salvo excluidas"))
                .clicked(){self.host_mode=PublishMode::Exclude;}
            if ui.selectable_label(!self.mode_is_exclude(),tr("Compartir solo incluidas"))
                .clicked(){self.host_mode=PublishMode::Include;}
        });
    }

    pub fn picker_indices(&self)->Vec<usize>{
        let filter=self.host_filter.trim().to_lowercase();
        self.host_products.iter().enumerate().filter(|(_,asset)|{
            (!self.host_selected_only || if self.mode_is_exclude(){
                !self.selected(asset.id)
            }else{self.selected(asset.id)})
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
        if nav_choice(ui,if self.mode_is_exclude(){
            "Solo excluidas"
        }else{"Solo seleccionadas"},show){self.host_selected_only=!show;}
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
        egui::ComboBox::from_id_salt("picker-avatar")
            .selected_text(if self.host_avatar_filter.is_empty(){
                tr("Todos los avatares")
            }else{&self.host_avatar_filter}).width(184.0)
            .show_ui(ui,|ui|{
                ui.selectable_value(&mut self.host_avatar_filter,String::new(),
                    tr("Todos los avatares"));
                for name in &self.host_avatar_options{
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
    pub fn remote_library_name(&self)->Option<&str>{
        self.connection.as_ref().map(|c|c.info.name.as_str())
    }
    pub fn is_loading_products(&self)->bool{self.host_load_rx.is_some()}
    pub fn reset_picker_filters(&mut self){
        self.host_filter.clear();self.host_category.clear();
        self.host_avatar_filter.clear();self.host_files_filter=FileFilter::All;
        self.host_show_nsfw=false;self.host_selected_only=false;
    }
    pub fn remote_catalog_open(&self)->bool{
        self.remote_active && self.connection.is_some() && self.detail.is_none()
    }
    pub fn remote_search(&mut self,ui:&mut egui::Ui){
        icon(ui,Icon::Search,colors().muted,19.0);
        ui.add_sized([ui.available_width().min(610.0).max(180.0),36.0],
            egui::TextEdit::singleline(&mut self.search)
                .hint_text(tr("Buscar productos, tiendas y etiquetas...")));
        if !self.search.is_empty() && ui.button(tr("Limpiar")).clicked(){
            self.search.clear();
        }
    }
    pub fn remote_sidebar(&mut self,ui:&mut egui::Ui){
        ui.label(RichText::new(tr("BIBLIOTECA REMOTA"))
            .color(colors().accent).strong().size(11.0));
        ui.add_space(9.0);
        if nav_choice(ui,"Volver a mi biblioteca",false){self.leave_remote();return;}
        if self.connection.is_none(){return;}
        ui.add_space(12.0);
        if nav_choice(ui,"Todos los productos",self.category.is_empty()){
            self.category.clear();
        }
        ui.add_space(9.0);
        ui.label(RichText::new(tr("CONTENIDO")).color(colors().accent).strong().size(11.0));
        if nav_sensitive_choice(ui,"Mostrar NSFW",self.show_nsfw){
            self.show_nsfw=!self.show_nsfw;
        }
        for &category in CATEGORIES{
            if nav_choice(ui,category,self.category==category){
                self.category=category.into();
            }
        }
        ui.add_space(12.0);ui.separator();ui.add_space(8.0);
        ui.label(RichText::new(tr("AVATAR COMPATIBLE"))
            .color(colors().accent).strong().size(11.0));
        let displayed=if self.avatar_filter.is_empty(){
            tr("Todos los avatares")
        }else{&self.avatar_filter};
        egui::ComboBox::from_id_salt("remote-filter-avatar").width(184.0)
            .selected_text(displayed).show_ui(ui,|ui|{
                ui.selectable_value(&mut self.avatar_filter,String::new(),
                    tr("Todos los avatares"));
                if let Some(connection)=&self.connection{
                    for name in &connection.info.avatars{
                        ui.selectable_value(&mut self.avatar_filter,name.clone(),name);
                    }
                }
            });
        ui.add_space(12.0);ui.separator();ui.add_space(8.0);
        ui.label(RichText::new(tr("ARCHIVOS"))
            .color(colors().accent).strong().size(11.0));
        if nav_choice(ui,"Todos",self.files_filter==FileFilter::All){
            self.files_filter=FileFilter::All;
        }
        if nav_choice(ui,"Con archivos",self.files_filter==FileFilter::WithFiles){
            self.files_filter=FileFilter::WithFiles;
        }
        if nav_choice(ui,"Sin archivos",self.files_filter==FileFilter::WithoutFiles){
            self.files_filter=FileFilter::WithoutFiles;
        }
        ui.add_space(12.0);
        if nav_choice(ui,"Limpiar filtros",false){
            self.search.clear();self.category.clear();
            self.avatar_filter.clear();self.files_filter=FileFilter::All;
            self.show_nsfw=false;
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
                ui.label(tr("Preparando fichas y permisos..."));
                if ui.button(tr("Cancelar")).clicked(){
                    self.host_cancel.store(true,Ordering::Relaxed);
                }
            });
            return;
        }
        if !self.host_loaded && self.host_load_rx.is_none(){
            self.load_host_products(db_path);
        }
        if self.host_load_rx.is_some(){
            ui.horizontal(|ui|{ui.spinner();ui.label(tr("Cargando selección de productos..."));});
        }
        ui.add_space(9.0);
        self.mode_options(ui);
        ui.label(RichText::new(tr(if self.mode_is_exclude(){
            "Se compartirán todas las fichas salvo las excluidas al iniciar la sesión."
        }else{
            "Solo se compartirán las fichas marcadas como incluidas."
        })).size(12.0).color(colors().muted));
        ui.checkbox(&mut self.host_publish_nsfw,
            tr("Permitir compartir también fichas NSFW"));
        ui.add_space(5.0);
        ui.horizontal_wrapped(|ui|{
            if ui.add_enabled(self.host_load_rx.is_none(),
                egui::Button::new(tr("Elegir fichas para compartir"))
                    .min_size(egui::vec2(200.0,36.0))).clicked(){
                self.picker_open=true;
                if self.host_products.is_empty(){
                    self.host_loaded=false;
                    self.load_host_products(db_path);
                }
            }
            if ui.add_enabled(self.host_load_rx.is_none()
                && self.host_session.is_none(),
                egui::Button::new(tr("Actualizar lista de productos")))
                .clicked(){self.load_host_products(db_path);}
        });
        ui.label(RichText::new(format!("{}: {}",tr("Fichas que se compartirán"),
            self.selection_count())).color(colors().accent).size(13.0));
        ui.add_space(6.0);
        ui.label(RichText::new(tr("Nombre que verá el visitante"))
            .size(12.5).color(colors().muted));
        ui.add_sized([ui.available_width().min(540.0),36.0],
            egui::TextEdit::singleline(&mut self.host_name)
                .hint_text(tr("Mi biblioteca compartida")));
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
        if ui.add_enabled(!self.product_share_busy && self.selection_count()>0,
            egui::Button::new(tr("Iniciar compartir biblioteca"))
                .fill(colors().primary).min_size(egui::vec2(220.0,36.0))).clicked(){
            self.pending_share_confirm=true;
        }
        if !self.host_status.is_empty(){
            ui.label(RichText::new(tr(&self.host_status))
                .size(12.0).color(colors().muted));
        }
    }
    fn request_start(&mut self,db_path:&Path,data_dir:&Path){
        if self.product_share_busy || self.host_busy(){return;}
        if self.selection_count()==0{
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
        let choice=self.selection();
        let db=db_path.to_path_buf();
        let data=data_dir.to_path_buf();
        let internet=self.internet_access;
        let name=self.host_name.trim().to_owned();
        let (tx,rx)=mpsc::channel();
        self.host_rx=Some(rx);
        self.host_status=tr("Preparando fichas y permisos...").into();
        thread::spawn(move||{
            let result=(||{
                if install{share_transfer::install_component_with_consent(&data)?;}
                library_lan::start_library(&db,&choice,&data,internet,&name,&cancel)
            })().map_err(|err:anyhow::Error|format!("{err:#}"));
            let _=tx.send(result);
        });
    }
    pub fn render_consent(&mut self,ctx:&egui::Context,db_path:&Path,data_dir:&Path){
        if self.pending_share_confirm {
            let count=self.selection_count();
            let internet=self.internet_access;
            let choice=confirmation_dialog(ctx,"shared-library-scope-consent",
                "Confirmar biblioteca compartida","Publicar biblioteca",
                ConfirmTone::Primary,!self.product_share_busy && count>0,|ui|{
                    dialog_subject(ui,&format!("{}: {count}",
                        tr("Fichas que se compartirán")));
                    dialog_text(ui,tr(if internet{
                        "Acceso por Internet mediante un enlace HTTPS temporal de Cloudflare."
                    }else{
                        "Acceso mediante HTTP en la red local de confianza."
                    }));
                    dialog_text(ui,tr(
                        "Se compartirán todos los adjuntos de las fichas elegidas. Comprueba que sus licencias permitan redistribuirlos."));
                    dialog_text(ui,tr(
                        "Las fichas excluidas no aparecerán en el catálogo remoto."));
                });
            match choice {
                ConfirmChoice::Accept=>{
                    self.pending_share_confirm=false;
                    self.request_start(db_path,data_dir);
                },
                ConfirmChoice::Cancel=>self.pending_share_confirm=false,
                ConfirmChoice::Pending=>{},
            }
            return;
        }
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
                    self.host_included.retain(|id|valid.contains(id));
                    self.host_excluded.retain(|id|valid.contains(id));
                    self.host_loaded=true;
                    let mut avatar_names=BTreeSet::new();
                    for product in &products {
                        for item in product.avatars.split([',',';']){
                            let name=item.trim();
                            if !name.is_empty(){avatar_names.insert(name.to_owned());}
                        }
                    }
                    self.host_avatar_options=avatar_names.into_iter().take(512).collect();
                    self.host_product_ids=valid;
                    self.host_sensitive_ids=products.iter().filter(|product|product.nsfw)
                        .map(|product|product.id).collect();
                    self.host_products=products;
                    self.host_status=tr("Selecciona las fichas que deseas publicar.").into();
                },
                Ok(_)=>{
                    self.host_loaded=true;
                    self.host_status=tr("La biblioteca supera el límite de 2000 productos.").into();
                },
                Err(err)=>{
                    self.host_loaded=true;
                    self.host_status=format!("{}: {err}",tr("Error leyendo biblioteca"));
                },
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
                    self.cover_queue.clear();
                    self.cover_inflight.clear();
                    self.cover_failed.clear();
                    self.cover_recent.clear();
                    self.cover_rx=None;
                    self.request_browse();
                    self.status=tr("Biblioteca remota conectada.").into();
                },
                Err(err)=>self.status=format!("{}: {err}",tr("Error al conectar")),
            }
        }
        if let Some(answer)=poll_result(&mut self.browse_rx){
            let obsolete=self.browse_parameters.as_ref().is_none_or(|params|{
                params.0!=self.search || params.1!=self.category
                    || params.2!=self.avatar_filter
                    || params.3!=self.files_filter || params.4!=self.show_nsfw
            });
            if obsolete {
                // A filter may change while the HTTP request is in flight.
                // Never append results from an older filter to the new catalog.
                self.offset=0;
                self.remote_page=RemotePage{total:0,items:Vec::new()};
                self.browse_parameters=None;
            }else{
                match answer {
                    Ok(page)=>{
                        if self.offset==0 {
                            self.remote_page=page;
                        }else{
                            self.remote_page.total=page.total;
                            for item in page.items{
                                if !self.remote_page.items.iter().any(|a|a.id==item.id){
                                    self.remote_page.items.push(item);
                                }
                            }
                        }
                        // Visible tiles enqueue only covers they actually need.
                    },
                    Err(err)=>self.status=format!("{}: {err}",tr("Error al cargar catálogo")),
                }
            }
        }
        if let Some(answer)=poll_result(&mut self.detail_rx){
            match answer{
                Ok((asset,state))=>{
                    self.detail_state=state;
                    self.detail=Some(asset);
                },
                Err(err)=>self.status=format!("{}: {err}",tr("Error al cargar ficha")),
            }
        }
        if let Some(answer)=poll_result(&mut self.import_rx){
            match answer{
                Ok(report)=>{
                    self.status=tr(if report.is_new{
                        "Ficha completa importada correctamente."
                    }else{"Ficha actualizada; las versiones locales anteriores se conservan."}).into();
                    self.leave_remote();
                    return Some(report.asset_id);
                },
                Err(err)=>self.status=format!("{}: {err}",tr("Error al importar")),
            }
        }
        if let Some(rx)=&self.cover_rx {
            loop {
                match rx.try_recv() {
                    Ok(cover)=>{
                        self.cover_inflight.remove(&cover.id);
                        if let Some(image)=cover.image{
                            let tex=ctx.load_texture(
                                format!("szkura-remote-cover-{}",cover.id),image,
                                egui::TextureOptions::LINEAR);
                            self.cover_textures.insert(cover.id,tex);
                            self.cover_recent.retain(|id|*id!=cover.id);
                            self.cover_recent.push_back(cover.id);
                            while self.cover_recent.len()>MAX_COVER_TEXTURES{
                                if let Some(old)=self.cover_recent.pop_front(){
                                    self.cover_textures.remove(&old);
                                }
                            }
                        }else{self.cover_failed.insert(cover.id);}
                    },
                    Err(mpsc::TryRecvError::Empty)=>break,
                    Err(mpsc::TryRecvError::Disconnected)=>{
                        self.cover_rx=None;
                        self.cover_inflight.clear();
                        break;
                    },
                }
            }
        }
        self.start_cover_batch();
        if self.remote_active && self.connection.is_some() && self.detail.is_none()
            && self.browse_rx.is_none() {
            if let Some((previous_search,previous_cat,previous_avatar,previous_files,
                previous_nsfw,_))=&self.browse_parameters {
                if previous_search!=&self.search || previous_cat!=&self.category
                    || previous_avatar!=&self.avatar_filter
                    || previous_files!=&self.files_filter
                    || previous_nsfw!=&self.show_nsfw {self.offset=0;}
            }
            let request=(self.search.clone(),self.category.clone(),
                self.avatar_filter.clone(),self.files_filter,self.show_nsfw,self.offset);
            if self.browse_parameters.as_ref()!=Some(&request){
                if self.offset==0{
                    self.remote_page=RemotePage{total:0,items:Vec::new()};
                }
                self.request_browse();
            }
        }
        if self.host_load_rx.is_some() || self.host_rx.is_some() || self.connect_rx.is_some()
            || self.browse_rx.is_some() || self.detail_rx.is_some()
            || self.import_rx.is_some() || self.cover_rx.is_some()
            || !self.cover_queue.is_empty(){
            ctx.request_repaint_after(Duration::from_millis(120));
        }
        None
    }

    fn start_cover_batch(&mut self){
        if self.cover_rx.is_some(){return;}
        let Some(conn)=self.connection.clone() else{return;};
        let mut ids=Vec::new();
        while ids.len()<COVER_BATCH{
            let Some(id)=self.cover_queue.pop_front() else{break;};
            if self.cover_failed.contains(&id)
                || self.cover_textures.contains_key(&id)
                || !self.cover_inflight.insert(id){continue;}
            ids.push(id);
        }
        if ids.is_empty(){return;}
        let (tx,rx)=mpsc::channel();
        self.cover_rx=Some(rx);
        thread::spawn(move||{
            for id in ids{
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
            self.avatar_filter.clone(),self.files_filter,self.show_nsfw,self.offset);
        self.browse_parameters=Some(params.clone());
        let path=self.local_db_path.clone();
        let (tx,rx)=mpsc::channel();
        self.browse_rx=Some(rx);
        thread::spawn(move||{
            let result=(||->anyhow::Result<_>{
                let mut page=library_lan::browse(&connection,&params.0,&params.1,&params.2,
                    params.3,params.4,params.5)?;
                if let Some(path)=path{
                    library_lan::decorate_local_states(&path,&mut page)?;
                }
                Ok(page)
            })().map_err(|err|format!("{err:#}"));
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
        let Some(connection)=self.connection.clone() else{return;};
        let Some(db_path)=self.local_db_path.clone() else{return;};
        if self.detail_rx.is_some(){return;}
        let (tx,rx)=mpsc::channel();
        self.detail_rx=Some(rx);
        thread::spawn(move||{
            let result=(||->anyhow::Result<_>{
                let asset=library_lan::get_asset(&connection,id)?;
                let state=library_lan::verified_local_state(&db_path,&asset)?;
                Ok((asset,state))
            })().map_err(|err|format!("{err:#}"));
            let _=tx.send(result);
        });
    }
    fn start_import(&mut self,id:u64,
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
            let result=library_lan::import_asset(&conn,id,&db,&storage,&covers,&cancel)
                .map_err(|err|format!("{err:#}"));
            let _=tx.send(result);
        });
    }

    /// Import confirmation is required even for updates to already-linked
    /// BOOTH assets, because the old local versions must be preserved.
    pub fn render_import_confirm(&mut self,ctx:&egui::Context,
        db_path:&Path,storage:&Path,covers:&Path){
        let Some(id)=self.pending_import else{return;};
        let (name,count)=self.detail.as_ref().map(|asset|
            (asset.title.clone(),asset.files.len())).unwrap_or_default();
        let decision=confirmation_dialog(ctx,"remote-library-import",
            "Importar ficha completa","Confirmar importación",
            ConfirmTone::Primary,self.import_rx.is_none(),|ui|{
                dialog_subject(ui,&name);
                dialog_text(ui,tr(
                    "Se comprobarán los archivos existentes y solo se descargarán los nuevos o cambiados."));
                dialog_text(ui,tr(
                    "La operación se confirma completa o no modifica la biblioteca. Los metadatos y versiones locales anteriores se conservan."));
                ui.label(RichText::new(format!("{}: {count}",
                    tr("Archivos de la ficha"))).size(12.5).color(colors().muted));
            });
        match decision {
            ConfirmChoice::Accept=>{
                self.pending_import=None;
                self.start_import(id,db_path,storage,covers);
            },
            ConfirmChoice::Cancel=>self.pending_import=None,
            ConfirmChoice::Pending=>{},
        }
    }

    fn draw_cover(&mut self,ui:&mut egui::Ui,id:u64,
        has_cover:bool,width:f32,height:f32){
        if self.cover_textures.contains_key(&id){
            self.cover_recent.retain(|item|*item!=id);
            self.cover_recent.push_back(id);
        }else if has_cover && !self.cover_failed.contains(&id)
            && !self.cover_inflight.contains(&id)
            && !self.cover_queue.contains(&id){
            self.cover_queue.push_back(id);
            ui.ctx().request_repaint_after(Duration::from_millis(40));
        }
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

    fn state_label(state:RemoteLocalState)->&'static str{
        match state{
            RemoteLocalState::New=>"Nuevo en mi biblioteca",
            RemoteLocalState::Complete=>"Ya en mi biblioteca",
            RemoteLocalState::NeedsUpdate(_)=>"Archivos por actualizar",
            RemoteLocalState::Unknown=>"Comprobando disponibilidad local",
        }
    }
    fn state_badge(ui:&mut egui::Ui,state:RemoteLocalState){
        let tint=if state==RemoteLocalState::Complete{
            colors().muted
        }else{colors().accent};
        ui.label(RichText::new(tr(Self::state_label(state)))
            .size(11.5).strong().color(tint));
    }

    fn remote_tile(&mut self,ui:&mut egui::Ui,asset:&RemoteSummary,width:f32){
        ui.allocate_ui_with_layout(egui::vec2(width,330.0),
            egui::Layout::top_down(egui::Align::Min),|ui|{
            ui.set_width(width);
            egui::Frame::new().fill(colors().card)
                .stroke(egui::Stroke::new(1.0,colors().stroke))
                .corner_radius(12).inner_margin(13).show(ui,|ui|{
                    let inner=width-26.0;
                    ui.set_width(inner);
                    ui.set_min_height(302.0);
                    self.draw_cover(ui,asset.id,asset.has_cover,inner,170.0);
                    ui.add_space(9.0);
                    ui.add(egui::Label::new(RichText::new(&asset.title)
                        .strong().color(colors().text).size(16.0))
                        .truncate()).on_hover_text(&asset.title);
                    ui.add_space(1.0);
                    ui.add(egui::Label::new(
                        RichText::new(format!("{}{}",if asset.creator.is_empty(){
                            tr(&asset.category)
                        }else{&asset.creator},
                        if asset.nsfw{" · NSFW"}else{""}))
                        .color(colors().muted).size(12.5)).truncate());
                    ui.add_space(4.0);
                    if let Some(first)=asset.avatars.first(){
                        ui.label(RichText::new(format!("{} {}{}",
                            tr("Avatar:"),first,
                            if asset.avatars.len()>1{
                                format!(" +{}",asset.avatars.len()-1)
                            }else{String::new()}))
                            .size(12.0).color(colors().accent))
                            .on_hover_text(asset.avatars.join(", "));
                    }else{ui.add_space(18.0);}
                    ui.add_space(4.0);
                    Self::state_badge(ui,asset.local_state);
                    ui.horizontal(|ui|{
                        ui.label(RichText::new(format!("{} · {}",
                            asset.files,crate::human_size(asset.total_bytes as i64)))
                            .size(12.0).color(colors().muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                            if action(ui,Icon::File,"Abrir ficha",false){
                                self.fetch_detail(asset.id);
                            }
                        });
                    });
                });
        });
    }
    fn remote_row(&mut self,ui:&mut egui::Ui,asset:&RemoteSummary,compact:bool){
        egui::Frame::new().fill(colors().card).corner_radius(8)
            .stroke(egui::Stroke::new(1.0,colors().stroke))
            .inner_margin(egui::Margin::symmetric(12,8)).show(ui,|ui|{
            ui.horizontal(|ui|{
                self.draw_cover(ui,asset.id,asset.has_cover,76.0,64.0);
                ui.add_space(10.0);
                ui.vertical(|ui|{
                    ui.set_width(if compact{
                        (ui.available_width()-120.0).max(120.0)
                    }else{(ui.available_width()-300.0).max(150.0)});
                    ui.add(egui::Label::new(RichText::new(&asset.title)
                        .strong().size(15.0)).truncate()).on_hover_text(&asset.title);
                    ui.add(egui::Label::new(RichText::new(format!("{} · {}{}",
                        asset.creator,tr(&asset.category),
                        if asset.nsfw{" · NSFW"}else{""}))
                        .size(11.5).color(colors().muted)).truncate());
                    if let Some(first)=asset.avatars.first(){
                        ui.label(RichText::new(format!("{} {}",
                            tr("Avatar:"),first)).size(11.5).color(colors().accent));
                    }
                    Self::state_badge(ui,asset.local_state);
                });
                if !compact{
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                        if action(ui,Icon::File,"Abrir ficha",false){
                            self.fetch_detail(asset.id);
                        }
                        ui.label(RichText::new(format!("{} · {}",
                            asset.files,crate::human_size(asset.total_bytes as i64)))
                            .size(12.0).color(colors().muted));
                    });
                }
            });
            if compact{
                ui.horizontal(|ui|{
                    ui.label(RichText::new(format!("{} · {}",
                        asset.files,crate::human_size(asset.total_bytes as i64)))
                        .size(12.0).color(colors().muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                        if action(ui,Icon::File,"Abrir ficha",false){
                            self.fetch_detail(asset.id);
                        }
                    });
                });
            }
        });
    }
    fn detail_view(&mut self,ui:&mut egui::Ui,asset:&RemoteAsset,
        db_path:&Path,storage:&Path,covers:&Path){
        ui.horizontal_wrapped(|ui|{
            if action(ui,Icon::Back,"Volver al catálogo remoto",true){
                self.detail=None;
                return;
            }
            ui.add_space(8.0);
            ui.label(RichText::new(&asset.title)
                .strong().size(23.0).color(colors().text));
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui|{
            ui.label(RichText::new(tr("Biblioteca remota (solo lectura)"))
                .color(colors().accent).size(12.5));
            let button_label=match self.detail_state{
                RemoteLocalState::Complete=>"Ya está en mi biblioteca",
                RemoteLocalState::NeedsUpdate(_)=>"Actualizar ficha completa",
                _=>"Importar ficha completa",
            };
            if ui.add_enabled(self.import_rx.is_none()
                && self.detail_state!=RemoteLocalState::Complete,
                egui::Button::new(tr(button_label))
                    .min_size(egui::vec2(180.0,36.0))
                    .fill(colors().primary)).clicked(){
                self.pending_import=Some(asset.id);
            }
            if let RemoteLocalState::NeedsUpdate(files)=self.detail_state{
                ui.label(RichText::new(format!("{}: {files}",
                    tr("Archivos nuevos o modificados")))
                    .color(colors().accent).size(12.0));
            }
            if self.import_rx.is_some(){
                ui.spinner();
                if ui.button(tr("Cancelar descarga")).clicked(){
                    self.import_cancel.store(true,Ordering::Relaxed);
                }
            }
        });
        ui.separator();ui.add_space(12.0);
        egui::ScrollArea::vertical().id_salt("remote-detail-scroll")
            .show(ui,|ui|{
            let wide=ui.available_width()>=940.0;
            let draw_fields=|ui:&mut egui::Ui|{
                ui.label(RichText::new(format!("{} · {}",
                    asset.creator,tr(&asset.category)))
                    .color(colors().muted).size(13.0));
                if !asset.booth_url.is_empty(){
                    ui.hyperlink_to(tr("Abrir producto en BOOTH"),&asset.booth_url);
                }
                if !asset.avatars.is_empty(){
                    ui.label(RichText::new(format!("{}: {}",
                        tr("Avatares compatibles"),asset.avatars.join(", ")))
                        .color(colors().accent).size(13.0));
                }
                if !asset.tags.is_empty(){
                    ui.label(RichText::new(format!("{}: {}",
                        tr("Otras etiquetas"),asset.tags))
                        .size(12.5).color(colors().muted));
                }
                ui.add_space(12.0);
                if !asset.description.is_empty(){
                    ui.label(RichText::new(tr("Descripción")).strong().size(17.0));
                    ui.label(RichText::new(&asset.description)
                        .size(13.0).color(colors().text));
                }
                ui.add_space(15.0);
                ui.separator();
                ui.label(RichText::new(tr("Archivos vinculados"))
                    .strong().size(17.0));
                if asset.files.is_empty(){
                    ui.label(RichText::new(tr("Sin archivos"))
                        .color(colors().muted).size(12.5));
                }
                for file in &asset.files{
                    ui.horizontal_wrapped(|ui|{
                        icon(ui,Icon::Archive,colors().muted,16.0);
                        ui.label(RichText::new(&file.name)
                            .size(12.5).color(colors().text));
                        ui.label(RichText::new(crate::human_size(file.size as i64))
                            .size(11.5).color(colors().muted));
                    });
                }
            };
            if wide{
                let content=ui.available_width();
                let image_width=(content*0.4).min(482.0);
                ui.horizontal_top(|ui|{
                    self.draw_cover(ui,asset.id,asset.has_cover,image_width,320.0);
                    ui.add_space(20.0);
                    ui.vertical(|ui|{
                        ui.set_width((content-image_width-20.0).max(230.0));
                        draw_fields(ui);
                    });
                });
            }else{
                let width=ui.available_width().min(460.0);
                self.draw_cover(ui,asset.id,asset.has_cover,width,260.0);
                ui.add_space(15.0);
                draw_fields(ui);
            }
        });
    }

    /// Remote pages reuse the local library's visual language and filters.
    /// No editing, deletion, favorite changes or database writes to the host.
    pub fn page(&mut self,ctx:&egui::Context,db_path:&Path,
        storage:&Path,covers:&Path){
        self.local_db_path=Some(db_path.to_path_buf());
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(colors().bg)
                .inner_margin(egui::Margin::symmetric(19,17)))
            .show(ctx,|ui|{
            if let Some(info)=self.connection.as_ref().map(|connection|connection.info.clone()){
                egui::Frame::new().fill(colors().panel)
                    .stroke(egui::Stroke::new(1.0,colors().accent))
                    .corner_radius(10).inner_margin(12).show(ui,|ui|{
                        ui.horizontal_wrapped(|ui|{
                            icon(ui,Icon::Link,colors().accent,20.0);
                            ui.vertical(|ui|{
                                ui.label(RichText::new(format!("{} {}",tr("Explorando biblioteca:"),
                                    info.name)).strong().size(15.0).color(colors().accent));
                                ui.label(RichText::new(tr(
                                    "Catálogo ajeno en solo lectura. Importar no modifica la biblioteca anfitriona."))
                                    .size(12.0).color(colors().muted));
                            });
                            ui.label(RichText::new(tr("Solo lectura"))
                                .size(12.0).color(colors().accent));
                        });
                    });
                ui.add_space(12.0);
            }
            if self.connection.is_none(){
                ui.label(RichText::new(tr("Conectar a biblioteca"))
                    .size(23.0).strong().color(colors().text));
                ui.add_space(14.0);
                ui.label(RichText::new(tr(
                    "Introduce la invitación LAN o HTTPS de Cloudflare que compartió el anfitrión."))
                    .color(colors().muted).size(13.0));
                ui.add_sized([ui.available_width().min(700.0),36.0],
                    egui::TextEdit::singleline(&mut self.invite_input)
                        .hint_text("https://...trycloudflare.com/l/...#key=..."));
                if ui.add_enabled(self.connect_rx.is_none(),
                    egui::Button::new(tr("Conectar a biblioteca"))
                        .min_size(egui::vec2(190.0,36.0))
                        .fill(colors().primary)).clicked(){
                    self.start_connect();
                }
                if self.connect_rx.is_some(){ui.spinner();}
            }else if let Some(asset)=self.detail.clone(){
                self.detail_view(ui,&asset,db_path,storage,covers);
            }else{
                let name=self.connection.as_ref().unwrap().info.name.clone();
                ui.horizontal(|ui|{
                    ui.label(RichText::new(tr("Biblioteca remota"))
                        .strong().size(23.0).color(colors().text));
                    ui.label(RichText::new(&name).size(13.0).color(colors().muted));
                    ui.label(RichText::new(format!("{} {}",
                        self.remote_page.total,tr("productos")))
                        .size(13.0).color(colors().muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                        if ui.selectable_label(!self.remote_grid,tr("Lista")).clicked(){
                            self.remote_grid=false;
                        }
                        if ui.selectable_label(self.remote_grid,tr("Cuadrícula")).clicked(){
                            self.remote_grid=true;
                        }
                    });
                });
                if !self.avatar_filter.is_empty(){
                    ui.horizontal(|ui|{
                        icon(ui,Icon::Filter,colors().accent,15.0);
                        ui.label(RichText::new(format!("{} {}",
                            tr("Avatar:"),self.avatar_filter))
                            .size(13.0).color(colors().accent));
                        if ui.button(tr("Quitar filtro")).clicked(){
                            self.avatar_filter.clear();
                        }
                    });
                }
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui|{
                    ui.label(RichText::new(format!("{}: {}",tr("Resultados"),
                        self.remote_page.total))
                        .color(colors().muted).size(12.5));
                    if self.browse_rx.is_some(){ui.spinner();}
                    ui.label(RichText::new(tr("Los productos se cargan al desplazarse"))
                        .size(11.5).color(colors().muted));
                });
                if self.remote_page.items.is_empty() && self.browse_rx.is_none(){
                    ui.vertical_centered(|ui|{
                        ui.add_space(45.0);
                        ui.label(RichText::new(tr("No hay resultados")).size(20.0));
                        ui.label(RichText::new(tr(
                            "Prueba otra categoría, quita el filtro de ZIP o borra la búsqueda."))
                            .size(13.0).color(colors().muted));
                    });
                }else{
                    let item_count=self.remote_page.items.len();
                    let output=if !self.remote_grid {
                        let compact=ui.available_width()<1020.0;
                        let row_height=if compact{136.0}else{96.0};
                        egui::ScrollArea::vertical().id_salt("remote-product-rows")
                            .show_rows(ui,row_height,item_count,|ui,range|{
                                for index in range{
                                    let item=self.remote_page.items[index].clone();
                                    self.remote_row(ui,&item,compact);
                                }
                            })
                    }else{
                        let space=ui.available_width().max(210.0);
                        let gap=ui.spacing().item_spacing.x;
                        let columns=(((space+gap)/(325.0+gap)).floor() as usize).max(1);
                        let width=((space-gap*(columns-1) as f32)/columns as f32).max(200.0);
                        let rows=item_count.div_ceil(columns);
                        egui::ScrollArea::vertical().id_salt("remote-product-grid")
                            .show_rows(ui,330.0,rows,|ui,range|{
                                for index in range{
                                    let start=index*columns;
                                    let end=(start+columns).min(item_count);
                                    let visible=self.remote_page.items[start..end].to_vec();
                                    ui.horizontal_top(|ui|{
                                        for item in &visible{
                                            self.remote_tile(ui,item,width);
                                        }
                                    });
                                }
                            })
                    };
                    // Hide protocol pagination. Keep exactly the local catalog's
                    // virtualized scrolling and fetch the next page near its end.
                    let near_end=output.state.offset.y+output.inner_rect.height()+560.0
                        >=output.content_size.y;
                    if near_end && self.browse_rx.is_none()
                        && self.remote_page.items.len()<self.remote_page.total {
                        self.offset=self.remote_page.items.len();
                        self.request_browse();
                    }
                }
            }
            if !self.status.is_empty(){
                ui.add_space(7.0);
                ui.add(egui::Label::new(
                    RichText::new(tr(&self.status))
                        .size(12.0).color(colors().muted)).truncate())
                    .on_hover_text(tr(&self.status));
            }
        });
    }

}

#[cfg(test)]
mod picker_tests {
    use super::*;
    #[test]
    fn changing_filters_while_fetching_does_not_leak_stale_results(){
        let (tx,rx)=mpsc::channel();
        let mut ui=LibraryUi::new();
        ui.search="original".into();
        ui.browse_parameters=Some(("original".into(),String::new(),
            String::new(),FileFilter::All,false,0));
        ui.browse_rx=Some(rx);
        ui.search="nuevo".into();
        tx.send(Ok(RemotePage{total:1,items:vec![RemoteSummary{
            id:1,origin_key:"a".repeat(64),booth_url:String::new(),
            signatures:Vec::new(),title:"No debería aparecer".into(),
            category:"Ropa".into(),creator:String::new(),tags:String::new(),
            avatars:Vec::new(),nsfw:false,has_cover:false,files:0,
            total_bytes:0,local_state:RemoteLocalState::Unknown,
        }]})).unwrap();
        let _=ui.poll(&egui::Context::default());
        assert_eq!(ui.remote_page.total,0);
        assert!(ui.remote_page.items.is_empty());
        assert_eq!(ui.offset,0);
        assert!(ui.browse_parameters.is_none());
    }

    #[test]
    fn loads_more_than_one_sql_page_for_publication_picker(){
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("catalog.sqlite");
        let mut db=Catalog::open(&path).unwrap();
        for i in 0..530{
            db.create_product(&format!("Producto {i}"),"Ropa").unwrap();
        }
        let products=load_publishable_products(&db).unwrap();
        assert_eq!(products.len(),530);
    }

    #[test]
    fn publishing_defaults_exclude_mode_but_never_includes_nsfw_without_opt_in(){
        let tmp=tempfile::tempdir().unwrap();
        let db_path=tmp.path().join("catalog.sqlite");
        let mut db=Catalog::open(&db_path).unwrap();
        let first=db.create_product("Normal A","Ropa").unwrap();
        let second=db.create_product("Normal B","Ropa").unwrap();
        let sensitive=db.create_product("Content flagged","Ropa").unwrap();
        db.set_nsfw(sensitive,true).unwrap();
        let mut ui=LibraryUi::new();
        ui.host_products=load_publishable_products(&db).unwrap();
        ui.host_product_ids=ui.host_products.iter().map(|asset|asset.id).collect();
        ui.host_sensitive_ids=ui.host_products.iter()
            .filter(|asset|asset.nsfw).map(|asset|asset.id).collect();
        assert!(ui.mode_is_exclude());
        assert_eq!(ui.selection_count(),2);
        assert!(!ui.selected(sensitive));
        ui.toggle_selected(first);
        assert!(!ui.selected(first));
        assert_eq!(ui.selection_count(),1);
        ui.host_mode=PublishMode::Include;
        assert_eq!(ui.selection_count(),0);
        ui.toggle_selected(second);
        assert!(ui.selected(second));
        assert_eq!(ui.selection_count(),1);
        ui.toggle_selected(sensitive);
        assert!(!ui.selected(sensitive));
        ui.host_publish_nsfw=true;
        ui.toggle_selected(sensitive);
        assert!(ui.selected(sensitive));
        assert_eq!(ui.selection_count(),2);
        assert!(ui.selection().products.contains(&second));
        assert!(!ui.selection().products.contains(&first));
    }

    #[test]
    fn pending_publication_consent_blocks_concurrent_tunnel_workflows(){
        let mut ui=LibraryUi::new();
        assert!(!ui.host_busy());
        ui.pending_share_confirm=true;
        assert!(ui.host_busy());
        ui.pending_share_confirm=false;
        ui.pending_tunnel_consent=true;
        assert!(ui.host_busy());
        ui.stop_share();
        assert!(!ui.host_busy());
    }

    #[test]
    fn host_and_remote_are_disabled_at_start(){
        let ui=LibraryUi::new();
        assert!(!ui.host_busy());
        assert!(!ui.picker_open);
        assert!(!ui.remote_active);
        assert_eq!(ui.selection_count(),0);
    }
}
