//! Biblioteca: las carpetas que vigila el usuario, los cómics que hay en ellas, sus
//! metadatos y por dónde va leyendo cada uno. Todo se guarda en una base de datos SQLite.
//!
//! La biblioteca nunca toca los archivos de los cómics: solo los lee.

mod escaneo;
mod esquema;
mod externas;
mod favoritos;
mod serie;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use comic_core::Metadatos;
use rusqlite::{params, Connection, OptionalExtension, Row};

pub use escaneo::{huella, Resumen};
pub use externas::{Coleccion, EstadoVinculo, SerieExterna, Vinculo};
pub use favoritos::Favorito;
pub use serie::clave_de_serie;

use serie::nombre_de_grupo;

pub struct Biblioteca {
    db: Connection,
}

/// Una carpeta de la biblioteca: una del disco que el usuario ha añadido y que se vigila,
/// o una propia, que solo existe en la biblioteca y guarda los cómics sueltos que se le
/// añadan a mano.
#[derive(Clone, Debug, PartialEq)]
pub struct Carpeta {
    pub id: i64,
    /// En una carpeta propia, su nombre.
    pub ruta: PathBuf,
    pub propia: bool,
    /// Cuántos cómics tiene ahora mismo.
    pub comics: usize,
}

/// Por dónde va la lectura de un cómic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progreso {
    pub pagina: usize,
    pub terminado: bool,
    /// Cuándo se leyó por última vez, en segundos desde 1970.
    pub leido_en: i64,
}

/// Todo lo que la biblioteca sabe de un cómic.
#[derive(Clone, Debug)]
pub struct Ficha {
    pub id: i64,
    pub ruta: PathBuf,
    /// Identifica el cómic aunque se mueva o se renombre.
    pub huella: String,
    pub formato: String,
    pub paginas: usize,
    /// Por qué no se pudo abrir, si no se pudo.
    pub error: Option<String>,
    /// La serie siempre está: si no venía en los metadatos, es el nombre de la carpeta.
    pub metadatos: Metadatos,
    pub progreso: Option<Progreso>,
    /// El nombre que el usuario le ha puesto a mano, si lo ha hecho. Sustituye al que
    /// saldría de los metadatos o del archivo; el archivo no se toca.
    pub nombre_personalizado: Option<String>,
}

impl Ficha {
    pub fn serie(&self) -> &str {
        self.metadatos.serie.as_deref().unwrap_or_default()
    }

    pub fn portada(&self) -> Portada {
        Portada {
            ruta: self.ruta.clone(),
            huella: self.huella.clone(),
            pagina: self.metadatos.portada.unwrap_or(0),
        }
    }
}

/// Dónde está la portada de un cómic, para hacer su miniatura.
#[derive(Clone, Debug, PartialEq)]
pub struct Portada {
    pub ruta: PathBuf,
    /// La miniatura se guarda por huella: si el cómic se mueve, sirve la misma.
    pub huella: String,
    /// Normalmente la primera, salvo que su `ComicInfo.xml` diga otra.
    pub pagina: usize,
}

/// Una serie con los cómics que tiene.
#[derive(Clone, Debug, PartialEq)]
pub struct Serie {
    /// Cómo se muestra: el nombre más repetido entre sus cómics.
    pub nombre: String,
    /// Identifica la serie; es la misma para `Vinland Saga` y `vinland-saga`.
    pub clave: String,
    pub comics: usize,
    pub terminados: usize,
    /// La del cómic que el usuario haya elegido o, si no, la del primero de la serie.
    pub portada: Portada,
}

/// Dónde se pone un cómic al ordenar a mano los de una serie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Movimiento {
    Principio,
    Antes,
    Despues,
    Final,
}

/// Una serie que se ha unido a otra.
#[derive(Clone, Debug, PartialEq)]
pub struct SerieUnida {
    pub clave: String,
    pub nombre: String,
}

/// Columnas que lee [`ficha_de`], en su orden.
const SELECCION: &str = "
    SELECT c.id, c.ruta, c.huella, c.formato, c.paginas, c.error,
           c.serie, c.numero, c.volumen, c.total, c.titulo, c.anio, c.mes, c.resumen,
           c.guion, c.dibujo, c.editorial, c.generos, c.idioma,
           c.derecha_a_izquierda, COALESCE(e.pagina, c.portada),
           p.pagina, p.terminado, p.leido_en, n.titulo
    FROM comics c
         LEFT JOIN progreso p ON p.huella = c.huella
         LEFT JOIN series_unidas u ON u.clave = c.serie_clave
         LEFT JOIN comics_nombres n ON n.huella = c.huella
         LEFT JOIN portadas_elegidas e ON e.huella = c.huella";

/// La serie a la que pertenece un cómic de [`SELECCION`]: la suya, o la que el usuario
/// le haya dicho que es la misma.
const GRUPO: &str = "COALESCE(u.destino, c.serie_clave)";

