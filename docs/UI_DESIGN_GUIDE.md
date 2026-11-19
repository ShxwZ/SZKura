# SZKura — Guía de interfaz (egui)

**Estado:** norma de diseño para nuevas funciones y revisiones visuales. Si una pantalla necesita una excepción, documéntala en el PR y consérvala accesible. Implementación de referencia: `src/theme.rs`, `src/ui_support.rs` y `src/main.rs`.

## Principios

1. **Una interfaz, cinco paletas.** Usar siempre `theme::colors()` para superficies y controles. `bg` = fondo general, `panel` = ventanas/barra lateral, `card` = tarjetas, `faint_bg` = fondo de portadas, `stroke` = bordes, `text`/`muted` = texto y `accent`/`primary` = énfasis y acciones. No fijar verdes u otros colores del tema original. Rojo tenue se reserva para acciones destructivas y avisos, no para acciones normales.
2. **Escala consistente.** Inputs y botones habituales tienen **36 pt** de altura (`style.spacing.interact_size.y`). Controles de navegación: **184×36 pt**, esquinas de 8 pt. Formularios usan el ancho de la columna real, con etiquetas, inputs y selectores alineados; sin espacios en el texto para simular iconos ni geometrías dependientes de la fuente. Los editores de producto pasan a **una sola columna** cuando el área central mide menos de **940 pt** (medir `ui.available_width()` *dentro del scroll*, no el ancho de la ventana, porque existe una barra lateral). Sobre ese umbral presentan portada e información en dos columnas.
3. **Jerarquía tipográfica.** Títulos de pantalla 23 pt, secciones 17–18 pt, títulos de diálogo 19 pt, objeto destacado 14 pt, explicaciones 13 pt y metadatos de 11–12,5 pt. Fondos y texto deben mantener contraste en las cinco paletas y durante `hover`/selección.
4. **Navegación predecible.** Configuración, Nuevo producto y ficha se muestran en el panel principal con una única acción de regreso fija, fuera del scroll. No añadir botones de retorno duplicados. El filtro NSFW utiliza el mismo alto que el resto y un círculo geométrico independiente de métricas de fuente.
5. **Paletas.** Mostrar **Ónix** (negro/gris), **Jade** (verde original predeterminado), **Amatista** (morado) y **Ámbar** (naranja) y **Vice** (morado neón, magenta y atardecer naranja) en tarjetas completas, sin botones internos ni etiquetas desplazadas. Guardar los IDs estables `graphite`, `sage`, `purple`, `orange`, `vice` en SQLite: **no renombrarlos** ni migrar datos por cambios de marca. Aplicar el tema inmediatamente y al restaurar. En Vice dibujar **una única vez por fotograma** el degradado sutil sobre el **rectángulo de la ventana** con `theme::paint_vice_background(ctx)` en la capa de fondo, antes de crear los paneles. Los paneles centrales han de usar `theme::content_panel_fill()` (transparente solo en Vice), y cabecera, barra lateral y barra de estado `theme::chrome_panel_fill()` (veladura oscura semitransparente solo en Vice). Así el atardecer se ve continuo sin rectángulos ni reinicios al cambiar de página. Reservar el rosa para acciones y selección; mantener las tarjetas opacas. **El bloque de Apariencia e idioma debe tener el mismo marco, borde, radio y padding en las cinco paletas**: usar `appearance_settings_frame(colors())` sin condiciones por tema. Solo el color del panel y su borde varían; el degradado es un tratamiento del fondo, no una vista diferente. Conservar intactos los IDs y los demás estilos de las cuatro paletas anteriores. No volver a dibujar gradientes locales dentro de `CentralPanel`.
6. **Portadas y archivos.** Las tarjetas de imagen deben usar `faint_bg` del tema, no un color fijo. Inspector de ZIP/7z/RAR: con **1–2 entradas** dibujar el contenido directamente dentro del frame, sin un `ScrollArea` que reserve altura vacía; con **3 o más**, altura proporcional al número de entradas (máximo adaptado al viewport) y scroll interno con `min_scrolled_height` y `max_height` iguales para evitar que el scroll exterior lo comprima. Mantener límites de inspección/extracción en segundo plano.

