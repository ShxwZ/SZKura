#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod ui_support;
mod cover_cache;
mod theme;
mod i18n;
mod sharing_ui;
mod startup_diagnostics;
use sharing_ui::SharingUi;
use cover_cache::{CoverRequest, CoverResult};
use theme::{ThemeKind,colors};
use i18n::{Language,tr};
use ui_support::{Icon, action, icon, icon_action, favorite_action, nav_choice, nav_sensitive_choice,
    install_cjk_font, confirmation_dialog, dialog_subject, dialog_text, ConfirmChoice, ConfirmTone};
use anyhow::{Context, Result};
use eframe::egui::{self, Color32, RichText, TextureHandle};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{mpsc::{self, Receiver, Sender},Arc,atomic::{AtomicBool,AtomicU64,Ordering}},
    time::Duration,
};
use szkura::{
    booth,
    db::{ArchiveMode, Asset, Catalog, FileFilter, CATEGORIES},
    import,
    managed_storage,
    backup,
    migration,
    updater::{self,AvailableUpdate},
    package_inspect::{self, ZipPreview, UnityPackage},
    unitypackage_inspect::{self, UnityPathsPreview},
    unitypackage_labels,
    avatar_detection,
};

// All surface, foreground and accent colors come from the selected palette.

#[derive(Clone)]
struct AppPaths { db: PathBuf, library: PathBuf, covers: PathBuf, unity_cache: PathBuf }
impl AppPaths {
    fn discover() -> Result<Self> {
        let dirs=directories::ProjectDirs::from("dev","ShxwZ","SZKura")
            .context("No se pudo localizar la carpeta de datos de SZKura")?;
        let old_dirs=directories::ProjectDirs::from("dev","ShxwZ","SZ_BOOTH")
            .context("No se pudo localizar la carpeta de datos anterior")?;
        let root=dirs.data_local_dir().to_path_buf();
        migration::migrate_if_needed(old_dirs.data_local_dir(),&root)
            .context("No se pudo migrar automáticamente la biblioteca de SZ_BOOTH a SZKura. Los originales siguen intactos")?;
        std::fs::create_dir_all(&root)?;
        Ok(Self { db:root.join("catalog.sqlite"),library:root.join("library"),
            covers:root.join("covers"),unity_cache:dirs.cache_dir().join("unitypackages") })
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum CatalogView { Grid, List }

struct JobResult { message:String, select_id:Option<i64> }
struct JobFinished { message:String, select_id:Option<i64>, ok:bool }

#[derive(Clone, Copy, PartialEq, Eq)]
enum AvatarScanTarget { New, Edit(i64) }
struct AvatarScanFinished {
    target: AvatarScanTarget,
    generation: u64,
    source_paths: Vec<PathBuf>,
    report: avatar_detection::ScanResult,
}

/// One LRU entry owns its foreground and optional 64px background textures.
/// Their combined bytes count toward the existing 48 MiB texture budget.
#[derive(Clone)]
struct CoverTextures {
    foreground:TextureHandle,
    background:Option<TextureHandle>,
    bytes:usize,
}

struct BoothApp {
    paths: AppPaths,
    db: Catalog,
    sharing: SharingUi,
    assets: Vec<Asset>,
    total: i64,
    filtered: i64,
    filtered_bytes: i64,
    query: String,
    category: String,
    avatar_filter: String,
    avatar_options: Vec<String>,
    files_filter: FileFilter,
    displayed_limit: i64,
    last_filters: (String,String,String,FileFilter,bool,bool),
    favorites_only: bool,
    favorites_dirty: bool,
    show_nsfw: bool,
    catalog_view: CatalogView,
    ui_palette: ThemeKind,
    ui_language: Language,
    configured: bool,
    show_settings: bool,
    setup_path: String,
    delete_sources: bool,
    confirm_new_import: bool,
    pending_managed_link: Option<(i64,Vec<PathBuf>)>,
    pending_restore: Option<PathBuf>,
    restore_in_flight: bool,
    check_updates_on_start:bool,
    auto_install_updates:bool,
    update_check_rx:Option<Receiver<Result<Option<AvailableUpdate>,String>>>,
    update_install_rx:Option<Receiver<Result<(),String>>>,
    available_update:Option<AvailableUpdate>,
    update_downloaded:Arc<AtomicU64>,
    auto_update_attempted:bool,
    selected: Option<i64>,
    edit_title:String, edit_creator:String, edit_category:String, edit_tags:String,
    edit_avatars:String, edit_nsfw:bool,
    edit_description:String, edit_booth:String, edit_description_expanded:bool,
    new_dialog: bool,
    new_url:String, new_title:String, new_category:String, new_tags:String,
    new_avatars:String, new_nsfw:bool,
    new_creator:String, new_description:String, new_cover:Option<PathBuf>,
    new_description_expanded:bool, new_files:Vec<PathBuf>,
    pending_quick_delete:Option<(i64,String)>,
    status:String,
    receiver:Option<Receiver<JobFinished>>,
    inspected_zip:Option<String>,
    archive_preview:Option<ZipPreview>,
    archive_scan:Option<Receiver<Result<(ZipPreview,package_inspect::CacheUse),String>>>,
    avatar_scan:Option<Receiver<AvatarScanFinished>>,
    avatar_scan_cancel:Option<Arc<AtomicBool>>,
    avatar_scan_generation:u64,
    unity_paths_key:Option<String>,
    unity_paths_label:String,
    unity_paths_scan:Option<Receiver<Result<UnityPathsPreview,String>>>,
    unity_paths_preview:Option<UnityPathsPreview>,
    unity_paths_error:Option<String>,
    confirm_clear_cache:bool,
    textures:HashMap<String,CoverTextures>, texture_keys:VecDeque<String>,
    texture_bytes:usize, cjk_loaded:bool,
    cover_job_tx:Sender<CoverRequest>, cover_result_rx:Receiver<CoverResult>,
    pending_covers:HashSet<String>, failed_covers:HashSet<String>,
}

impl BoothApp {
    fn apply_theme(ctx:&egui::Context){
        let p=colors();
        let mut style=(*ctx.style()).clone();
        style.visuals=egui::Visuals::dark();
        style.visuals.panel_fill=p.bg;
        style.visuals.window_fill=p.panel;
        style.visuals.widgets.inactive.bg_fill=p.button_bg;
        style.visuals.widgets.hovered.bg_fill=p.hover_bg;
        style.visuals.widgets.active.bg_fill=p.primary;
        style.visuals.selection.bg_fill=p.selection;
        style.visuals.extreme_bg_color=p.extreme_bg;
        style.visuals.faint_bg_color=p.faint_bg;
        style.visuals.widgets.inactive.fg_stroke.color=p.text;
        style.visuals.widgets.hovered.fg_stroke.color=p.text;
        style.visuals.widgets.active.fg_stroke.color=p.text;
        style.visuals.widgets.inactive.bg_stroke=egui::Stroke::new(1.0,p.stroke);
        style.visuals.widgets.hovered.bg_stroke=egui::Stroke::new(1.0,p.accent);
        style.visuals.window_stroke=egui::Stroke::new(1.0,p.stroke);
        style.visuals.widgets.noninteractive.fg_stroke.color=p.text;
        style.text_styles.insert(egui::TextStyle::Body,egui::FontId::proportional(16.0));
        style.text_styles.insert(egui::TextStyle::Button,egui::FontId::proportional(15.0));
        style.text_styles.insert(egui::TextStyle::Heading,egui::FontId::proportional(23.0));
        style.spacing.item_spacing=egui::vec2(9.0,9.0);
        style.spacing.interact_size.y=36.0;
        style.spacing.button_padding=egui::vec2(12.0,7.0);
        ctx.set_style(style);
    }
    fn new(cc:&eframe::CreationContext<'_>) -> Result<Self> {
        let cjk_loaded=install_cjk_font(&cc.egui_ctx);
        let mut paths=AppPaths::discover()?;
        let db=Catalog::open(&paths.db)?;
        let storage=db.storage_root()?;
        let configured=storage.as_ref().is_some_and(|p|p.is_dir());
        let check_updates_on_start=db.preference("check_updates_on_start",true)?;
        let auto_install_updates=db.preference("auto_install_updates",false)?;
        let sharing_enabled=db.preference("sharing_enabled",false)?;
        let delete_sources=db.preference("delete_sources_after_import",false)?;
        if let Some(root)=storage.as_ref(){paths.library=root.clone();}
        let (cover_job_tx, cover_result_rx)=cover_cache::spawn_workers(&cc.egui_ctx);
        let ui_palette=ThemeKind::from_id(&db.palette_id()?).unwrap_or(ThemeKind::Sage);
        theme::select(ui_palette);
        let ui_language=Language::from_id(&db.language_id()?).unwrap_or(Language::Spanish);
        i18n::select(ui_language);
        Self::apply_theme(&cc.egui_ctx);
        let mut app=Self {
            paths,db,sharing:SharingUi::new(sharing_enabled),assets:Vec::new(),total:0,filtered:0,filtered_bytes:0,
            query:String::new(),category:String::new(),avatar_filter:String::new(),
            avatar_options:Vec::new(),files_filter:FileFilter::All,displayed_limit:96,
            last_filters:(String::new(),String::new(),String::new(),FileFilter::All,false,false),
            favorites_only:false,favorites_dirty:false,show_nsfw:false,
            catalog_view:CatalogView::Grid,ui_palette,ui_language,
            configured,show_settings:!configured,setup_path:storage
                .as_ref().map(|p|p.display().to_string()).unwrap_or_default(),
            delete_sources,confirm_new_import:false,pending_managed_link:None,
            pending_restore:None,restore_in_flight:false,
            check_updates_on_start,auto_install_updates,
            update_check_rx:None,update_install_rx:None,available_update:None,
            update_downloaded:Arc::new(AtomicU64::new(0)),
            auto_update_attempted:false,
            selected:None,
            edit_title:String::new(),edit_creator:String::new(),edit_category:"Otros".into(),
            edit_tags:String::new(),edit_avatars:String::new(),edit_nsfw:false,
            edit_description:String::new(),edit_booth:String::new(),
            edit_description_expanded:false,new_dialog:false,new_url:String::new(),
            new_title:String::new(),new_category:"Otros".into(),new_tags:String::new(),
            new_avatars:String::new(),new_nsfw:false,
            new_creator:String::new(),new_description:String::new(),new_cover:None,
            new_description_expanded:false,new_files:Vec::new(),
            pending_quick_delete:None,
            status:"Biblioteca lista. Cada producto puede tener varios archivos vinculados.".into(),
            receiver:None,inspected_zip:None,archive_preview:None,
            archive_scan:None,avatar_scan:None,
            avatar_scan_cancel:None,avatar_scan_generation:0,
            confirm_clear_cache:false,
            unity_paths_key:None,unity_paths_label:String::new(),
            unity_paths_scan:None,unity_paths_preview:None,unity_paths_error:None,
            textures:HashMap::new(),texture_keys:VecDeque::new(),
            texture_bytes:0,cjk_loaded,
            cover_job_tx,cover_result_rx,pending_covers:HashSet::new(),
            failed_covers:HashSet::new(),
        };
        app.reload()?;
        if (app.check_updates_on_start || app.auto_install_updates)
            && updater::installed_version().contains("-alpha.") {
            app.request_update_check();
        }
        Ok(app)
    }
    fn reload(&mut self)->Result<()> {
        self.avatar_options=self.db.avatar_options()?;
        if !self.avatar_filter.is_empty()
            && !self.avatar_options.iter().any(|x|x.eq_ignore_ascii_case(&self.avatar_filter)) {
            self.avatar_filter.clear();
        }
        self.assets=self.db.list_filtered_visibility(&self.query,&self.category,&self.avatar_filter,
            self.files_filter,self.favorites_only,self.show_nsfw,self.displayed_limit,0)?;
        self.total=self.db.count()?;
        (self.filtered,self.filtered_bytes)=self.db.stats_filtered_visibility(
            &self.query,&self.category,&self.avatar_filter,
            self.files_filter,self.favorites_only,self.show_nsfw)?;
        self.last_filters=(self.query.clone(),self.category.clone(),self.avatar_filter.clone(),
            self.files_filter,self.favorites_only,self.show_nsfw);
        Ok(())
    }
    fn persist_delete_source_choice(&mut self,previous:bool) {
        if let Err(error)=self.db.update_preference(
            "delete_sources_after_import",self.delete_sources
        ) {
            self.delete_sources=previous;
            self.status=format!("No se pudo guardar la preferencia de originales: {error}");
        }
    }

    fn select_id(&mut self,id:i64) {
        self.cancel_avatar_scan();
        match self.db.get(id) {
            Ok(Some(a))=>{
                self.selected=Some(id);self.edit_title=a.title;self.edit_category=a.category;
                self.edit_tags=a.tags;self.edit_creator=a.creator;self.edit_description=a.description;
                self.edit_avatars=self.db.avatars_for(id).unwrap_or_default().join(", ");
                self.edit_nsfw=a.nsfw;
                self.edit_booth=a.booth_url;
                self.inspected_zip=None;self.archive_preview=None;self.archive_scan=None;
                self.clear_unity_paths();
                self.edit_description_expanded=false;
            }
            Ok(None)=>self.status="El producto ya no existe".into(),
            Err(e)=>self.status=format!("Error de lectura: {e}"),
        }
    }
    /// Discard stale results and ask the background worker to stop at its next
    /// safe interruption point. Navigation never waits on decompression.
    fn cancel_avatar_scan(&mut self) {
        if let Some(cancel)=self.avatar_scan_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.avatar_scan=None;
        self.avatar_scan_generation=self.avatar_scan_generation.wrapping_add(1);
    }

    /// Explicit, asynchronous scan: never commits edits to SQLite.
    fn start_avatar_scan(&mut self,target:AvatarScanTarget) {
        if self.avatar_scan.is_some(){return;}
        let mut known=match self.db.avatar_names_for_scan(){
            Ok(names)=>names,
            Err(error)=>{
                self.status=format!("{}: {error}",tr("Error al leer avatares"));
                return;
            }
        };
        if known.is_empty(){
            self.status=tr("Primero añade algún avatar compatible a tu biblioteca.").into();
            return;
        }
        let current=match target {
            AvatarScanTarget::New=>&self.new_avatars,
            AvatarScanTarget::Edit(_)=>&self.edit_avatars,
        };
        if avatar_detection::remaining_slots(current)==0 {
            self.status=tr("Límite de 64 avatares por ficha").into();
            return;
        }
        let existing:HashSet<String>=current.split([',','\n',';'])
            .map(|name|name.trim().to_uppercase())
            .filter(|name|!name.is_empty()).collect();
        known.retain(|name|!existing.contains(name));
        if known.is_empty(){
            self.status=tr("No se encontraron nuevos avatares compatibles.").into();
            return;
        }
        let sources:Vec<(PathBuf,String)>=match target {
            AvatarScanTarget::New=>self.new_files.iter().map(|path|{
                let name=path.file_name().map(|n|n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (path.clone(),name)
            }).collect(),
            AvatarScanTarget::Edit(id)=>match self.db.attachments_for(id){
                Ok(files)=>files.into_iter().map(|attachment|
                    (PathBuf::from(attachment.stored_path),attachment.original_name)).collect(),
                Err(error)=>{
                    self.status=format!("{}: {error}",tr("Error al leer archivos"));
                    return;
                }
            },
        };
        if sources.is_empty(){
            self.status=tr("No hay archivos para analizar.").into();
            return;
        }
        let source_paths=sources.iter().map(|(path,_)|path.clone()).collect();
        let generation=self.avatar_scan_generation;
        let cancel=Arc::new(AtomicBool::new(false));
        let cancel_for_worker=Arc::clone(&cancel);
        let (sender,receiver)=mpsc::channel();
        self.avatar_scan_cancel=Some(cancel);
        self.avatar_scan=Some(receiver);
        self.status=tr("Analizando archivos...").into();
        std::thread::spawn(move||{
            let report=avatar_detection::scan_with_cancel(&sources,&known,&cancel_for_worker);
            let _=sender.send(AvatarScanFinished{
                target,generation,source_paths,report,
            });
        });
    }

    fn poll_avatar_scan(&mut self,ctx:&egui::Context) {
        if let Some(finished)=self.avatar_scan.as_ref().and_then(|rx|rx.try_recv().ok()){
            self.avatar_scan=None;
            self.avatar_scan_cancel=None;
            if finished.generation!=self.avatar_scan_generation { return; }
            let unchanged_sources=match finished.target {
                AvatarScanTarget::New=>self.new_files==finished.source_paths,
                AvatarScanTarget::Edit(id)=>self.db.attachments_for(id)
                    .map(|files| files.into_iter()
                        .map(|file| PathBuf::from(file.stored_path))
                        .collect::<Vec<_>>()==finished.source_paths)
                    .unwrap_or(false),
            };
            if !unchanged_sources {
                self.status=tr("Han cambiado los archivos; vuelve a analizarlos.").into();
                return;
            }
            let field=match finished.target {
                AvatarScanTarget::New if self.new_dialog=>Some(&mut self.new_avatars),
                AvatarScanTarget::Edit(id) if self.selected==Some(id) && !self.new_dialog=>
                    Some(&mut self.edit_avatars),
                _=>None,
            };
            if let Some(field)=field {
                let original_slots=avatar_detection::remaining_slots(field);
                let (merged,added)=avatar_detection::merge_avatars(field,&finished.report.matches);
                *field=merged;
                self.status=if added.is_empty(){
                    tr("No se encontraron nuevos avatares compatibles.").to_owned()
                }else{
                    format!("{}: {}",tr("Avatares añadidos"),added.join(", "))
                };
                // The catalog rejects more than 64 compatible avatars.
                if added.len()==original_slots && original_slots>0
                    && finished.report.matches.len()>added.len() {
                    self.status.push_str(&format!(" | {}",
                        tr("Límite de 64 avatares por ficha")));
                }
                if !finished.report.warnings.is_empty() {
                    let warning_count=finished.report.warnings.len();
                    self.status.push_str(&format!(" | {}: {warning_count}",
                        tr("Advertencias de análisis")));
                }
            }
        }
        if self.avatar_scan.is_some(){ctx.request_repaint_after(Duration::from_millis(100));}
    }

    fn launch<F>(&mut self, operation:F)
    where F:FnOnce(AppPaths)->Result<JobResult>+Send+'static {
        if self.receiver.is_some() || self.update_install_rx.is_some() {return;}
        let paths=self.paths.clone();let(sender,receiver)=mpsc::channel();
        self.receiver=Some(receiver);self.status="Procesando en segundo plano...".into();
        std::thread::spawn(move||{
            let finished=match operation(paths) {
                Ok(r)=>JobFinished{message:r.message,select_id:r.select_id,ok:true},
                Err(e)=>JobFinished{message:format!("Error: {e:#}"),select_id:None,ok:false},
            };
            let _=sender.send(finished);
        });
    }
    fn request_update_check(&mut self){
        if self.update_check_rx.is_some() || self.update_install_rx.is_some(){return;}
        let (tx,rx)=mpsc::channel();
        self.update_check_rx=Some(rx);
        self.auto_update_attempted=false;
        self.status="Comprobando nuevas versiones en GitHub Releases...".into();
        std::thread::spawn(move||{
            let result=updater::check_github().map_err(|error|format!("{error:#}"));
            let _=tx.send(result);
        });
    }
    fn start_verified_update(&mut self){
        if self.receiver.is_some() || self.update_install_rx.is_some(){return;}
        let Some(update)=self.available_update.clone() else{return};
        if !update.can_install(){
            self.status="La actualización automática solo está disponible para Windows y con SHA-256 oficial".into();
            return;
        }
        self.auto_update_attempted=true;
        self.update_downloaded.store(0,Ordering::Relaxed);
        let progress=Arc::clone(&self.update_downloaded);
        let (tx,rx)=mpsc::channel();
        self.update_install_rx=Some(rx);
        self.status=format!("Descargando {} con verificación SHA-256...",update.tag);
        std::thread::spawn(move||{
            let result=updater::install_verified(&update,&progress)
                .map_err(|error|format!("{error:#}"));
            let _=tx.send(result);
        });
    }
    fn poll_updates(&mut self,ctx:&egui::Context){
        let check=self.update_check_rx.as_ref().and_then(|rx|rx.try_recv().ok());
        if let Some(answer)=check {
            self.update_check_rx=None;
            match answer {
                Ok(Some(release))=>{
                    self.status=format!("Nueva versión {} disponible en GitHub",release.tag);
                    self.available_update=Some(release);
                },
                Ok(None)=>{
                    self.available_update=None;
                    self.status=if updater::installed_version().contains("-local"){
                        "Versión de desarrollo: consulta GitHub Releases para versiones publicadas".into()
                    }else{"SZKura ya está actualizado".into()};
                },
                Err(error)=>self.status=format!("No se pudieron comprobar actualizaciones: {error}"),
            }
        }
        if self.auto_install_updates && !self.auto_update_attempted
            && self.available_update.as_ref().is_some_and(AvailableUpdate::can_install)
            && self.receiver.is_none() && self.update_check_rx.is_none()
            && self.update_install_rx.is_none() {
            self.start_verified_update();
        }
        let install=self.update_install_rx.as_ref().and_then(|rx|rx.try_recv().ok());
        if let Some(result)=install {
            self.update_install_rx=None;
            match result {
                Ok(())=>{
                    self.status="Versión nueva instalada y reiniciada correctamente".into();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                },
                Err(error)=>self.status=format!("No se pudo instalar la actualización: {error}"),
            }
        }
        if self.update_check_rx.is_some() || self.update_install_rx.is_some(){
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn process_finished(&mut self,ctx:&egui::Context) {
        let result=self.receiver.as_ref().and_then(|rx|rx.try_recv().ok());
        if let Some(done)=result {
            self.receiver=None;self.status=done.message;
            if self.restore_in_flight {
                self.restore_in_flight=false;
                if done.ok {
                    match Catalog::open(&self.paths.db){
                        Ok(catalog)=>{
                            self.db=catalog;
                            self.selected=None;self.new_dialog=false;
                            self.inspected_zip=None;self.archive_preview=None;self.archive_scan=None;
                self.clear_unity_paths();
                            let root=self.db.storage_root().ok().flatten()
                                .filter(|root|root.is_dir());
                            self.configured=root.is_some();
                            self.show_settings=!self.configured;
                            self.delete_sources=self.db.preference(
                                "delete_sources_after_import",false
                            ).unwrap_or(false);
                            self.ui_palette=ThemeKind::from_id(
                                &self.db.palette_id().unwrap_or_else(|_|"sage".into())
                            ).unwrap_or(ThemeKind::Sage);
                            theme::select(self.ui_palette);
                            Self::apply_theme(ctx);
                            if let Some(root)=root {
                                self.paths.library=root.clone();
                                self.setup_path=root.display().to_string();
                            } else {
                                self.setup_path.clear();
                            }
                        },
                        Err(error)=>{
                            self.status=format!("Error reabriendo la BD restaurada: {error}");
                        }
                    }
                }
            }
            if let Err(err)=self.reload(){self.status=format!("Error actualizando la biblioteca: {err}");}
            if done.ok {
                self.textures.clear();self.texture_keys.clear();self.texture_bytes=0;
                self.failed_covers.clear();
            }
            if let Some(id)=done.select_id {self.select_id(id);}
        }
    }
    fn add_product(&mut self) {
        if !self.configured {
            self.show_settings=true;
            self.status="Configura primero la carpeta de almacenamiento".into();
            return;
        }
        if self.delete_sources && !self.new_files.is_empty() {
            self.confirm_new_import=true;
            return;
        }
        self.add_product_confirmed();
    }
    fn add_product_confirmed(&mut self) {
        let url=self.new_url.trim().to_owned();
        if url.is_empty() && self.new_title.trim().is_empty() {
            self.status="Introduce un enlace BOOTH o un nombre para el producto".into();return;
        }
        let canonical=if url.is_empty(){String::new()}else{
            match booth::canonical_url(&url){Ok(v)=>v,Err(e)=>{self.status=e.to_string();return;}}
        };
        let title=self.new_title.trim().to_string();let cat=self.new_category.clone();
        let creator=self.new_creator.trim().to_string();
        let description=self.new_description.trim().to_string();
        let cover=self.new_cover.clone();
        let tags=self.new_tags.trim().to_string();let avatars=self.new_avatars.trim().to_string();
        let nsfw=self.new_nsfw;
        let files=self.new_files.clone();
        let delete_sources=self.delete_sources;
        self.new_dialog=false;
        self.new_url.clear();self.new_title.clear();self.new_tags.clear();self.new_files.clear();
        self.new_creator.clear();self.new_description.clear();self.new_cover=None;
        self.new_avatars.clear();self.new_nsfw=false;
        self.new_description_expanded=false;
        self.launch(move|paths|{
            // Resolve BOOTH metadata before creating/organizing any linked files.
            // managed_storage::folder_for reads the catalog category and writes
            // directly to <root>/<category>/<product>: never import to "Otros"
            // and then recopy to the detected category.
            let booth_result=if canonical.is_empty(){None}else{
                Some(booth::lookup(&canonical,&paths.covers))
            };
            let booth_data=booth_result.as_ref().and_then(|result|result.as_ref().ok());
            let detected_category=booth_data.and_then(|item|item.category);
            let mut db=Catalog::open(&paths.db)?;
            let already_linked=!canonical.is_empty()
                && db.existing_booth_link(&canonical)?.is_some();
            let id=if canonical.is_empty(){db.create_product(&title,&cat)?}
                else{db.add_booth_link(&canonical)?};
            let existing=db.get(id)?.context("Producto no encontrado")?;
            let use_category=resolved_import_category(
                &cat,&existing.category,detected_category,already_linked);
            let use_title=if !title.is_empty(){title.clone()}
                else if existing.title.starts_with("BOOTH - "){
                    booth_data.map(|item|item.title.clone())
                        .unwrap_or_else(||existing.title.clone())
                }else{existing.title.clone()};
            let use_creator=if !creator.is_empty(){creator.clone()}
                else if !existing.creator.is_empty(){existing.creator.clone()}
                else{booth_data.map(|item|item.creator.clone()).unwrap_or_default()};
            let use_tags=if tags.is_empty(){existing.tags.clone()}else{tags.clone()};
            let use_description=if !description.is_empty(){description.clone()}
                else if !existing.description.is_empty(){existing.description.clone()}
                else{booth_data.map(|item|item.description.clone()).unwrap_or_default()};
            // Persist the FINAL category before the first attachment is copied.
            db.update_details(id,&use_title,&use_category,&use_creator,&use_tags,&use_description)?;
            db.set_avatars(id,&avatars)?;
            db.set_nsfw(id,nsfw || existing.nsfw)?;
            let mut warning=String::new();
            if let Some(result)=booth_result {
                match result {
                    Ok(b)=>{
                        let preferred=booth::product_url_with_shop(
                            &canonical,b.shop_subdomain.as_deref())?;
                        db.associate_booth(id,&preferred)?;
                        if !b.cover_path.is_empty(){db.set_cover(id,&b.cover_path)?;}
                    },
                    Err(err)=>warning=format!(
                        " | No se pudieron obtener los datos BOOTH ({err}). Puedes reintentar desde la ficha"),
                }
            }
            if let Some(source)=cover{
                let stored=cache_local_cover(&paths.covers,&source)?;
                db.set_cover(id,&stored)?;
            }
            drop(db);
            // The managed importer now observes a stable, resolved category.
            // A category fetch failure leaves the user's choice or "Otros"
            // intact and must not trigger a second filesystem move.
            let detected_notice=if !already_linked && cat=="Otros" &&
                detected_category==Some(use_category.as_str()){
                format!(" | Categoría BOOTH: {use_category}")
            }else{String::new()};
            let message=if !files.is_empty(){
                let report=managed_storage::import_paths(
                    &paths.db,&paths.library,&files,id,delete_sources)?;
                format!("Producto guardado | {} archivos organizados | {} repetidos | {} errores | {} originales a papelera{}{}{}",
                    report.linked,report.duplicates,report.rejected,report.moved_to_trash,
                    if report.warnings.is_empty(){String::new()}
                    else{format!(" ({})",report.warnings.join("; "))},detected_notice,warning)
            }else{format!(
                "Producto guardado. Puedes asociar archivos desde su ficha{detected_notice}{warning}")};
            Ok(JobResult{message,select_id:Some(id)})
        });
    }
    fn link_files(&mut self,id:i64,files:Vec<PathBuf>,_mode:ArchiveMode) {
        if files.is_empty(){return;}
        if !self.configured {
            self.show_settings=true;
            self.status="Primero configura el almacén de assets".into();
            return;
        }
        if self.delete_sources {
            self.pending_managed_link=Some((id,files));
            return;
        }
        self.link_files_confirmed(id,files,false);
    }
    fn link_files_confirmed(&mut self,id:i64,files:Vec<PathBuf>,delete_sources:bool){
        self.launch(move|paths|{
            let report=managed_storage::import_paths(
                &paths.db,&paths.library,&files,id,delete_sources)?;
            Ok(JobResult {
                message:format!("{} archivos organizados | {} repetidos | {} rechazados | {} originales enviados a papelera{}",
                    report.linked,report.duplicates,report.rejected,report.moved_to_trash,
                    if report.warnings.is_empty(){String::new()}
                    else{format!(" | {}",report.warnings.join("; "))}),
                select_id:Some(id)
            })
        });
    }
    /// Inspect archive metadata on a worker (including bounded nested ZIP reads).
    fn inspect_archive(&mut self,source:String,force:bool){
        if !force && self.inspected_zip.as_deref()==Some(&source) {
            self.inspected_zip=None;self.archive_preview=None;self.archive_scan=None;
                self.clear_unity_paths();
            return;
        }
        if self.archive_scan.is_some(){return;}
        self.inspected_zip=Some(source.clone());
        self.archive_preview=None;
        let (sender,receiver)=mpsc::channel();
        self.archive_scan=Some(receiver);
        self.status=if force {
            "Recalculando el hash y examinando el archivo de nuevo...".into()
        }else{
            "Buscando vista en SQLite; se examinará el archivo si es necesario...".into()
        };
        let db_path=self.paths.db.clone();
        std::thread::spawn(move||{
            let result=package_inspect::inspect_zip_cached(
                Path::new(&source),&db_path,force
            ).map_err(|e|format!("{e:#}"));
            let _=sender.send(result);
        });
    }
    fn clear_unity_paths(&mut self) {
        self.unity_paths_key=None;
        self.unity_paths_label.clear();
        self.unity_paths_scan=None;
        self.unity_paths_preview=None;
        self.unity_paths_error=None;
    }
    /// Inspect UnityPackage pathname metadata off the UI thread. Nested
    /// packages use the existing validated, bounded, on-demand cache extractor.
    fn inspect_unity_paths(&mut self, source:PathBuf, package:Option<UnityPackage>) {
        if self.unity_paths_scan.is_some() {return;}
        self.unity_paths_label=package.as_ref().map(|pkg|pkg.display_path())
            .unwrap_or_else(||source.file_name()
                .map(|name|name.to_string_lossy().into_owned())
                .unwrap_or_default());
        self.unity_paths_key=Some(unity_paths_id(&source,package.as_ref()));
        self.unity_paths_preview=None;
        self.unity_paths_error=None;
        let (sender,receiver)=mpsc::channel();
        self.unity_paths_scan=Some(receiver);
        let cache=self.paths.unity_cache.clone();
        std::thread::spawn(move||{
            let result=(|| -> Result<UnityPathsPreview> {
                let path=if let Some(package)=package {
                    package_inspect::extract_unitypackage_nested(&source,&package,&cache)?
                } else {source};
                unitypackage_inspect::inspect_unitypackage_paths(&path)
            })().map_err(|error|format!("{error:#}"));
            let _=sender.send(result);
        });
    }
    /// The same action opens and collapses the current package preview.
    /// Closing drops the receiver: an already-running worker cannot reopen it.
    fn toggle_unity_paths(&mut self,source:PathBuf,package:Option<UnityPackage>) {
        let key=unity_paths_id(&source,package.as_ref());
        if self.unity_paths_key.as_deref()==Some(key.as_str()) {
            self.clear_unity_paths();
        } else {
            self.inspect_unity_paths(source,package);
        }
    }
    fn poll_unity_paths(&mut self,ctx:&egui::Context) {
        if let Some(answer)=self.unity_paths_scan.as_ref().and_then(|rx|rx.try_recv().ok()){
            self.unity_paths_scan=None;
            match answer {
                Ok(preview)=>{self.unity_paths_preview=Some(preview);self.unity_paths_error=None;},
                Err(error)=>{self.unity_paths_preview=None;self.unity_paths_error=Some(error);},
            }
        }
        if self.unity_paths_scan.is_some() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }
    fn poll_archive_scan(&mut self){
        if let Some(answer)=self.archive_scan.as_ref().and_then(|rx|rx.try_recv().ok()){
            self.archive_scan=None;
            match answer {
                Ok((preview,cache_use))=>{
                    self.status=format!(
                        "{} | {} entradas exteriores, {} interiores, {} ZIP internos, {} paquetes Unity{}",
                        cache_use.label(),preview.total_entries,preview.nested_entries,
                        preview.nested_scanned,preview.total_packages,
                        if preview.nested_skipped>0 {
                            format!(" | {} ZIP internos omitidos por seguridad o error",preview.nested_skipped)
                        }else{String::new()}
                    );
                    self.archive_preview=Some(preview);
                },
                Err(error)=>{
                    self.archive_preview=None;
                    self.status=format!("No se pudo examinar el archivo: {error}");
                }
            }
        }
    }
    /// Opening from SZKura always creates (or reuses) a labeled copy in the
    /// bounded Unity cache. Never open the untagged source if labeling fails:
    /// the user explicitly requested that the normal import action tags assets.
    fn launch_unity_package(
        &mut self,source:PathBuf,entry:Option<UnityPackage>,id:i64,category:String
    ){
        self.launch(move|paths|{
            let original=if let Some(entry)=entry {
                package_inspect::extract_unitypackage_nested(
                    &source,&entry,&paths.unity_cache
                )?
            }else{source};
            let label=unitypackage_labels::label_for_category(&category)?;
            let tagged=unitypackage_labels::cached_labeled_unitypackage(
                &original,&paths.unity_cache,&category
            )?;
            let message=match open_unity_file(&tagged){
                Ok(())=>format!("Copia etiquetada enviada a Unity: {label}"),
                Err(error)=>{
                    // Unity may be absent or OS associations unavailable. Only
                    // reveal the labeled copy for importing manually.
                    let _=reveal_file(&tagged.to_string_lossy());
                    format!("No se abrió Unity; importa manualmente la copia etiquetada: {} ({error})",
                        tagged.display())
                }
            };
            Ok(JobResult{message,select_id:Some(id)})
        });
    }
    fn start_new(&mut self) {
        self.cancel_avatar_scan();
        if !self.configured {self.show_settings=true;return;}
        self.selected=None;
        self.inspected_zip=None;self.archive_preview=None;self.archive_scan=None;
                self.clear_unity_paths();
        self.new_url.clear();self.new_title.clear();self.new_creator.clear();
        self.new_category="Otros".into();self.new_tags.clear();self.new_avatars.clear();
        self.new_nsfw=false;
        self.new_description.clear();self.new_description_expanded=false;
        self.new_files.clear();self.new_cover=None;self.new_dialog=true;
        // La preferencia de mover originales se mantiene entre importaciones.
    }

    fn queue_new_paths(&mut self, paths:Vec<PathBuf>) {
        if !self.configured {self.show_settings=true;
            self.status="Antes de importar elige la carpeta de almacenamiento".into();return;}
        let valid=paths.into_iter().filter(|p|p.is_dir()||import::supported(p)).collect::<Vec<_>>();
        if valid.is_empty(){
            self.status="Arrastra ZIP, 7z, RAR, UnityPackages o carpetas para vincularlos".into();
            return;
        }
        if !self.new_dialog {self.start_new();}
        if self.new_title.trim().is_empty() {
            if let Some(p)=valid.first() {
                self.new_title=p.file_stem()
                    .or_else(||p.file_name()).map(|n|n.to_string_lossy().into_owned())
                    .unwrap_or_else(||"Colección de archivos".into());
            }
        }
        self.new_files.extend(valid);self.new_files.sort();self.new_files.dedup();
        self.status="Archivos preparados: se copiarán al almacén al guardar el producto".into();
    }

    fn import_bulk(&mut self, folder:bool) {
        if !self.configured {self.show_settings=true;return;}
        if self.receiver.is_some(){return;}
        let paths=if folder{
            rfd::FileDialog::new().pick_folder().map(|p|vec![p])
        }else{
            rfd::FileDialog::new()
                .add_filter(tr("ZIP, 7z, RAR y UnityPackage"),&["zip","7z","rar","unitypackage"]).pick_files()
        };
        if let Some(paths)=paths {self.queue_new_paths(paths);}
    }

    fn handle_dropped_files(&mut self,ctx:&egui::Context) {
        let paths=ctx.input(|i|i.raw.dropped_files.iter()
            .filter_map(|f|f.path.clone()).collect::<Vec<_>>());
        if paths.is_empty(){return;}
        if self.receiver.is_some(){
            self.status="La biblioteca está procesando otra operación".into();
            return;
        }
        let valid=paths.into_iter()
            .filter(|p|p.is_dir()||import::supported(p)).collect::<Vec<_>>();
        if valid.is_empty(){
            self.status="Solo se pueden vincular ZIP, 7z, RAR, UnityPackages o carpetas".into();
        }else if self.new_dialog {
            self.queue_new_paths(valid);
        }else if let Some(id)=self.selected {
            self.link_files(id,valid,ArchiveMode::Reference);
        }else {
            self.queue_new_paths(valid);
        }
    }
    fn refresh_booth(&mut self,id:i64) {
        let raw=self.edit_booth.trim().to_owned();
        let url=match booth::canonical_url(&raw){Ok(u)=>u,Err(e)=>{self.status=e.to_string();return;}};
        self.launch(move|paths|{
            let mut db=Catalog::open(&paths.db)?;
            db.associate_booth(id,&url)?;
            // Do not lose the product or its ZIP files when BOOTH is unavailable.
            let response=match booth::lookup(&url,&paths.covers){
                Ok(b)=>{
                    let preferred=booth::product_url_with_shop(
                        &url,b.shop_subdomain.as_deref()
                    )?;
                    db.update_metadata(id,&preferred,&b.title,&b.description,&b.cover_path)?;
                    db.fill_creator_if_blank(id,&b.creator)?;
                    format!("Datos y tienda actualizados desde BOOTH: {}",b.title)
                },
                Err(e)=>format!("Enlace guardado; BOOTH no ha devuelto datos ({e}). Puedes reintentar"),
            };
            Ok(JobResult{message:response,select_id:Some(id)})
        });
    }
    fn import_cover(&mut self,id:i64,source:&Path)->Result<()> {
        let stored=cache_local_cover(&self.paths.covers,source)?;
        self.db.set_cover(id,&stored)?;
        self.textures.clear();self.texture_keys.clear();self.texture_bytes=0;
        self.failed_covers.clear();self.reload()?;
        Ok(())
    }

    /// Upload a few completed covers per frame to avoid a resize/scroll hitch.
    fn poll_cover_results(&mut self,ctx:&egui::Context) {
        const MAX_UPLOADS_PER_FRAME:usize=2;
        for _ in 0..MAX_UPLOADS_PER_FRAME {
            let Ok(finished)=self.cover_result_rx.try_recv() else {break};
            self.pending_covers.remove(&finished.key);
            if let Some(image)=finished.image {
                let mut bytes=image.pixels.len().saturating_mul(4);
                let foreground=ctx.load_texture(&finished.key,image,egui::TextureOptions::LINEAR);
                let background=finished.backdrop.map(|backdrop|{
                    bytes=bytes.saturating_add(backdrop.pixels.len().saturating_mul(4));
                    ctx.load_texture(format!("{}:blur",finished.key),
                        backdrop,egui::TextureOptions::LINEAR)
                });
                self.texture_bytes=self.texture_bytes.saturating_add(bytes);
                self.textures.insert(finished.key.clone(),CoverTextures{
                    foreground,background,bytes,
                });
                self.texture_keys.retain(|key|key!=&finished.key);
                self.texture_keys.push_back(finished.key);
                // LRU rather than FIFO: frequently displayed cards stay cached.
                const MAX_TEXTURE_BYTES:usize=48*1024*1024;
                while self.texture_bytes>MAX_TEXTURE_BYTES && self.texture_keys.len()>1 {
                    if let Some(old)=self.texture_keys.pop_front(){
                        if let Some(removed)=self.textures.remove(&old) {
                            self.texture_bytes=self.texture_bytes.saturating_sub(removed.bytes);
                        }
                    }
                }
            }else{
                // Avoid re-reading corrupt/missing images on every redraw.
                self.failed_covers.insert(finished.key);
            }
        }
        if !self.pending_covers.is_empty(){
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }
    /// Return only cached textures. Image decoding and scaling run in the workers.
    fn image_for(&mut self,_ctx:&egui::Context,path:&str,max_side:u32)->Option<CoverTextures> {
        if path.is_empty(){return None;}
        let key=format!("{max_side}:{path}");
        if let Some(texture)=self.textures.get(&key) {
            let texture=texture.clone();
            self.texture_keys.retain(|entry|entry!=&key);
            self.texture_keys.push_back(key);
            return Some(texture);
        }
        if self.failed_covers.contains(&key) || self.pending_covers.contains(&key)
            || self.pending_covers.len()>=12 {
            return None;
        }
        self.pending_covers.insert(key.clone());
        if self.cover_job_tx.send(CoverRequest{
            key:key.clone(),path:PathBuf::from(path),max_side
        }).is_err(){
            self.pending_covers.remove(&key);
            self.failed_covers.insert(key);
        }
        None
    }
    /// Reserve a *real* fixed rectangle. A child UI with a preferred size could
    /// expand a card and cause the next columns to overlap (alpha.4 layout bug).
    fn draw_cover(&mut self, ui: &mut egui::Ui, ctx: &egui::Context,
                  path: &str, width: f32, height: f32, max_side:u32) {
        let texture = self.image_for(ctx, path, max_side);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        ui.painter().rect_filled(rect, 10.0, colors().faint_bg);
        if let Some(tex)=texture {
            if let Some(bg)=&tex.background {
                // egui Painter::image ignores corner radius. Using Image's
                // rounded textured shape keeps the blur INSIDE the entire
                // same rectangle as the foreground (no inset/visible stripes).
                egui::Image::new((bg.id(),bg.size_vec2()))
                    .corner_radius(10)
                    .tint(Color32::from_rgb(210,210,210))
                    .paint_at(ui,rect);
                ui.painter().rect_filled(rect,10.0,
                    Color32::from_black_alpha(45));
            }
            let bounds=fitted_cover_rect(rect,tex.foreground.size_vec2());
            // A small rounded edge on the sharp image also prevents square
            // corners when a wide cover touches the container's outer edge.
            egui::Image::new((tex.foreground.id(),tex.foreground.size_vec2()))
                .corner_radius(4)
                .paint_at(ui,bounds);
        } else {
            ui.painter().text(
                rect.center(), egui::Align2::CENTER_CENTER, tr("Sin portada"),
                egui::FontId::proportional(13.0), colors().muted,
            );
        }
    }

    /// Shared edit/new cover picker. The preview and action share one bounded
    /// surface, so the button stays anchored to the picture when columns resize.
    fn cover_editor(&mut self,ui:&mut egui::Ui,ctx:&egui::Context,
        image_path:&str,max_width:f32,button_text:&str)->bool {
        let width=max_width.min(ui.available_width()).max(1.0);
        let inner_width=(width-20.0).max(1.0);
        let mut clicked=false;
        egui::Frame::new()
            .fill(colors().panel)
            .stroke(egui::Stroke::new(1.0,colors().stroke))
            .corner_radius(10)
            .inner_margin(10)
            .show(ui,|ui|{
                ui.set_width(inner_width);
                self.draw_cover(ui,ctx,image_path,inner_width,250.0,768);
                ui.add_space(8.0);
                // The action belongs to the cover surface and tracks its width.
                // Do not place it next to a long format label that can push it
                // outside a narrow product-editor column.
                clicked=ui.add_sized([inner_width,36.0],
                    egui::Button::new(tr(button_text))
                        .corner_radius(8.0)
                        .fill(colors().button_bg)
                        .stroke(egui::Stroke::new(1.0,colors().stroke)))
                    .clicked();
            });
        clicked
    }

    fn header(&mut self,ctx:&egui::Context) {
        let busy=self.receiver.is_some() || self.update_install_rx.is_some();
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::new().fill(colors().panel).inner_margin(egui::Margin::symmetric(22,11)))
            .show(ctx,|ui|{
                ui.horizontal(|ui|{
                    icon(ui,Icon::Grid,colors().accent,23.0);
                    ui.add_space(5.0);
                    ui.label(RichText::new(tr("SZKura")).strong().size(23.0).color(colors().text));
                    ui.add_space(15.0);
                    ui.label(RichText::new(tr("Biblioteca local de assets")).size(13.0).color(colors().muted));
                    ui.label(RichText::new(updater::installed_version()).size(11.5).color(colors().accent));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                        if busy{ui.spinner();}
                        if !self.show_settings {
                            if action(ui,Icon::Folder,"Configuración",false){
                                self.show_settings=true;
                            }
                            let create=ui.add_enabled_ui(!busy,|ui|{
                                action(ui,Icon::Plus,"Nuevo producto",true)
                            }).inner;
                            if create{self.start_new();}
                            if self.configured && self.sharing.enabled
                                && action(ui,Icon::Link,"Importar enlace SZKura",false){
                                self.sharing.open_import();
                            }
                        }
                    });
                });
                self.sharing.active_header(ui);
                if !self.show_settings && self.selected.is_none() && !self.new_dialog {
                    ui.add_space(10.0);
                    ui.horizontal(|ui|{
                        icon(ui,Icon::Search,colors().muted,19.0);
                        ui.add_sized(
                            [ui.available_width().min(610.0).max(180.0),36.0],
                            egui::TextEdit::singleline(&mut self.query)
                                .vertical_align(egui::Align::Center)
                                .hint_text(tr("Buscar productos, tiendas y etiquetas...")));
                        if !self.query.is_empty()&&ui.button(tr("Limpiar")).clicked(){self.query.clear();}
                    });
                }
                if !self.cjk_loaded{
                    ui.add_space(3.0);
                    ui.label(RichText::new(tr("Fuente japonesa del sistema no encontrada: instala fuentes japonesas para mostrar títulos BOOTH."))
                        .size(12.0).color(Color32::from_rgb(242,188,140)));
                }
            });
    }
    fn sidebar(&mut self,ctx:&egui::Context) {
        egui::SidePanel::left("sidebar").resizable(false).exact_width(218.0)
            .frame(egui::Frame::new().fill(colors().panel).inner_margin(egui::Margin::symmetric(14,18)))
            .show(ctx,|ui|{
                egui::ScrollArea::vertical().id_salt("nav-scroll")
                    .auto_shrink([false,false]).show(ui,|ui|{
                        ui.spacing_mut().item_spacing.y=5.0;
                        ui.label(RichText::new(tr("BIBLIOTECA")).color(colors().accent).strong().size(11.0));
                        ui.add_space(9.0);
                        if self.category_button(ui,"Todos los productos",""){
                            self.category.clear();self.selected=None;self.new_dialog=false;
                        }
                        if nav_choice(ui,"Solo favoritos",self.favorites_only){
                            self.favorites_only=!self.favorites_only;
                            self.selected=None;self.new_dialog=false;
                        }
                        ui.add_space(9.0);
                        ui.label(RichText::new(tr("CONTENIDO")).color(colors().accent).strong().size(11.0));
                        if nav_sensitive_choice(ui,"Mostrar NSFW",self.show_nsfw) {
                            self.show_nsfw=!self.show_nsfw;
                            self.selected=None;
                            self.new_dialog=false;
                        }
                        ui.add_space(5.0);
                        for &cat in CATEGORIES{
                            if self.category_button(ui,cat,cat){
                                self.category=cat.into();self.selected=None;self.new_dialog=false;
                            }
                        }
                        ui.add_space(16.0);ui.separator();ui.add_space(10.0);
                        ui.horizontal(|ui|{
                            icon(ui,Icon::Filter,colors().accent,15.0);
                            ui.label(RichText::new(tr("AVATAR COMPATIBLE")).color(colors().accent)
                                .strong().size(11.0));
                        });
                        ui.add_space(6.0);
                        let selected=if self.avatar_filter.is_empty(){
                            "Todos los avatares"
                        }else{self.avatar_filter.as_str()};
                        let previous=self.avatar_filter.clone();
                        egui::ComboBox::from_id_salt("filter-avatar")
                            .selected_text(tr(selected)).width(184.0)
                            .show_ui(ui,|ui|{
                                ui.selectable_value(&mut self.avatar_filter,String::new(),
                                    tr("Todos los avatares"));
                                for name in &self.avatar_options{
                                    ui.selectable_value(&mut self.avatar_filter,name.clone(),name);
                                }
                            });
                        if previous!=self.avatar_filter {
                            self.selected=None;self.new_dialog=false;
                        }
                        if self.avatar_options.is_empty(){
                            ui.label(RichText::new(tr("Añade avatares en la ficha de un producto."))
                                .color(colors().muted).size(11.0));
                        }
                        ui.add_space(14.0);ui.separator();ui.add_space(9.0);
                        ui.label(RichText::new(tr("ARCHIVOS")).color(colors().accent).strong().size(11.0));
                        ui.add_space(5.0);
                        if nav_choice(ui,"Todos",self.files_filter==FileFilter::All){
                            self.files_filter=FileFilter::All;
                        }
                        if nav_choice(ui,"Con archivos",self.files_filter==FileFilter::WithFiles){
                            self.files_filter=FileFilter::WithFiles;
                        }
                        if nav_choice(ui,"Sin archivos",self.files_filter==FileFilter::WithoutFiles){
                            self.files_filter=FileFilter::WithoutFiles;
                        }
                        ui.add_space(13.0);
                        if nav_choice(ui,"Limpiar filtros",false){
                            self.category.clear();self.avatar_filter.clear();
                            self.files_filter=FileFilter::All;self.query.clear();self.favorites_only=false;
                            self.show_nsfw=false;
                            self.selected=None;self.new_dialog=false;
                        }
                        ui.add_space(14.0);
                        ui.separator();ui.add_space(8.0);
                        ui.label(RichText::new(tr("IMPORTACIÓN")).color(colors().accent).strong().size(11.0));
                        ui.label(RichText::new(tr("Arrastra archivos ZIP, 7z, RAR, UnityPackage o carpetas para vincularlos."))
                            .color(colors().muted).size(12.0));
                    });
            });
    }
    fn category_button(&self,ui:&mut egui::Ui,label:&str,category:&str)->bool {
        nav_choice(ui,label,self.category==category)
    }

    /// Full-width product workspace; no floating window or overlapping modal.
    /// The detail/new editors share a responsive, bounded cover-and-fields
    /// layout. Re-use these sections in both the split and stacked variants,
    /// so the two breakpoints cannot drift into different UI implementations.
    fn detail_cover_section(&mut self,ui:&mut egui::Ui,ctx:&egui::Context,
        asset:&Asset,workupload:&str,attachment_count:usize)->bool {
        let cover_width=ui.available_width().min(480.0);
        ui.label(RichText::new(tr("PORTADA")).color(colors().accent).strong().size(12.0));
        ui.add_space(8.0);
        let choose=self.cover_editor(ui,ctx,&asset.image_path,cover_width,"Cambiar portada local");
        ui.add_space(8.0);
        if !asset.booth_url.is_empty(){
            ui.hyperlink_to(tr("Abrir producto en BOOTH"),&asset.booth_url);
        }
        if !workupload.is_empty(){
            ui.add_space(6.0);
            ui.hyperlink_to(tr("Abrir descarga en Workupload"),workupload);
        }
        ui.add_space(15.0);
        product_summary_card(ui,cover_width,attachment_count,&asset.avatars);
        choose
    }

    fn detail_fields_section(&mut self,ui:&mut egui::Ui) {
        ui.label(RichText::new(tr("INFORMACIÓN")).color(colors().accent).strong().size(12.0));
        ui.add_space(6.0);
        let scan=product_fields(ui,
            &mut self.edit_booth,&mut self.edit_title,&mut self.edit_creator,
            &mut self.edit_category,&mut self.edit_tags,&mut self.edit_avatars,
            &mut self.edit_nsfw,&mut self.edit_description,
            &mut self.edit_description_expanded,"product-edit-category",
            self.avatar_scan.is_some());
        if scan {
            if self.avatar_scan.is_some() {
                self.cancel_avatar_scan();
                self.status=tr("Análisis cancelado.").into();
            } else if let Some(id)=self.selected {
                self.start_avatar_scan(AvatarScanTarget::Edit(id));
            }
        }
    }

    fn new_cover_section(&mut self,ui:&mut egui::Ui,ctx:&egui::Context)->bool{
        let cover_width=ui.available_width().min(480.0);
        ui.label(RichText::new(tr("PORTADA")).color(colors().accent).strong().size(12.0));
        ui.add_space(8.0);
        let cover=self.new_cover.as_ref()
            .map(|p|p.to_string_lossy().into_owned()).unwrap_or_default();
        let choose=self.cover_editor(ui,ctx,&cover,cover_width,"Elegir portada local");
        ui.add_space(7.0);
        ui.add(egui::Label::new(RichText::new(tr("Si BOOTH no proporciona una imagen, puedes elegir una portada aquí."))
            .color(colors().muted).size(12.0)).wrap());
        choose
    }

    fn new_fields_section(&mut self,ui:&mut egui::Ui){
        ui.label(RichText::new(tr("INFORMACIÓN")).color(colors().accent).strong().size(12.0));
        ui.add_space(6.0);
        let scan=product_fields(ui,
            &mut self.new_url,&mut self.new_title,&mut self.new_creator,
            &mut self.new_category,&mut self.new_tags,&mut self.new_avatars,
            &mut self.new_nsfw,&mut self.new_description,
            &mut self.new_description_expanded,"product-new-category",
            self.avatar_scan.is_some());
        if scan {
            if self.avatar_scan.is_some() {
                self.cancel_avatar_scan();
                self.status=tr("Análisis cancelado.").into();
            } else {
                self.start_avatar_scan(AvatarScanTarget::New);
            }
        }
    }

    fn detail(&mut self,ctx:&egui::Context) {
        let Some(id)=self.selected else{return;};
        let asset=match self.db.get(id) {
            Ok(Some(a))=>a,
            _=>{self.selected=None;return;}
        };
        let attachments=self.db.attachments_for(id).unwrap_or_default();
        let source_workupload=self.db.workupload_url_for(id).unwrap_or_default();
        let busy=self.receiver.is_some();
        let(mut back,mut save,mut refresh,mut add_files,mut add_folder,mut choose_cover,mut detach,mut remove)
            =(false,false,false,false,false,false,None,false);
        let(mut show_path,mut folder_path):(Option<String>,Option<String>)=(None,None);
        let(mut inspect_zip,mut force_reinspect,mut open_direct):
            (Option<String>,Option<String>,Option<PathBuf>)=(None,None,None);
        let mut open_nested:Option<(PathBuf,UnityPackage)>=None;
        let mut inspect_unity:Option<(PathBuf,Option<UnityPackage>)>=None;
        let mut export_labeled:Option<(PathBuf,Option<UnityPackage>)>=None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(colors().bg).inner_margin(egui::Margin::symmetric(24,18)))
            .show(ctx,|ui|{
                let compact_header=ui.available_width()<800.0;
                ui.horizontal(|ui|{
                    back=action(ui,Icon::Back,"Biblioteca",false);
                    ui.add_space(12.0);
                    icon(ui,Icon::File,colors().accent,22.0);
                    ui.label(RichText::new(tr("Ficha del producto")).strong().size(23.0).color(colors().text));
                    if !compact_header {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                            if ui.add_enabled(!busy,egui::Button::new(tr("Guardar cambios")).fill(colors().primary)).clicked(){save=true;}
                            if ui.add_enabled(!busy && !self.edit_booth.is_empty(),
                                egui::Button::new(tr("Actualizar BOOTH"))).clicked(){refresh=true;}
                        });
                    }
                });
                if compact_header{
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui|{
                        if ui.add_enabled(!busy && !self.edit_booth.is_empty(),
                            egui::Button::new(tr("Actualizar BOOTH"))).clicked(){refresh=true;}
                        if ui.add_enabled(!busy,
                            egui::Button::new(tr("Guardar cambios")).fill(colors().primary)).clicked(){save=true;}
                    });
                }
                ui.add_space(3.0);
                self.sharing.detail_actions(ui,id,&asset.title,!attachments.is_empty(),busy);
                 ui.label(RichText::new(format!("{}  /  {}",tr("Producto en tu biblioteca"),asset.title))
                    .color(colors().muted).size(13.0));
                ui.add_space(14.0);ui.separator();ui.add_space(11.0);
                egui::ScrollArea::vertical().id_salt("edit-inline-page")
                    .auto_shrink([false,false]).show(ui,|ui|{
                        if editor_two_columns(ui.available_width()) {
                            let left_width=editor_cover_column_width(ui.available_width());
                            ui.horizontal_top(|ui|{
                                ui.allocate_ui_with_layout(egui::vec2(left_width,0.0),
                                    egui::Layout::top_down(egui::Align::Min),|ui|{
                                        choose_cover=self.detail_cover_section(ui,ctx,
                                            &asset,&source_workupload,attachments.len());
                                    });
                                ui.add_space(20.0);
                                let remaining=ui.available_width().max(1.0);
                                ui.allocate_ui_with_layout(egui::vec2(remaining,0.0),
                                    egui::Layout::top_down(egui::Align::Min),|ui|{
                                        self.detail_fields_section(ui);
                                    });
                            });
                        }else{
                            // Keep one full-width form below the compact cover/summary.
                            // A 700px content area cannot fit two 350px edit columns.
                            choose_cover=self.detail_cover_section(ui,ctx,
                                &asset,&source_workupload,attachments.len());
                            ui.add_space(20.0);
                            ui.separator();
                            ui.add_space(12.0);
                            self.detail_fields_section(ui);
                        }
                        ui.add_space(21.0);ui.separator();ui.add_space(12.0);
                        ui.horizontal(|ui|{
                            icon(ui,Icon::Archive,colors().accent,19.0);
                            ui.label(RichText::new(format!("{} ({})",tr("Archivos vinculados"),attachments.len()))
                                .strong().color(colors().text).size(18.0));
                        });
                        ui.label(RichText::new(tr("Los archivos nuevos se copian y organizan en el almacén configurado. Los archivos antiguos conservan sus referencias hasta reorganizarlos.")
                        ).color(colors().muted).size(13.0));
                        ui.add_space(7.0);
                        ui.horizontal_wrapped(|ui|{
                            if ui.add_enabled(!busy,egui::Button::new(tr("Añadir archivos"))).clicked(){add_files=true;}
                            if ui.add_enabled(!busy,egui::Button::new(tr("Importar carpeta"))).clicked(){add_folder=true;}

                        });
                        let previous=self.delete_sources;
                        if ui.checkbox(&mut self.delete_sources,tr("Enviar los originales a la papelera tras copiar y verificar")).changed(){
                            self.persist_delete_source_choice(previous);
                        }
                        ui.label(RichText::new(tr("Esta preferencia se recuerda entre sesiones. Siempre se pedirá confirmación antes de mover los originales.")
                        ).size(12.0).color(colors().muted));
                        if attachments.is_empty(){
                            ui.add_space(10.0);
                            ui.label(RichText::new(tr("No hay archivos asociados. También puedes arrastrarlos a esta página."))
                                .color(colors().muted));
                        }
                        ui.add_space(7.0);
                        for file in &attachments{
                            let source=PathBuf::from(&file.stored_path);
                            let available=source.is_file();
                            let is_archive=source.extension().and_then(|x|x.to_str())
                                .is_some_and(|x|x.eq_ignore_ascii_case("zip")
                                    ||x.eq_ignore_ascii_case("7z")
                                    ||x.eq_ignore_ascii_case("rar"));
                            let direct_package=source.extension().and_then(|x|x.to_str())
                                .is_some_and(|x|x.eq_ignore_ascii_case("unitypackage"));
                            let expanded=self.inspected_zip.as_deref()==Some(&file.stored_path);
                            let unity_direct_open=self.unity_paths_key.as_deref()
                                ==Some(unity_paths_id(&source,None).as_str());
                            egui::Frame::new().fill(colors().panel)
                                .stroke(egui::Stroke::new(1.0,colors().stroke))
                                .corner_radius(10).inner_margin(14).show(ui,|ui|{
                                    ui.horizontal(|ui|{
                                        icon(ui,Icon::Archive,colors().accent,19.0);
                                        ui.add_space(4.0);
                                        ui.add(egui::Label::new(RichText::new(&file.original_name)
                                            .strong().size(14.0).color(colors().text)).truncate())
                                            .on_hover_text(&file.stored_path);
                                    });
                                    let availability=tr(if available{"Disponible"}else{"Ruta no encontrada"});
                                    ui.label(RichText::new(format!(
                                        "{} | {} | {availability}",
                                        if file.mode==ArchiveMode::Copy{
                                            tr("Copia gestionada")
                                        }else{tr("Ruta vinculada")},
                                        human_size(file.bytes)
                                    )).size(12.0).color(if available{colors().muted}else{
                                        Color32::from_rgb(244,151,139)
                                    }));
                                    ui.add_space(7.0);
                                    ui.horizontal_wrapped(|ui|{
                                        if ui.add_enabled(available,egui::Button::new(tr("Mostrar archivo"))
                                            .min_size(egui::vec2(0.0,34.0))).clicked(){
                                            show_path=Some(file.stored_path.clone());
                                        }
                                        if ui.add_enabled(available,egui::Button::new(tr("Abrir carpeta"))
                                            .min_size(egui::vec2(0.0,34.0))).clicked(){
                                            folder_path=Some(file.stored_path.clone());
                                        }
                                        if is_archive && ui.add_enabled(available,
                                            egui::Button::new(tr(if expanded{"Ocultar contenido"}else{"Examinar archivo"}))
                                                .min_size(egui::vec2(0.0,34.0))).clicked(){
                                            inspect_zip=Some(file.stored_path.clone());
                                        }
                                        if direct_package && ui.add_enabled(available && !busy,
                                            egui::Button::new(tr("Abrir en Unity"))
                                                .min_size(egui::vec2(0.0,34.0)))
                                            .on_hover_text(tr("Al abrir, se añade automáticamente la etiqueta SZKura en una copia de caché."))
                                            .clicked(){
                                            open_direct=Some(source.clone());
                                        }
                                        if direct_package && ui.add_enabled(available &&
                                            (self.unity_paths_scan.is_none() || unity_direct_open),
                                            egui::Button::new(tr(unity_folders_action_label(unity_direct_open)))
                                                .min_size(egui::vec2(0.0,34.0))).clicked(){
                                            inspect_unity=Some((source.clone(),None));
                                        }
                                        if ui.button(tr("Desvincular")).clicked(){detach=Some(file.id);}
                                    });
                                    if direct_package && unity_direct_open {
                                        if show_unity_paths_result(ui,&self.unity_paths_label,
                                            self.unity_paths_scan.is_some(),
                                            self.unity_paths_preview.as_ref(),
                                            self.unity_paths_error.as_deref(),
                                            &asset.category,busy) {
                                            export_labeled=Some((source.clone(),None));
                                        }
                                    }
                                    if expanded && is_archive{
                                        ui.add_space(10.0);ui.separator();ui.add_space(7.0);
                                        if self.archive_scan.is_some(){
                                            ui.horizontal(|ui|{
                                                ui.spinner();
                                                ui.label(tr("Examinando ZIP, 7z, RAR y ZIP internos..."));
                                            });
                                        }else if let Some(preview)=self.archive_preview.as_ref(){
                                            ui.label(RichText::new(if i18n::is_english(){format!("{} outer entries | {} nested entries | {} Unity packages",
    preview.total_entries,preview.nested_entries,preview.total_packages)}
 else{format!("{} entradas exteriores | {} interiores | {} paquetes Unity",
    preview.total_entries,preview.nested_entries,preview.total_packages)}).strong().color(colors().accent).size(13.0));
                                            if preview.nested_archives>0 {
                                                ui.label(RichText::new(if i18n::is_english(){format!("Nested ZIPs: {} scanned out of {}",
    preview.nested_scanned,preview.nested_archives)}
 else{format!("ZIP internos: {} examinados de {}",
    preview.nested_scanned,preview.nested_archives)}).color(colors().muted).size(12.0));
                                            }
                                            ui.add_space(5.0);
                                            ui.horizontal_wrapped(|ui|{
                                                ui.label(RichText::new(tr("La vista se guarda en SQLite y se reutiliza si el archivo no cambia.")
                                                ).size(12.0).color(colors().muted));
                                                if ui.add_enabled(!busy,
                                                    egui::Button::new(tr("Reexaminar archivo"))
                                                        .min_size(egui::vec2(0.0,34.0)))
                                                    .on_hover_text(tr("Recalcula BLAKE3 y vuelve a inspeccionar el archivo y sus ZIP internos."))
                                                    .clicked(){
                                                    force_reinspect=Some(file.stored_path.clone());
                                                }
                                            });
                                            if preview.nested_skipped>0 {
                                                ui.label(RichText::new(if i18n::is_english(){format!("Partial inspection: {} nested ZIPs skipped for size, nesting, encryption, or corruption.",preview.nested_skipped)}
 else{format!("Inspección parcial: {} ZIP internos omitidos por tamaño, profundidad, protección o archivo dañado.",preview.nested_skipped)}).color(Color32::from_rgb(239,194,133)).size(12.5)).on_hover_text(tr("Los paquetes Unity de los ZIP omitidos no se han contado. Puedes examinarlos por separado."));
                                            }
                                            if preview.total_packages==0{
                                                ui.label(RichText::new(tr(if preview.nested_skipped>0 {
                                                        "No se encontraron paquetes en los ZIP que pudieron examinarse."
                                                    }else{
                                                        "No se ha encontrado ningún .unitypackage, incluidos los ZIP internos examinados."} )).color(colors().muted).size(12.5));
                                            }else{
                                                ui.label(RichText::new(tr("Extrae solamente el paquete elegido a la caché y lo abre con la aplicación asociada.")
                                                ).color(colors().muted).size(12.0));
                                            }
                                            ui.add_space(6.0);
                                            ui.label(RichText::new(tr("Contenido por ZIP: pulsa la flecha a la izquierda de cada ZIP interno para desplegar sus archivos.")
                                            ).size(12.5).color(colors().muted));
                                            ui.add_space(4.0);
                                            egui::Frame::new().fill(colors().card).corner_radius(8)
                                                .inner_margin(egui::Margin::symmetric(10,9))
                                                .show(ui,|ui|{
                                                    let rows=preview.root_files.len()
                                                        .saturating_add(preview.inner_archives.iter()
                                                            .filter(|item|item.chain.len()==1).count())
                                                        .saturating_add(usize::from(preview.root_hidden_files>0));
                                                    // Package rows have a separate action toolbar.
                                                    // Budget two lines for each visible package.
                                                    let row_units=rows.saturating_add(preview.root_files.iter()
                                                        .filter(|item|item.package.is_some()).count());
                                                    let tree_height=archive_tree_height(
                                                        row_units,ui.ctx().content_rect().height());
                                                    if rows<=2 {
                                                        // A short list needs no inner scroll area:
                                                        // avoid the large empty bottom under one file.
                                                        show_zip_tree(ui,preview,&[],&source,busy,
                                                            self.unity_paths_key.as_deref(),self.unity_paths_scan.is_some(),
                                                            &mut open_nested,&mut inspect_unity);
                                                    }else{
                                                        egui::ScrollArea::vertical()
                                                            .id_salt(("zip-insides-tree",file.id))
                                                            .min_scrolled_height(tree_height)
                                                            .max_height(tree_height)
                                                            .auto_shrink([false,false])
                                                            .show(ui,|ui|{
                                                                show_zip_tree(ui,preview,&[],
                                                                    &source,busy,
                                                                    self.unity_paths_key.as_deref(),
                                                                    self.unity_paths_scan.is_some(),
                                                                    &mut open_nested,&mut inspect_unity);
                                                            });
                                                    }
                                                });
                                            if self.unity_paths_key.as_ref().is_some_and(|key|
                                                key.starts_with(&format!("{}|nested|",source.display()))) {
                                                let selected_package=preview.packages.iter()
                                                    .find(|package|self.unity_paths_key.as_deref()
                                                        ==Some(unity_paths_id(&source,Some(package)).as_str()))
                                                    .cloned();
                                                if show_unity_paths_result(ui,&self.unity_paths_label,
                                                    self.unity_paths_scan.is_some(),
                                                    self.unity_paths_preview.as_ref(),
                                                    self.unity_paths_error.as_deref(),
                                                    &asset.category,busy && selected_package.is_some()) {
                                                    if let Some(package)=selected_package {
                                                        export_labeled=Some((source.clone(),Some(package)));
                                                    }
                                                }
                                            }
                                        }else{
                                            ui.label(RichText::new(tr("No hay vista previa disponible."))
                                                .color(colors().muted));
                                        }
                                    }
                                });
                            ui.add_space(9.0);
                        }
                        if self.paths.unity_cache.exists(){
                            ui.add_space(8.0);
                            ui.label(RichText::new(tr("Los paquetes extraídos para Unity se guardan en una caché separada. Los archivos vinculados no cambian.")
                            ).size(12.0).color(colors().muted));
                            if ui.add_enabled(!busy,egui::Button::new(tr("Vaciar caché Unity")))
                                .clicked(){self.confirm_clear_cache=true;}
                        }
                        ui.add_space(19.0);ui.separator();ui.add_space(9.0);
                        ui.label(RichText::new(tr("Zona de mantenimiento")).strong().size(15.0).color(colors().text));
                        ui.label(RichText::new(tr("Eliminar una ficha no borra los archivos de tu ordenador."))
                            .color(colors().muted).size(12.0));
                        if ui.add_enabled(!busy,
                            egui::Button::new(tr("Eliminar ficha del catálogo"))).clicked(){
                            remove=true;
                        }
                    });
            });
        if back{self.cancel_avatar_scan();self.selected=None;return;}
        if let Some(path)=inspect_zip{self.inspect_archive(path,false);}
        if let Some(path)=force_reinspect{self.inspect_archive(path,true);}
        if let Some(path)=open_direct{
            self.launch_unity_package(path,None,id,asset.category.clone());
        }
        if let Some((archive,entry))=open_nested{
            self.launch_unity_package(archive,Some(entry),id,asset.category.clone());
        }
        if let Some((source,package))=inspect_unity{
            self.toggle_unity_paths(source,package);
        }
        if let Some((source,package))=export_labeled {
            let category=asset.category.clone();
            match unitypackage_labels::label_for_category(&category) {
                Ok(unity_label)=>{
                    let package_name=package.as_ref().map(|entry|entry.name.as_str())
                        .or_else(||source.file_name().and_then(|name|name.to_str()))
                        .unwrap_or("SZKura");
                    let stem=Path::new(package_name).file_stem()
                        .and_then(|part|part.to_str()).unwrap_or("SZKura");
                    let suggestion=format!("{stem}_{unity_label}.unitypackage");
                    if let Some(destination)=rfd::FileDialog::new()
                        .add_filter(tr("UnityPackage"),&["unitypackage"])
                        .set_file_name(&suggestion).save_file() {
                        self.launch(move|paths|{
                            let input=if let Some(entry)=package {
                                package_inspect::extract_unitypackage_nested(
                                    &source,&entry,&paths.unity_cache)?
                            } else {source};
                            let report=unitypackage_labels::export_labeled_unitypackage(
                                &input,&destination,&category)?;
                            Ok(JobResult {
                                message:format!("{}: {} | {}: {}",
                                    tr("Copia etiquetada creada"),destination.display(),
                                    report.label,report.labeled_assets+report.already_labeled),
                                select_id:Some(id),
                            })
                        });
                    }
                },
                Err(error)=>self.status=format!("{}: {error}",
                    tr("Categoría no válida para etiquetar")),
            }
        }
        if let Some(path)=show_path{
            match reveal_file(&path){Ok(())=>self.status="Ubicación solicitada al explorador".into(),
                Err(e)=>self.status=format!("No se pudo mostrar el archivo: {e}")}
        }
        if let Some(path)=folder_path{
            match open_folder(&path){Ok(())=>self.status="Carpeta solicitada al explorador".into(),
                Err(e)=>self.status=format!("No se pudo abrir la carpeta: {e}")}
        }
        if choose_cover{
            if let Some(path)=rfd::FileDialog::new()
                .add_filter(tr("Imagen"),&["png","jpg","jpeg","webp"]).pick_file(){
                match self.import_cover(id,&path){
                    Ok(())=>self.status="Portada guardada".into(),
                    Err(e)=>self.status=format!("Error guardando portada: {e}"),
                }
            }
        }
        if save {
            self.cancel_avatar_scan();
            let candidate=self.edit_booth.trim().to_owned();
            let validated=if candidate.is_empty(){Ok(String::new())}
                else{booth::canonical_url(&candidate)};
            let result=validated.and_then(|url|{
                self.db.update_details(id,&self.edit_title,&self.edit_category,
                    &self.edit_creator,&self.edit_tags,&self.edit_description)?;
                self.db.set_avatars(id,&self.edit_avatars)?;
                self.db.set_nsfw(id,self.edit_nsfw)?;
                self.db.set_booth_url(id,&url)
            });
            match result {
                Ok(())=>{
                    self.edit_avatars=self.db.avatars_for(id).unwrap_or_default().join(", ");
                    self.status="Producto actualizado".into();
                    let _=self.reload();
                },
                Err(e)=>self.status=format!("No se pudo guardar: {e}"),
            }
        }
        if refresh {self.refresh_booth(id);}
        if add_files {
            if let Some(files)=rfd::FileDialog::new()
                .add_filter(tr("ZIP, 7z, RAR y UnityPackages"),&["zip","7z","rar","unitypackage"]).pick_files(){
                self.link_files(id,files,ArchiveMode::Reference);
            }
        }
        if add_folder {
            if let Some(folder)=rfd::FileDialog::new().pick_folder(){
                self.link_files(id,vec![folder],ArchiveMode::Reference);
            }
        }
        if let Some(file_id)=detach{
            match self.db.detach(file_id,id){
                Ok(())=>{self.status="Referencia eliminada sin tocar el archivo".into();
                    let _=self.reload();},
                Err(e)=>self.status=format!("Error al desvincular: {e}"),
            }
        }
        if remove {
            // La ficha y la cuadrícula comparten la misma confirmación.
            self.pending_quick_delete=Some((id,asset.title.clone()));
        }
    }
    /// Confirmation shared by card, list and product detail deletion.
    fn quick_delete_dialog(&mut self,ctx:&egui::Context) {
        let Some((id,title))=self.pending_quick_delete.clone() else {return};
        let choice=confirmation_dialog(ctx,"quick-delete-confirmation",
            "Eliminar del catálogo","Eliminar ficha",
            ConfirmTone::Destructive,self.receiver.is_none(),|ui|{
                dialog_subject(ui,&title);
                ui.add_space(9.0);
                dialog_text(ui,
                    "Se eliminará la ficha del catálogo. Los archivos ZIP, 7z, RAR y UnityPackage originales permanecerán en el disco.");
            });
        match choice {
            ConfirmChoice::Cancel=>self.pending_quick_delete=None,
            ConfirmChoice::Accept=>{
                self.pending_quick_delete=None;
                match self.db.delete(id) {
                    Ok(())=>{
                        if self.selected==Some(id){self.selected=None;}
                        self.status=format!("Ficha eliminada: {title}. Los archivos originales se conservan.");
                        if let Err(error)=self.reload(){
                            self.status=format!("Ficha eliminada, pero falló la actualización del catálogo: {error}");
                        }
                    },
                    Err(error)=>self.status=format!("No se pudo eliminar la ficha: {error}"),
                }
            },
            ConfirmChoice::Pending=>{},
        }
    }

    /// Cache is disposable, but confirmation uses the same modal style.
    fn confirm_cache_dialog(&mut self,ctx:&egui::Context){
        if !self.confirm_clear_cache{return;}
        let choice=confirmation_dialog(ctx,"confirm-clear-unity-cache",
            "Vaciar caché Unity","Vaciar caché",
            ConfirmTone::Primary,self.receiver.is_none(),|ui|{
                dialog_subject(ui,tr("Paquetes Unity extraídos"));
                ui.add_space(9.0);
                dialog_text(ui,
                    "Se eliminarán los archivos temporales de Unity. No se borran los ZIP, 7z, RAR ni UnityPackages originales y se podrán extraer de nuevo.");
            });
        match choice {
            ConfirmChoice::Cancel=>self.confirm_clear_cache=false,
            ConfirmChoice::Accept=>{
                self.confirm_clear_cache=false;
                match package_inspect::clear_cache(&self.paths.unity_cache){
                    Ok(num)=>self.status=format!(
                        "Caché Unity vaciada: {num} archivos. Los originales no se han tocado"),
                    Err(err)=>self.status=format!("No se pudo vaciar caché Unity: {err}"),
                }
            },
            ConfirmChoice::Pending=>{},
        }
    }

    fn toggle_favorite(&mut self,asset:&Asset) {
        match self.db.set_favorite(asset.id,!asset.favorite) {
            Ok(())=>{
                self.status=if asset.favorite{"Quitado de favoritos".into()}
                    else{"Añadido a favoritos".into()};
                // Never replace self.assets during the virtualized list's paint loop.
                // It would invalidate row offsets while show_rows is iterating.
                self.favorites_dirty=true;
            },
            Err(e)=>self.status=format!("No se pudo actualizar favoritos: {e}"),
        }
    }

    fn asset_row(&mut self,ui:&mut egui::Ui,ctx:&egui::Context,
        asset:&Asset,compact:bool) {
        let mut favorite=false;let mut open=false;let mut delete=false;
        let available=ui.available_width();
        let text_width=if compact{
            (available-130.0).max(120.0)
        }else{
            (available-480.0).max(150.0)
        };
        egui::Frame::new().fill(colors().card).corner_radius(8)
            .stroke(egui::Stroke::new(1.0,colors().stroke))
            .inner_margin(egui::Margin::symmetric(12,8)).show(ui,|ui|{
                ui.horizontal(|ui|{
                    self.draw_cover(ui,ctx,&asset.image_path,76.0,64.0,160);
                    ui.add_space(10.0);
                    ui.vertical(|ui|{
                        ui.set_width(text_width);
                        ui.add(egui::Label::new(
                            RichText::new(&asset.title).strong().size(15.0)).truncate())
                            .on_hover_text(&asset.title);
                        let creator=if asset.creator.is_empty(){
                            tr(&asset.category)
                        }else{&asset.creator};
                        let meta=format!("{creator} · {}{}",tr(&asset.category),
                            if asset.nsfw{" · NSFW"}else{""});
                        ui.add(egui::Label::new(RichText::new(meta)
                            .size(11.5).color(colors().muted)).truncate());
                        if let Some(summary)=avatar_summary(&asset.avatars) {
                            ui.add(egui::Label::new(RichText::new(summary)
                                .size(11.5).color(colors().accent)).truncate())
                                .on_hover_text(&asset.avatars);
                        }
                    });
                    if !compact {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                            open=action(ui,Icon::File,"Abrir",false);
                            favorite=favorite_action(ui,asset.favorite);
                            delete=ui.add_enabled_ui(self.receiver.is_none(),|ui|{
                                icon_action(ui,Icon::Trash,"Eliminar producto del catálogo",true)
                            }).inner;
                            ui.label(RichText::new(format!("{} · {} {}",
                                human_size(asset.total_bytes),asset.attachments,tr("archivos")))
                                .size(12.0).color(colors().muted));
                        });
                    }
                });
                if compact {
                    ui.add_space(5.0);
                    ui.horizontal(|ui|{
                        ui.label(RichText::new(format!("{} · {} {}",
                            human_size(asset.total_bytes),asset.attachments,
                            tr(if asset.attachments==1{"archivo"}else{"archivos"})))
                            .size(12.0).color(colors().muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                            open=action(ui,Icon::File,"Abrir",false);
                            favorite=favorite_action(ui,asset.favorite);
                            delete=ui.add_enabled_ui(self.receiver.is_none(),|ui|{
                                icon_action(ui,Icon::Trash,"Eliminar producto del catálogo",true)
                            }).inner;
                        });
                    });
                }
            });
        if favorite{self.toggle_favorite(asset);}
        if delete{
            self.pending_quick_delete=Some((asset.id,asset.title.clone()));
        }
        if open{self.select_id(asset.id);}
    }

    fn asset_card(&mut self,ui:&mut egui::Ui,ctx:&egui::Context,asset:&Asset,width:f32) {
        let can_delete=self.receiver.is_none();
        ui.allocate_ui_with_layout(egui::vec2(width,330.0),
            egui::Layout::top_down(egui::Align::Min),|ui|{
                ui.set_width(width);
                egui::Frame::new().fill(colors().card)
                    .stroke(egui::Stroke::new(1.0,colors().stroke))
                    .corner_radius(12).inner_margin(13).show(ui,|ui|{
                        let inner=width-26.0;
                        ui.set_width(inner);
                        ui.set_min_height(302.0);
                        self.draw_cover(ui,ctx,&asset.image_path,inner,170.0,320);
                        ui.add_space(9.0);
                        ui.add(egui::Label::new(
                            RichText::new(&asset.title).strong().color(colors().text).size(16.0)
                        ).truncate()).on_hover_text(&asset.title);
                        ui.add_space(1.0);
                        ui.add(egui::Label::new(RichText::new(format!("{}{}",
                            if asset.creator.is_empty(){tr(&asset.category)}else{&asset.creator},
                            if asset.nsfw{" · NSFW"}else{""}
                        )).color(colors().muted).size(12.5)).truncate());
                        ui.add_space(4.0);
                        if let Some(summary)=avatar_summary(&asset.avatars){
                            ui.add(egui::Label::new(RichText::new(summary)
                                .size(12.0).color(colors().accent)).truncate())
                                .on_hover_text(&asset.avatars);
                        }else{
                            ui.add_space(18.0);
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui|{
                            ui.label(RichText::new(format!("{} · {}",asset.attachments,
                                human_size(asset.total_bytes))).size(12.0).color(colors().muted));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                                if ui.add_enabled_ui(can_delete,|ui|{
                                    icon_action(ui,Icon::Trash,"Eliminar producto del catálogo",true)
                                }).inner {
                                    self.pending_quick_delete=Some((asset.id,asset.title.clone()));
                                }
                                if action(ui,Icon::File,"Abrir ficha",false){
                                    self.select_id(asset.id);
                                }
                                if favorite_action(ui,asset.favorite) {
                                    self.toggle_favorite(asset);
                                }
                            });
                        });
                    });
            });
    }

    fn catalog(&mut self,ctx:&egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(colors().bg).inner_margin(egui::Margin::symmetric(19,17)))
            .show(ctx,|ui|{
                ui.horizontal(|ui|{
                    ui.label(RichText::new(tr(if self.category.is_empty(){"Tu biblioteca"}else{&self.category}))
                        .strong().size(23.0).color(colors().text));
                    ui.add_space(6.0);
                    ui.label(RichText::new(format!("{} {} · {}",
                        self.filtered,tr("productos"),human_size(self.filtered_bytes)))
                        .color(colors().muted).size(13.0))
                        .on_hover_text(tr("Tamaño de archivos vinculados a los productos mostrados. Cada ruta se cuenta una sola vez; no incluye portadas ni cachés."));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                        if ui.selectable_label(self.catalog_view==CatalogView::List,tr("Lista")).clicked(){
                            self.catalog_view=CatalogView::List;
                        }
                        if ui.selectable_label(self.catalog_view==CatalogView::Grid,tr("Cuadrícula")).clicked(){
                            self.catalog_view=CatalogView::Grid;
                        }
                        if ui.add_enabled_ui(self.receiver.is_none(),|ui|{
                            action(ui,Icon::Folder,"Vincular carpeta",false)
                        }).inner{self.import_bulk(true);}
                        if ui.add_enabled_ui(self.receiver.is_none(),|ui|{
                            action(ui,Icon::Archive,"Añadir archivos",false)
                        }).inner{self.import_bulk(false);}
                    });
                });
                if !self.avatar_filter.is_empty(){
                    ui.add_space(7.0);
                    ui.horizontal(|ui|{
                        icon(ui,Icon::Filter,colors().accent,15.0);
                        ui.label(RichText::new(format!("Avatar: {}",self.avatar_filter))
                            .size(13.0).color(colors().accent));
                        if ui.button(tr("Quitar filtro")).clicked(){self.avatar_filter.clear();}
                    });
                }
                ui.add_space(14.0);
                if self.assets.is_empty(){
                    ui.add_space(55.0);ui.vertical_centered(|ui|{
                        ui.label(RichText::new(tr(if self.total==0{"Empieza con tu primer producto"}else{"No hay resultados"})).size(21.0).color(colors().text));
                        ui.add_space(8.0);
                        ui.label(RichText::new(tr(if self.total==0{"Pega una URL de BOOTH, recupera su portada y asocia tus ZIP en una sola ficha."}else{"Prueba otra categoría, quita el filtro de ZIP o borra la búsqueda."})).color(colors().muted));
                        ui.add_space(15.0);
                        if self.total==0 && ui.button(tr("Crear primer producto")).clicked(){self.start_new();}
                        if self.total>0 && ui.button(tr("Limpiar filtros")).clicked(){
                            self.query.clear();self.category.clear();self.avatar_filter.clear();
                            self.files_filter=FileFilter::All;
                        }
                    });return;
                }
                if self.catalog_view==CatalogView::List {
                    // Match virtual row heights to the responsive row layout.
                    let compact=ui.available_width()<1020.0;
                    let row_height=if compact{136.0}else{96.0};
                    egui::ScrollArea::vertical().id_salt("products-rows")
                        .auto_shrink([false,false]).show_rows(ui,row_height,self.assets.len(),|ui,range|{
                            for index in range{
                                let asset=self.assets[index].clone();
                                self.asset_row(ui,ctx,&asset,compact);
                            }
                        });
                    if self.filtered>self.displayed_limit
                        && ui.button(tr("Cargar más productos")).clicked(){
                        self.displayed_limit=(self.displayed_limit+96).min(self.filtered);
                        let _=self.reload();
                    }
                    return;
                }
                let space=ui.available_width().max(210.0);
                let gap=ui.spacing().item_spacing.x;
                let columns=grid_column_count(space,gap);
                let width=((space-gap*(columns-1) as f32)/columns as f32).max(200.0);
                let rows=self.assets.len().div_ceil(columns);
                // Virtual rows: only cards within the viewport decode cover textures.
                egui::ScrollArea::vertical().id_salt("products-list")
                    .auto_shrink([false,false]).show_rows(ui,330.0,rows,|ui,range|{
                        for row_index in range{
                            let first=row_index*columns;
                            let last=(first+columns).min(self.assets.len());
                            // Copy only the visible row, not every product every frame.
                            let visible=self.assets[first..last].to_vec();
                            ui.horizontal_top(|ui|{
                                for item in &visible{
                                    self.asset_card(ui,ctx,item,width);
                                }
                            });
                        }
                    });
                if self.filtered>self.displayed_limit{
                    ui.add_space(7.0);
                    if action(ui,Icon::Plus,"Cargar más productos",false){
                        self.displayed_limit=(self.displayed_limit+96).min(self.filtered);
                        if let Err(err)=self.reload(){self.status=format!("Error: {err}");}
                    }
                }
            });
    }
    /// Same inline layout and fields as the editor. Never creates a floating popup.
    fn settings_page(&mut self,ctx:&egui::Context) {
        let mut choose=false;
        let mut save=false;
        let mut back=false;
        let mut organize=false;
        let mut export=false;
        let mut restore=false;
        let mut check_updates=false;
        let mut install_update=false;
        let mut requested_palette=None;
        let mut requested_language=None;
        let busy=self.receiver.is_some() || self.update_install_rx.is_some();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(colors().bg).inner_margin(egui::Margin::symmetric(28,24)))
            .show(ctx,|ui|{
                ui.horizontal(|ui|{
                    if self.configured {
                        back=action(ui,Icon::Back,"Volver a biblioteca",true);
                        ui.add_space(12.0);
                    }
                    ui.label(RichText::new(tr(if self.configured {
                        "Configuración de biblioteca"
                    } else {
                        "Configuración inicial obligatoria"
                    })).strong().size(23.0).color(colors().text));
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(10.0);
                egui::ScrollArea::vertical().id_salt("settings-scroll")
                    .auto_shrink([false,false]).show(ui,|ui|{
                ui.add_space(8.0);
                ui.label(RichText::new(tr("Antes de añadir productos o archivos, elige una carpeta para guardar tus assets organizados. Esta ruta se conserva en SQLite.")
                ).size(14.0).color(colors().text));
                ui.add_space(12.0);
                ui.label(RichText::new(tr("Carpeta principal de assets")).strong().color(colors().accent));
                ui.add_sized([ui.available_width().min(720.0),36.0],
                    egui::TextEdit::singleline(&mut self.setup_path)
                        .vertical_align(egui::Align::Center));
                ui.horizontal_wrapped(|ui|{
                    if ui.button(tr("Elegir carpeta")).clicked(){choose=true;}
                    if ui.add_enabled(!busy && !self.setup_path.trim().is_empty(),
                        egui::Button::new(tr("Guardar configuración")).fill(colors().primary)).clicked(){
                        save=true;
                    }
                    // Volver está fijo en la cabecera, no dentro del scroll.
                });
                ui.add_space(12.0);
                ui.label(RichText::new(tr("Estructura: carpeta elegida / categoría / ID y nombre del producto / archivos ZIP, 7z, RAR y UnityPackage.")
                ).color(colors().muted));
                ui.label(RichText::new(tr("Al cambiar la carpeta principal, los archivos ya gestionados mantienen sus rutas hasta que uses Organizar existentes.")
                ).color(colors().muted).size(12.0));
                ui.add_space(18.0);ui.separator();ui.add_space(8.0);
                ui.label(RichText::new(tr("Paleta de colores")).strong().size(17.0));
                ui.label(RichText::new(tr("Cambia la apariencia de toda la app. Tu elección se guarda para el siguiente inicio.")
                ).color(colors().muted).size(12.5));
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui|{
                    for kind in ThemeKind::ALL {
                        let preview=theme::swatches(kind);
                        let selected=self.ui_palette==kind;
                        let p=colors();
                        // Use one clickable tile per theme with an identical label
                        // baseline; avoid a second, nested selectable button.
                        let response=ui.add(egui::Button::new(
                            RichText::new(tr(kind.label())).size(13.0)
                                .color(Color32::TRANSPARENT))
                            .min_size(egui::vec2(128.0,104.0))
                            .corner_radius(9.0)
                            .fill(if selected{p.card}else{p.panel})
                            .stroke(egui::Stroke::new(
                                if selected{2.0}else{1.0},
                                if selected{p.accent}else{p.stroke}
                            )));
                        let rect=response.rect;
                        let swatch_size=17.0;
                        let swatch_gap=8.0;
                        let row_width=4.0*swatch_size+3.0*swatch_gap;
                        for (index,color) in [preview.bg,preview.card,
                            preview.primary,preview.accent].into_iter().enumerate() {
                            let x=rect.center().x-row_width*0.5
                                +index as f32*(swatch_size+swatch_gap);
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(x,rect.top()+19.0),
                                    egui::vec2(swatch_size,swatch_size)),4.0,color);
                        }
                        let label=ui.painter().layout_no_wrap(tr(kind.label()).to_owned(),
                            egui::FontId::proportional(13.0),p.text);
                        let label_pos=egui::pos2(
                            rect.center().x-label.size().x*0.5,
                            rect.top()+68.0);
                        ui.painter().galley(label_pos,label,p.text);
                        if response.clicked(){requested_palette=Some(kind);}
                    }
                });
                ui.add_space(18.0);ui.separator();ui.add_space(8.0);
                ui.label(RichText::new(tr("Idioma")).strong().size(17.0));
                ui.label(RichText::new(tr("Elige el idioma de la interfaz. Se guarda para futuros inicios.")
                ).color(colors().muted).size(12.5));
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui|{
                    for language in Language::ALL {
                        if ui.selectable_label(self.ui_language==language,language.label()).clicked(){
                            requested_language=Some(language);
                        }
                    }
                });
                ui.add_space(4.0);
                ui.label(RichText::new(tr("¿Quieres otro idioma? Abre una Issue en GitHub y solicita una traducción.")
                ).color(colors().muted).size(12.0));
                ui.hyperlink_to(tr("Solicitar otro idioma"),
                    "https://github.com/ShxwZ/SZKura/issues/new");
                ui.add_space(18.0);ui.separator();ui.add_space(8.0);
                ui.label(RichText::new(tr("Copia de seguridad de la biblioteca"))
                    .strong().size(17.0));
                ui.label(RichText::new(tr("Exporta o importa SQLite y las portadas locales. Los archivos originales no se incluyen: se conservan como archivos externos en sus carpetas.")
                ).color(colors().muted).size(12.5));
                ui.horizontal_wrapped(|ui|{
                    if ui.add_enabled(!busy,
                        egui::Button::new(tr("Exportar biblioteca (.szkura)"))).clicked(){
                        export=true;
                    }
                    if ui.add_enabled(!busy,
                        egui::Button::new(tr("Importar biblioteca"))).clicked(){
                        restore=true;
                    }
                });
                ui.label(RichText::new(tr("Antes de importar, SZKura guarda automáticamente una copia de recuperación de tu SQLite actual.")
                ).color(colors().muted).size(11.5));
                ui.add_space(18.0);ui.separator();ui.add_space(8.0);
                self.sharing.settings_ui(ui,&mut self.db,
                    self.paths.db.parent().unwrap_or(Path::new(".")));
                ui.add_space(18.0);ui.separator();ui.add_space(8.0);
                ui.label(RichText::new(tr("Versión y actualizaciones")).strong().size(17.0));
                ui.horizontal_wrapped(|ui|{
                    ui.label(RichText::new(format!("{} {}",tr("Versión instalada:"),
                        updater::installed_version())).size(13.0).strong().color(colors().accent));
                    ui.hyperlink_to(tr("Ver versiones oficiales"),updater::github_release_page());
                });
                if ui.checkbox(&mut self.check_updates_on_start,tr("Comprobar actualizaciones al iniciar la app")).changed(){
                    if let Err(error)=self.db.update_preference(
                        "check_updates_on_start",self.check_updates_on_start){
                        self.status=format!("No se guardó preferencia: {error}");
                    }
                }
                if ui.checkbox(&mut self.auto_install_updates,tr("Instalar automáticamente las versiones nuevas (Windows)")).changed(){
                    if let Err(error)=self.db.update_preference(
                        "auto_install_updates",self.auto_install_updates){
                        self.status=format!("No se guardó preferencia: {error}");
                    }
                }
                ui.label(RichText::new(tr("La instalación automática está desactivada por defecto. Siempre verifica el SHA-256 publicado por GitHub; no instala archivos sin digest.")
                ).color(colors().muted).size(12.0));
                ui.horizontal_wrapped(|ui|{
                    if ui.add_enabled(self.update_check_rx.is_none()
                        && self.update_install_rx.is_none(),
                        egui::Button::new(tr("Comprobar en GitHub"))).clicked(){
                        check_updates=true;
                    }
                    if let Some(update)=&self.available_update {
                        ui.label(RichText::new(format!(
                            "Disponible: {} · {}",update.tag,human_size(update.size as i64)
                        )).color(colors().accent).size(12.5));
                        if ui.add_enabled(!busy && update.can_install(),
                            egui::Button::new(tr("Actualizar automáticamente")).fill(colors().primary))
                            .clicked(){install_update=true;}
                        if !update.can_install(){
                            ui.label(RichText::new(tr("Instalación automática no disponible en este sistema o falta SHA-256")
                            ).color(colors().muted).size(12.0));
                        }
                    }
                });
                if self.update_check_rx.is_some(){ui.horizontal(|ui|{
                    ui.spinner();ui.label(tr("Consultando GitHub Releases..."));
                });}
                if self.update_install_rx.is_some(){
                    let count=self.update_downloaded.load(Ordering::Relaxed);
                    let total=self.available_update.as_ref().map(|x|x.size).unwrap_or(0);
                    let fraction=if total>0 {
                        (count as f32/total as f32).clamp(0.0,1.0)
                    }else{0.0};
                    ui.add(egui::ProgressBar::new(fraction).text(format!(
                        "Descarga verificada: {} / {}",human_size(count as i64),
                        human_size(total as i64)
                    )));
                }
                if self.configured {
                    ui.add_space(18.0);ui.separator();ui.add_space(8.0);
                    ui.label(RichText::new(tr("Organizar archivos antiguos")).strong().size(17.0));
                    ui.label(RichText::new(tr("Copia al almacén actual los ZIP vinculados de versiones anteriores, verifica sus hashes y actualiza sus rutas. No borra los originales.")
                    ).color(colors().muted).size(12.5));
                    if ui.add_enabled(!busy,
                        egui::Button::new(tr("Organizar biblioteca existente"))).clicked(){
                        organize=true;
                    }
                }
                });
            });
        if let Some(kind)=requested_palette {
            if kind!=self.ui_palette {
                match self.db.set_palette_id(kind.id()) {
                    Ok(())=>{
                        self.ui_palette=kind;
                        theme::select(kind);
                        Self::apply_theme(ctx);
                        ctx.request_repaint();
                        self.status=format!("Paleta aplicada: {}",tr(kind.label()));
                    },
                    Err(error)=>self.status=format!("No se guardó la paleta: {error}"),
                }
            }
        }
        if let Some(lang)=requested_language{
            if lang!=self.ui_language {
                match self.db.set_language_id(lang.id()) {
                    Ok(())=>{
                        self.ui_language=lang;
                        i18n::select(lang);
                        ctx.request_repaint();
                        self.status=tr("Idioma aplicado").to_owned();
                    },
                    Err(_)=>self.status=tr("No se pudo guardar el idioma. Inténtalo de nuevo.").into(),
                }
            }
        }
        if check_updates {self.request_update_check();}
        if install_update {self.start_verified_update();}
        if choose {
            if let Some(folder)=rfd::FileDialog::new().pick_folder() {
                self.setup_path=folder.display().to_string();
            }
        }
        if save {
            let target=PathBuf::from(self.setup_path.trim());
            match self.db.configure_storage(&target) {
                Ok(root)=>{
                    self.paths.library=root.clone();
                    self.setup_path=root.display().to_string();
                    self.configured=true;self.show_settings=false;
                    self.status=format!("Almacén configurado: {}",root.display());
                },
                Err(error)=>self.status=format!("Error en configuración: {error:#}"),
            }
        }
        if back {
            self.show_settings=false;
            self.setup_path=self.paths.library.display().to_string();
        }
        if export {
            if let Some(destination)=rfd::FileDialog::new()
                .add_filter(tr("Copia de SZKura"),&["szkura"])
                .set_file_name("SZKura-backup.szkura").save_file(){
                let destination=if destination.extension().is_none(){
                    destination.with_extension("szkura")
                }else{destination};
                self.launch(move|paths|{
                    let result=backup::export_bundle(&paths.db,&paths.covers,&destination)?;
                    Ok(JobResult{select_id:None,message:format!(
                        "Biblioteca exportada a {} ({} portadas)",
                        destination.display(),result.covers)})
                });
            }
        }
        if restore {
            if let Some(source)=rfd::FileDialog::new()
                .add_filter(tr("Copia de SZKura"),&["szkura","szbooth"]).pick_file(){
                self.pending_restore=Some(source);
            }
        }
        if organize {
            self.launch(|paths|{
                let report=managed_storage::organize_existing(&paths.db,&paths.library)?;
                Ok(JobResult{select_id:None,message:format!(
                    "Organización: {} archivos copiados | {} ya ubicados | {} rechazados{}",
                    report.linked,report.duplicates,report.rejected,
                    if report.warnings.is_empty(){String::new()}
                    else{format!(" | {}",report.warnings.join("; "))})
                })
            });
        }
    }

    fn confirm_restore_dialog(&mut self,ctx:&egui::Context){
        let Some(source)=self.pending_restore.clone() else{return};
        let filename=source.file_name().map(|n|n.to_string_lossy().into_owned())
            .unwrap_or_else(||source.display().to_string());
        let choice=confirmation_dialog(ctx,"confirm-import-backup",
            "Restaurar biblioteca","Importar y reemplazar",
            ConfirmTone::Destructive,self.receiver.is_none(),|ui|{
                dialog_subject(ui,&filename);
                ui.add_space(9.0);
                dialog_text(ui,
                    "La copia reemplazará las fichas de la biblioteca actual. Antes se comprobará su integridad y se guardará una copia recuperable de SQLite.");
                ui.add_space(7.0);
                dialog_text(ui,
                    "Los archivos ZIP, 7z, RAR y UnityPackages originales no se borrarán.");
            });
        match choice {
            ConfirmChoice::Cancel=>self.pending_restore=None,
            ConfirmChoice::Accept=>{
                self.pending_restore=None;
                self.restore_in_flight=true;
                self.launch(move|paths|{
                    let result=backup::import_bundle(&paths.db,&paths.covers,&source)?;
                    Ok(JobResult{select_id:None,message:format!(
                        "Copia importada: {} portadas | SQLite anterior recuperable en {}",
                        result.covers,
                        result.recovery.map(|p|p.display().to_string()).unwrap_or_default()
                    )})
                });
            },
            ConfirmChoice::Pending=>{},
        }
    }

    /// A second explicit confirmation is required before moving originals to
    /// the operating system's recoverable trash. Verification happens in worker.
    fn confirm_source_dialog(&mut self,ctx:&egui::Context) {
        if !self.confirm_new_import && self.pending_managed_link.is_none(){return;}
        let count=if self.confirm_new_import{self.new_files.len()}
            else{self.pending_managed_link.as_ref().map(|(_,files)|files.len()).unwrap_or(0)};
        let choice=confirmation_dialog(ctx,"confirm-managed-source-trash",
            "Gestionar originales","Copiar y enviar a papelera",
            ConfirmTone::Destructive,self.receiver.is_none(),|ui|{
                dialog_subject(ui,&format!("{count} {}",
                    if i18n::is_english(){if count==1{"path selected"}else{"paths selected"}}
                    else if count==1{"ruta seleccionada"}else{"rutas seleccionadas"}));
                ui.add_space(9.0);
                dialog_text(ui,
                    "Se copiarán al almacén configurado. Solo tras comprobar el hash, guardar la ficha y verificar que ningún otro producto los referencia, los originales pasarán a la papelera recuperable.");
            });
        match choice {
            ConfirmChoice::Cancel=>{
                self.confirm_new_import=false;
                self.pending_managed_link=None;
            },
            ConfirmChoice::Accept=>{
                if self.confirm_new_import {
                    self.confirm_new_import=false;
                    self.add_product_confirmed();
                }else if let Some((id,files))=self.pending_managed_link.take() {
                    self.link_files_confirmed(id,files,true);
                }
            },
            ConfirmChoice::Pending=>{},
        }
    }

    fn new_product_window(&mut self,ctx:&egui::Context) {
        if !self.new_dialog{return;}
        let busy=self.receiver.is_some();
        let(mut back,mut submit,mut choose_cover)=(false,false,false);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(colors().bg).inner_margin(egui::Margin::symmetric(24,18)))
            .show(ctx,|ui|{
                let compact_header=ui.available_width()<700.0;
                ui.horizontal(|ui|{
                    back=action(ui,Icon::Back,"Biblioteca",false);
                    ui.add_space(12.0);
                    icon(ui,Icon::Plus,colors().accent,22.0);
                    ui.label(RichText::new(tr("Nuevo producto")).strong().size(23.0).color(colors().text));
                    if !compact_header {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                            submit=ui.add_enabled(!busy,
                                egui::Button::new(tr("Guardar producto")).fill(colors().primary)).clicked();
                        });
                    }
                });
                if compact_header {
                    ui.add_space(8.0);
                    submit=ui.add_enabled(!busy,
                        egui::Button::new(tr("Guardar producto")).fill(colors().primary)).clicked();
                }
                ui.add_space(3.0);
                ui.label(RichText::new(tr("Pega un enlace BOOTH para recuperar sus datos públicos al guardar, o crea una ficha manual con tus ZIP.")
                ).color(colors().muted).size(13.0));
                ui.add_space(14.0);ui.separator();ui.add_space(11.0);
                egui::ScrollArea::vertical().id_salt("create-inline-page")
                    .auto_shrink([false,false]).show(ui,|ui|{
                        if editor_two_columns(ui.available_width()) {
                            let left_width=editor_cover_column_width(ui.available_width());
                            ui.horizontal_top(|ui|{
                                ui.allocate_ui_with_layout(egui::vec2(left_width,0.0),
                                    egui::Layout::top_down(egui::Align::Min),|ui|{
                                        choose_cover=self.new_cover_section(ui,ctx);
                                    });
                                ui.add_space(20.0);
                                let remaining=ui.available_width().max(1.0);
                                ui.allocate_ui_with_layout(egui::vec2(remaining,0.0),
                                    egui::Layout::top_down(egui::Align::Min),|ui|{
                                        self.new_fields_section(ui);
                                    });
                            });
                        }else{
                            choose_cover=self.new_cover_section(ui,ctx);
                            ui.add_space(20.0);
                            ui.separator();
                            ui.add_space(12.0);
                            self.new_fields_section(ui);
                        }
                        ui.add_space(20.0);ui.separator();ui.add_space(12.0);
                        ui.horizontal(|ui|{
                            icon(ui,Icon::Archive,colors().accent,20.0);
                            ui.label(RichText::new(tr("Archivos vinculados")).strong().size(18.0).color(colors().text));
                        });
                        ui.label(RichText::new(tr("Al guardar se copiarán archivos ZIP, 7z, RAR y UnityPackages a carpetas organizadas dentro del almacén configurado. No se extraen."))
                            .color(colors().muted).size(13.0));
                        ui.add_space(9.0);
                        ui.horizontal_wrapped(|ui|{
                            if action(ui,Icon::Archive,"Seleccionar archivos",false){
                                if let Some(files)=rfd::FileDialog::new()
                                    .add_filter(tr("ZIP, 7z, RAR y UnityPackage"),&["zip","7z","rar","unitypackage"]).pick_files(){
                                    self.new_files.extend(files);self.new_files.sort();self.new_files.dedup();
                                }
                            }
                            if action(ui,Icon::Folder,"Seleccionar carpeta",false){
                                if let Some(folder)=rfd::FileDialog::new().pick_folder(){
                                    self.new_files.push(folder);
                                    self.new_files.sort();self.new_files.dedup();
                                }
                            }
                        });
                        let previous=self.delete_sources;
                        if ui.checkbox(&mut self.delete_sources,tr("Enviar originales a la papelera tras copiar y verificar (opcional)")).changed(){
                            self.persist_delete_source_choice(previous);
                        }
                        ui.label(RichText::new(tr("Se solicitará confirmación. Si la copia falla o el archivo original está vinculado en otra ficha, se conserva.")
                        ).size(12.0).color(colors().muted));
                        if self.new_files.is_empty(){
                            ui.add_space(8.0);
                            ui.label(RichText::new(tr("También puedes arrastrar archivos a cualquier punto de esta página."))
                                .color(colors().muted).size(13.0));
                        }else{
                            ui.add_space(8.0);
                            ui.label(RichText::new(format!("{} {}",self.new_files.len(),tr("rutas preparadas")))
                                .strong().color(colors().accent).size(14.0));
                            let mut remove_index=None;
                            for (index,path) in self.new_files.iter().enumerate().take(16){
                                ui.horizontal(|ui|{
                                    icon(ui,Icon::File,colors().muted,18.0);
                                    ui.label(path.file_name()
                                        .map(|s|s.to_string_lossy().into_owned())
                                        .unwrap_or_default()).on_hover_text(path.display().to_string());
                                    if ui.small_button(tr("Quitar")).clicked(){remove_index=Some(index);}
                                });
                            }
                            if let Some(index)=remove_index{self.new_files.remove(index);}
                            if self.new_files.len()>16{ui.label(tr("Hay más rutas seleccionadas"));}
                            if ui.button(tr("Vaciar selección")).clicked(){self.new_files.clear();}
                        }
                        ui.add_space(20.0);
                        ui.separator();ui.add_space(10.0);
                        if ui.add_enabled(!busy,
                            egui::Button::new(tr("Guardar producto")).fill(colors().primary))
                            .clicked(){submit=true;}
                    });
            });
        if back{
            self.cancel_avatar_scan();
            self.new_dialog=false;self.new_files.clear();
            return;
        }
        if choose_cover{
            if let Some(file)=rfd::FileDialog::new()
                .add_filter(tr("Imagen"),&["png","jpg","jpeg","webp"]).pick_file(){
                self.new_cover=Some(file);
            }
        }
        if submit{self.cancel_avatar_scan();self.add_product();}
    }

}