fn ficha_de(fila: &Row) -> rusqlite::Result<Ficha> {
    let progreso = match fila.get::<_, Option<i64>>(21)? {
        Some(pagina) => Some(Progreso {
            pagina: pagina as usize,
            terminado: fila.get(22)?,
            leido_en: fila.get(23)?,
        }),
        None => None,
    };

    Ok(Ficha {
        id: fila.get(0)?,
        ruta: PathBuf::from(fila.get::<_, String>(1)?),
        huella: fila.get(2)?,
        formato: fila.get(3)?,
        paginas: fila.get::<_, i64>(4)? as usize,
        error: fila.get(5)?,
        metadatos: Metadatos {
            serie: Some(fila.get(6)?),
            numero: fila.get(7)?,
            volumen: fila.get(8)?,
            total: fila.get(9)?,
            titulo: fila.get(10)?,
            anio: fila.get(11)?,
            mes: fila.get(12)?,
            resumen: fila.get(13)?,
            guion: fila.get(14)?,
            dibujo: fila.get(15)?,
            editorial: fila.get(16)?,
            generos: fila.get(17)?,
            idioma: fila.get(18)?,
            derecha_a_izquierda: fila.get(19)?,
            portada: fila.get::<_, Option<i64>>(20)?.map(|p| p as usize),
        },
        progreso,
        nombre_personalizado: fila.get(24)?,
    })
}

/// Orden en el que se leen los cómics de una serie: por volumen y, dentro de cada
/// volumen, por número en orden natural (el 2 antes que el 10).
fn orden_de_lectura(
    (volumen_a, numero_a, ruta_a): (Option<u32>, Option<&str>, &str),
    (volumen_b, numero_b, ruta_b): (Option<u32>, Option<&str>, &str),
) -> Ordering {
    volumen_a
        .cmp(&volumen_b)
        .then_with(|| natord::compare(numero_a.unwrap_or(""), numero_b.unwrap_or("")))
        .then_with(|| natord::compare(ruta_a, ruta_b))
}

fn clave_de_orden(ficha: &Ficha) -> (Option<u32>, Option<&str>, &str) {
    (
        ficha.metadatos.volumen,
        ficha.metadatos.numero.as_deref(),
        ficha.ruta.to_str().unwrap_or_default(),
    )
}

/// Las rutas se guardan como texto.
fn texto_de(ruta: &Path) -> String {
    ruta.to_string_lossy().into_owned()
}

