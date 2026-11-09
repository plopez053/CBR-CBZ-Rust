//! Tablas de la base de datos. Cada versión nueva del esquema se añade al final de
//! `MIGRACIONES`, y al abrir una base de datos antigua se aplican las que le falten.
//! La versión que tiene cada base de datos se guarda en `PRAGMA user_version`.

use rusqlite::Connection;

const MIGRACIONES: [&str; 12] = [
    // 1: carpetas, cómics y progreso.
    "
    CREATE TABLE carpetas (
        id   INTEGER PRIMARY KEY,
        ruta TEXT NOT NULL UNIQUE
    );

    CREATE TABLE comics (
        id         INTEGER PRIMARY KEY,
        carpeta    INTEGER NOT NULL REFERENCES carpetas(id) ON DELETE CASCADE,
        ruta       TEXT NOT NULL UNIQUE,
        -- Tamaño y fecha de modificación (ms) en el último escaneo: si no cambian,
        -- el archivo no se vuelve a abrir.
        tamano     INTEGER NOT NULL,
        modificado INTEGER NOT NULL,
        huella     TEXT NOT NULL,
        formato    TEXT NOT NULL,
        paginas    INTEGER NOT NULL,
        error      TEXT,
        -- Nunca vacía: si no viene en los metadatos, es el nombre de la carpeta.
        serie      TEXT NOT NULL,
        numero     TEXT,
        volumen    INTEGER,
        total      INTEGER,
        titulo     TEXT,
        anio       INTEGER,
        mes        INTEGER,
        resumen    TEXT,
        guion      TEXT,
        dibujo     TEXT,
        editorial  TEXT,
        generos    TEXT,
        idioma     TEXT,
        derecha_a_izquierda INTEGER NOT NULL,
        portada    INTEGER
    );
    CREATE INDEX comics_serie ON comics(serie COLLATE NOCASE);
    CREATE INDEX comics_huella ON comics(huella);

    -- Va por huella y no por cómic: sobrevive a mover o renombrar el archivo, y a
    -- quitar la carpeta de la biblioteca y volver a añadirla.
    CREATE TABLE progreso (
        huella    TEXT PRIMARY KEY,
        pagina    INTEGER NOT NULL,
        terminado INTEGER NOT NULL,
        leido_en  INTEGER NOT NULL
    );
    ",
    // 2: series agrupadas por clave (sin mayúsculas ni puntuación), uniones manuales y
    // versión del análisis de cada cómic.
    "
    ALTER TABLE comics ADD COLUMN serie_clave TEXT NOT NULL DEFAULT '';
    ALTER TABLE comics ADD COLUMN analisis INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX comics_serie_clave ON comics(serie_clave);

    -- El usuario ha dicho que la serie `clave` es la misma que `destino`.
    CREATE TABLE series_unidas (
        clave   TEXT PRIMARY KEY,
        destino TEXT NOT NULL
    );
    ",
    // 3: la serie que sale de la carpeta, para los cómics que no dicen la suya. Es una
    // pista débil: si hay cerca una serie con nombre fiable que encaja, se usa esa.
    "
    ALTER TABLE comics ADD COLUMN serie_carpeta TEXT;
    ",
    // 4: a qué serie de un catálogo en internet (AniList, Metron) corresponde cada serie,
    // para avisar de los números nuevos.
    "
    CREATE TABLE series_externas (
        -- La clave del grupo, la de `Serie::clave`.
        clave   TEXT PRIMARY KEY,
        -- Vacíos si no se encontró o el usuario no quiere que se busque.
        fuente  TEXT,
        externo TEXT,
        nombre  TEXT,
        estado  TEXT NOT NULL,
        -- Cuándo se buscó por última vez, en segundos desde 1970.
        buscada INTEGER NOT NULL
    );
    ",
    // 5: nombres puestos a mano, sin tocar los archivos: para una serie (por su clave) o
    // para un cómic suelto (por su huella).
    "
    CREATE TABLE series_nombres (
        clave  TEXT PRIMARY KEY,
        nombre TEXT NOT NULL
    );
    CREATE TABLE comics_nombres (
        huella TEXT PRIMARY KEY,
        titulo TEXT NOT NULL
    );
    ",
    // 6: cómics que el usuario ha quitado de la colección (sin tocar el archivo). No se
    // vuelven a añadir solos en el siguiente escaneo.
    "
    CREATE TABLE comics_ignorados (
        huella TEXT PRIMARY KEY
    );
    ",
    // 7: favoritos de las novedades (obras y números de los catálogos), por su clave en
    // el catálogo. Se guarda lo justo para enseñarlos sin pedir nada a internet.
    "
    CREATE TABLE favoritos (
        clave TEXT PRIMARY KEY,
        titulo TEXT NOT NULL,
        portada TEXT,
        linea TEXT NOT NULL DEFAULT '',
        anadido INTEGER NOT NULL
    );
    ",
    // 8: carpetas propias de la biblioteca, que no existen en el disco: su `ruta` es el
    // nombre. Los cómics sueltos que el usuario les añade a mano van en `archivos_propios`
    // y el escaneo los lee como si los hubiera encontrado en una carpeta.
    "
    ALTER TABLE carpetas ADD COLUMN propia INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE archivos_propios (
        carpeta INTEGER NOT NULL REFERENCES carpetas(id) ON DELETE CASCADE,
        ruta    TEXT NOT NULL UNIQUE
    );
    ",
    // 9: la página que el usuario ha elegido como portada de un cómic (por su huella).
    // Gana a la que diga su `ComicInfo.xml`.
    "
    CREATE TABLE portadas_elegidas (
        huella TEXT PRIMARY KEY,
        pagina INTEGER NOT NULL
    );
    ",
    // 10: el cómic cuya portada hace de portada de una serie (por la clave de la serie y la
    // huella del cómic). Sin fila, vale la del primer cómic de la serie.
    "
    CREATE TABLE series_portadas (
        clave  TEXT PRIMARY KEY,
        huella TEXT NOT NULL
    );
    ",
    // 11: cómics que el usuario ha metido a mano en una serie, por la ruta del archivo (o
    // carpeta): mandan sobre la serie que salga de sus metadatos. Los que no estaban en
    // la biblioteca se guardan además en `archivos_propios`, dentro de una carpeta propia
    // oculta que solo existe para darles dónde estar.
    "
    ALTER TABLE carpetas ADD COLUMN oculta INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE series_forzadas (
        ruta  TEXT PRIMARY KEY,
        -- La clave del grupo, la de `Serie::clave`.
        grupo TEXT NOT NULL
    );
    ",
    // 12: el orden que el usuario ha dado a mano a los cómics de una serie. Se guarda la
    // lista entera (por huella) cada vez que se mueve uno; sin filas, vale el orden de
    // lectura de siempre.
    "
    CREATE TABLE series_orden (
        -- La clave del grupo, la de `Serie::clave`.
        grupo    TEXT NOT NULL,
        huella   TEXT NOT NULL,
        posicion INTEGER NOT NULL,
        PRIMARY KEY (grupo, huella)
    );
    ",
];

pub fn migrar(db: &Connection) -> Result<(), String> {
    let version: i64 = db
        .pragma_query_value(None, "user_version", |fila| fila.get(0))
        .map_err(|e| e.to_string())?;
    let version = version as usize;

    if version > MIGRACIONES.len() {
        return Err("La biblioteca es de una versión más nueva del lector".to_string());
    }

    for (i, sql) in MIGRACIONES.iter().enumerate().skip(version) {
        // Cada migración entera o nada: si falla a medias, la base de datos queda como estaba.
        db.execute_batch(&format!(
            "BEGIN; {sql}; PRAGMA user_version = {}; COMMIT;",
            i + 1
        ))
        .map_err(|e| {
            format!(
                "No se pudo actualizar la biblioteca a la versión {}: {e}",
                i + 1
            )
        })?;
    }
    Ok(())
}