## Paridad de diseño entre paletas

- **La paleta nunca decide qué widgets, vistas, márgenes, anchos, alturas, radios, separadores o contenedores se usan.** Si un panel es necesario, dibujarlo en todas las paletas con idéntica geometría. Las diferencias se limitan a `theme::colors()`, el degradado Vice y superficies cromáticas.
- `appearance_settings_frame(palette)` centraliza el marco de «Paleta de colores / Idioma» para todos los temas: 18 pt de padding, borde de 1 pt y radio de 12 pt, independientes de la paleta.
- El selector de paletas utiliza `theme::preview_colors(kind)`, siempre cuatro muestras colocadas igual. Vice puede usar un segundo swatch cálido, pero no un layout especial. La prueba `appearance_geometry_is_identical_across_all_five_themes` verifica los márgenes, bordes y radios.
- Verificar el cambio de tema con Configuración, biblioteca, ficha de producto, nuevo producto, selección y biblioteca remota, manteniendo la misma estructura visual. Evitar condiciones `ThemeKind::Vice` fuera de `theme.rs` para construir componentes.

## Consistencia visual de controles egui

- La paleta se aplica a los **cinco estados de los widgets** de egui mediante `theme::apply_widget_visuals`: `noninteractive`, `inactive`, `hovered`, `active` y `open`.
- Un botón normal y un `ComboBox` usan `weak_bg_fill`, no solo `bg_fill`. Ajustar **ambos** fondos y los bordes en el estilo global; no dejar los grises predeterminados de `Visuals::dark()` en botones como «Mostrar más / editar» ni selectores como «Categoría».
- Mantener las acciones principales (`primary`) diferenciadas de botones secundarios (`button_bg`). Los controles deshabilitados deben seguir identificándose como tales por la opacidad nativa de egui y no activarse por motivos estéticos.
- Comprobar los desplegables tanto cerrados como abiertos y las cinco paletas, incluido Vice, en Configuración, catálogo, ficha y biblioteca remota.

## Indicadores de biblioteca

- El encabezado muestra el recuento filtrado y el **peso total de los archivos vinculados** a ese mismo conjunto de productos. Utilizar `Catalog::stats_filtered_visibility` (consulta SQL independiente de la paginación), actualizada en `reload()` cuando cambien filtros o archivos, no sumar exclusivamente las tarjetas ya visibles.
- No sumar dos veces la misma ruta `stored_path` si varios productos la comparten. Son tamaños registrados en SQLite para archivos externos o gestionados; no equivalen al uso de disco propio de SZKura ni incluyen WebP, caché, SQLite u otros archivos temporales. El tooltip debe aclarar el alcance.
- Formatear con la función `human_size` ya usada en otros apartados; mantener un texto secundario compacto y coherente con las cinco paletas.

## Portadas y rendimiento gráfico

- La tarjeta de cuadrícula, la fila de lista y la **ficha de producto** muestran un fondo de la misma portada, **desenfocado a 64×64 px**. Generarlo en los **dos workers de portadas** tanto para miniaturas (320) como para ficha (768), nunca en `draw_cover` por fotograma. La imagen central continúa nítida y sin recortar. El fondo ocupa el rectángulo completo con esquinas redondeadas mediante `egui::Image::corner_radius(...).paint_at(...)`, no `Painter::image` (que dibuja esquinas cuadradas). La portada nítida usa `fitted_cover_rect` sin margen forzado: en imágenes verticales llega exactamente arriba y abajo del fondo y, en horizontales, hasta los laterales. El redondeo real de la textura mantiene el recorte correcto; la relación de aspecto se conserva sin estirar.
- El fondo se sube junto a la textura principal (máximo dos resultados por fotograma). Ambas texturas deben formar una sola entrada en el LRU con límite agregado de **48 MiB** y liberarse juntas. Si una imagen no existe, conservar el fondo `faint_bg` del tema.
- Las **nuevas imágenes descargadas de BOOTH**, no las imágenes locales, se pueden reducir a máximo **1280 px**, WebP de calidad **83**. Conservar el formato de origen si el WebP ocupa más o si falla la conversión. Escritura temporal seguida de renombrado sin sustituir rutas antiguas; no modificar imágenes a las que ya apunta SQLite.
- Evitar peticiones adicionales o conversiones durante el scroll. Pruebas automáticas para tamaño/decodificación WebP, límites de la miniatura desenfocada y fallback cuando no hay ahorro. Verificar visualmente esquinas, temas y color de los laterales.