/// Las bibliotecas creadas antes de agrupar por clave no la tienen: se calcula a partir
/// de la serie que ya guardaban, sin esperar a un escaneo.
fn rellenar_claves(db: &Connection) -> Result<(), String> {
    let mut consulta = db
        .prepare("SELECT id, serie FROM comics WHERE serie_clave = ''")
        .map_err(|e| e.to_string())?;
    let sin_clave: Vec<(i64, String)> = consulta
        .query_map([], |f| Ok((f.get(0)?, f.get(1)?)))
        .and_then(Iterator::collect)
        .map_err(|e| e.to_string())?;
    for (id, serie) in sin_clave {
        db.execute(
            "UPDATE comics SET serie_clave = ?2 WHERE id = ?1",
            params![id, clave_de_serie(&serie)],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn ahora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Biblioteca {
    /// Abre la biblioteca guardada en `ruta`, o la crea si no existe.
    pub fn abrir(ruta: impl AsRef<Path>) -> Result<Biblioteca, String> {
        let ruta = ruta.as_ref();
        if let Some(padre) = ruta.parent() {
            std::fs::create_dir_all(padre).map_err(|e| e.to_string())?;
        }

        let db = Connection::open(ruta).map_err(|e| e.to_string())?;
        // WAL deja leer mientras otro hilo escribe: la interfaz podrá mostrar la biblioteca
        // mientras se escanea en segundo plano.
        db.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))
            .map_err(|e| e.to_string())?;
        // SQLite no aplica `ON DELETE CASCADE` si no se le pide.
        db.pragma_update(None, "foreign_keys", true)
            .map_err(|e| e.to_string())?;
        esquema::migrar(&db)?;
        rellenar_claves(&db)?;

        Ok(Biblioteca { db })
    }

    /// Las carpetas que el usuario ve (no la oculta donde van los cómics metidos a mano en
    /// una serie).
    pub fn carpetas(&self) -> Result<Vec<Carpeta>, String> {
        Ok(self
            .todas_las_carpetas()?
            .into_iter()
            .filter(|(_, oculta)| !oculta)
            .map(|(carpeta, _)| carpeta)
            .collect())
    }

    /// Todas las carpetas, con si están ocultas.
    fn todas_las_carpetas(&self) -> Result<Vec<(Carpeta, bool)>, String> {
        let mut consulta = self
            .db
            .prepare(
                "SELECT c.id, c.ruta, c.propia, COUNT(m.id), c.oculta
                 FROM carpetas c LEFT JOIN comics m ON m.carpeta = c.id
                 GROUP BY c.id
                 ORDER BY c.propia, c.ruta",
            )
            .map_err(|e| e.to_string())?;
        let carpetas = consulta
            .query_map([], |fila| {
                let carpeta = Carpeta {
                    id: fila.get(0)?,
                    ruta: PathBuf::from(fila.get::<_, String>(1)?),
                    propia: fila.get(2)?,
                    comics: fila.get::<_, i64>(3)? as usize,
                };
                Ok((carpeta, fila.get(4)?))
            })
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;
        Ok(carpetas)
    }

    /// Añade una carpeta. Sus cómics aparecen en el siguiente [`escanear`](Self::escanear).
    ///
    /// No deja añadir una carpeta que esté dentro de otra ya añadida (ni al revés):
    /// sus cómics saldrían repetidos.
    pub fn agregar_carpeta(&self, ruta: impl AsRef<Path>) -> Result<Carpeta, String> {
        let ruta = std::path::absolute(ruta.as_ref()).map_err(|e| e.to_string())?;
        if !ruta.is_dir() {
            return Err(format!("{} no es una carpeta", ruta.display()));
        }
        for otra in self.carpetas()?.into_iter().filter(|c| !c.propia) {
            if ruta.starts_with(&otra.ruta) || otra.ruta.starts_with(&ruta) {
                return Err(format!(
                    "{} se solapa con {}, que ya está en la biblioteca",
                    ruta.display(),
                    otra.ruta.display()
                ));
            }
        }

        self.db
            .execute("INSERT INTO carpetas (ruta) VALUES (?1)", [texto_de(&ruta)])
            .map_err(|e| e.to_string())?;
        Ok(Carpeta {
            id: self.db.last_insert_rowid(),
            ruta,
            propia: false,
            comics: 0,
        })
    }

    /// Crea una carpeta propia, vacía: no existe en el disco, solo en la biblioteca. Sirve
    /// para juntar cómics sueltos que no están en ninguna carpeta vigilada (ver
    /// [`anadir_archivos`](Self::anadir_archivos)).
    pub fn crear_carpeta_propia(&self, nombre: &str) -> Result<Carpeta, String> {
        let nombre = nombre.trim();
        if nombre.is_empty() {
            return Err("La carpeta necesita un nombre".to_string());
        }
        let repetida: bool = self
            .db
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM carpetas WHERE propia = 1 AND ruta = ?1 COLLATE NOCASE)",
                [nombre],
                |fila| fila.get(0),
            )
            .map_err(|e| e.to_string())?;
        if repetida {
            return Err(format!("Ya hay una carpeta que se llama {nombre}"));
        }
        self.db
            .execute(
                "INSERT INTO carpetas (ruta, propia) VALUES (?1, 1)",
                [nombre],
            )
            .map_err(|e| e.to_string())?;
        Ok(Carpeta {
            id: self.db.last_insert_rowid(),
            ruta: PathBuf::from(nombre),
            propia: true,
            comics: 0,
        })
    }

    /// Añade cómics sueltos a una carpeta propia. Sus archivos no se tocan; aparecen en el
    /// siguiente [`escanear`](Self::escanear). Se saltan los que no son archivos de cómic
    /// y los que la biblioteca ya tiene (en una carpeta vigilada o en otra propia).
    /// Devuelve cuántos se han añadido.
    pub fn anadir_archivos(&self, carpeta: i64, archivos: &[PathBuf]) -> Result<usize, String> {
        let es_propia: bool = self
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM carpetas WHERE id = ?1 AND propia = 1)",
                [carpeta],
                |fila| fila.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !es_propia {
            return Err("Solo se pueden añadir cómics sueltos a una carpeta propia".to_string());
        }

        let mut anadidos = 0;
        for archivo in archivos {
            let ruta = std::path::absolute(archivo).map_err(|e| e.to_string())?;
            let valido = ruta.is_file()
                && ruta.to_str().is_some()
                && escaneo::es_archivo_de_comic(&ruta.to_string_lossy());
            if !valido {
                continue;
            }
            let texto = texto_de(&ruta);
            let ya_esta: bool = self
                .db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM comics WHERE ruta = ?1)
                         OR EXISTS(SELECT 1 FROM archivos_propios WHERE ruta = ?1)",
                    [&texto],
                    |fila| fila.get(0),
                )
                .map_err(|e| e.to_string())?;
            if ya_esta {
                continue;
            }
            self.db
                .execute(
                    "INSERT INTO archivos_propios (carpeta, ruta) VALUES (?1, ?2)",
                    params![carpeta, texto],
                )
                .map_err(|e| e.to_string())?;
            anadidos += 1;
        }
        Ok(anadidos)
    }

    /// La carpeta oculta donde viven los cómics metidos a mano en una serie que no estaban
    /// en ninguna otra. Se crea la primera vez que hace falta.
    fn carpeta_de_series(&self) -> Result<i64, String> {
        let existente: Option<i64> = self
            .db
            .query_row("SELECT id FROM carpetas WHERE oculta = 1", [], |f| f.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(id) = existente {
            return Ok(id);
        }
        self.db
            .execute(
                "INSERT INTO carpetas (ruta, propia, oculta) VALUES (char(1) || 'series', 1, 1)",
                [],
            )
            .map_err(|e| e.to_string())?;
        Ok(self.db.last_insert_rowid())
    }

    /// Mete cómics en la serie de clave `grupo` (la de [`Serie::clave`]), mande lo que
    /// mande su `ComicInfo.xml` o su nombre. Cada ruta puede ser un archivo de cómic, una
    /// carpeta con cómics (se buscan también en sus subcarpetas) o una carpeta de imágenes.
    /// Los archivos no se tocan; los que la biblioteca aún no tenía aparecen en el siguiente
    /// [`escanear`](Self::escanear). Devuelve cuántos han cambiado de serie o se han añadido.
    pub fn anadir_a_serie(&self, grupo: &str, rutas: &[PathBuf]) -> Result<usize, String> {
        let existe: bool = self
            .db
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM comics c
                         LEFT JOIN series_unidas u ON u.clave = c.serie_clave
                     WHERE {GRUPO} = ?1)"
                ),
                [grupo],
                |fila| fila.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !existe {
            return Err("Esa serie ya no existe".to_string());
        }

        let mut anadidos = 0;
        for ruta in rutas {
            let ruta = std::path::absolute(ruta).map_err(|e| e.to_string())?;
            for comic in escaneo::comics_en(&ruta) {
                let texto = texto_de(&comic);
                let actual: Option<String> = self
                    .db
                    .query_row(
                        &format!(
                            "SELECT {GRUPO} FROM comics c
                                 LEFT JOIN series_unidas u ON u.clave = c.serie_clave
                             WHERE c.ruta = ?1"
                        ),
                        [&texto],
                        |fila| fila.get(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                let ya_forzado: bool = self
                    .db
                    .query_row(
                        "SELECT EXISTS(
                             SELECT 1 FROM series_forzadas WHERE ruta = ?1 AND grupo = ?2)",
                        params![texto, grupo],
                        |fila| fila.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if ya_forzado || actual.as_deref() == Some(grupo) {
                    continue;
                }
                if actual.is_none() {
                    // Aún no está en la biblioteca: se guarda como un cómic suelto más.
                    let propio: bool = self
                        .db
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM archivos_propios WHERE ruta = ?1)",
                            [&texto],
                            |fila| fila.get(0),
                        )
                        .map_err(|e| e.to_string())?;
                    if !propio {
                        self.db
                            .execute(
                                "INSERT INTO archivos_propios (carpeta, ruta) VALUES (?1, ?2)",
                                params![self.carpeta_de_series()?, texto],
                            )
                            .map_err(|e| e.to_string())?;
                    }
                }
                self.db
                    .execute(
                        "INSERT INTO series_forzadas (ruta, grupo) VALUES (?1, ?2)
                         ON CONFLICT(ruta) DO UPDATE SET grupo = excluded.grupo",
                        params![texto, grupo],
                    )
                    .map_err(|e| e.to_string())?;
                anadidos += 1;
            }
        }
        Ok(anadidos)
    }

    /// Quita una carpeta y sus cómics de la biblioteca (los archivos no se tocan).
    /// El progreso se conserva: si se vuelve a añadir, cada cómic sigue por donde iba.
    pub fn quitar_carpeta(&self, id: i64) -> Result<(), String> {
        self.db
            .execute("DELETE FROM carpetas WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Todas las series, por nombre en orden natural.
    pub fn series(&self) -> Result<Vec<Serie>, String> {
        let mut consulta = self
            .db
            .prepare(&format!(
                "SELECT {GRUPO}, c.serie, c.volumen, c.numero, c.ruta, c.huella,
                        COALESCE(e.pagina, c.portada), p.terminado
                 FROM comics c
                     LEFT JOIN progreso p ON p.huella = c.huella
                     LEFT JOIN series_unidas u ON u.clave = c.serie_clave
                     LEFT JOIN portadas_elegidas e ON e.huella = c.huella"
            ))
            .map_err(|e| e.to_string())?;

        // Solo lo necesario para agrupar y ordenar: leer las fichas enteras (con sus
        // resúmenes) de miles de cómics sería mucho más lento.
        struct Fila {
            grupo: String,
            serie: String,
            volumen: Option<u32>,
            numero: Option<String>,
            ruta: String,
            huella: String,
            portada: Option<i64>,
            terminado: Option<bool>,
        }
        impl Fila {
            fn clave(&self) -> (Option<u32>, Option<&str>, &str) {
                (self.volumen, self.numero.as_deref(), &self.ruta)
            }
        }

        let filas: Vec<Fila> = consulta
            .query_map([], |f| {
                Ok(Fila {
                    grupo: f.get(0)?,
                    serie: f.get(1)?,
                    volumen: f.get(2)?,
                    numero: f.get(3)?,
                    ruta: f.get(4)?,
                    huella: f.get(5)?,
                    portada: f.get(6)?,
                    terminado: f.get(7)?,
                })
            })
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;

        let nombres_puestos = self.nombres_de_series()?;
        let portadas_puestas = self.portadas_de_series()?;
        let orden_puesto = self.ordenes_manuales()?;

        let mut grupos: HashMap<&str, Vec<&Fila>> = HashMap::new();
        for fila in &filas {
            grupos.entry(&fila.grupo).or_default().push(fila);
        }

        let mut series: Vec<Serie> = grupos
            .into_iter()
            .map(|(clave, comics)| {
                // La que el usuario haya elegido, si ese cómic sigue en la serie; si no, la
                // del primero.
                let elegido = portadas_puestas
                    .get(clave)
                    .and_then(|huella| comics.iter().find(|c| &c.huella == huella));
                let primero = elegido.unwrap_or_else(|| {
                    // El primero según el orden que se muestra: el puesto a mano, si lo hay.
                    let sitio = |c: &Fila| {
                        orden_puesto
                            .get(&(clave.to_string(), c.huella.clone()))
                            .copied()
                            .unwrap_or(i64::MAX)
                    };
                    comics
                        .iter()
                        .min_by(|a, b| {
                            sitio(a)
                                .cmp(&sitio(b))
                                .then_with(|| orden_de_lectura(a.clave(), b.clave()))
                        })
                        .expect("un grupo nunca está vacío")
                });
                let nombre = nombres_puestos
                    .get(clave)
                    .cloned()
                    .unwrap_or_else(|| nombre_de_grupo(comics.iter().map(|c| c.serie.as_str())));
                Serie {
                    nombre,
                    clave: clave.to_string(),
                    comics: comics.len(),
                    terminados: comics.iter().filter(|c| c.terminado == Some(true)).count(),
                    portada: Portada {
                        ruta: PathBuf::from(&primero.ruta),
                        huella: primero.huella.clone(),
                        pagina: primero.portada.unwrap_or(0) as usize,
                    },
                }
            })
            .collect();
        series.sort_by(|a, b| natord::compare_ignore_case(&a.nombre, &b.nombre));
        Ok(series)
    }

    /// El orden puesto a mano de todas las series, por (clave de la serie, huella).
    fn ordenes_manuales(&self) -> Result<HashMap<(String, String), i64>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT grupo, huella, posicion FROM series_orden")
            .map_err(|e| e.to_string())?;
        consulta
            .query_map([], |f| Ok(((f.get(0)?, f.get(1)?), f.get(2)?)))
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())
    }

    /// Los cómics que el usuario ha elegido para la portada de cada serie: la huella del
    /// cómic, por la clave de la serie.
    fn portadas_de_series(&self) -> Result<HashMap<String, String>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT clave, huella FROM series_portadas")
            .map_err(|e| e.to_string())?;
        consulta
            .query_map([], |f| Ok((f.get(0)?, f.get(1)?)))
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())
    }

    /// Hace que la portada de la serie de clave `clave` (la de [`Serie::clave`]) sea la
    /// del cómic de huella `huella`, o, con `None`, vuelve a ser la del primero. Los
    /// archivos no se tocan.
    pub fn elegir_portada_de_serie(&self, clave: &str, huella: Option<&str>) -> Result<(), String> {
        match huella {
            Some(huella) => self.db.execute(
                "INSERT INTO series_portadas (clave, huella) VALUES (?1, ?2)
                 ON CONFLICT(clave) DO UPDATE SET huella = excluded.huella",
                params![clave, huella],
            ),
            None => self
                .db
                .execute("DELETE FROM series_portadas WHERE clave = ?1", [clave]),
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Los nombres que el usuario ha puesto a mano a series, por su clave.
    fn nombres_de_series(&self) -> Result<HashMap<String, String>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT clave, nombre FROM series_nombres")
            .map_err(|e| e.to_string())?;
        consulta
            .query_map([], |f| Ok((f.get(0)?, f.get(1)?)))
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())
    }

    /// Pone el nombre con que se muestra la serie de clave `clave` (la de
    /// [`Serie::clave`]), sin tocar los archivos. Con `nombre` en blanco, vuelve a
    /// mostrarse con el nombre automático.
    pub fn renombrar_serie(&self, clave: &str, nombre: &str) -> Result<(), String> {
        let nombre = nombre.trim();
        if nombre.is_empty() {
            self.db
                .execute("DELETE FROM series_nombres WHERE clave = ?1", [clave])
        } else {
            self.db.execute(
                "INSERT INTO series_nombres (clave, nombre) VALUES (?1, ?2)
                 ON CONFLICT(clave) DO UPDATE SET nombre = excluded.nombre",
                params![clave, nombre],
            )
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Pone el nombre con que se muestra el cómic de huella `huella`, sin tocar el
    /// archivo. Con `nombre` en blanco, vuelve a mostrarse el automático (de
    /// `ComicInfo.xml` o del nombre del archivo).
    pub fn renombrar_comic(&self, huella: &str, nombre: &str) -> Result<(), String> {
        let nombre = nombre.trim();
        if nombre.is_empty() {
            self.db
                .execute("DELETE FROM comics_nombres WHERE huella = ?1", [huella])
        } else {
            self.db.execute(
                "INSERT INTO comics_nombres (huella, titulo) VALUES (?1, ?2)
                 ON CONFLICT(huella) DO UPDATE SET titulo = excluded.titulo",
                params![huella, nombre],
            )
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Quita de la colección los cómics de huella `huella` (el archivo no se toca). El
    /// siguiente escaneo no los vuelve a añadir solos; para eso hace falta quitar y
    /// volver a añadir la carpeta.
    pub fn quitar_comic(&self, huella: &str) -> Result<(), String> {
        let transaccion = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        // A uno añadido a mano a una carpeta propia le basta con salir de su lista: nadie
        // lo va a volver a encontrar solo.
        let de_carpeta_propia = transaccion
            .execute(
                "DELETE FROM archivos_propios
                 WHERE ruta IN (SELECT ruta FROM comics WHERE huella = ?1)",
                [huella],
            )
            .map_err(|e| e.to_string())?;
        transaccion
            .execute(
                "DELETE FROM series_forzadas
                 WHERE ruta IN (SELECT ruta FROM comics WHERE huella = ?1)",
                [huella],
            )
            .map_err(|e| e.to_string())?;
        transaccion
            .execute("DELETE FROM comics WHERE huella = ?1", [huella])
            .map_err(|e| e.to_string())?;
        if de_carpeta_propia == 0 {
            transaccion
                .execute(
                    "INSERT OR IGNORE INTO comics_ignorados (huella) VALUES (?1)",
                    [huella],
                )
                .map_err(|e| e.to_string())?;
        }
        transaccion.commit().map_err(|e| e.to_string())
    }

    /// La clave del grupo al que pertenece una serie, escrita como se escriba.
    fn grupo_de(&self, serie: &str) -> Result<String, String> {
        let clave = clave_de_serie(serie);
        let unida: Option<String> = self
            .db
            .query_row(
                "SELECT destino FROM series_unidas WHERE clave = ?1",
                [&clave],
                |f| f.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(unida.unwrap_or(clave))
    }

    fn cuantos_comics(&self, grupo: &str) -> Result<i64, String> {
        self.db
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM comics c
                         LEFT JOIN series_unidas u ON u.clave = c.serie_clave
                     WHERE {GRUPO} = ?1"
                ),
                [grupo],
                |f| f.get(0),
            )
            .map_err(|e| e.to_string())
    }

    /// Dice que dos series son la misma (por ejemplo, `Vinland` y `Vinland Saga`): sus
    /// cómics pasan a mostrarse juntos. Los archivos no se tocan, y se puede deshacer con
    /// [`separar_serie`](Self::separar_serie). Devuelve la clave de la serie resultante.
    pub fn unir_series(&self, una: &str, otra: &str) -> Result<String, String> {
        let (una, otra) = (self.grupo_de(una)?, self.grupo_de(otra)?);
        if una == otra {
            return Err("Esas series ya están unidas".to_string());
        }
        // La serie con más cómics es la que se queda: así la que aparece como "unida" es
        // siempre la pequeña, y el nombre que se muestra es el de la grande.
        let (origen, destino) = if self.cuantos_comics(&una)? > self.cuantos_comics(&otra)? {
            (otra, una)
        } else {
            (una, otra)
        };

        let transaccion = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        // Lo que ya estaba unido a `origen` pasa con él: las uniones nunca se encadenan.
        transaccion
            .execute(
                "UPDATE series_unidas SET destino = ?2 WHERE destino = ?1",
                [&origen, &destino],
            )
            .map_err(|e| e.to_string())?;
        transaccion
            .execute(
                "INSERT OR REPLACE INTO series_unidas (clave, destino) VALUES (?1, ?2)",
                [&origen, &destino],
            )
            .map_err(|e| e.to_string())?;
        transaccion.commit().map_err(|e| e.to_string())?;
        Ok(destino)
    }

    /// Las series que se han unido a la de clave `grupo` (la de [`Serie::clave`]), con
    /// el nombre con que se mostraban antes de unirlas.
    pub fn series_unidas_a(&self, grupo: &str) -> Result<Vec<SerieUnida>, String> {
        let mut consulta = self
            .db
            .prepare(
                "SELECT u.clave, c.serie FROM series_unidas u
                     JOIN comics c ON c.serie_clave = u.clave
                 WHERE u.destino = ?1",
            )
            .map_err(|e| e.to_string())?;
        let filas: Vec<(String, String)> = consulta
            .query_map([grupo], |f| Ok((f.get(0)?, f.get(1)?)))
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;

        let mut por_clave: HashMap<&str, Vec<&str>> = HashMap::new();
        for (clave, serie) in &filas {
            por_clave.entry(clave).or_default().push(serie);
        }
        let mut unidas: Vec<SerieUnida> = por_clave
            .into_iter()
            .map(|(clave, nombres)| SerieUnida {
                clave: clave.to_string(),
                nombre: nombre_de_grupo(nombres.into_iter()),
            })
            .collect();
        unidas.sort_by(|a, b| natord::compare_ignore_case(&a.nombre, &b.nombre));
        Ok(unidas)
    }

    /// Deshace una unión: la serie vuelve a mostrarse aparte.
    pub fn separar_serie(&self, unida: &SerieUnida) -> Result<(), String> {
        self.db
            .execute("DELETE FROM series_unidas WHERE clave = ?1", [&unida.clave])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Los cómics de una serie, en orden de lectura o, si el usuario los ha ordenado a
    /// mano, en el orden que les dio (los que lleguen después van al final).
    pub fn comics_de_serie(&self, serie: &str) -> Result<Vec<Ficha>, String> {
        let grupo = self.grupo_de(serie)?;
        let mut consulta = self
            .db
            .prepare(&format!("{SELECCION} WHERE {GRUPO} = ?1"))
            .map_err(|e| e.to_string())?;
        let mut fichas: Vec<Ficha> = consulta
            .query_map([&grupo], ficha_de)
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;
        fichas.sort_by(|a, b| orden_de_lectura(clave_de_orden(a), clave_de_orden(b)));

        // La ordenación es estable: los que no tienen sitio guardado conservan entre
        // ellos el orden de lectura.
        let manual = self.orden_manual(&grupo)?;
        if !manual.is_empty() {
            fichas.sort_by_key(|f| manual.get(&f.huella).copied().unwrap_or(i64::MAX));
        }
        Ok(fichas)
    }

    /// El sitio que el usuario ha dado a cada cómic de la serie de clave `grupo`, por
    /// huella. Vacío si nunca los ha ordenado.
    fn orden_manual(&self, grupo: &str) -> Result<HashMap<String, i64>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT huella, posicion FROM series_orden WHERE grupo = ?1")
            .map_err(|e| e.to_string())?;
        consulta
            .query_map([grupo], |f| Ok((f.get(0)?, f.get(1)?)))
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())
    }

    /// Mueve el cómic de huella `huella` dentro de su serie. Desde ese momento la serie
    /// se muestra en el orden que quede, no por número. Los archivos no se tocan.
    pub fn mover_comic(
        &self,
        serie: &str,
        huella: &str,
        movimiento: Movimiento,
    ) -> Result<(), String> {
        let huellas = self.huellas_de_serie(serie)?;
        let Some(actual) = huellas.iter().position(|h| h == huella) else {
            return Ok(());
        };
        let ultimo = huellas.len() - 1;
        let destino = match movimiento {
            Movimiento::Principio => 0,
            Movimiento::Antes => actual.saturating_sub(1),
            Movimiento::Despues => (actual + 1).min(ultimo),
            Movimiento::Final => ultimo,
        };
        self.guardar_orden(serie, huellas, actual, destino)
    }

    /// Pone el cómic de huella `huella` en el sitio que ocupa ahora el de huella
    /// `ocupante`, y desplaza los demás. Es lo que pasa al arrastrar un cómic sobre otro.
    /// Los archivos no se tocan.
    pub fn colocar_en_lugar_de(
        &self,
        serie: &str,
        huella: &str,
        ocupante: &str,
    ) -> Result<(), String> {
        let huellas = self.huellas_de_serie(serie)?;
        let (Some(actual), Some(destino)) = (
            huellas.iter().position(|h| h == huella),
            huellas.iter().position(|h| h == ocupante),
        ) else {
            return Ok(());
        };
        self.guardar_orden(serie, huellas, actual, destino)
    }

    /// Las huellas de los cómics de la serie, en el orden en que se muestran. Un cómic
    /// repetido ocupa un solo sitio.
    fn huellas_de_serie(&self, serie: &str) -> Result<Vec<String>, String> {
        let mut huellas: Vec<String> = self
            .comics_de_serie(serie)?
            .into_iter()
            .map(|f| f.huella)
            .collect();
        let mut vistas = std::collections::HashSet::new();
        huellas.retain(|h| vistas.insert(h.clone()));
        Ok(huellas)
    }

    /// Guarda `huellas` con el elemento de `actual` movido a `destino`. Si no se mueve,
    /// no guarda nada: así no se fija un orden que nadie ha cambiado.
    fn guardar_orden(
        &self,
        serie: &str,
        mut huellas: Vec<String>,
        actual: usize,
        destino: usize,
    ) -> Result<(), String> {
        if destino == actual {
            return Ok(());
        }
        let grupo = self.grupo_de(serie)?;
        let movido = huellas.remove(actual);
        huellas.insert(destino, movido);

        let transaccion = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        transaccion
            .execute("DELETE FROM series_orden WHERE grupo = ?1", [&grupo])
            .map_err(|e| e.to_string())?;
        for (posicion, huella) in huellas.iter().enumerate() {
            transaccion
                .execute(
                    "INSERT INTO series_orden (grupo, huella, posicion) VALUES (?1, ?2, ?3)",
                    params![grupo, huella, posicion as i64],
                )
                .map_err(|e| e.to_string())?;
        }
        transaccion.commit().map_err(|e| e.to_string())
    }

    /// Si la serie tiene un orden puesto a mano.
    pub fn tiene_orden_manual(&self, serie: &str) -> Result<bool, String> {
        Ok(!self.orden_manual(&self.grupo_de(serie)?)?.is_empty())
    }

    /// Deshace el orden puesto a mano: la serie vuelve al orden de lectura.
    pub fn restablecer_orden(&self, serie: &str) -> Result<(), String> {
        self.db
            .execute(
                "DELETE FROM series_orden WHERE grupo = ?1",
                [self.grupo_de(serie)?],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// El cómic que se lee después del de huella `huella` dentro de su serie, si lo hay y
    /// se puede abrir. Si la serie tiene el mismo cómic repetido, no se salta a la copia.
    pub fn siguiente_de(&self, huella: &str) -> Result<Option<Ficha>, String> {
        let actual: Option<Ficha> = self
            .db
            .query_row(
                &format!("{SELECCION} WHERE c.huella = ?1 LIMIT 1"),
                [huella],
                ficha_de,
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(actual) = actual else {
            return Ok(None);
        };
        let comics = self.comics_de_serie(actual.serie())?;
        let Some(posicion) = comics.iter().position(|f| f.huella == huella) else {
            return Ok(None);
        };
        Ok(comics
            .into_iter()
            .skip(posicion + 1)
            .find(|f| f.error.is_none() && f.huella != huella))
    }

    /// Pone la página `pagina` (desde 0) como portada del cómic de huella `huella`, o,
    /// con `None`, vuelve a la que le toque por sus metadatos. El archivo no se toca.
    pub fn elegir_portada(&self, huella: &str, pagina: Option<usize>) -> Result<(), String> {
        match pagina {
            Some(pagina) => self.db.execute(
                "INSERT INTO portadas_elegidas (huella, pagina) VALUES (?1, ?2)
                 ON CONFLICT(huella) DO UPDATE SET pagina = excluded.pagina",
                params![huella, pagina as i64],
            ),
            None => self
                .db
                .execute("DELETE FROM portadas_elegidas WHERE huella = ?1", [huella]),
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn ficha(&self, id: i64) -> Result<Option<Ficha>, String> {
        self.db
            .query_row(&format!("{SELECCION} WHERE c.id = ?1"), [id], ficha_de)
            .optional()
            .map_err(|e| e.to_string())
    }

    /// Los cómics empezados y sin terminar, del leído más recientemente al más antiguo.
    pub fn seguir_leyendo(&self, limite: usize) -> Result<Vec<Ficha>, String> {
        let mut consulta = self
            .db
            .prepare(&format!(
                "{SELECCION} WHERE p.terminado = 0 ORDER BY p.leido_en DESC, c.id"
            ))
            .map_err(|e| e.to_string())?;
        let fichas: Vec<Ficha> = consulta
            .query_map([], ficha_de)
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;

        // Dos copias del mismo cómic comparten progreso: basta con que salga una.
        let mut vistas = std::collections::HashSet::new();
        Ok(fichas
            .into_iter()
            .filter(|f| vistas.insert(f.huella.clone()))
            .take(limite)
            .collect())
    }

    /// Apunta por qué página va un cómic (por su huella).
    pub fn guardar_progreso(
        &self,
        huella: &str,
        pagina: usize,
        terminado: bool,
    ) -> Result<(), String> {
        self.db
            .execute(
                "INSERT INTO progreso (huella, pagina, terminado, leido_en)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(huella) DO UPDATE SET
                     pagina = excluded.pagina,
                     terminado = excluded.terminado,
                     leido_en = excluded.leido_en",
                params![huella, pagina as i64, terminado, ahora()],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn progreso(&self, huella: &str) -> Result<Option<Progreso>, String> {
        self.db
            .query_row(
                "SELECT pagina, terminado, leido_en FROM progreso WHERE huella = ?1",
                [huella],
                |f| {
                    Ok(Progreso {
                        pagina: f.get::<_, i64>(0)? as usize,
                        terminado: f.get(1)?,
                        leido_en: f.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())
    }
}
