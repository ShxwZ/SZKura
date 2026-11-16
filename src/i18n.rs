//! UI translations for SZKura. Stable catalog/category IDs stay in Spanish
//! to preserve SQLite compatibility; only the displayed text changes.
use std::sync::atomic::{AtomicU8,Ordering};

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
#[repr(u8)]
pub enum Language { Spanish=0, English=1 }
impl Language {
    pub const ALL:[Self;2]=[Self::Spanish,Self::English];
    pub fn id(self)->&'static str {match self{Self::Spanish=>"es",Self::English=>"en"}}
    pub fn label(self)->&'static str {match self{Self::Spanish=>"Español",Self::English=>"English"}}
    pub fn from_id(id:&str)->Option<Self>{Self::ALL.into_iter().find(|lang|lang.id()==id)}
}
static SELECTED:AtomicU8=AtomicU8::new(Language::Spanish as u8);
pub fn selected()->Language {
    if SELECTED.load(Ordering::Relaxed)==1 {Language::English}else{Language::Spanish}
}
pub fn select(language:Language){SELECTED.store(language as u8,Ordering::Relaxed);}
pub fn is_english()->bool {selected()==Language::English}
pub fn tr(s:&str)->&str { translate(selected(),s) }
pub fn translate(language:Language,s:&str)->&str {
    if language==Language::Spanish {return s;}
    match s {
        "Biblioteca local de assets"=>"Local asset library",
        "Configuración"=>"Settings",
        "Nuevo producto"=>"New product",
        "Buscar productos, tiendas y etiquetas..."=>"Search products, shops and tags...",
        "Limpiar"=>"Clear",
        "Fuente japonesa del sistema no encontrada: instala fuentes japonesas para mostrar títulos BOOTH."=>"Japanese system font not found. Install Japanese fonts to display BOOTH titles.",
        "Biblioteca"=>"Library",
        "BIBLIOTECA"=>"LIBRARY",
        "Vista cargada desde SQLite (archivo sin cambios)"=>"Loaded from SQLite (file unchanged)",
        "Vista reutilizada: hash BLAKE3 idéntico"=>"Reused cached view: matching BLAKE3 hash",
        "Archivo examinado y vista guardada en SQLite"=>"Archive inspected and view saved to SQLite",
        "Archivo reexaminado y caché actualizada"=>"Archive rescanned and cache updated",
        "Archivo examinado; caché no disponible para esta vista"=>"Archive inspected; cache unavailable for this view",
        "Todos los productos"=>"All products",
        "Solo favoritos"=>"Favorites only",
        "CONTENIDO"=>"CONTENT",
        "Mostrar NSFW"=>"Show NSFW",
        "Alternar visibilidad de las fichas marcadas como NSFW. Se ocultan por defecto."=>"Toggle NSFW-marked products. Hidden by default.",
        "Avatar"=>"Avatar",
        "Ropa"=>"Clothing",
        "Accesorios"=>"Accessories",
        "Pelo"=>"Hair",
        "Texturas"=>"Textures",
        "Animaciones"=>"Animations",
        "Mundos"=>"Worlds",
        "Herramientas"=>"Tools",
        "Otros"=>"Other",
        "AVATAR COMPATIBLE"=>"COMPATIBLE AVATAR",
        "Todos los avatares"=>"All avatars",
        "Añade avatares en la ficha de un producto."=>"Add compatible avatars in a product's details.",
        "ARCHIVOS"=>"FILES",
        "Todos"=>"All",
        "Con archivos"=>"With files",
        "Sin archivos"=>"Without files",
        "Limpiar filtros"=>"Clear filters",
        "IMPORTACIÓN"=>"IMPORT",
        "Arrastra archivos ZIP, 7z, RAR, UnityPackage o carpetas para vincularlos."=>"Drop ZIP, 7z, RAR, UnityPackage files or folders to attach them.",
        "Cambiar portada local"=>"Change local cover",
        "Abrir producto en BOOTH"=>"Open product on BOOTH",
        "Abrir descarga en Workupload"=>"Open Workupload download",
        "PORTADA"=>"COVER",
        "INFORMACIÓN"=>"INFORMATION",
        "Elegir portada local"=>"Choose local cover",
        "Si BOOTH no proporciona una imagen, puedes elegir una portada aquí."=>"If BOOTH does not provide an image, choose a local cover here.",
        "Ficha del producto"=>"Product details",
        "Guardar cambios"=>"Save changes",
        "Actualizar BOOTH"=>"Refresh from BOOTH",
        "Producto en tu biblioteca  /  {}"=>"Product in your library / {}",
        "Archivos vinculados"=>"Linked files",
        "Archivos vinculados ({})"=>"Linked files ({})",
        "Los archivos nuevos se copian y organizan en el almacén configurado. Los archivos antiguos conservan sus referencias hasta reorganizarlos."=>"New files are copied into managed storage. Existing references remain unchanged until organized.",
        "Añadir archivos"=>"Add files",
        "Importar carpeta"=>"Import folder",
        "Enviar los originales a la papelera tras copiar y verificar"=>"Move originals to Trash after copying and verifying",
        "Esta preferencia se recuerda entre sesiones. Siempre se pedirá confirmación antes de mover los originales."=>"This preference is saved. You will always be asked before moving original files.",
        "No hay archivos asociados. También puedes arrastrarlos a esta página."=>"No files attached. You can also drop files onto this page.",
        "Ruta no encontrada"=>"Path not found",
        "Copia gestionada"=>"Managed copy",
        "Ruta vinculada"=>"Linked path",
        "Mostrar archivo"=>"Show file",
        "Abrir carpeta"=>"Open folder",
        "Ocultar contenido"=>"Hide contents",
        "Examinar archivo"=>"Inspect archive",
        "Abrir en Unity"=>"Open in Unity",
        "Examinando ZIP, 7z, RAR y ZIP internos..."=>"Inspecting ZIP, 7z, RAR, and nested ZIP files...",
        "La vista se guarda en SQLite y se reutiliza si el archivo no cambia."=>"Inspection is cached in SQLite and reused until the file changes.",
        "Reexaminar archivo"=>"Rescan archive",
        "Recalcula BLAKE3 y vuelve a inspeccionar el archivo y sus ZIP internos."=>"Recalculate BLAKE3 and inspect the archive and nested ZIPs again.",
        "Los paquetes Unity de los ZIP omitidos no se han contado. Puedes examinarlos por separado."=>"Unity packages in skipped ZIPs were not counted. Inspect them separately.",
        "No se encontraron paquetes en los ZIP que pudieron examinarse."=>"No packages were found in the ZIP files that could be inspected.",
        "No se ha encontrado ningún .unitypackage, incluidos los ZIP internos examinados."=>"No .unitypackage was found, including inspected nested ZIPs.",
        "Extrae solamente el paquete elegido a la caché y lo abre con la aplicación asociada."=>"Extract only the selected package into the cache and open it with its associated app.",
        "Contenido por ZIP: pulsa la flecha a la izquierda de cada ZIP interno para desplegar sus archivos."=>"ZIP contents: expand nested ZIPs using the arrow on their left.",
        "No hay vista previa disponible."=>"No preview available.",
        "Los paquetes extraídos para Unity se guardan en una caché separada. Los archivos vinculados no cambian."=>"Extracted Unity packages are kept in a separate cache. Linked archives stay unchanged.",
        "Vaciar caché Unity"=>"Clear Unity cache",
        "Zona de mantenimiento"=>"Maintenance",
        "Eliminar una ficha no borra los archivos de tu ordenador."=>"Removing a catalog entry does not delete files from your computer.",
        "Eliminar ficha del catálogo"=>"Remove product from catalog",
        "Eliminar del catálogo"=>"Remove from catalog",
        "Eliminar ficha"=>"Remove entry",
        "El producto ya no existe"=>"The product no longer exists",
        "Eliminar producto del catálogo"=>"Delete product from catalog",
        "Se eliminará la ficha del catálogo. Los archivos ZIP, 7z, RAR y UnityPackage originales permanecerán en el disco."=>"The catalog entry will be removed. Original ZIP, 7z, RAR, and UnityPackage files will stay on disk.",
        "Vaciar caché"=>"Clear cache",
        "Paquetes Unity extraídos"=>"Extracted Unity packages",
        "Se eliminarán los archivos temporales de Unity. No se borran los ZIP, 7z, RAR ni UnityPackages originales y se podrán extraer de nuevo."=>"Temporary Unity packages will be deleted. Original ZIP, 7z, RAR, and UnityPackage archives remain untouched.",
        "Quitar de favoritos"=>"Remove from favorites",
        "Añadir a favoritos"=>"Add to favorites",
        "Abrir ficha"=>"Open details",
        "Abrir"=>"Open",
        "Tu biblioteca"=>"Your library",
        "productos"=>"products",
        "archivo"=>"file",
        "archivos"=>"files",
        "vinculados"=>"linked",
        "Tamaño de archivos vinculados a los productos mostrados. Cada ruta se cuenta una sola vez; no incluye portadas ni cachés."=>"Size of files linked to shown products. Each path is counted once; excludes covers and caches.",
        "Cuadrícula"=>"Grid",
        "Lista"=>"List",
        "Vincular carpeta"=>"Link folder",
        "Quitar filtro"=>"Remove filter",
        "Empieza con tu primer producto"=>"Start with your first product",
        "No hay resultados"=>"No results",
        "Pega una URL de BOOTH, recupera su portada y asocia tus ZIP en una sola ficha."=>"Paste a BOOTH URL, fetch its cover, and attach files to one product.",
        "Prueba otra categoría, quita el filtro de ZIP o borra la búsqueda."=>"Try another category, remove the archive filter, or clear the search.",
        "Crear primer producto"=>"Create first product",
        "Cargar más productos"=>"Load more products",
        "Volver a biblioteca"=>"Back to library",
        "Configuración de biblioteca"=>"Library settings",
        "Configuración inicial obligatoria"=>"Initial setup required",
        "Antes de añadir productos o archivos, elige una carpeta para guardar tus assets organizados. Esta ruta se conserva en SQLite."=>"Before adding products or files, select a folder for your organized assets. The path is saved in SQLite.",
        "Carpeta principal de assets"=>"Main asset folder",
        "Elegir carpeta"=>"Choose folder",
        "Guardar configuración"=>"Save settings",
        "Estructura: carpeta elegida / categoría / ID y nombre del producto / archivos ZIP, 7z, RAR y UnityPackage."=>"Structure: chosen folder / category / product ID and name / ZIP, 7z, RAR, UnityPackage files.",
        "Al cambiar la carpeta principal, los archivos ya gestionados mantienen sus rutas hasta que uses Organizar existentes."=>"Changing the main folder does not move existing managed files until you organize them.",
        "Paleta de colores"=>"Color palette",
        "Cambia la apariencia de toda la app. Tu elección se guarda para el siguiente inicio."=>"Change the app's appearance. Your choice is saved for the next launch.",
        "Ónix"=>"Onyx",
        "Jade"=>"Jade",
        "Amatista"=>"Amethyst",
        "Ámbar"=>"Amber",
        "Copia de seguridad de la biblioteca"=>"Library backup",
        "Exporta o importa SQLite y las portadas locales. Los archivos originales no se incluyen: se conservan como archivos externos en sus carpetas."=>"Export or import SQLite and local covers. Original archives are excluded and remain in their folders.",
        "Exportar biblioteca (.szkura)"=>"Export library (.szkura)",
        "Importar biblioteca"=>"Import library",
        "Antes de importar, SZKura guarda automáticamente una copia de recuperación de tu SQLite actual."=>"Before importing, SZKura automatically backs up your current SQLite database.",
        "Versión y actualizaciones"=>"Version and updates",
        "Ver versiones oficiales"=>"View official releases",
        "Comprobar actualizaciones al iniciar la app"=>"Check for updates at startup",
        "Instalar automáticamente las versiones nuevas (Windows)"=>"Automatically install new versions (Windows)",
        "La instalación automática está desactivada por defecto. Siempre verifica el SHA-256 publicado por GitHub; no instala archivos sin digest."=>"Automatic installation is off by default. SHA-256 from GitHub is always verified; unsigned artifacts are never installed.",
        "Comprobar en GitHub"=>"Check GitHub",
        "Actualizar automáticamente"=>"Install update",
        "Instalación automática no disponible en este sistema o falta SHA-256"=>"Automatic updates are unavailable on this system or SHA-256 is missing",
        "Consultando GitHub Releases..."=>"Checking GitHub Releases...",
        "Organizar archivos antiguos"=>"Organize existing files",
        "Copia al almacén actual los ZIP vinculados de versiones anteriores, verifica sus hashes y actualiza sus rutas. No borra los originales."=>"Copy previously linked ZIP files into managed storage, verify hashes, and update links. Originals are preserved.",
        "Organizar biblioteca existente"=>"Organize existing library",
        "Restaurar biblioteca"=>"Restore library",
        "Importar y reemplazar"=>"Import and replace",
        "La copia reemplazará las fichas de la biblioteca actual. Antes se comprobará su integridad y se guardará una copia recuperable de SQLite."=>"The backup replaces your current catalog. It will be checked first and a recoverable SQLite backup will be saved.",
        "Los archivos ZIP, 7z, RAR y UnityPackages originales no se borrarán."=>"Original ZIP, 7z, RAR, and UnityPackage files will not be deleted.",
        "Gestionar originales"=>"Manage originals",
        "Copiar y enviar a papelera"=>"Copy and move to Trash",
        "Se copiarán al almacén configurado. Solo tras comprobar el hash, guardar la ficha y verificar que ningún otro producto los referencia, los originales pasarán a la papelera recuperable."=>"The files will be copied to managed storage. Originals are moved to recoverable Trash only after verification, catalog save, and reference checks.",
        "Guardar producto"=>"Save product",
        "Pega un enlace BOOTH para recuperar sus datos públicos al guardar, o crea una ficha manual con tus ZIP."=>"Paste a BOOTH link to fetch public details when saving, or create a product manually.",
        "Al guardar se copiarán archivos ZIP, 7z, RAR y UnityPackages a carpetas organizadas dentro del almacén configurado. No se extraen."=>"When saving, ZIP, 7z, RAR, and UnityPackage archives are copied into organized managed storage; they are not extracted.",
        "Seleccionar archivos"=>"Select files",
        "Seleccionar carpeta"=>"Select folder",
        "Enviar originales a la papelera tras copiar y verificar (opcional)"=>"Move originals to Trash after copying and verifying (optional)",
        "Se solicitará confirmación. Si la copia falla o el archivo original está vinculado en otra ficha, se conserva."=>"Confirmation is required. Originals are preserved if copying fails or another product references them.",
        "También puedes arrastrar archivos a cualquier punto de esta página."=>"You can also drop files anywhere on this page.",
        "Hay más rutas seleccionadas"=>"More paths selected",
        "Vaciar selección"=>"Clear selection",
        "Quitar"=>"Remove",
        "RESUMEN"=>"SUMMARY",
        "Marcar como NSFW (contenido sensible)"=>"Mark as NSFW (sensitive content)",
        "Categoría"=>"Category",
        "Enlace de BOOTH"=>"BOOTH URL",
        "Nombre del producto"=>"Product name",
        "Nombre visible en la biblioteca"=>"Name shown in the library",
        "Creador / tienda"=>"Creator / shop",
        "Nombre de tienda"=>"Shop name",
        "Avatares compatibles"=>"Compatible avatars",
        "Se guardan en MAYÚSCULAS y sin nombres repetidos. Sepáralos con comas."=>"Names are saved in UPPERCASE without duplicates. Separate them with commas.",
        "Otras etiquetas"=>"Other tags",
        "Color, estilo, temática..."=>"Color, style, theme...",
        "Los productos marcados se ocultan del catálogo y de la búsqueda, salvo que actives «Mostrar NSFW»."=>"Marked products are hidden from the catalog and search unless you turn on Show NSFW.",
        "Descripción"=>"Description",
        "Mostrar menos"=>"Show less",
        "Añadir descripción"=>"Add description",
        "Mostrar más / editar"=>"Show more / edit",
        "Descripción y notas del producto"=>"Product description and notes",
        "Sin descripción"=>"No description",
        "Sin portada"=>"No cover",
        "No se pudo leer la portada"=>"Could not read cover",
        "Imagen mayor de 12 MB"=>"Image larger than 12 MB",
        "Imagen no válida"=>"Invalid image",
        "Dimensiones de imagen no admitidas"=>"Image dimensions unsupported",
        "Extraer y abrir"=>"Extract and open",
        "Ver carpetas en Unity"=>"View Unity folders",
        "Ocultar carpetas en Unity"=>"Hide Unity folders",
        "Exportar copia etiquetada..."=>"Export labeled copy...",
        "Etiqueta de la categoría guardada"=>"Saved category label",
        "Experimental: genera una copia; no modifica originales ni carpetas de Unity."=>"Experimental: creates a copy; originals and Unity folders stay unchanged.",
        "Conserva GUIDs, rutas y demás etiquetas; importar manualmente en Unity."=>"Keeps GUIDs, paths, and existing labels; import manually into Unity.",
        "Copia etiquetada creada"=>"Labeled copy created",
        "Al abrir, se añade automáticamente la etiqueta SZKura en una copia de caché."=>"Opening automatically adds the SZKura label to a cached copy.",
        "Al abrir en Unity desde SZKura, esta etiqueta se aplica automáticamente a una copia de caché."=>"Opening in Unity from SZKura automatically applies this label to a cached copy.",
        "Categoría no válida para etiquetar"=>"Invalid category for labeling",
        "Carpetas de importación en Unity"=>"Unity import folders",
        "Carpetas principales"=>"Main folders",
        "Carpetas exactas"=>"Exact folders",
        "Copiar ruta"=>"Copy path",
        "Rutas originales del paquete; pueden diferir del proyecto Unity actual."=>"Original package paths; they may differ from the current Unity project.",
        "Examinando rutas del UnityPackage..."=>"Inspecting UnityPackage paths...",
        "No contiene rutas válidas dentro de Assets/."=>"No valid paths inside Assets/ were found.",
        "No se pudieron leer las rutas del UnityPackage:"=>"Could not read UnityPackage paths:",
        "Archivos con ruta en Assets/"=>"Files with an Assets/ path",
        "Rutas fuera de Assets/ ignoradas"=>"Paths outside Assets/ ignored",
        "Este ZIP está vacío o no contiene archivos seguros."=>"This ZIP is empty or has no safe files.",
        "Cancelar"=>"Cancel",
        "Ficha de ejemplo"=>"Example product",
        "Solo se quita del catálogo y sus archivos se conservan."=>"Only the catalog entry is removed; files are kept.",
        "Error"=>"Error",
        "Error de filtros"=>"Filter error",
        "Copia de SZKura"=>"SZKura backup",
        "Imagen"=>"Image",
        "Colección de archivos"=>"Archive collection",
        "Mostrar contenido"=>"Show contents",
        "No existe un archivo .unitypackage válido en esa ruta"=>"No valid .unitypackage exists at this path",
        "SZKura ya está actualizado"=>"SZKura is already up to date",
        "Biblioteca lista. Cada producto puede tener varios archivos vinculados."=>"Library ready. Each product may have multiple linked files.",
        "Comprobando nuevas versiones en GitHub Releases..."=>"Checking GitHub Releases for updates...",
        "Recalculando el hash y examinando el archivo de nuevo..."=>"Recomputing hash and inspecting archive again...",
        "Buscando vista en SQLite; se examinará el archivo si es necesario..."=>"Looking up the inspection in SQLite; scanning if needed...",
        "La biblioteca está procesando otra operación"=>"The library is busy with another operation",
        "Antes de importar elige la carpeta de almacenamiento"=>"Choose a storage folder before importing",
        "Configura primero la carpeta de almacenamiento"=>"Configure the storage folder first",
        "Primero configura el almacén de assets"=>"Configure managed asset storage first",
        "No se pudo guardar la preferencia de originales: {error}"=>"Couldn't save original-file preference: {error}",
        "ZIP, 7z, RAR y UnityPackages"=>"ZIP, 7z, RAR and UnityPackages",
        "ZIP, 7z, RAR y UnityPackage"=>"ZIP, 7z, RAR and UnityPackage",
        "Configuración de idioma"=>"Language settings",
        "Idioma"=>"Language",
        "Español"=>"Spanish",
        "Inglés"=>"English",
        "Elige el idioma de la interfaz. Se guarda para futuros inicios."=>"Choose the interface language. This preference is saved for future launches.",
        "¿Quieres otro idioma? Abre una Issue en GitHub y solicita una traducción."=>"Want another language? Open a GitHub Issue to request a translation.",
        "Solicitar otro idioma"=>"Request another language",
        "No se pudo guardar el idioma. Inténtalo de nuevo."=>"Couldn't save the language. Please try again.",
        "Idioma aplicado"=>"Language changed",
        "Producto en tu biblioteca"=>"Product in your library",
        "Versión instalada:"=>"Installed version:",
        "rutas preparadas"=>"paths selected",
        "Disponible"=>"Available",
        "Desvincular"=>"Unlink",
        "no examinado"=>"not inspected",
        "entradas"=>"entries",
        "Entradas exteriores"=>"Outer entries",
        "Entradas interiores"=>"Nested entries",
        "Paquetes Unity"=>"Unity packages",
        "ZIP internos"=>"Nested ZIPs",
        "Contenido no disponible"=>"Content unavailable",
        "Procesando en segundo plano..."=>"Processing in the background...",
        "La actualización automática solo está disponible para Windows y con SHA-256 oficial"=>"Automatic installation is only available on Windows with an official SHA-256.",
        "Versión nueva instalada y reiniciada correctamente"=>"New version installed and restarted successfully.",
        "Introduce un enlace BOOTH o un nombre para el producto"=>"Enter a BOOTH URL or product name.",
        "Arrastra ZIP, 7z, RAR, UnityPackages o carpetas para vincularlos"=>"Drop ZIP, 7z, RAR, UnityPackage files or folders to attach them.",
        "Solo se pueden vincular ZIP, 7z, RAR, UnityPackages o carpetas"=>"Only ZIP, 7z, RAR, UnityPackage files or folders can be attached.",
        "Ubicación solicitada al explorador"=>"Requested file location in the file manager.",
        "Carpeta solicitada al explorador"=>"Requested folder in the file manager.",
        "Compartir temporalmente (opcional)"=>"Temporary sharing (optional)",
        "Transferencia HTTPS entre equipos. Cloudflare Tunnel no se descarga al instalar ni al abrir SZKura."=>"HTTPS transfers between computers. Cloudflare Tunnel is not downloaded when installing or launching SZKura.",
        "Habilitar compartir archivos"=>"Enable file sharing",
        "Espera a que termine la transferencia para cambiar esta opción."=>"Finish the transfer before changing this setting.",
        "Componente opcional: instalado (se comprueba SHA-256 antes de ejecutarlo)."=>"Optional component: installed (SHA-256 is checked before running it).",
        "Componente opcional: no descargado. Se solicitará permiso en el primer uso."=>"Optional component: not downloaded. Permission will be requested on first use.",
        "Eliminar Cloudflare Tunnel descargado"=>"Remove downloaded Cloudflare Tunnel",
        "Componente eliminado. Se volverá a solicitar permiso para descargarlo."=>"Component removed. Permission will be requested before downloading it again.",
        "Descargando y verificando Cloudflare Tunnel..."=>"Downloading and verifying Cloudflare Tunnel...",
        "Creando enlace temporal..."=>"Creating temporary link...",
        "Comprobando el manifiesto del enlace..."=>"Checking the link manifest...",
        "Descargando, verificando e importando la ficha..."=>"Downloading, verifying, and importing the product...",
        "Enlace activo"=>"Active link",
        "Copiar enlace"=>"Copy link",
        "Enlace copiado al portapapeles."=>"Link copied to clipboard.",
        "Dejar de compartir"=>"Stop sharing",
        "Compartiendo ahora:"=>"Currently sharing:",
        "Ver ficha"=>"View product",
        "Compartir ficha"=>"Share product",
        "Enlace revocado."=>"Link revoked.",
        "Enlace creado. Puedes copiarlo desde la cabecera de la ficha."=>"Link created. You can copy it from the product header.",
        "Enlace cancelado."=>"Link canceled.",
        "Compartir archivos desactivado."=>"File sharing disabled.",
        "Enlace no válido"=>"Invalid link",
        "Solo se admiten enlaces HTTPS de SZKura en trycloudflare.com"=>"Only SZKura HTTPS links on trycloudflare.com are accepted",
        "El enlace no tiene el formato de transferencia SZKura"=>"This link is not a valid SZKura transfer link",
        "El enlace requiere una clave de transferencia SZKura válida"=>"This link requires a valid SZKura transfer key",
        "Solo para importar en SZKura; no hay descarga desde el navegador."=>"Import via SZKura only; browser downloads are unavailable.",
        "Ficha comprobada. Revisa antes de importar."=>"Product checked. Review before importing.",
        "Ficha y archivos importados correctamente."=>"Product and files imported successfully.",
        "El enlace ha caducado y el túnel se ha detenido."=>"The link expired and the tunnel was stopped.",
        "Compartir ficha temporalmente"=>"Temporarily share product",
        "Se compartirán esta ficha y todos sus archivos vinculados. Comparte solo archivos cuya licencia permita redistribuirlos."=>"This product and all its linked files will be shared. Only share files you are licensed to redistribute.",
        "Caducidad:"=>"Expires after:",
        "min"=>"min",
        "Descargas por archivo:"=>"Downloads per file:",
        "Sin límite"=>"Unlimited",
        "Contraseña adicional (opcional, se entrega por separado):"=>"Extra password (optional; share separately):",
        "Crear enlace temporal"=>"Create temporary link",
        "Descargar Cloudflare Tunnel"=>"Download Cloudflare Tunnel",
        "Descargar y compartir"=>"Download and share",
        "Descarga opcional desde los lanzamientos oficiales de Cloudflare en GitHub."=>"Optional download from Cloudflare's official GitHub releases.",
        "SZKura verificará SHA-256, guardará el ejecutable localmente y no necesitará permisos de administrador."=>"SZKura will verify SHA-256 and store the executable locally without administrator privileges.",
        "Sin aceptar no se descarga nada. Puedes eliminar el componente desde Configuración."=>"Nothing is downloaded without your consent. You can remove the component in Settings.",
        "Importar ficha desde enlace SZKura"=>"Import product from SZKura link",
        "Pega el enlace HTTPS que te ha enviado otra instalación de SZKura:"=>"Paste the HTTPS link shared by another SZKura installation:",
        "Enlace HTTPS de SZKura"=>"SZKura HTTPS link",
        "Contraseña, si el remitente estableció una:"=>"Password, if the sender set one:",
        "Creador"=>"Creator",
        "Avatares"=>"Avatars",
        "bytes"=>"bytes",
        "Si la ficha ya existe, se conservarán sus metadatos locales. Los archivos recibidos no se ejecutarán."=>"If the product already exists, its local metadata is preserved. Received files will not be executed.",
        "Continuar con la importación"=>"Continue to import",
        "Examinar enlace"=>"Preview link",
        "Confirmar importación"=>"Confirm import",
        "Importar ficha y archivos"=>"Import product and files",
        "Se descargarán y verificarán los archivos antes de añadirlos a tu biblioteca."=>"Files will be downloaded and verified before being added to your library.",
        "Las fichas BOOTH existentes conservarán sus metadatos. No se ejecutarán los archivos."=>"Existing BOOTH products will retain their metadata. Files will not be executed.",
        "Importar enlace SZKura"=>"Import SZKura link",
        "Ficha transferida e importada correctamente"=>"Transferred product imported successfully.",
        _=>s,
    }
}