impl eframe::App for BoothApp {
    fn update(&mut self,ctx:&egui::Context,_frame:&mut eframe::Frame) {
        self.poll_updates(ctx);
        self.process_finished(ctx);
        if let Some(result)=self.sharing.poll(ctx) {
            match result {
                Ok(id)=>{let _=self.reload();self.select_id(id);
                    self.status="Ficha transferida e importada correctamente".into();},
                Err(error)=>self.status=format!("Error en transferencia: {error}"),
            }
        }
        self.poll_archive_scan();
        self.poll_avatar_scan(ctx);
        self.poll_unity_paths(ctx);
        self.poll_cover_results(ctx);
        if self.favorites_dirty {
            self.favorites_dirty=false;
            if let Err(error)=self.reload(){
                self.status=format!("Error actualizando favoritos: {error}");
            }
        }
        if !self.show_settings && self.configured {
            self.handle_dropped_files(ctx);
        }
        self.header(ctx);
        if self.configured && !self.show_settings {self.sidebar(ctx);}
        // Apply sidebar visibility changes before painting the catalog:
        // unchecking NSFW must not leave hidden products visible for one frame.
        if (self.query.clone(),self.category.clone(),self.avatar_filter.clone(),
            self.files_filter,self.favorites_only,self.show_nsfw)!=self.last_filters {
            self.displayed_limit=96;
            if let Err(e)=self.reload(){self.status=format!("Error de filtros: {e}");}
        }
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::new().fill(colors().panel).inner_margin(egui::Margin::symmetric(18,8)))
            .show(ctx,|ui|{
                let color=if self.status.starts_with("Error")||self.status.starts_with("No se"){
                    Color32::from_rgb(242,157,145)
                }else{colors().muted};
                let visible_status=i18n::tr_message(&self.status);
                ui.add(egui::Label::new(
                    RichText::new(visible_status.to_string()).size(12.0).color(color)).truncate())
                    .on_hover_text(visible_status.to_string());
            });
        // Exactly one CentralPanel, replacing the old floating create/edit windows.
        if !self.configured || self.show_settings {self.settings_page(ctx);}
        else if self.new_dialog{self.new_product_window(ctx);}
        else if self.selected.is_some(){self.detail(ctx);}
        else{self.catalog(ctx);}
        self.quick_delete_dialog(ctx);
        self.confirm_cache_dialog(ctx);
        self.confirm_source_dialog(ctx);
        self.confirm_restore_dialog(ctx);
        self.sharing.render(ctx,self.paths.db.parent().unwrap_or(Path::new(".")),
            &self.paths.db,&self.paths.library,&self.paths.covers);
        if let Some(id)=self.sharing.take_navigation() {
            // Navigate through the existing product selector so the editor
            // loads the correct details, cover and attachment state.
            self.show_settings=false;
            self.new_dialog=false;
            self.select_id(id);
            ctx.request_repaint();
        }
        if self.receiver.is_some(){ctx.request_repaint_after(Duration::from_millis(150));}
    }
}