- **Editor ancho:** no usar `ui.columns(2)` de igual ancho para la ficha o Nuevo producto. Reservar para la portada el 40 % del panel hasta 482 pt y situar los campos restantes a su derecha en `ui.horizontal_top` con un margen uniforme de 20 pt. Bajo el umbral responsive de 940 pt, mantener portada y formulario apilados. Revisar portadas verticales, horizontales y cuadradas en ambos diseños.

## Categorías BOOTH y almacenamiento

- Al crear un producto con URL BOOTH, consultar **una sola vez** los metadatos públicos antes de tocar la estructura del almacén. La categoría explícita de su JSON (`category.name` o subcategoría) se relaciona con las categorías locales si es reconocible: 3D Clothing/3D衣装 → **Ropa**, 3D Characters → **Avatar**, 3D Hair → **Pelo**, etc. Evitar inferencias del título, la descripción, las etiquetas o enlaces de productos recomendados.
- Precedencia: **selección del usuario > categoría de una ficha existente > categoría BOOTH detectada > «Otros»**. Las fichas ya existentes no deben reclasificarse automáticamente durante una importación ni al pulsar «Actualizar BOOTH»: puede haber archivos vinculados en su carpeta original.
- **Orden obligatorio:** validar URL → obtener BOOTH (sin copiar ZIP/7z) → decidir categoría final → guardar categoría en SQLite → organizar **una sola vez** los archivos en `<almacén>/<categoría>/<ficha>/`. Si BOOTH falla o el dato es ambiguo, conservar la categoría seleccionada o «Otros». Nunca copiar primero a «Otros» para mover después a «Ropa».
- Validar con una prueba de almacenamiento que un nuevo artículo detectado como «Ropa» crea su único destino en `Ropa/` y no crea una carpeta `Otros/`. Mantener la extracción/lectura de BOOTH limitada y sin credenciales.

## Confirmaciones y diálogos

**Obligatorio:** usar `ui_support::confirmation_dialog(...)` (implementado con `egui::Modal` nativo) y sus auxiliares `dialog_subject` y `dialog_text`; no construir nuevos `egui::Window` para confirmaciones salvo justificación expresa. `Modal` crea una ventana ajustada al contenido y bloquea la interacción con la pantalla de fondo. Esta función define:

- Modal centrado y ajustado al contenido, con anchura máxima de 440 pt (según el viewport), **sin segunda barra de título ni espacios verticales sobrantes**. Panel y borde derivados de la paleta, radios de 12 pt y márgenes internos de **20×18 pt**. No forzar altura fija ni reutilizar dimensiones de una ventana anterior.
- Título 19 pt, separadores, sujeto destacado en `accent` (14 pt), explicación `muted` (13 pt) con wrap y acciones al pie alineadas a la derecha. Espaciado y ancho iguales para todas las confirmaciones.
- **Cancelar a la izquierda** de la acción final, siempre visible. El pie se crea dentro de un `ui.horizontal` de una sola fila (36 pt), nunca con `with_layout` ocupando toda la altura restante. Escape o clic fuera del modal cancelan. Acción final de 36 pt; `ConfirmTone::Primary` usa el color de tema y `ConfirmTone::Destructive` el rojo de advertencia. La acción final se deshabilita mientras hay una tarea incompatible en curso.
- No afirmar que una acción ha terminado antes de ejecutarla. El texto tiene que distinguir **quitar la ficha del catálogo** de **borrar un archivo del disco**.
- Operaciones con riesgo requieren consentimiento en el contexto: eliminar ficha (sin borrar originales), restaurar SQLite (copia recuperable), enviar originales a la papelera (confirmación explícita tras activar preferencia). Vaciar la caché también muestra confirmación, y especifica que solo elimina temporales.
- La confirmación de eliminación es **la misma** desde lista, cuadrícula y detalle; no usar un aviso inline diferente dentro de la ficha.

