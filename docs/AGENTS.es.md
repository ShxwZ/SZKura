# Normas para agentes de SZKura — Español

> **Aplicación:** cualquier agente de programación, automatización o contribuidor que modifique SZKura.
>
> La versión completa y normativa en inglés está en [`AGENTS.md`](../AGENTS.md). Esta referencia resume las reglas en español para la revisión humana. Ante diferencias, actualizar ambas versiones y consultar [`docs/UI_DESIGN_GUIDE.md`](UI_DESIGN_GUIDE.md), que define el sistema visual con más detalle.

## Antes de tocar código

1. Leer `AGENTS.md`, `docs/UI_DESIGN_GUIDE.md` y los archivos afectados.
2. Revisar `src/main.rs` (flujo y navegación), `src/ui_support.rs` (componentes compartidos), `src/theme.rs` (colores), `src/i18n.rs` (ES/EN) y `src/db.rs` (persistencia).
3. Determinar dónde encaja cada acción en la interfaz existente, cómo se activa/desactiva, qué datos toca y cómo se probará. **No crear una interfaz nueva independiente del diseño de SZKura.**

## Diseño y navegación obligatorios

- Utilizar `theme::colors()` y las cuatro paletas **Ónix, Jade, Amatista y Ámbar**; no fijar colores del tema verde original.
- Reutilizar `ui_support::action`, `icon_action`, `nav_choice` y demás componentes existentes. Entradas y botones normales de **36 pt**, radios coherentes; evitar iconos simulados con espacios o emojis.
- Respetar tipografía, alineaciones, dimensiones, diseños responsivos y la ventana mínima **980×650**. Las pantallas principales utilizan un solo `CentralPanel` y una única acción clara de regreso fuera del scroll.
- Para confirmaciones y consentimiento, **`ui_support::confirmation_dialog(...)`** y los auxiliares `dialog_subject` / `dialog_text`: el modal centrado del proyecto, con **Cancelar y Escape**, acción principal a la derecha y colores del tema. **No improvisar confirmaciones con `egui::Window`**.
- Los flujos de varios pasos deben usar el patrón de página principal o un componente compartido, adaptable y tematizado; no ventanas flotantes añadidas sin justificación.
- Las acciones sobre una ficha van **en la propia ficha**. Para compartir, el botón de copiar enlace debe estar **arriba, en el contexto de esa ficha**, cuando el enlace esté activo, no escondido en un diálogo.
- Cuando una función solo permita **una operación activa a la vez**, mostrar el nombre de la ficha afectada y un indicador visible; si se intenta iniciar otra, abrir el modal estándar con un botón para ir a la ficha activa y revisarla o detenerla. **Nunca ignorar silenciosamente** la petición ni dejar al usuario sin saber qué se está compartiendo.
- Las funcionalidades opcionales deben ser **invisibles e inactivas** hasta que se habiliten. En particular, **«Importar enlace SZKura» no debe mostrarse si compartir/transferir está desactivado en Ajustes**. Desactivar debe detener los procesos de esa función.
- Todas las operaciones necesitan vuelta o cierre, cancelación, estados de carga, errores y confirmación tras éxito real.

## Traducciones: parte de la definición de terminado

- SZKura admite **español e inglés**. **Todo** texto visible nuevo debe traducirse: botones, títulos, descripciones, campos, tooltips, Ajustes, confirmaciones, consentimientos, estados de espera, avisos, errores y mensajes dinámicos.
- Utilizar `i18n::tr` y añadir traducciones a `src/i18n.rs`. Para mensajes dinámicos, seguir `i18n::tr_message` o las plantillas existentes.
- No traducir ni cambiar identificadores persistidos de categorías, IDs de paleta o idioma, rutas, nombres de archivos, URLs, nombres de productos ni campos de protocolo.
- Añadir tests de internacionalización y probar toda la experiencia en **ambos idiomas**, incluyendo textos largos y títulos japoneses.
- Una pantalla que aparece en español estando el idioma inglés seleccionado **no está terminada**.

## Datos, almacenamiento y seguridad

- Proteger la biblioteca SQLite y los archivos: nada de borrados, movimientos, sobrescrituras o recategorizaciones silenciosas.
- Una preferencia nueva necesita cambios tanto en la lista de claves permitidas de **`Catalog::preference`** como en la de **`Catalog::update_preference`**. Añadir pruebas del valor por defecto, actualización, persistencia tras reiniciar y rechazo de claves desconocidas. Ya hubo un error de arranque al olvidar `sharing_enabled`.
- Las dependencias externas son opcionales salvo decisión expresa. Descargar `cloudflared` solo tras **consentimiento explícito**, desde fuente oficial y verificando SHA-256; nunca al iniciar ni al marcar un ajuste sin confirmar. Reutilizar la copia verificada y permitir eliminarla desde Ajustes.
- La importación remota exige previsualización y confirmación, integridad/tamaños, validación de rutas, límites, no ejecutar archivos y no sobrescribir fichas existentes sin consentimiento. Evitar revelar tokens o datos privados.
- Los enlaces de compartición **no deben abrir un listado HTML ni permitir descargas directas desde el navegador**. El servidor exige en todas las rutas una clave aleatoria independiente, transferida desde el fragmento URL mediante la cabecera `X-SZKura-Transfer-Key` que envía la app receptora. Probar con peticiones HTTP locales que, sin la cabecera, todas las rutas responden 404. **Una cabecera no certifica la identidad del programa**; otro cliente HTTP puede reproducir la petición si dispone del enlace completo.
- Respetar las licencias de BOOTH: adquirir un asset no significa disponer de permiso para redistribuirlo.
- No bloquear el hilo gráfico con redes, copias grandes, inspección de ZIP u operaciones SQLite costosas.

## Control de calidad antes de fusionar

- [ ] Diseño, temas y componentes compartidos consistentes con SZKura.
- [ ] Diálogos correctos con Cancelar/Escape y presentación adaptable.
- [ ] Acciones contextuales en su pantalla y funciones opcionales ocultas mientras están deshabilitadas.
- [ ] Interfaz **100 % ES/EN**, incluyendo errores, consentimientos y estados dinámicos.
- [ ] Preferencias y migraciones comprobadas; biblioteca antigua intacta.
- [ ] Consentimiento y seguridad de archivos cubiertos donde corresponda.
- [ ] Pruebas `cargo test --all-targets`, compilación `cargo build --release` y comprobaciones en Windows/Linux.
- [ ] Revisión visual manual en los cuatro temas, con ventana pequeña, grande y escalado diferente.
- [ ] PR en una rama independiente, con limitaciones explicadas. **No fusionar sin autorización**.
- [ ] Si se entrega un `.exe`, obtenerlo del artefacto CI de **esa PR y ese commit**, no de una versión anterior.

**Que compile no significa que cumpla el proyecto.** Si falla algún punto, la PR no está lista para fusionar.

Al crear una nueva convención hay que actualizar la guía visual, el documento inglés y esta referencia en español.