/// Translate transient status and dialog messages that embed dynamic details.
/// Longer prefixes are matched first. This does not touch user-entered product
/// titles, paths, stored categories, or archive filenames.
pub fn translate_message(language:Language, message:&str)->std::borrow::Cow<'_,str> {
    use std::borrow::Cow;
    if language==Language::Spanish{return Cow::Borrowed(message);}
    let direct=translate(language,message);
    if direct!=message{return Cow::Borrowed(direct);}
    // Archive cache statuses are stored in Spanish so changing language also
    // retranslates the most recent inspection without touching SQLite.
    if let Some((label,counts))=message.split_once(" | ") {
        if ARCHIVE_CACHE_LABELS.contains(&label) {
            if let Some(translated)=translate_archive_counts(counts) {
                return Cow::Owned(format!("{} | {translated}",translate(language,label)));
            }
        }
    }
    for &(es,en) in MESSAGE_PREFIXES {
        if let Some(tail)=message.strip_prefix(es) {
            // Translate a known nested error too, without changing paths or
            // user-supplied text that lacks a translation key.
            return Cow::Owned(format!("{en}{}", translate(language,tail)));
        }
    }
    Cow::Borrowed(message)
}
const ARCHIVE_CACHE_LABELS:&[&str]=&[
    "Vista cargada desde SQLite (archivo sin cambios)",
    "Vista reutilizada: hash BLAKE3 idéntico",
    "Archivo examinado y vista guardada en SQLite",
    "Archivo reexaminado y caché actualizada",
    "Archivo examinado; caché no disponible para esta vista",
];

