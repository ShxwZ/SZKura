# Compartir biblioteca en LAN (experimental)

Función opcional de la issue #6. Es independiente del sistema actual de enlace temporal «Compartir ficha» (SHARING.md).

## Anfitrión

1. Configuración → Compartir biblioteca → Cargar productos para seleccionar.
2. Marca **solo las fichas** que quieres publicar. Puedes filtrar y seleccionar la página visible; no se publica ninguna por defecto.
3. Dentro de cada ficha marcada, elige individualmente los adjuntos descargables. **Ningún archivo queda autorizado por defecto.**
4. Pulsa Iniciar compartir biblioteca. La aplicación prepara una instantánea de lo seleccionado y verifica los SHA-256 en segundo plano.
5. Copia la invitación privada. Usa Detener biblioteca compartida para revocar el acceso. Al salir de SZKura se detiene la sesión.

Puedes seguir usando tu biblioteca local. Las ediciones posteriores no expanden la lista de permisos de una sesión existente; para modificarlos debes detenerla e iniciarla otra vez.

El visitante **no recibe nombres, cantidades, identificadores, rutas ni indicios de elementos ocultos**. Los identificadores públicos se asignan correlativamente solo a los elementos visibles, independientemente de SQLite.

## Visitante

1. Pulsa Conectar a biblioteca en la barra lateral de SZKura.
2. Pega la invitación LAN que te proporcionó el anfitrión.
3. Explora catálogo, filtros, portadas, búsqueda y fichas en modo de solo lectura.
4. Importa solo los metadatos con Añadir ficha sin archivos, o selecciona los adjuntos permitidos y pulsa Descargar e importar seleccionados.
5. SZKura verifica tamaño y SHA-256, importa los archivos sin ejecutarlos y conserva tus datos existentes si ya tienes la ficha BOOTH.

## Seguridad

- El servidor se inicia solo al pulsar Iniciar compartir biblioteca; se vincula a una interfaz IPv4 privada disponible o a loopback.
- Invitación con token aleatorio de ruta y clave aleatoria independiente de 256 bits en el fragmento de URL. El cliente envía la clave en la cabecera X-SZKura-Library-Key. Todas las peticiones deben estar autorizadas.
- HTTP sin cifrar: **solo redes LAN privadas de confianza**. No abrir ni redirigir este puerto a Internet, ni utilizarlo en redes públicas o compartidas con usuarios desconocidos.
- La cabecera privada no acredita la identidad del cliente: cualquiera con el enlace completo puede reproducir el protocolo.
- No se envían rutas locales, la base de datos ni adjuntos no elegidos. Solo se admiten consultas GET; los otros métodos, rutas ocultas y peticiones sin clave devuelven 404.
- Publicar archivos adquiridos en BOOTH solo si la licencia del creador permite su redistribución.
- Nada de esto cambia el protocolo de la compartición de una sola ficha, cuyos enlaces siguen sin interfaz web.

## Límites del MVP

- Máximo 2000 fichas y 64 adjuntos publicados, 8 GiB por archivo y 16 GiB en total.
- Paginación hasta 32 fichas por solicitud y máximo cuatro conexiones de API simultáneas.
- No hay cuentas, registro de bibliotecas guardadas, reconexión persistente, edición colaborativa ni acceso HTTPS remoto.
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

La lista filtra por búsqueda, categoría y avatar; soporta offset y limit. Las respuestas muestran exclusivamente el contenido que el anfitrión permitió compartir.
