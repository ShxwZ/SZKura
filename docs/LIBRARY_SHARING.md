# Compartir biblioteca en LAN o por Cloudflare Tunnel (experimental)

Función opcional de la issue #6. Es independiente del sistema actual de enlace temporal «Compartir ficha» (SHARING.md).

## Anfitrión

1. Configuración → Compartir biblioteca → **Elegir fichas para compartir**. Se abre una pantalla con la cuadrícula, búsqueda, categorías, avatares, filtros con/sin archivos y selección de vista Lista/Cuadrícula.
2. Marca **fichas completas** con sus tarjetas. No se comparte ninguna por defecto. Puedes mostrar solo las seleccionadas para revisarlas.
3. Los adjuntos forman parte de cada ficha elegida: **no se seleccionan archivos individualmente**. La ficha se comparte con todos sus adjuntos, así que respeta las licencias de redistribución.
4. Vuelve a Configuración y pulsa **Iniciar compartir biblioteca**. Puedes mantener la sesión solo en LAN privada o habilitar **Compartir también por Internet (Cloudflare Tunnel)** para tener una invitación HTTPS temporal. Si falta el componente se solicitará consentimiento antes de descargarlo desde GitHub oficial y verificar su SHA-256.
5. Copia la invitación privada. Usa **Detener biblioteca compartida** desde el encabezado para revocar el acceso. Al salir de SZKura se detiene la sesión y su túnel, si lo había.

Puedes seguir usando tu biblioteca local. Las ediciones posteriores no expanden la lista de permisos de una sesión existente; para modificarlos debes detenerla e iniciarla otra vez.

El visitante **no recibe nombres, cantidades, identificadores, rutas ni indicios de elementos ocultos**. Los identificadores públicos se asignan correlativamente solo a los elementos visibles, independientemente de SQLite.

## Visitante

1. En la barra lateral, **debajo de los filtros**, abre la sección **BIBLIOTECAS REMOTAS → Conectar a biblioteca**.
2. Pega la invitación privada LAN o HTTPS de Cloudflare que te proporcionó el anfitrión.
3. Explora un catálogo con apariencia similar a tu biblioteca normal: cuadrícula/lista, búsqueda, filtros por categoría y avatar, NSFW y con/sin archivos. La ficha remota es de solo lectura.
4. Pulsa **Importar a mi biblioteca** para importar la ficha completa con sus adjuntos; o **Solo guardar ficha** para conservar únicamente sus metadatos sin descargar archivos.
5. SZKura verifica tamaño y SHA-256, importa los archivos sin ejecutarlos y conserva tus datos existentes si ya tienes la ficha BOOTH.

## Seguridad

- El servidor se inicia solo al pulsar Iniciar compartir biblioteca; se vincula a una interfaz IPv4 privada disponible o a loopback.
- Invitación con token aleatorio de ruta y clave aleatoria independiente de 256 bits en el fragmento de URL. El cliente envía la clave en la cabecera X-SZKura-Library-Key. Todas las peticiones deben estar autorizadas.
- LAN usa HTTP sin cifrar: **solo redes privadas de confianza**. La alternativa por Internet usa una URL HTTPS temporal de Cloudflare Quick Tunnel, enlazada al servidor local en 127.0.0.1 con clave privada obligatoria. No abrir puertos manualmente.
- La cabecera privada no acredita la identidad del cliente: cualquiera con el enlace completo puede reproducir el protocolo.
- No se envían rutas locales, la base de datos ni fichas/adjuntos de fichas no elegidas. Solo se admiten consultas GET; otros métodos, rutas ocultas y peticiones sin clave devuelven 404.
- Publicar archivos adquiridos en BOOTH solo si la licencia del creador permite su redistribución.
- **Compartir ficha** y **Compartir biblioteca** no se pueden activar a la vez: reutilizan el componente Cloudflare, sin duplicar sesiones. Ninguna de las dos ofrece una interfaz web.

## Límites del MVP

- Máximo 2000 fichas y 512 adjuntos publicados, 8 GiB por archivo y 16 GiB en total. El cálculo de SHA-256 ocurre en segundo plano y puede tardar en bibliotecas grandes.
- Paginación hasta 32 fichas por solicitud y máximo cuatro conexiones de API simultáneas.
- No hay cuentas, registro de bibliotecas guardadas, reconexión persistente ni edición colaborativa. Cloudflare Quick Tunnel HTTPS es temporal, no garantiza permanencia ni un servicio estable de hosting.
- El catálogo remoto es una instantánea; no se sincroniza mientras la sesión está activa.
- Algunas transmisiones en curso pueden tardar en interrumpirse cuando el anfitrión detiene el servidor; las peticiones nuevas se rechazan.
- Pendiente revisión visual manual ES/EN, paletas y ventanas estrechas/anchas.

## API LAN v1

Los recursos de solo lectura son:

- GET /l/{token}/v1/library
- GET /l/{token}/v1/assets
- GET /l/{token}/v1/assets/{id}
- GET /l/{token}/v1/covers/{id}
- GET /l/{token}/v1/files/{id}

La lista filtra por búsqueda, categoría, avatar, NSFW y con/sin archivos; soporta offset y limit. Las respuestas muestran exclusivamente el contenido que el anfitrión permitió compartir.