/// Convert the known inspector status layout while leaving counts and other
/// unrecognized (possibly user-supplied) strings untouched.
fn translate_archive_counts(message:&str)->Option<String>{
    let (outer,rest)=message.split_once(" entradas exteriores, ")?;
    let (inner,rest)=rest.split_once(" interiores, ")?;
    let (nested,rest)=rest.split_once(" ZIP internos, ")?;
    let (packages,remainder)=rest.split_once(" paquetes Unity")?;
    let counts=[outer,inner,nested,packages];
    let parsed:Vec<usize>=counts.iter()
        .map(|s|s.parse::<usize>()).collect::<Result<_,_>>().ok()?;
    let trailing=if remainder.is_empty(){
        String::new()
    }else{
        let hidden=remainder.strip_prefix(" | ")?
            .strip_suffix(" ZIP internos omitidos por seguridad o error")?;
        let amount=hidden.parse::<usize>().ok()?;
        format!(" | {amount} nested ZIPs skipped for safety or errors")
    };
    Some(format!(
        "{} outer {}, {} nested {}, {} nested ZIPs, {} Unity {}{}",
        parsed[0],if parsed[0]==1{"entry"}else{"entries"},
        parsed[1],if parsed[1]==1{"entry"}else{"entries"},
        parsed[2],parsed[3],if parsed[3]==1{"package"}else{"packages"},trailing
    ))
}
/// Error messages produced by the read-only UnityPackage pathname inspector.
/// Match whole error-chain segments, not substrings: filenames, local paths and
/// raw OS diagnostics must never be modified by the UI translator.
const UNITYPACKAGE_READER_ERRORS:&[(&str,&str)]=&[
    ("No se pudo abrir el UnityPackage","Could not open UnityPackage"),
    ("No es un archivo UnityPackage regular","Not a regular UnityPackage file"),
    ("El UnityPackage supera el límite de inspección de 2 GiB","UnityPackage exceeds the 2 GiB inspection limit"),
    ("No se pudo leer el TAR del UnityPackage","Could not read the UnityPackage TAR"),
    ("Entrada TAR no válida en el UnityPackage","Invalid TAR entry in UnityPackage"),
    ("Ruta TAR no válida","Invalid TAR path"),
    ("El UnityPackage contiene demasiadas entradas","UnityPackage contains too many entries"),
    ("Un pathname de UnityPackage excede el límite de 16 KiB","A UnityPackage pathname exceeds the 16 KiB limit"),
    ("No se pudo leer un pathname","Could not read a pathname"),
    ("Un pathname no es UTF-8 válido","A pathname is not valid UTF-8"),
    ("El UnityPackage contiene demasiados pathnames","UnityPackage contains too many pathnames"),
    ("El UnityPackage contiene demasiadas carpetas distintas","UnityPackage contains too many distinct folders"),
];

