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
  Si una misma serie sale partida (por ejemplo `Vinland` y `Vinland Saga`), se pueden
  unir a mano desde la pantalla de la serie con "Unir con otra serie…", y se pueden volver
  a separar. Con el botón derecho en el nombre de una serie o en el título de un cómic
  sale "Renombrar", que cambia cómo se ve en la app (el campo se abre en el sitio;
  dejándolo en blanco, vuelve a mostrarse el automático) y, en un cómic, también
  "Eliminar", que lo quita de la colección sin tocar el archivo (no vuelve a aparecer
  solo; para eso hay que quitar y volver a añadir su carpeta).
  **Solo lee tus archivos: nunca los mueve ni los cambia.**
- **Novedades:** avisa de los tomos y números que han salido después del último que
  tienes de cada serie, enseña el manga que más se lee, el cómic americano que sale esta
  semana y las noticias de las webs que elijas, con su foto. Al pulsar cualquier portada
  se abre su ficha: sinopsis, autores, editorial, géneros, páginas... Solo consulta:
  nunca descarga ningún cómic.
  - Manga: [AniList](https://anilist.co), sin cuenta. Compara con la edición original
    japonesa, que puede tener otro número de tomos que la española. También da el manga
    japonés en tendencia y las series que han empezado en los últimos meses (al bajar
    se van cargando más), y "Para ti": lo que recomiendan los lectores de AniList a quien
    lee los manga de tu biblioteca. La ficha de cada manga trae también sus recomendadas.
  - Cómic americano y lo que sale cada semana: [Metron](https://metron.cloud), con una
    cuenta gratuita que se pone en "Novedades > Ajustes…".
  - Noticias: el RSS de las webs que quieras. Si el RSS no trae foto, se toma la que
    pone la web en la cabecera del artículo.

  Las sinopsis vienen en inglés, como las dan los catálogos. Cada serie se busca sola
  por su nombre y solo se acepta si coincide sin dudas; si no, se puede elegir a mano
  ("No es esta serie…" o "Buscar…"). Cada pestaña se pone al día sola al entrar en ella
  (si no se ha hecho en los últimos 10 minutos), sin avisos de "cargando": lo nuevo
  aparece cuando llega, en uno o dos segundos.

  Cualquier manga o cómic de las listas, de las recomendadas o de lo que sale esta semana
  se puede guardar en favoritos, con el botón derecho sobre su portada o desde su ficha
  ("Añadir a favoritos"), para buscarlo más adelante: quedan en la pestaña "Favoritos".

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

## Instalar

Compila la versión final, la instala para tu usuario (sin permisos de administrador) y
crea los accesos directos. Para actualizar tras cambiar el código, se vuelve a ejecutar.

**Windows:** clic derecho en `instalar.ps1` > "Ejecutar con PowerShell". Se instala en
`%LOCALAPPDATA%\Programs\LectorComics`, con acceso directo en el escritorio y en el
menú Inicio.

**Linux:** `sh instalar.sh`. Se instala en `~/.local/bin/lector-comics`, con su entrada
en el menú de aplicaciones y un acceso directo en el escritorio.

## Controles del lector

Estas son las teclas de serie; cada una se puede cambiar en "Ajustes" (pulsando sobre
la tecla y luego la nueva), y ahí también se puede quitar lo que hace el ratón.

| Tecla | Acción |
|---|---|
| ← / → | Página anterior / siguiente |
| ↑ / ↓ | Desplazar la página |
| Rueda del ratón, + / - | Zoom (con la rueda, hacia donde apunta el ratón) |
| 0 / W | Ajustar a la página / al ancho |
| P | Ver todas las páginas |
| N | Luz cálida |
| T | Tema claro u oscuro |
| F11 o doble clic | Pantalla completa (y salir de ella) |
| Clic en la página | Página siguiente |
| Esc | Salir de pantalla completa o volver a la biblioteca |

Con el doble clic activado, el clic pasa de página tras un instante (lo que tarda en
llegar un posible segundo clic).

**Siguiente cómic:** al pedir la página siguiente en la última de un cómic, si la serie
tiene otro después, sale un aviso para seguir con él sin volver a la biblioteca (Intro,
la tecla de página siguiente o "Leer ahora"; Esc, la tecla de página anterior o un clic
fuera lo cierran y te quedas donde estabas).

**Portada:** con el botón derecho sobre un cómic de la biblioteca (dentro de su serie)
sale "Cambiar portada…", que enseña todas sus páginas para elegir una. También se puede
hacer leyendo, con el botón derecho sobre una página del selector de páginas (P):
"Usar como portada". Se guarda en la biblioteca; el archivo no se toca.

La portada de una serie se elige igual, con el botón derecho sobre la serie (en su
tarjeta o en su título): "Cambiar portada…" enseña las portadas de todos sus cómics. Sin
elegir, es la del primero.

## Dónde se guardan los datos

| | Biblioteca | Miniaturas y novedades | Preferencias |
|---|---|---|---|
| Windows | `%APPDATA%\lector-comics\data` | `%LOCALAPPDATA%\lector-comics\cache` | `%APPDATA%\lector-comics\config` |
| Linux | `~/.local/share/lector-comics` | `~/.cache/lector-comics` | `~/.config/lector-comics` |

Los ajustes de las novedades van en `novedades.txt`, junto a las preferencias. La
contraseña de Metron se guarda ahí **sin cifrar**: mejor una que no uses en otros sitios.

Con la variable de entorno `LECTOR_COMICS_DATOS` se guarda todo en otra carpeta, para
probar con una biblioteca aparte sin tocar la de verdad.

Con `LECTOR_COMICS_CAPTURA=<carpeta>`, la app se abre fuera de la pantalla y sin coger el
foco, pasa por cada página (biblioteca, novedades, una ficha, el lector, el tema claro…),
guarda una imagen de cada una en esa carpeta y se cierra: sirve para revisar la interfaz
sin tocar la pantalla.

## Aspecto

Sigue el sistema de diseño de Linear: fondo casi negro, paneles un escalón más claros con
una línea fina, un solo color de acento (un ámbar oscuro) para la acción principal, lo elegido y
el progreso, y sin sombras. Detrás del contenido se ve, en gris, algo difuminada y muy
tenue, la portada del último cómic que estás leyendo (en el lector no). El texto va en [Inter](https://rsms.me/inter/) y las cifras
que cambian en IBM Plex Mono; las dos fuentes van dentro del programa (licencia OFL, en
`assets/fuentes`).

## Estructura

| Crate | Qué hace |
|---|---|
| `comic-core` | Abrir cómics de cada formato y leer sus metadatos |
| `comic-imagen` | Decodificar y encoger imágenes (se optimiza también en desarrollo) |
| `comic-biblioteca` | Base de datos SQLite: carpetas, escaneo, series y progreso |
| `comic-novedades` | Novedades: AniList, Metron y RSS (solo consulta, en un hilo aparte) |
| `comic-app` | La interfaz (egui): biblioteca, novedades, miniaturas y lector |

Para probar el escaneo sin la interfaz:
`cargo run -p comic-biblioteca --example escanear -- "ruta/a/tus/comics"`.

Para probar las novedades sin la interfaz (guarda en esa biblioteca a qué serie del
catálogo corresponde cada una; con `METRON_USUARIO` y `METRON_CONTRASENA` consulta
también Metron):
`cargo run -p comic-novedades --example comprobar -- "ruta/a/biblioteca.db" [rss...]`.