/// Fit the sharp cover exactly inside the blurred background without
/// cropping or stretching. For tall covers the sharp image now reaches both
/// vertical edges; the blurred backdrop only fills the empty sidebars.
fn fitted_cover_rect(frame:egui::Rect,natural:egui::Vec2)->egui::Rect {
    if natural.x<=0.0 || natural.y<=0.0 {
        return egui::Rect::from_center_size(frame.center(),egui::Vec2::ZERO);
    }
    let factor=(frame.width()/natural.x)
        .min(frame.height()/natural.y)
        .min(1.0)
        .max(0.0);
    egui::Rect::from_center_size(frame.center(),natural*factor)
}

/// Reserve a bounded cover column and let the form use the remaining width.
/// Equal-width columns created a 200-300pt empty gutter on wide monitors.
fn editor_cover_column_width(available:f32)->f32 {
    (available*0.40).min(482.0).max(1.0)
}

#[cfg(test)]
mod cover_geometry_tests {
    use super::*;
    #[test]
    fn cover_fits_background_without_forced_two_point_gap() {
        let backdrop=egui::Rect::from_min_size(
            egui::Pos2::new(11.0,24.0),egui::vec2(320.0,170.0));
        // Vertical and square images reach the top/bottom edge. Wide images
        // reach the left/right edge; any remaining space is intentional blur.
        for source in [
            egui::vec2(160.0,320.0),
            egui::vec2(640.0,120.0),
            egui::vec2(200.0,200.0),
            egui::vec2(320.0,170.0),
        ] {
            let sharp=fitted_cover_rect(backdrop,source);
            const EPS:f32=0.01;
            assert!(sharp.min.x>=backdrop.min.x-EPS);
            assert!(sharp.min.y>=backdrop.min.y-EPS);
            assert!(sharp.max.x<=backdrop.max.x+EPS);
            assert!(sharp.max.y<=backdrop.max.y+EPS);
            let fills_height=source.y/source.x>=backdrop.height()/backdrop.width();
            if fills_height {
                assert!((sharp.top()-backdrop.top()).abs()<EPS);
                assert!((sharp.bottom()-backdrop.bottom()).abs()<EPS);
            }else{
                assert!((sharp.left()-backdrop.left()).abs()<EPS);
                assert!((sharp.right()-backdrop.right()).abs()<EPS);
            }
        }
    }
    #[test]
    fn editor_cover_column_does_not_waste_wide_screen_space(){
        assert!((editor_cover_column_width(940.0)-376.0).abs()<1.0);
        assert!((editor_cover_column_width(1400.0)-482.0).abs()<1.0);
        assert!(1400.0-editor_cover_column_width(1400.0)-32.0>880.0);
    }
}

