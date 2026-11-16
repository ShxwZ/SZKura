# Compartir assets temporalmente (experimental)

Función opcional y desactivada por defecto. SZKura **no descarga ni ejecuta Cloudflare Tunnel al instalarse, al iniciarse ni al activar la opción en Ajustes**.

## Compartir

1. Configura el almacén local y activa **Configuración → Habilitar compartir archivos**.
2. Desde una ficha con archivos adjuntos pulsa **Compartir ficha**.
3. Elige caducidad (15-360 minutos), descargas máximas **por archivo** y contraseña opcional.
4. La primera vez, SZKura muestra un diálogo de consentimiento con origen y finalidad de Cloudflare Tunnel; **Cancelar** no descarga nada. Si aceptas, descarga la versión fijada desde GitHub oficial y verifica su SHA-256 antes de ejecutarla.
5. Cuando se haya creado, el diálogo se cerrará y aparecerá **Copiar enlace** en la cabecera de la ficha, junto a **Detener compartición**. El receptor podrá descargar los archivos desde el navegador o importarlos desde otra instalación de SZKura.
6. Pulsa **Detener compartición** en la cabecera de esa ficha para revocar el enlace.

La primera versión comparte la **ficha y todos sus adjuntos**; no ofrece selección individual de archivos. No comparte otras fichas, rutas originales, credenciales, preferencias ni la base de datos completa.

## Importar de SZKura a SZKura

1. En el equipo receptor activa primero **Configuración → Habilitar compartir archivos**. Solo entonces aparecerá **Importar enlace SZKura** en la cabecera de la aplicación.
2. Pega el enlace y, si la hay, la contraseña compartida por separado.
3. Usa **Examinar enlace** para revisar metadatos, avatares, lista de adjuntos y tamaños; confirma desde el diálogo con el estilo de SZKura.
4. Confirma. SZKura descarga los archivos por streaming a archivos temporales, valida tamaños y SHA-256 y los importa al almacenamiento gestionado. No ejecuta ni extrae los archivos.
5. Si ya existe una ficha con el mismo ID BOOTH, conserva los metadatos locales y añade solo adjuntos nuevos.

La ficha transferida incluye título, creador, descripción, categoría, etiquetas, avatares compatibles, enlace BOOTH, indicador de contenido sensible y portada disponible. No se transfieren favoritos ni ajustes personales.

## Dependencia opcional

El ejecutable queda bajo la subcarpeta components del directorio de datos de SZKura. Se descarga solo tras aceptar, se verifica también antes de cada ejecución y se puede borrar desde Ajustes sin privilegios de administrador. En la primera versión es compatible con Windows x64 y Linux x64.

El componente está fijado a una versión exacta; una actualización requiere modificar la versión y sus hashes en el código y volver a obtener consentimiento del usuario para descargarlo. No hay actualizaciones silenciosas.

## Seguridad y límites

- Servidor local limitado a 127.0.0.1, token aleatorio de 256 bits, caducidad, límite de descargas por archivo y contraseña opcional HTTP Basic sobre HTTPS.
- En el receptor: validación estricta de enlaces HTTPS de trycloudflare.com, manifiesto versionado, nombres de archivos, extensiones, tamaños y SHA-256.
- El enlace es un secreto de acceso. No lo publiques y no reutilices contraseñas importantes.
- Solo puedes compartir assets cuando la licencia o el titular lo permitan. Una compra de BOOTH no autoriza automáticamente a redistribuirla.
- Cloudflare Quick Tunnels es un servicio para **pruebas y desarrollo**, no garantiza estabilidad para transferencias grandes. Esta primera versión no permite reanudar descargas interrumpidas.
- Los archivos se envían desde el equipo del emisor a través del proveedor del túnel; SZKura no aloja copias permanentes en un servidor propio.

Documentación de Cloudflare: https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/

## Validación

El workflow CI de esta rama comprueba Rust en Linux y Windows. También hay pruebas de validación de enlaces, manifiestos y detección de URL de túnel. Antes de publicarlo como estable se debe realizar una prueba real entre equipos distintos.

## Interfaz y traducciones

La compartición usa los modales de confirmación estándar para el consentimiento de Cloudflare y la importación. La copia del enlace aparece en la cabecera de la propia ficha; la entrada de importación solo aparece si la opción está habilitada. Los flujos, estados y errores disponen de traducción española e inglesa (ver `src/i18n.rs` y `AGENTS.md`).