/// Localize only known UnityPackage error-chain messages, preserving paths and
/// any diagnostic details from the OS, TAR/GZIP libraries or nested archives.
pub fn translate_unitypackage_error(language:Language,message:&str)->String {
    if language==Language::Spanish {return message.to_owned();}
    message.split(": ").map(|part| {
        UNITYPACKAGE_READER_ERRORS.iter()
            .find_map(|(spanish,english)|(*spanish==part).then_some(*english))
            .unwrap_or(part)
    }).collect::<Vec<_>>().join(": ")
}
pub fn tr_unitypackage_error(message:&str)->String {
    translate_unitypackage_error(selected(),message)
}

pub fn tr_message(message:&str)->std::borrow::Cow<'_,str>{
    translate_message(selected(),message)
}
const MESSAGE_PREFIXES:&[(&str,&str)]=&[
    ("Error al compartir: ","Sharing error: "),
    ("Error al abrir enlace: ","Could not open link: "),
    ("Error al importar: ","Could not import: "),
    ("Copia etiquetada enviada a Unity: ","Labeled copy sent to Unity: "),
    ("No se abrió Unity; importa manualmente la copia etiquetada: ","Unity did not open; manually import the labeled copy: "),
    ("No se pudo guardar la preferencia: ","Could not save preference: "),
    ("No se pudo eliminar el componente: ","Could not remove component: "),
    ("Error en transferencia: ","Transfer error: "),
    ("Error reabriendo la BD restaurada: ","Error reopening restored database: "),
    ("Error al desvincular: ","Could not unlink file: "),
    ("Ficha eliminada, pero falló la actualización del catálogo: ","Entry removed, but reloading the catalog failed: "),
    ("Archivo enviado a la aplicación asociada a .unitypackage","Package sent to the associated application"),
    ("Enlace guardado; BOOTH no ha devuelto datos (","Link saved; BOOTH returned no data ("),
    ("No se pudieron comprobar actualizaciones: ","Could not check for updates: "),
    ("Datos y tienda actualizados desde BOOTH: ","Product and shop updated from BOOTH: "),
    ("Referencia eliminada sin tocar el archivo","Unlinked without deleting the file"),
    ("No se pudo instalar la actualización: ","Could not install update: "),
    ("No se pudieron obtener los datos BOOTH","Could not fetch BOOTH information"),
    ("No se pudo actualizar la biblioteca","Could not update library"),
    ("Error actualizando la biblioteca: ","Library update error: "),
    ("Enviado a la aplicación asociada: ","Sent to associated application: "),
    ("No se pudo actualizar favoritos: ","Could not update favorites: "),
    ("No se pudo examinar el archivo: ","Could not inspect archive: "),
    ("No se pudo mostrar el archivo: ","Could not show file: "),
    ("No se pudo vaciar caché Unity: ","Could not clear Unity cache: "),
    ("La copia reemplazará las fichas","The backup will replace catalog entries"),
    ("Producto en tu biblioteca  /  ","Product in your library / "),
    ("No se pudo eliminar la ficha: ","Could not delete entry: "),
    ("Error actualizando favoritos: ","Could not update favorites: "),
    ("No se pudo abrir la carpeta: ","Could not open folder: "),
    ("No se pudo abrir en Unity (","Could not open in Unity ("),
    ("No se guardó preferencia: ","Could not save preference: "),
    ("Error guardando portada: ","Could not save cover: "),
    ("Error en configuración: ","Settings error: "),
    ("No se guardó la paleta: ","Could not save palette: "),
    ("Se eliminó la referencia","Reference removed"),
    ("Biblioteca exportada a ","Library exported to "),
    ("Versión de desarrollo: ","Development version: "),
    ("Descarga verificada: ","Verified download: "),
    ("Caché Unity vaciada: ","Unity cache cleared: "),
    ("Almacén configurado: ","Storage configured: "),
    ("Archivos preparados: ","Files prepared: "),
    ("Producto guardado | ","Product saved | "),
    ("Producto actualizado","Product updated"),
    ("No se pudo guardar: ","Could not save: "),
    ("Quitado de favoritos","Removed from favorites"),
    ("Versión instalada: ","Installed version: "),
    ("Añadido a favoritos","Added to favorites"),
    ("Producto guardado.","Product saved."),
    ("Error de lectura: ","Read error: "),
    ("Error de filtros: ","Filter error: "),
    ("Ficha eliminada: ","Catalog entry removed: "),
    ("Paleta aplicada: ","Palette changed: "),
    ("Copia importada: ","Backup imported: "),
    ("Portada guardada","Cover saved"),
    ("Nueva versión ","New version "),
    ("Organización: ","Organization: "),
    ("Disponible: ","Available: "),
    ("Descargando ","Downloading "),
    ("Error: ","Error: "),
];

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn language_identifiers_are_stable(){
        assert_eq!(Language::from_id("es"),Some(Language::Spanish));
        assert_eq!(Language::from_id("en"),Some(Language::English));
        assert_eq!(Language::from_id("xx"),None);
    }
    #[test]
    fn transfer_ui_is_translated_in_english_and_spanish() {
        for (spanish,english) in [
            ("Importar enlace SZKura","Import SZKura link"),
            ("Compartir ficha","Share product"),
            ("Copiar enlace","Copy link"),
            ("Descargar y compartir","Download and share"),
            ("Confirmar importación","Confirm import"),
            ("Habilitar compartir archivos","Enable file sharing"),
            ("Crear enlace temporal","Create temporary link"),
            ("Dejar de compartir","Stop sharing"),
            ("Compartiendo ahora:","Currently sharing:"),
            ("Enlace activo","Active link"),
            ("Enlace cancelado.","Link canceled."),
            ("Compartir archivos desactivado.","File sharing disabled."),
            ("Ver ficha","View product"),
        ] {
            assert_eq!(translate(Language::Spanish,spanish),spanish);
            assert_eq!(translate(Language::English,spanish),english);
        }
        assert_eq!(
            translate_message(Language::English,"Error al compartir: conexión fallida"),
            "Sharing error: conexión fallida"
        );
        assert_eq!(
            translate_message(Language::English,
                "Error al abrir enlace: El enlace requiere una clave de transferencia SZKura válida"),
            "Could not open link: This link requires a valid SZKura transfer key"
        );
    }
    #[test]
    fn unity_package_path_preview_is_translated() {
        for (es, en) in [
            ("Ver carpetas en Unity", "View Unity folders"),
            ("Ocultar carpetas en Unity", "Hide Unity folders"),
            ("Exportar copia etiquetada...", "Export labeled copy..."),
            ("Etiqueta de la categoría guardada", "Saved category label"),
            ("Copia etiquetada creada", "Labeled copy created"),
            ("Al abrir, se añade automáticamente la etiqueta SZKura en una copia de caché.", "Opening automatically adds the SZKura label to a cached copy."),
            ("Al abrir en Unity desde SZKura, esta etiqueta se aplica automáticamente a una copia de caché.", "Opening in Unity from SZKura automatically applies this label to a cached copy."),
            ("Carpetas de importación en Unity", "Unity import folders"),
            ("Carpetas principales", "Main folders"),
            ("Carpetas exactas", "Exact folders"),
            ("Copiar ruta", "Copy path"),
            ("Examinando rutas del UnityPackage...", "Inspecting UnityPackage paths..."),
            ("No contiene rutas válidas dentro de Assets/.", "No valid paths inside Assets/ were found."),
        ] {
            assert_eq!(translate(Language::Spanish, es), es);
            assert_eq!(translate(Language::English, es), en);
        }
    }
    #[test]
    fn automatic_labeled_open_status_is_translated() {
        let status="Copia etiquetada enviada a Unity: SZKura_Ropa";
        assert_eq!(translate_message(Language::Spanish,status),status);
        assert_eq!(translate_message(Language::English,status),
            "Labeled copy sent to Unity: SZKura_Ropa");
        let fallback="No se abrió Unity; importa manualmente la copia etiquetada: /tmp/example.unitypackage";
        assert_eq!(translate_message(Language::English,fallback),
            "Unity did not open; manually import the labeled copy: /tmp/example.unitypackage");
    }
    #[test]
    fn unity_package_reader_errors_are_localized_in_both_languages() {
        for &(spanish,english) in UNITYPACKAGE_READER_ERRORS {
            assert_eq!(translate_unitypackage_error(Language::Spanish,spanish),spanish);
            assert_eq!(translate_unitypackage_error(Language::English,spanish),english);
        }
        let nested="No se pudo leer el TAR del UnityPackage: Entrada TAR no válida en el UnityPackage: invalid gzip header";
        assert_eq!(
            translate_unitypackage_error(Language::English,nested),
            "Could not read the UnityPackage TAR: Invalid TAR entry in UnityPackage: invalid gzip header"
        );
        // A path happens to contain a translated phrase: it is user data, not
        // an error-chain label, and must remain unchanged.
        let open_error="No se pudo abrir el UnityPackage: /home/user/No se pudo leer un pathname/asset.unitypackage: Permission denied";
        assert_eq!(
            translate_unitypackage_error(Language::English,open_error),
            "Could not open UnityPackage: /home/user/No se pudo leer un pathname/asset.unitypackage: Permission denied"
        );
        assert_eq!(translate_unitypackage_error(Language::English,"unknown failure"),"unknown failure");
    }

    #[test]
    fn navigation_and_settings_translate_without_changing_catalog_ids(){
        assert_eq!(translate(Language::English,"Todos los productos"),"All products");
        assert_eq!(translate(Language::English,"Ropa"),"Clothing");
        assert_eq!(translate(Language::Spanish,"Ropa"),"Ropa");
        assert_eq!(translate(Language::English,"Guardar configuración"),"Save settings");
        assert_eq!(translate(Language::English,"Mostrar NSFW"),"Show NSFW");
        assert_eq!(translate(Language::English,"Biblioteca"),"Library");
        assert_eq!(translate(Language::English,"Sin portada"),"No cover");
        assert_eq!(translate(Language::English,"Cambiar portada local"),"Change local cover");
        assert_eq!(translate(Language::English,"Elegir portada local"),"Choose local cover");
        let cached="Vista cargada desde SQLite (archivo sin cambios) | 1 entradas exteriores, 0 interiores, 0 ZIP internos, 1 paquetes Unity";
        assert_eq!(translate_message(Language::Spanish,cached),cached);
        assert_eq!(translate_message(Language::English,cached),
            "Loaded from SQLite (file unchanged) | 1 outer entry, 0 nested entries, 0 nested ZIPs, 1 Unity package");
        let skipped="Archivo reexaminado y caché actualizada | 2 entradas exteriores, 3 interiores, 4 ZIP internos, 5 paquetes Unity | 1 ZIP internos omitidos por seguridad o error";
        assert_eq!(translate_message(Language::English,skipped),
            "Archive rescanned and cache updated | 2 outer entries, 3 nested entries, 4 nested ZIPs, 5 Unity packages | 1 nested ZIPs skipped for safety or errors");
        let unknown="Vista cargada desde SQLite (archivo sin cambios) | untouched";
        assert_eq!(translate_message(Language::English,unknown),unknown);
        assert_eq!(translate(Language::English,"Procesando en segundo plano..."),"Processing in the background...");
        assert_eq!(translate_message(Language::English,"Error al desvincular: file missing"),"Could not unlink file: file missing");
        assert_eq!(translate_message(Language::English,"Versión instalada: v0.1.0-alpha.99"),"Installed version: v0.1.0-alpha.99");
    }
}
