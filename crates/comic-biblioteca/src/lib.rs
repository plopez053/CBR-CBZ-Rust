//! Biblioteca: las carpetas que vigila el usuario, los cómics que hay en ellas, sus
//! metadatos y por dónde va leyendo cada uno. Todo se guarda en una base de datos SQLite.
//!
//! La biblioteca nunca toca los archivos de los cómics: solo los lee.

mod escaneo;
mod esquema;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use comic_core::Metadatos;
use rusqlite::{params, Connection, OptionalExtension, Row};

pub use escaneo::{huella, Resumen};

pub struct Biblioteca {
    db: Connection,
}

/// Una carpeta que el usuario ha añadido a la biblioteca.
#[derive(Clone, Debug, PartialEq)]
pub struct Carpeta {
    pub id: i64,
    pub ruta: PathBuf,
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
    pub nombre: String,
    pub comics: usize,
    pub terminados: usize,
    /// La del primer cómic de la serie.
    pub portada: Portada,
}

/// Columnas que lee [`ficha_de`], en su orden.
const SELECCION: &str = "
    SELECT c.id, c.ruta, c.huella, c.formato, c.paginas, c.error,
           c.serie, c.numero, c.volumen, c.total, c.titulo, c.anio, c.mes, c.resumen,
           c.guion, c.dibujo, c.editorial, c.generos, c.idioma,
           c.derecha_a_izquierda, c.portada,
           p.pagina, p.terminado, p.leido_en
    FROM comics c LEFT JOIN progreso p ON p.huella = c.huella";

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

        Ok(Biblioteca { db })
    }

    pub fn carpetas(&self) -> Result<Vec<Carpeta>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT id, ruta FROM carpetas ORDER BY ruta")
            .map_err(|e| e.to_string())?;
        let carpetas = consulta
            .query_map([], |fila| {
                Ok(Carpeta {
                    id: fila.get(0)?,
                    ruta: PathBuf::from(fila.get::<_, String>(1)?),
                })
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
        for otra in self.carpetas()? {
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
        })
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
            .prepare(
                "SELECT c.serie, c.volumen, c.numero, c.ruta, c.huella, c.portada, p.terminado
                 FROM comics c LEFT JOIN progreso p ON p.huella = c.huella",
            )
            .map_err(|e| e.to_string())?;
        // Solo lo necesario para agrupar y ordenar: leer las fichas enteras (con sus
        // resúmenes) de miles de cómics sería mucho más lento.
        struct Fila {
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
                    serie: f.get(0)?,
                    volumen: f.get(1)?,
                    numero: f.get(2)?,
                    ruta: f.get(3)?,
                    huella: f.get(4)?,
                    portada: f.get(5)?,
                    terminado: f.get(6)?,
                })
            })
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;

        // Agrupadas sin distinguir mayúsculas: "Saga" y "SAGA" son la misma serie.
        let mut grupos: HashMap<String, Vec<&Fila>> = HashMap::new();
        for fila in &filas {
            grupos
                .entry(fila.serie.to_lowercase())
                .or_default()
                .push(fila);
        }

        let mut series: Vec<Serie> = grupos
            .into_values()
            .map(|comics| {
                let primero = comics
                    .iter()
                    .min_by(|a, b| orden_de_lectura(a.clave(), b.clave()))
                    .expect("un grupo nunca está vacío");
                Serie {
                    nombre: primero.serie.clone(),
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

    /// Los cómics de una serie, en orden de lectura.
    pub fn comics_de_serie(&self, serie: &str) -> Result<Vec<Ficha>, String> {
        let mut consulta = self
            .db
            .prepare(&format!("{SELECCION} WHERE c.serie = ?1 COLLATE NOCASE"))
            .map_err(|e| e.to_string())?;
        let mut fichas: Vec<Ficha> = consulta
            .query_map([serie], ficha_de)
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;
        fichas.sort_by(|a, b| orden_de_lectura(clave_de_orden(a), clave_de_orden(b)));
        Ok(fichas)
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