/// Below this content width, two columns leave too little room for the
/// product fields, especially long URLs, Japanese text and action buttons.
const TWO_COL_EDITOR_MIN_WIDTH:f32=940.0;
fn editor_two_columns(content_width:f32)->bool{
    content_width>=TWO_COL_EDITOR_MIN_WIDTH
}

#[cfg(test)]
mod responsive_editor_tests {
    use super::*;
    #[test]
    fn narrow_form_does_not_expand_beyond_its_column(){
        let ctx=egui::Context::default();
        let mut max_width=0.0_f32;
        for _ in 0..2 {
            let _=ctx.run(egui::RawInput{
                screen_rect:Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,egui::vec2(980.0,800.0))),
                ..Default::default()
            },|ctx|{
                egui::CentralPanel::default().show(ctx,|ui|{
                    ui.allocate_ui_with_layout(egui::vec2(360.0,0.0),
                        egui::Layout::top_down(egui::Align::Min),|ui|{
                            ui.set_width(360.0);
                            let mut url=String::new();
                            let mut title=String::new();
                            let mut creator=String::new();
                            let mut category="Avatar".to_owned();
                            let mut tags=String::new();
                            let mut avatars=String::new();
                            let mut nsfw=false;
                            let mut description=String::new();
                            let mut expanded=false;
                            product_fields(ui,&mut url,&mut title,&mut creator,
                                &mut category,&mut tags,&mut avatars,&mut nsfw,
                                &mut description,&mut expanded,"test-responsive-category",false);
                            max_width=ui.min_rect().width();
                        });
                });
            });
        }
        assert!(max_width<=372.0,
            "Narrow product form should fit within 360pt, got {max_width}pt");
    }

    #[test]
    fn chooses_stacked_layout_before_fields_get_cramped(){
        assert!(!editor_two_columns(320.0));
        assert!(!editor_two_columns(710.0));
        assert!(!editor_two_columns(939.0));
        assert!(editor_two_columns(940.0));
        assert!(editor_two_columns(1150.0));
    }
}

