# Flujo de uso previsto (v0.2)

1. Crear un producto con enlace BOOTH. La ficha intenta obtener título, descripción y portada.
2. Elegir ZIP o UnityPackages opcionales (no hace falta comprarlos/descargarlos desde la app).
3. Elegir entre **Vincular sin copiar** (ruta original) o **Guardar copia** (archivo gestionado).
4. Abrir la ficha para editar descripción, categoría, creador, etiquetas, consultar la URL y asociar más ZIP.
5. Filtrar por categoría o archivos vinculados de forma independiente, buscar texto y restablecer filtros.

La migración desde alpha.3 debe conservar todas las fichas e importar sus referencias ZIP a la tabla attachments sin duplicarlas.

Pruebas CI: catalogación SQLite/FTS5, migración idempotente, importación de archivos, deduplicación, consulta de URL BOOTH. La descarga de metadatos de una web real y el diseño visual deben validarse manualmente; una build verde no garantiza acceso web o apariencia.