## Componentes compartidos

- `ui_support::action` para botones con icono/texto centrados medidos; `icon_action` para botones iconográficos; `favorite_action` para favoritos; `nav_choice` para filtros y `nav_sensitive_choice` para visibilidad.
- No incrustar un `selectable_label` dentro de una tarjeta si visualmente crea un segundo botón. La tarjeta completa debe ser la interacción, con colores y coordenadas uniformes.
- **Vista lista adaptable:** reservar una altura de fila coherente con su layout virtualizado; por debajo de ~1020 pt de espacio en catálogo, distribuir datos arriba y botones/medidas abajo. Calcular el breakpoint fuera de `ScrollArea::show_rows` y pasar el mismo estado a cada fila para evitar cambios de altura internos. Los títulos y metadatos deben truncarse y mostrar el contenido completo en hover.
- **Avatares:** tanto en cuadrícula como en lista mostrar el primero y `+N` para los restantes (por ejemplo, `Avatar: MAYA +10`), sin insertar todos los nombres en una fila; tooltip con el texto íntegro.
- **Editor de portada:** portada y botón de elegir/cambiar imagen comparten un panel; botón de 36 pt ocupando el mismo ancho interior que la imagen, sin rótulos laterales que lo empujen o rompan al reducir columnas.
- **Editor adaptable (ficha y nuevo producto):** extraer portada e información a funciones reutilizadas por ambas modalidades; no duplicar lógica de formulario entre layout ancho y estrecho. En una ventana estrecha, presentar la portada y su resumen, un separador y después los campos a ancho completo. Dentro del formulario, **Creador y Categoría** comparten fila solo con al menos **550 pt** disponibles; si no, se apilan. Cabeceras de editor con acciones en segunda fila por debajo de 800 pt (ficha) o 700 pt (nuevo producto). Evitar que la barra inferior de estado crezca o desborde: truncar textos largos y ofrecer su contenido completo en tooltip.
- **Ficha de producto:** inputs de 36 pt y checkbox NSFW con ancho natural, dentro de `ui.horizontal` para que la casilla y el texto comiencen en el borde izquierdo de los inputs. **No** usar `ui.add_sized([ui.available_width(), ...], Checkbox)` porque centra su contenido en la columna. Descripción y su botón de despliegue comparten ancho de columna. Aplicar las mismas reglas a Nuevo producto.
- **Resumen de producto:** nunca expandirlo al ancho completo de la columna si la portada está limitada a 480 pt. Usar un sub-UI de ancho exacto al editor de portada y altura dictada por su contenido, con márgenes verticales compactos, recuento de archivos y avatares abreviados (`Avatar: MAYA +N`). Evitar espacios en blanco causados por alturas heredadas de la columna vecina.
- `ScrollArea` solo donde hay contenido potencialmente largo; títulos, volver y acciones principales deben permanecer localizables. Evitar scrolls anidados salvo visores de archivos que lo requieran.

## Bibliotecas compartidas (lectura remota)

- **Una sola portada:** `ui_support::paint_cover` dibuja imágenes locales y remotas con idéntica geometría de encaje y fondo desenfocado de 64×64. Se descargan por HTTP y se decodifican fuera del hilo de egui; portadas de hasta **32 MiB / 24 MP** como en el catálogo nativo, con texturas limitadas por presupuesto de memoria.
- **Sin bloqueo en las fichas:** al abrir una ficha remota, primero se consulta y dibuja su manifiesto JSON. La verificación del contenido local con BLAKE3 arranca después, en otro trabajador cancelable, indicando su estado sin impedir navegar o volver.
- **Diseño común:** misma portada, columna del 40 % hasta 482 pt, tarjeta RESUMEN, campos informativos, sección de archivos y encabezado de 23 pt. Solo cambian los controles: biblioteca remota es de solo lectura y permite importar, no editar ni borrar el host.
- **Portadas de red:** hasta tres peticiones simultáneas, prioridad para la ficha abierta, reintento manual si hay errores y mensajes distintos para cargando/sin portada/portada no disponible. Mantener pruebas de codificación, tamaños y geometría.