/// Existing catalog records preserve their manually selected categories.
/// Only a NEW BOOTH product left at the form's default "Otros" may inherit a
/// recognized public BOOTH category. This runs before managed import/copy.
fn resolved_import_category<'a>(
    requested:&'a str,existing:&'a str,detected:Option<&'static str>,already_linked:bool
)->String{
    if requested!="Otros"{return requested.to_string();}
    if already_linked{return existing.to_string();}
    detected.unwrap_or(requested).to_string()
}

#[cfg(test)]
mod booth_import_category_tests {
    use super::resolved_import_category;
    #[test]
    fn category_is_finalized_before_copy_without_overwriting_manual_choices(){
        assert_eq!(resolved_import_category("Otros","Otros",Some("Ropa"),false),"Ropa");
        assert_eq!(resolved_import_category("Otros","Otros",Some("Pelo"),false),"Pelo");
        assert_eq!(resolved_import_category("Otros","Otros",None,false),"Otros");
        assert_eq!(resolved_import_category("Avatar","Otros",Some("Ropa"),false),"Avatar");
        assert_eq!(resolved_import_category("Otros","Accesorios",Some("Ropa"),true),"Accesorios");
        assert_eq!(resolved_import_category("Ropa","Avatar",Some("Pelo"),true),"Ropa");
    }
}

