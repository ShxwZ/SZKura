# Compartir biblioteca · revisión de catálogo y sincronización (experimental)

Función opcional de la [issue #6](https://github.com/ShxwZ/SZKura/issues/6). Se inicia **manualmente** desde la aplicación de escritorio y es independiente de «Compartir ficha» (véase [SHARING.md](SHARING.md)). La interfaz remota es nativa en SZKura: no se publica una web.

## Anfitrión: seleccionar el alcance

1. En **Configuración → Compartir biblioteca**, asigna un **nombre** reconocible a tu biblioteca.
2. Selecciona **Compartir salvo excluidas** (modo predeterminado) o **Compartir solo incluidas**. El primer modo permite marcar únicamente los productos que quieres ocultar; el segundo comienza con ninguno incluido. **No se publica nada hasta pulsar Iniciar compartir biblioteca.**
3. Por precaución, los productos marcados como contenido sensible están excluidos de ambos modos, salvo que actives expresamente «Permitir compartir también fichas NSFW».
4. Pulsa **Elegir fichas para compartir**. Abre una página de ancho completo, con tarjetas y portadas como la biblioteca local: búsqueda, cuadrícula/lista, categorías, avatares y filtros con/sin archivos. Hay opciones de **Solo excluidas** o **Solo seleccionadas**, según el modo. Puedes volver a Configuración sin perder la selección.
5. Al publicar una **ficha completa**, se ofrecen **todos sus adjuntos**. No existe un selector de archivos por separado. Antes de publicar, comprueba que los archivos están disponibles y que la licencia permite su redistribución.
6. Pulsa **Iniciar compartir biblioteca**. Primero se muestra una **confirmación explícita** con el número de fichas, el tipo de acceso (LAN o Internet) y la advertencia sobre los adjuntos y sus licencias. Al aceptar, SZKura crea una instantánea y calcula en segundo plano SHA-256 y BLAKE3 de los adjuntos. Puedes elegir **LAN privada** o **Compartir también por Internet (Cloudflare Tunnel)**. La instalación de cloudflared requiere consentimiento expreso y comprobación del SHA-256 oficial.
7. Copia la invitación privada. **Detener biblioteca compartida** en el encabezado o salir de SZKura revoca la sesión y su túnel.

Los productos ocultos son invisibles para el visitante: no se exponen sus títulos, recuentos, identificadores de SQLite, avatares, portadas ni archivos. Los cambios posteriores del anfitrión **no actualizan** una sesión ya iniciada; detén y reinicia la publicación para renovar su instantánea.

## Visitante: navegar y actualizar

1. En la barra lateral, **debajo de los filtros**, abre **BIBLIOTECAS REMOTAS → Conectar a biblioteca**.
2. Pega la invitación LAN o HTTPS del anfitrión. La barra superior y un banner del panel central muestran el **nombre de la biblioteca ajena**, con la indicación **Solo lectura**. La biblioteca local no se sustituye.
3. Navega con cuadrícula/lista, búsqueda, categorías, avatares, NSFW y filtros con/sin archivos. **No hay botones de página**: al desplazarte se van solicitando más resultados. La API continúa paginada internamente en lotes de 32 para controlar memoria y transferencia.
4. Cada tarjeta compara el identificador de origen o el artículo BOOTH y las firmas de adjuntos con tu SQLite local. Muestra **Nuevo**, **Ya en mi biblioteca** o **Archivos por actualizar**. En la ficha, SZKura comprueba además **los hashes reales de las copias locales en un worker**, no solo las firmas guardadas.
5. Pulsa **Importar ficha completa** o **Actualizar ficha completa**, según corresponda. Se muestra una confirmación que explica las descargas y que se conservarán tus metadatos y versiones anteriores.
6. SZKura descarga **solo** los archivos nuevos, distintos o ausentes, verifica tamaños y ambos hashes y revisa los contenedores ZIP/7z/RAR. El commit de producto y adjuntos ocurre después, en **una única transacción SQLite**. Si algo falla o se cancela antes del commit, no se crea una ficha importada a medias. **No hay opción de importar únicamente metadatos.**

### Identificar productos y archivos

- El anfitrión conserva una **identidad secreta local** que permite derivar una clave opaca estable por producto. En el cliente se registra la clave asociada a la ficha local, **sin guardar los tokens o claves de invitación**.
- Si todavía no existe un vínculo por origen, se utiliza el **ID canónico BOOTH** cuando está disponible. No se fusionan registros por parecido de nombre o ruta.
- El almacén local usa **BLAKE3** como checksum de adjuntos; la transferencia compara también **SHA-256**. Un archivo local supuestamente igual, pero ausente o modificado, se recupera y verifica.
- Si una versión nueva tiene el mismo nombre de archivo pero otro contenido, **se añade sin sobrescribir la versión anterior**. Los campos locales editados manualmente no se sustituyen por los del anfitrión. Las nuevas versiones de archivos no implican sincronización bidireccional.

## Seguridad

- Cada sesión usa un token aleatorio en la ruta y una clave de 256 bits en el fragmento `#key=...` de la invitación. El cliente envía la clave en `X-SZKura-Library-Key`. Todos los endpoints deben verificarla; la cabecera es una **credencial de portador**, no una prueba de identidad de la aplicación.
- LAN usa HTTP sin cifrar: **solo redes privadas de confianza**. El modo Internet usa HTTPS de Cloudflare Quick Tunnel, conectado al servidor de SZKura por `127.0.0.1`. Las URL temporales no garantizan disponibilidad a largo plazo.
- El catálogo solo admite peticiones GET autenticadas; no ofrece edición remota ni comparte rutas de archivos locales o SQLite. No abrir puertos del servidor LAN directamente a Internet.
- **Compartir ficha** y **Compartir biblioteca** son mutuamente excluyentes: no se deben iniciar dos túneles de SZKura en paralelo.
- Las licencias BOOTH deben permitir expresamente la redistribución de cualquier asset que quieras ofrecer.

## Límites y advertencias

- Hasta **2000 fichas** y **512 adjuntos** por sesión, **8 GiB por archivo** y **16 GiB** en total. Cuatro peticiones concurrentes del servidor y páginas HTTP de 32 productos. En bibliotecas grandes el hash inicial puede tardar.
- La UI limita y carga bajo demanda las portadas remotas visibles para evitar acumular texturas en memoria.
- Sin cuentas, reconexión persistente, sincronización bidireccional, borrado automático de versiones antiguas ni reanudación de descargas.
- Si el proceso se interrumpe abruptamente durante la incorporación física de archivos, pueden quedar copias huérfanas no enlazadas; no se sobrescriben originales. Para errores normales, se limpian archivos recién incorporados y se revierte la transacción SQLite.
- **La compilación automática no sustituye la revisión visual** ES/EN, cuatro paletas, ventanas pequeñas/grandes, ni las pruebas manuales entre dos equipos mediante LAN y Cloudflare.

## API de solo lectura v2

- `GET /l/{token}/v1/library` — versión de protocolo, nombre, número de fichas publicadas y filtros de avatar disponibles (no incluye ocultas)
- `GET /l/{token}/v1/assets` — catálogo y firmas de contenido, paginado internamente, con filtros
- `GET /l/{token}/v1/assets/{id}` — ficha completa autorizada con origen opaco y adjuntos
- `GET /l/{token}/v1/covers/{id}` — portada visible
- `GET /l/{token}/v1/files/{id}` — archivo autorizado, con verificación de contenido en cliente

**La versión de protocolo es 2** aunque las rutas mantengan `/v1/` por compatibilidad de forma; cliente y anfitrión necesitan una versión compatible de SZKura. Los productos no seleccionados no aparecen en ningún resultado.
