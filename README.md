# CBR-CBZ-Rust

Lector de cómics para Windows y Linux escrito en Rust, con biblioteca.

- **Formatos:** CBZ, CBR, CB7, CBT y carpetas de imágenes (JPG, PNG, WebP, GIF, BMP, TIFF).
  El formato se reconoce por el contenido, así que un `.cbr` que en realidad es un zip
  se abre igual.
- **Calidad:** las páginas se encogen con Lanczos3 al tamaño exacto de la pantalla y se
  dibujan píxel a píxel.
- **Biblioteca:** añades las carpetas donde guardas tus cómics y la app los agrupa en
  series (con su `ComicInfo.xml` o, si no lo tienen, con el nombre del archivo).
  Recuerda por qué página vas de cada cómic, aunque lo muevas o lo renombres.
  **Solo lee tus archivos: nunca los mueve ni los cambia.**

## Compilar

Hace falta [Rust](https://rustup.rs) y un compilador de C++, porque la librería de RAR
y SQLite están en C/C++.

**Windows:** las [Build Tools de Visual Studio](https://visualstudio.microsoft.com/es/visual-cpp-build-tools/)
con "Desarrollo para el escritorio con C++" (el mismo requisito que pide `rustup`).

**Linux:** `gcc` y `g++`. En Debian y Ubuntu:

```sh
sudo apt install build-essential
```

Para que funcionen los selectores de archivos y carpetas hace falta `xdg-desktop-portal`,
que ya traen GNOME, KDE y casi todos los escritorios.

Después, en los dos sistemas:

```sh
cargo run --release -p comic-app
```

También se le puede pasar un cómic para abrirlo directamente:
`cargo run --release -p comic-app -- "ruta/al/comic.cbz"`.

## Teclas del lector

| Tecla | Acción |
|---|---|
| ← / → | Página anterior / siguiente |
| ↑ / ↓ | Desplazar la página |
| Rueda del ratón, + / - | Zoom (con la rueda, hacia donde apunta el ratón) |
| 0 / W | Ajustar a la página / al ancho |
| T | Tema claro u oscuro |
| F11 | Pantalla completa |
| Esc | Salir de pantalla completa o volver a la biblioteca |

## Dónde se guardan los datos

| | Biblioteca | Miniaturas |
|---|---|---|
| Windows | `%APPDATA%\lector-comics\data` | `%LOCALAPPDATA%\lector-comics\cache` |
| Linux | `~/.local/share/lector-comics` | `~/.cache/lector-comics` |

Con la variable de entorno `LECTOR_COMICS_DATOS` se guarda todo en otra carpeta, para
probar con una biblioteca aparte sin tocar la de verdad.

## Estructura

| Crate | Qué hace |
|---|---|
| `comic-core` | Abrir cómics de cada formato y leer sus metadatos |
| `comic-imagen` | Decodificar y encoger imágenes (se optimiza también en desarrollo) |
| `comic-biblioteca` | Base de datos SQLite: carpetas, escaneo, series y progreso |
| `comic-app` | La interfaz (egui): biblioteca, miniaturas y lector |

Para probar el escaneo sin la interfaz:
`cargo run -p comic-biblioteca --example escanear -- "ruta/a/tus/comics"`.