/// Compact avatar summaries are identical in grid and list views, regardless
/// of window width. Keep the full string in the tooltip of the widget.
fn avatar_summary(avatars:&str)->Option<String>{
    let mut names=avatars.split(',').map(str::trim).filter(|name|!name.is_empty());
    let first=names.next()?;
    let remaining=names.count();
    if remaining>0 {Some(format!("Avatar: {first} +{remaining}"))}
    else {Some(format!("Avatar: {first}"))}
}

#[cfg(test)]
mod avatar_summary_tests {
    use super::avatar_summary;
    #[test]
    fn list_and_grid_use_one_summary_style(){
        assert_eq!(avatar_summary("MAYA, MANUKA, SHINANO"),Some("Avatar: MAYA +2".into()));
        assert_eq!(avatar_summary("MAYA"),Some("Avatar: MAYA".into()));
        assert_eq!(avatar_summary(""),None);
        assert_eq!(avatar_summary(" , MAYA, , MANUKA ,"),Some("Avatar: MAYA +1".into()));
    }
}

/// Size the inspector to its visible entries, while capping it to the window.
/// The caller also supplies this height as min_scrolled_height so a nested
/// scroll pane cannot be squeezed to the last 1-2 rows of the outer pane.
fn archive_tree_height(rows:usize,viewport_height:f32)->f32 {
    // This height is only used for lists with three or more entries.
    let content=(rows.max(3).min(13) as f32)*36.0+10.0;
    let viewport_cap=(viewport_height*0.70).clamp(260.0,550.0);
    content.max(68.0).min(viewport_cap)
}

