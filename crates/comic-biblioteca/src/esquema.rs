//! Tablas de la base de datos. Cada versión nueva del esquema se añade al final de
//! `MIGRACIONES`, y al abrir una base de datos antigua se aplican las que le falten.
//! La versión que tiene cada base de datos se guarda en `PRAGMA user_version`.

use rusqlite::Connection;

const MIGRACIONES: [&str; 1] = [
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