## Checklist para cada PR de interfaz

- [ ] Revisar Ónix, Jade, Amatista y Ámbar, incluyendo portadas, estados hover/selección y texto.
- [ ] Validar fondos desenfocados en tarjeta cuadrada, horizontal, vertical **y ficha**, comprobando que las esquinas estén recortadas, que la portada no sobresalga del fondo y que las columnas de la ficha no dejen un hueco enorme. Medir scroll y memoria GPU frente a `main` con catálogo real.
- [ ] Revisar anchos, alineación y wrapping a 100 %, 125 % y 150 % de escala de pantalla, y ventana mínima de 980×650.
- [ ] Revisar todos los diálogos afectados: teclado Escape, botón Cancelar, acción deshabilitada con tareas activas, títulos no duplicados, rutas largas y títulos japoneses.
- [ ] Verificar que ningún botón, input o título se solapa o se corta; un ZIP/7z con 1 archivo ocupa poco alto, uno con 10–50 archivos muestra varias filas a la vez, y los modales breves miden menos de 300 pt de alto en una ventana normal.
- [ ] Comprobar lista y cuadrícula en ventana mínima y grande: avatar resumido, botones visibles, altura virtual de fila correcta y portada/botón alineados en edición/creación.
- [ ] Comprobar **Ficha** y **Nuevo producto** con barra lateral visible en ventana de ~980×650: los campos deben aparecer en una sola columna, ningún borde ni texto salirse por la derecha, el título y las acciones del encabezado no superponerse, y los controles de Creador/Categoría apilarse correctamente con espacios estrechos.
- [ ] En la ficha, comparar ancho real de portada y tarjeta RESUMEN, comprobar que esta se ajusta a 1–2 líneas y que el checkbox NSFW comienza exactamente en el margen izquierdo de los campos de texto.
- [ ] Ejecutar `cargo test --all-targets` y `cargo build --release` en Windows y Linux mediante GitHub Actions; validar manualmente UX donde no existen pruebas gráficas.
- [ ] Si se crea un componente nuevo, reutilizar `theme::colors()`, añadir pruebas cuando sea posible y **actualizar esta guía**.

Consulta `docs/HANDOFF.md` para la evolución técnica de la app.


## Compatibilidad RAR

- Importar, almacenar, listar y extraer selectivamente paquetes de RAR4/RAR5 mediante el lector puro Rust `rars`. No ejecutar utilidades del sistema ni requerir WinRAR.
- Validar rutas, nombre, tamaño, índice y tipo de miembro al leer y volver a validar al extraer. No crear rutas a partir de nombres del archivo comprimido.
- Usar los mismos límites de inspección y caché Unity que ZIP y 7z; rechazar archivos cifrados, multivolumen y entradas enlazadas/simbólicas cuando no se admiten.
- Incluir `.rar` en todos los diálogos de selección, flujos de importación de carpetas, archivo gestionado, mantenimiento y mensajes de estado. Tests en CI Windows y Linux.


## Idiomas y transición a SZKura

- La aplicación, el crate Rust, la documentación y el repositorio se denominan **SZKura**. La compatibilidad con datos antiguos se mantiene en las rutinas internas de migración; los mensajes y la documentación para usuarios deben usar el nombre actual.
- Los idiomas iniciales son **es** y **en**, seleccionables en Configuración y guardados en `library_settings.ui_language`. Transcribir texto visible con `i18n::tr`, evitando traducir categorías almacenadas, URLs o nombres de archivo. Las traducciones nuevas se solicitan mediante [GitHub Issues](https://github.com/ShxwZ/SZKura/issues/new).
- En UI nuevos componentes, comprobar ambos idiomas a 980px y 1440px: el inglés puede ocupar más ancho. Evitar literales directos cuando existe una clave de traducción.
- Las cinco capturas del README (biblioteca, detalle, inspector, configuración y nuevo producto) ya están subidas a GitHub y fusionadas en `main` mediante PR #35.