#[cfg(test)]
mod archive_tree_layout_tests {
    use super::archive_tree_height;
    #[test]
    fn inspector_grows_with_entries_but_stays_compact_for_one() {
        let three=archive_tree_height(3,800.0);
        let five=archive_tree_height(5,800.0);
        let many=archive_tree_height(53,800.0);
        assert!(three>=110.0 && three<=140.0);
        assert!(five>three);
        assert!(many>=450.0 && many<=550.0);
        assert_eq!(archive_tree_height(0,800.0),three);
        assert!(archive_tree_height(53,650.0)<=455.0);
        // One or two entries are rendered directly, without reserving a
        // nested ScrollArea at all, so their frame is content-sized.
    }
}


fn unity_folders_action_label(is_visible:bool)->&'static str {
    if is_visible {"Ocultar carpetas en Unity"} else {"Ver carpetas en Unity"}
}

#[cfg(test)]
mod unity_folders_toggle_tests {
    use super::{unity_folders_action_label,unity_paths_id};
    use std::path::Path;

    #[test]
    fn button_label_changes_to_hide_while_preview_is_open() {
        assert_eq!(unity_folders_action_label(false),"Ver carpetas en Unity");
        assert_eq!(unity_folders_action_label(true),"Ocultar carpetas en Unity");
    }
    #[test]
    fn selection_identity_is_stable_for_direct_packages() {
        assert_eq!(unity_paths_id(Path::new("example.unitypackage"),None),
            unity_paths_id(Path::new("example.unitypackage"),None));
        assert_ne!(unity_paths_id(Path::new("other.unitypackage"),None),
            unity_paths_id(Path::new("example.unitypackage"),None));
    }
}

fn unity_paths_id(source:&Path,package:Option<&UnityPackage>)->String {
    match package {
        Some(package)=>format!("{}|nested|{}|{}|{:?}|{}",
            source.display(),package.name,package.index,package.zip_chain,package.crc32),
        None=>format!("{}|direct",source.display()),
    }
}

/// Compact themed read-only folder preview. No Unity application or script
/// is launched; clipboard buttons copy exact folder names from the package.
fn show_unity_paths_result(
    ui:&mut egui::Ui,label:&str,loading:bool,
    preview:Option<&UnityPathsPreview>,error:Option<&str>,
    category:&str,can_export:bool,
)->bool {
    let mut export_requested=false;
    ui.add_space(9.0);
    let full_width=ui.available_width();
    egui::Frame::new().fill(colors().card)
        .stroke(egui::Stroke::new(1.0,colors().stroke))
        .corner_radius(8).inner_margin(12).show(ui,|ui|{
            // The preview is a full-width section of the inspector, not a
            // floating content-sized card alongside the package list.
            ui.set_min_width((full_width-24.0).max(1.0));
            ui.label(RichText::new(tr("Carpetas de importación en Unity"))
                .strong().size(15.0).color(colors().accent));
            ui.add(egui::Label::new(RichText::new(label)
                .size(12.0).color(colors().text)).truncate()).on_hover_text(label);
            ui.label(RichText::new(tr("Rutas originales del paquete; pueden diferir del proyecto Unity actual."))
                .size(12.0).color(colors().muted));
            if loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Examinando rutas del UnityPackage..."));
                });
            } else if let Some(error)=error {
                // Localize inspector diagnostics at display time so changing
                // language also updates an already-open error panel.
                let visible_error=i18n::tr_unitypackage_error(error);
                ui.label(RichText::new(format!("{} {visible_error}",
                    tr("No se pudieron leer las rutas del UnityPackage:")))
                    .size(12.0).color(colors().muted));
            } else if let Some(preview)=preview {
                if preview.main_folders.is_empty() {
                    ui.label(RichText::new(tr("No contiene rutas válidas dentro de Assets/."))
                        .size(12.0).color(colors().muted));
                } else {
                    ui.label(RichText::new(format!("{}: {}",
                        tr("Archivos con ruta en Assets/"),preview.asset_count))
                        .size(12.0).color(colors().muted));
                    if preview.skipped_paths>0 {
                        ui.label(RichText::new(format!("{}: {}",
                            tr("Rutas fuera de Assets/ ignoradas"),preview.skipped_paths))
                            .size(12.0).color(colors().muted));
                    }
                    if let Ok(unity_label)=unitypackage_labels::label_for_category(category) {
                        ui.add_space(6.0);
                        ui.label(RichText::new(tr("Al abrir en Unity desde SZKura, se etiqueta solo la primera carpeta del paquete o la primera subcarpeta disponible."))
                            .size(12.0).color(colors().text));
                        ui.label(RichText::new(tr("La etiqueta de carpeta no se hereda por los archivos internos en Unity."))
                            .size(12.0).color(colors().muted));
                        ui.label(RichText::new(tr("Experimental: genera una copia; no modifica originales ni carpetas de Unity."))
                            .size(12.0).color(colors().muted));
                        ui.horizontal_wrapped(|ui|{
                            ui.label(RichText::new(format!("{}: {}",
                                tr("Etiqueta de la categoría guardada"),unity_label))
                                .size(12.0).color(colors().accent));
                            if ui.add_enabled(can_export,egui::Button::new(
                                tr("Exportar copia etiquetada...")))
                                .on_hover_text(tr("Conserva GUIDs, rutas y demás etiquetas; importar manualmente en Unity."))
                                .clicked() {
                                export_requested=true;
                            }
                        });
                    }
                    ui.label(RichText::new(tr("Carpetas principales")).strong()
                        .color(colors().text).size(13.0));
                    for folder in &preview.main_folders {
                        unity_folder_row(ui,folder);
                    }
                    if preview.folders!=preview.main_folders {
                        egui::CollapsingHeader::new(tr("Carpetas exactas"))
                            .id_salt(("unity-paths-exact",label))
                            .show(ui,|ui|{
                                // A few folders need no second scroll area.
                                // Large packages get a viewport-relative list that
                                // shows many more paths than the former 245 pt cap.
                                if let Some(height)=unity_folders_scroll_height(
                                    preview.folders.len(),ui.ctx().content_rect().height()
                                ) {
                                    egui::ScrollArea::vertical()
                                        .id_salt(("unity-exact-scroll",label))
                                        .min_scrolled_height(height)
                                        .max_height(height)
                                        .auto_shrink([false,false])
                                        .show(ui,|ui|{
                                            for folder in &preview.folders {
                                                unity_folder_row(ui,folder);
                                            }
                                        });
                                } else {
                                    for folder in &preview.folders {
                                        unity_folder_row(ui,folder);
                                    }
                                }
                            });
                    }
                }
            }
        });
    export_requested
}

/// Keep small lists content-sized and make large lists comfortable to browse.
/// This is intentionally independent of display scale and package filename.
fn unity_folders_scroll_height(count:usize,viewport_height:f32)->Option<f32> {
    if count<=10 {None}
    else {Some((viewport_height*0.65).clamp(320.0,500.0))}
}

#[cfg(test)]
mod unity_folder_layout_tests {
    use super::unity_folders_scroll_height;

    #[test]
    fn short_lists_do_not_create_nested_scrollbars() {
        assert_eq!(unity_folders_scroll_height(0,650.0),None);
        assert_eq!(unity_folders_scroll_height(10,650.0),None);
    }

    #[test]
    fn long_lists_show_a_taller_scrollable_viewport() {
        let height=unity_folders_scroll_height(11,650.0).expect("long list has a scroll");
        assert!((height-422.5).abs()<0.01);
        assert_eq!(unity_folders_scroll_height(100,1200.0),Some(500.0));
        assert_eq!(unity_folders_scroll_height(11,400.0),Some(320.0));
    }
}

fn unity_folder_row(ui:&mut egui::Ui,folder:&str) {
    ui.horizontal(|ui| {
        icon(ui,Icon::Archive,colors().accent,13.0);
        let copy_width=105.0;
        let name_width=(ui.available_width()-copy_width-ui.spacing().item_spacing.x)
            .max(70.0);
        // Reserve the full column for consistent Copy buttons. ui.put()
        // uses a centered-and-justified layout; instead, put the truncated
        // label in a left-to-right child anchored to this reserved rectangle.
        let (name_rect,_)=ui.allocate_exact_size(
            egui::vec2(name_width,30.0),egui::Sense::hover());
        let mut label_ui=ui.new_child(
            egui::UiBuilder::new().id_salt(("unity-folder-label",folder))
                .max_rect(name_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center))
        );
        label_ui.add(egui::Label::new(
            RichText::new(folder).size(12.0).color(colors().text)
        ).truncate().halign(egui::Align::LEFT)).on_hover_text(folder);
        if ui.add_sized([copy_width,30.0],egui::Button::new(tr("Copiar ruta")))
            .clicked() {
            ui.ctx().copy_text(folder.to_owned());
        }
    });
}

/// Render ZIPs as a real expandable hierarchy, including regular files even
/// if they have no .unitypackage. ZIPs remain on disk; this only draws metadata.
fn show_zip_tree(
    ui:&mut egui::Ui,
    preview:&ZipPreview,
    parent:&[package_inspect::NestedZip],
    source:&Path,
    busy:bool,
    active_unity_paths_key:Option<&str>,
    unity_paths_scanning:bool,
    open_package:&mut Option<(PathBuf,UnityPackage)>,
    inspect_package:&mut Option<(PathBuf,Option<UnityPackage>)>,
) {
    for child in preview.inner_archives.iter().filter(|node| {
        node.chain.len()==parent.len()+1 && node.chain.starts_with(parent)
    }) {
        let Some(last)=child.chain.last() else{continue};
        let leaf=last.name.rsplit('/').next().unwrap_or(&last.name);
        let suffix=if child.skipped_reason.is_some(){
            format!(" · {}",tr("no examinado"))
        }else{
            format!(" · {} {}",child.total_entries,tr("entradas"))
        };
        let title=RichText::new(format!(
            "ZIP  {}  ({}){}",leaf,human_size(last.size as i64),suffix
        )).color(if child.skipped_reason.is_some(){
            Color32::from_rgb(239,194,133)
        }else{colors().accent}).size(13.0).strong();
        // Full chain identity means two ZIPs with the same basename remain distinct.
        let identity=child.chain.iter().map(|hop|(hop.index,hop.crc32))
            .collect::<Vec<_>>();
        let response=egui::CollapsingHeader::new(title)
            .id_salt(("nested-zip-tree",identity))
            .default_open(false)
            .show(ui,|ui|{
                if let Some(reason)=&child.skipped_reason {
                    ui.label(RichText::new(format!("{}: {reason}",tr("Contenido no disponible"))).color(Color32::from_rgb(239,194,133)).size(12.0));
                } else {
                    if child.files.is_empty() &&
                       !preview.inner_archives.iter().any(|other| {
                           other.chain.len()==child.chain.len()+1
                               && other.chain.starts_with(&child.chain)
                       }) {
                        ui.label(RichText::new(tr("Este ZIP está vacío o no contiene archivos seguros."))
                            .size(12.0).color(colors().muted));
                    }
                    show_zip_tree(ui,preview,&child.chain,source,busy,active_unity_paths_key,
                        unity_paths_scanning,open_package,inspect_package);
                }
            });
        response.header_response.on_hover_text(&last.name);
    }

    let (files,hidden)=if parent.is_empty(){
        (preview.root_files.as_slice(),preview.root_hidden_files)
    }else{
        match preview.inner_archives.iter().find(|node|node.chain.as_slice()==parent){
            Some(node)=>(node.files.as_slice(),node.hidden_files),
            None=>return,
        }
    };
    for file in files {
        // The filename is independent of the actions. Long Japanese product
        // names cannot push a single button onto the next line.
        ui.vertical(|ui|{
            ui.spacing_mut().item_spacing.y=3.0;
            ui.horizontal(|ui|{
                icon(ui,Icon::File,
                    if file.package.is_some(){colors().accent}else{colors().muted},15.0);
                let text=RichText::new(format!("{}  ({})",
                    file.name,human_size(file.size as i64))).size(12.0)
                    .color(if file.package.is_some(){colors().text}else{colors().muted});
                ui.add(egui::Label::new(text).truncate())
                    .on_hover_text(&file.name);
            });
            if let Some(package)=&file.package {
                ui.horizontal(|ui|{
                    // Two related buttons share a stable action row even when
                    // file paths are long or contain multibyte characters.
                    ui.add_space(23.0);
                    if ui.add_enabled(!busy && package.size<=package_inspect::MAX_PACKAGE_BYTES,
                        egui::Button::new(tr("Extraer y abrir"))
                            .min_size(egui::vec2(0.0,32.0)))
                        .on_hover_text(tr("Al abrir, se añade automáticamente la etiqueta SZKura en una copia de caché."))
                        .clicked(){
                        *open_package=Some((source.to_path_buf(),package.clone()));
                    }
                    let unity_open=active_unity_paths_key
                        ==Some(unity_paths_id(source,Some(package)).as_str());
                    if ui.add_enabled(!busy && (!unity_paths_scanning || unity_open),
                        egui::Button::new(tr(unity_folders_action_label(unity_open)))
                            .min_size(egui::vec2(0.0,32.0))).clicked(){
                        *inspect_package=Some((source.to_path_buf(),Some(package.clone())));
                    }
                });
            }
        });
    }
    if hidden>0 {
        ui.label(RichText::new(if i18n::is_english(){format!("{hidden} more files hidden (limit {} per ZIP).",package_inspect::MAX_PREVIEW_FILES_PER_ARCHIVE)}
 else{format!("{hidden} archivos más sin mostrar (límite de {} por ZIP).",package_inspect::MAX_PREVIEW_FILES_PER_ARCHIVE)}).color(colors().muted).size(11.5));
    }
    if parent.is_empty() && preview.hidden_archives>0 {
        ui.label(RichText::new(if i18n::is_english(){format!("{} additional nested ZIPs hidden by safety limits.",preview.hidden_archives)}
 else{format!("{} ZIP internos adicionales no mostrados por límite de seguridad.",preview.hidden_archives)}).color(Color32::from_rgb(239,194,133)).size(12.0));
    }
}

/// Keep the summary the exact width of the cover, not the full left column.
/// Allocate a content-sized, top-down child: the left column can be much wider
/// than its cover and the other column can be much taller than this widget.
fn product_summary_card(ui:&mut egui::Ui,cover_width:f32,
    attachment_count:usize,avatars:&str)->egui::Rect {
    let width=cover_width.min(ui.available_width()).max(1.0);
    let response=ui.allocate_ui_with_layout(
        egui::vec2(width,0.0),
        egui::Layout::top_down(egui::Align::Min),|ui|{
            ui.set_width(width);
            egui::Frame::new()
                .fill(colors().panel)
                .stroke(egui::Stroke::new(1.0,colors().stroke))
                .corner_radius(10)
                .inner_margin(egui::Margin::symmetric(14,11))
                .show(ui,|ui|{
                    ui.set_width((width-28.0).max(1.0));
                    ui.spacing_mut().item_spacing.y=5.0;
                    ui.label(RichText::new(tr("RESUMEN"))
                        .strong().color(colors().accent).size(11.0));
                    ui.horizontal(|ui|{
                        icon(ui,Icon::Archive,colors().muted,16.0);
                        ui.label(RichText::new(format!(
                            "{attachment_count} {} {}",
                            tr(if attachment_count==1{"archivo"}else{"archivos"}),tr("vinculados")
                        )).color(colors().text).size(12.5));
                    });
                    if let Some(summary)=avatar_summary(avatars) {
                        ui.horizontal(|ui|{
                            icon(ui,Icon::Filter,colors().accent,16.0);
                            ui.add(egui::Label::new(RichText::new(summary)
                                .size(12.5).color(colors().accent)).truncate())
                                .on_hover_text(avatars);
                        });
                    }
                });
        });
    response.response.rect
}

/// Use the natural checkbox width: add_sized over the full form column
/// centers its contents, instead of aligning them with the left input edge.
fn left_aligned_nsfw_checkbox(ui:&mut egui::Ui,nsfw:&mut bool)->egui::Response {
    ui.horizontal(|ui|{
        ui.checkbox(nsfw,tr("Marcar como NSFW (contenido sensible)"))
    }).inner
}

#[cfg(test)]
mod product_editor_layout_tests {
    use super::*;

    #[test]
    fn summary_fits_cover_and_checkbox_starts_at_form_edge(){
        let ctx=egui::Context::default();
        let mut summary_rect=egui::Rect::NOTHING;
        let mut checkbox_offset=100.0_f32;
        for _ in 0..2 {
            let _=ctx.run(egui::RawInput{
                screen_rect:Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,egui::vec2(1100.0,800.0))),
                ..Default::default()
            },|ctx|{
                egui::CentralPanel::default().show(ctx,|ui|{
                    assert!(ui.available_width()>600.0);
                    summary_rect=product_summary_card(ui,480.0,1,"AIRI");
                    let left=ui.available_rect_before_wrap().left();
                    let mut marked=false;
                    let checkbox=left_aligned_nsfw_checkbox(ui,&mut marked);
                    checkbox_offset=(checkbox.rect.left()-left).abs();
                });
            });
        }
        // egui adds one pixel of frame stroke per side (482px overall).
        // The cover editor uses the same frame stroke and therefore matches.
        assert!((summary_rect.width()-482.0).abs()<2.0,
            "Summary card width should match framed cover: {:?}",summary_rect);
        assert!(summary_rect.height()<120.0,
            "Summary with one avatar should stay compact: {:?}",summary_rect);
        assert!(checkbox_offset<3.0,
            "Checkbox should align with input edge: offset {checkbox_offset}");
    }
}

/// One labeled input, including its spacing, used by both create and edit pages.
fn form_text(ui:&mut egui::Ui,label:&str,value:&mut String,hint:&str){
    ui.vertical(|ui|{
        ui.spacing_mut().item_spacing.y=5.0;
        ui.label(RichText::new(tr(label)).color(colors().muted).size(13.0));
        ui.add_sized([ui.available_width().max(1.0),36.0],
            egui::TextEdit::singleline(value)
                .vertical_align(egui::Align::Center)
                .hint_text(tr(hint)));
    });
}

/// A full-width category selector with the same label spacing as text inputs.
fn category_field(ui:&mut egui::Ui,category:&mut String,field_id:&str){
    ui.vertical(|ui|{
        ui.spacing_mut().item_spacing.y=5.0;
        ui.label(RichText::new(tr("Categoría")).color(colors().muted).size(13.0));
        egui::ComboBox::from_id_salt(field_id)
            .selected_text(tr(category.as_str()))
            .width((ui.available_width()-2.0).max(1.0)).show_ui(ui,|ui|{
                for &cat in CATEGORIES {
                    ui.selectable_value(category,cat.to_owned(),tr(cat));
                }
            });
    });
}

/// Fill available catalog width with cards at least 325pt wide (apart from
/// narrow one-column layouts). Account for egui's gaps between cards.
fn grid_column_count(available_width:f32,gap:f32)->usize {
    (((available_width+gap)/(325.0+gap)).floor() as usize).max(1)
}

#[cfg(test)]
mod grid_column_tests {
    use super::grid_column_count;

    #[test]
    fn adds_columns_when_the_viewport_grows() {
        assert_eq!(grid_column_count(280.0,8.0),1);
        assert_eq!(grid_column_count(720.0,8.0),2);
        assert_eq!(grid_column_count(1430.0,8.0),4);
        assert_eq!(grid_column_count(1850.0,8.0),5);
        assert_eq!(grid_column_count(2500.0,8.0),7);
    }
}

fn product_fields(
    ui:&mut egui::Ui,url:&mut String,title:&mut String,
    creator:&mut String,category:&mut String,tags:&mut String,
    avatars:&mut String,nsfw:&mut bool,
    description:&mut String,expanded:&mut bool,field_id:&str,
    scan_busy:bool,
)->bool {
    ui.spacing_mut().item_spacing.y=12.0;
    ui.spacing_mut().interact_size.y=36.0;
    form_text(ui,"Enlace de BOOTH",url,"https://chocolaterice.booth.pm/items/123456");
    form_text(ui,"Nombre del producto",title,"Nombre visible en la biblioteca");
    if ui.available_width()>=550.0{
        ui.columns(2,|cols|{
            form_text(&mut cols[0],"Creador / tienda",creator,"Nombre de tienda");
            category_field(&mut cols[1],category,field_id);
        });
    }else{
        form_text(ui,"Creador / tienda",creator,"Nombre de tienda");
        category_field(ui,category,field_id);
    }
    let mut scan_clicked=false;
    ui.vertical(|ui|{
        ui.spacing_mut().item_spacing.y=5.0;
        ui.label(RichText::new(tr("Avatares compatibles")).color(colors().muted).size(13.0));
        ui.horizontal(|ui|{
            let button_space=154.0;
            let input_width=(ui.available_width()-button_space-8.0).max(70.0);
            ui.add_sized([input_width,36.0],
                egui::TextEdit::singleline(avatars)
                    .vertical_align(egui::Align::Center)
                    .hint_text("SHINANO, MAYA, MANUKA..."));
            scan_clicked=ui.add(
                egui::Button::new(tr(if scan_busy {
                    "Cancelar análisis"
                } else {
                    "Analizar archivos"
                })).min_size(egui::vec2(145.0,36.0)))
                .on_hover_text(tr("Busca avatares conocidos en nombres y rutas de archivos."))
                .clicked();
        });
    });
    ui.label(RichText::new(tr("Se guardan en MAYÚSCULAS y sin nombres repetidos. Sepáralos con comas."))
        .color(colors().muted).size(12.0));
    form_text(ui,"Otras etiquetas",tags,"Color, estilo, temática...");
    // A full-width add_sized checkbox centers the label inside its large
    // allocated rectangle. Keep the checkbox left-aligned with the inputs.
    left_aligned_nsfw_checkbox(ui,nsfw);
    ui.label(RichText::new(tr("Los productos marcados se ocultan del catálogo y de la búsqueda, salvo que actives «Mostrar NSFW».")
    ).color(colors().muted).size(12.0));
    ui.add_space(5.0);
    ui.horizontal(|ui|{
        ui.label(RichText::new(tr("Descripción")).strong().color(colors().text).size(14.0));
        // An explicit 32pt footer avoids layout spillover into the preview
        // and keeps the button aligned to the right edge of the text input.
        let rest=ui.available_width().max(1.0);
        ui.allocate_ui_with_layout(egui::vec2(rest,32.0),
            egui::Layout::right_to_left(egui::Align::Center),|ui|{
                if ui.add(egui::Button::new(tr(if *expanded{"Mostrar menos"}
                    else if description.is_empty(){"Añadir descripción"}
                    else{"Mostrar más / editar"})).min_size(egui::vec2(0.0,32.0)))
                    .clicked(){*expanded=!*expanded;}
            });
    });
    if *expanded{
        ui.add_sized([ui.available_width(),185.0],
            egui::TextEdit::multiline(description)
                .desired_rows(7).hint_text(tr("Descripción y notas del producto")));
    }else{
        let brief=description.lines().map(str::trim)
            .filter(|line|!line.is_empty() && line.chars().any(char::is_alphanumeric))
            .take(16).collect::<Vec<_>>().join(" ");
        let short=brief.chars().take(170).collect::<String>();
        let preview=if short.is_empty(){tr("Sin descripción").to_string()}
            else if brief.chars().count()>170{format!("{short}...")}else{short};
        egui::Frame::new().fill(colors().panel)
            .stroke(egui::Stroke::new(1.0,colors().stroke))
            .corner_radius(8).inner_margin(egui::Margin::symmetric(12,11))
            .show(ui,|ui|{
                ui.set_min_width(ui.available_width().max(1.0));
                ui.add(egui::Label::new(RichText::new(preview)
                    .color(colors().muted).size(13.0)).wrap());
            });
    }
    scan_clicked
}
fn cache_local_cover(covers:&Path,source:&Path)->Result<String> {
    let bytes=std::fs::read(source).context("No se pudo leer la portada")?;
    if bytes.len()>12_000_000 {anyhow::bail!("Imagen mayor de 12 MB");}
    let decoded=image::load_from_memory(&bytes).context("Imagen no válida")?;
    if decoded.width()==0||decoded.height()==0||decoded.width()>6000||decoded.height()>6000 {
        anyhow::bail!("Dimensiones de imagen no admitidas");
    }
    std::fs::create_dir_all(covers)?;
    let ext=source.extension().and_then(|s|s.to_str()).unwrap_or("img");
    let hash=blake3::hash(&bytes).to_hex();
    let target=covers.join(format!("manual-{hash}.{ext}"));
    if !target.exists(){std::fs::write(&target,bytes)?;}
    Ok(target.to_string_lossy().into_owned())
}

fn human_size(n:i64)->String {
    let mut s=n.max(0) as f64;
    for unit in ["B","KB","MB","GB","TB"] {if s<1024.0||unit=="TB"{return format!("{s:.1} {unit}");}s/=1024.0;}
    "0 B".into()
}
/// Launch a local .unitypackage with the OS default file association.
/// This is equivalent to opening it in Explorer; Unity must be installed and
/// associated, and the user chooses which Unity project to import into.
fn open_unity_file(path:&Path)->Result<()> {
    if !path.is_file() || !path.extension().and_then(|s|s.to_str())
        .is_some_and(|ext|ext.eq_ignore_ascii_case("unitypackage")) {
        anyhow::bail!("No existe un archivo .unitypackage válido en esa ruta");
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        let wide=path.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        // ShellExecute respects the user's registered program; no cmd.exe/shell interpolation.
        let result=unsafe {
            ShellExecuteW(std::ptr::null_mut(),std::ptr::null(),wide.as_ptr(),
                std::ptr::null(),std::ptr::null(),1)
        };
        if result as isize <= 32 {
            anyhow::bail!("Windows no tiene una asociación para .unitypackage (código {})",
                result as isize);
        }
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(path)
            .spawn().context("No se pudo abrir el unitypackage con xdg-open")?;
    }
    Ok(())
}

fn reveal_file(path:&str)->Result<()> {
    let full=Path::new(path).canonicalize()
        .with_context(||format!("Archivo no encontrado: {path}"))?;
    if !full.is_file(){anyhow::bail!("La ruta no corresponde a un archivo");}
    #[cfg(target_os="windows")]
    {
        // Explorer accepts /select, <absolute-path> with a separate quoted path
        // argument; this also handles filenames containing spaces.
        std::process::Command::new("explorer.exe")
            .arg("/select,").arg(&full).spawn()
            .context("No se pudo abrir el Explorador de Windows")?;
    }
    #[cfg(not(target_os="windows"))]
    open_folder(path)?;
    Ok(())
}

fn open_folder(path:&str)->Result<()> {
    let full=Path::new(path).canonicalize()
        .with_context(||format!("No existe la ruta: {path}"))?;
    let dir=if full.is_dir(){full.as_path()}
        else{full.parent().context("No se encontró la carpeta del archivo")?};
    #[cfg(target_os="windows")]
    std::process::Command::new("explorer.exe").arg(dir).spawn()
        .context("No se pudo abrir la carpeta en el Explorador")?;
    #[cfg(not(target_os="windows"))]
    std::process::Command::new("xdg-open").arg(dir).spawn()
        .context("No se pudo abrir la carpeta en el gestor de archivos")?;
    Ok(())
}

/// Native window icon: vector-like pixels, no emoji font dependency.
fn application_icon()->egui::IconData {
    let n=64u32;
    let mut rgba=Vec::with_capacity((n*n*4) as usize);
    for y in 0..n {
        for x in 0..n {
            let base=(19u8,27u8,36u8);
            let folder=(x>=10&&x<=53&&y>=22&&y<=46)
                ||(x>=10&&x<=31&&y>=16&&y<24);
            let accent=(x>=14&&x<=49&&y>=29&&y<=42);
            let (r,g,b)=if accent {(40,155,151)}
                else if folder{(136,209,199)}else{base};
            rgba.extend([r,g,b,255]);
        }
    }
    egui::IconData{rgba,width:n,height:n}
}

fn main()->eframe::Result {
    startup_diagnostics::install();
    let options=eframe::NativeOptions{viewport:egui::ViewportBuilder::default()
        .with_inner_size([1360.0,825.0]).with_min_inner_size([980.0,650.0])
        .with_icon(std::sync::Arc::new(application_icon())),..Default::default()};
    let result=eframe::run_native("SZKura | Asset Library",options,Box::new(|cc|{
        Ok(Box::new(BoothApp::new(cc).expect("No se pudo iniciar la base de datos de SZKura")))
    }));
    if let Err(error)=&result {
        startup_diagnostics::report(&format!("No se pudo iniciar la ventana de SZKura: {error}"));
    }
    result
}
