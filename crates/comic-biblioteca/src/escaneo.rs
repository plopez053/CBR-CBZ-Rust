//! Recorrer las carpetas de la biblioteca y poner la base de datos al día.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use comic_core::{metadatos, Comic, Formato, Metadatos};
use rusqlite::{params, Transaction};
use xxhash_rust::xxh3::Xxh3;

use crate::{texto_de, Biblioteca};

/// Extensiones de los archivos que se miran al escanear. Dentro, el formato se
/// reconoce por el contenido, así que un `.cbr` que en realidad es un zip vale igual.
const EXTENSIONES_COMIC: [&str; 8] = ["cbz", "cbr", "cb7", "cbt", "zip", "rar", "7z", "tar"];

/// Imágenes que Kodi, Plex y compañía dejan en las carpetas de las series. No son
/// páginas: una carpeta con solo eso no es un cómic.
const CARATULAS: [&str; 10] = [
    "folder",
    "cover",
    "portada",
    "caratula",
    "carátula",
    "poster",
    "fanart",
    "banner",
    "thumb",
    "backdrop",
];

/// Cuántos cómics se guardan de golpe. Guardar uno a uno es lento; todos al final
/// haría que la interfaz no viera nada hasta acabar.
const POR_TANDA: usize = 50;

/// Bytes del principio y del final del archivo que entran en la huella.
const TROZO_HUELLA: u64 = 64 * 1024;

/// Qué cambió en un escaneo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resumen {
    pub nuevos: usize,
    pub actualizados: usize,
    pub sin_cambios: usize,
    pub borrados: usize,
    /// Cómics que no se pudieron abrir (quedan en la biblioteca, con su error).
    pub con_error: usize,
    /// Carpetas que no se pudieron leer, por ejemplo un disco desconectado. Sus cómics
    /// se dejan como estaban en vez de darlos por borrados.
    pub carpetas_inaccesibles: usize,
}

/// Lo que se sabía de un cómic antes de escanear.
struct Conocido {
    carpeta: i64,
    tamano: i64,
    modificado: i64,
    huella: String,
}

impl Biblioteca {
    /// Recorre todas las carpetas: añade los cómics nuevos, vuelve a leer los que han
    /// cambiado y quita los que ya no están. Los que no han cambiado ni se abren.
    ///
    /// `avance(hechos, total)` se llama tras cada cómic, para poder mostrar el progreso.
    pub fn escanear(&mut self, mut avance: impl FnMut(usize, usize)) -> Result<Resumen, String> {
        let mut resumen = Resumen::default();

        // Primero se buscan todos, para saber cuántos hay.
        let mut encontrados: Vec<(i64, PathBuf, PathBuf)> = Vec::new();
        let mut leidas = HashSet::new();
        for carpeta in self.carpetas()? {
            match buscar_comics(&carpeta.ruta) {
                Ok(rutas) => {
                    leidas.insert(carpeta.id);
                    encontrados.extend(
                        rutas
                            .into_iter()
                            .map(|ruta| (carpeta.id, carpeta.ruta.clone(), ruta)),
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[biblioteca] No se pudo leer {}: {e}",
                        carpeta.ruta.display()
                    );
                    resumen.carpetas_inaccesibles += 1;
                }
            }
        }

        let conocidos = self.conocidos()?;
        let total = encontrados.len();

        let mut hechos = 0;
        for grupo in encontrados.chunks(POR_TANDA) {
            // Primero se abren los cómics de la tanda, que es lo lento, y luego se guardan
            // todos de golpe. Así la base de datos solo queda bloqueada unos milisegundos
            // y la interfaz puede guardar el progreso de lectura mientras se escanea.
            let mut tanda = Vec::new();
            for (carpeta, raiz, ruta) in grupo {
                let (tamano, modificado) = sello(ruta);
                let anterior = conocidos.get(&texto_de(ruta));

                match anterior {
                    Some(c) if (c.tamano, c.modificado) == (tamano, modificado) => {
                        resumen.sin_cambios += 1;
                    }
                    _ => {
                        let leido = leer_comic(ruta, raiz);
                        if let Some(error) = &leido.error {
                            eprintln!("[biblioteca] {}: {error}", ruta.display());
                            resumen.con_error += 1;
                        }
                        tanda.push((*carpeta, ruta, tamano, modificado, leido, anterior));
                    }
                }
                hechos += 1;
                avance(hechos, total);
            }

            let transaccion = self.db.transaction().map_err(|e| e.to_string())?;
            for (carpeta, ruta, tamano, modificado, leido, anterior) in &tanda {
                guardar(&transaccion, *carpeta, ruta, *tamano, *modificado, leido)?;
                if let Some(anterior) = anterior {
                    // Mismo archivo con otro contenido (por ejemplo, alguien ha editado
                    // su ComicInfo.xml): que no se pierda por dónde iba.
                    trasladar_progreso(&transaccion, &anterior.huella, &leido.huella)?;
                    resumen.actualizados += 1;
                } else {
                    resumen.nuevos += 1;
                }
            }
            transaccion.commit().map_err(|e| e.to_string())?;
        }

        // Lo que estaba y ya no está, solo de las carpetas que se han podido leer.
        let presentes: HashSet<String> = encontrados.iter().map(|(_, _, r)| texto_de(r)).collect();
        let transaccion = self.db.transaction().map_err(|e| e.to_string())?;
        for (ruta, conocido) in &conocidos {
            if leidas.contains(&conocido.carpeta) && !presentes.contains(ruta) {
                transaccion
                    .execute("DELETE FROM comics WHERE ruta = ?1", [ruta])
                    .map_err(|e| e.to_string())?;
                resumen.borrados += 1;
            }
        }
        transaccion.commit().map_err(|e| e.to_string())?;

        Ok(resumen)
    }

    fn conocidos(&self) -> Result<HashMap<String, Conocido>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT ruta, carpeta, tamano, modificado, huella FROM comics")
            .map_err(|e| e.to_string())?;
        let conocidos = consulta
            .query_map([], |f| {
                Ok((
                    f.get(0)?,
                    Conocido {
                        carpeta: f.get(1)?,
                        tamano: f.get(2)?,
                        modificado: f.get(3)?,
                        huella: f.get(4)?,
                    },
                ))
            })
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;
        Ok(conocidos)
    }
}

/// Busca cómics en `raiz` y sus subcarpetas. Solo falla si no se puede leer la propia
/// `raiz`; una subcarpeta ilegible se salta.
fn buscar_comics(raiz: &Path) -> io::Result<Vec<PathBuf>> {
    let mut encontrados = Vec::new();
    recorrer(raiz, true, &mut encontrados)?;
    Ok(encontrados)
}

/// Añade a `encontrados` los cómics de `carpeta` y sus subcarpetas. Devuelve si ha
/// encontrado algún archivo de cómic (no cuentan los cómics-carpeta).
fn recorrer(carpeta: &Path, es_raiz: bool, encontrados: &mut Vec<PathBuf>) -> io::Result<bool> {
    let mut subcarpetas = Vec::new();
    let mut archivos = Vec::new();
    let mut hay_paginas = false;

    for entrada in fs::read_dir(carpeta)? {
        let Ok(entrada) = entrada else { continue };
        let nombre = entrada.file_name().to_string_lossy().into_owned();
        // Ocultas (`.git`, `.DS_Store`...) y la basura de macOS.
        if nombre.starts_with('.') || nombre.eq_ignore_ascii_case("__macosx") {
            continue;
        }
        let Ok(tipo) = entrada.file_type() else {
            continue;
        };
        // Las rutas se guardan como texto: una que no es UTF-8 (nombres antiguos en
        // Latin-1 en Linux, por ejemplo) no se podría volver a abrir.
        if entrada.path().to_str().is_none() {
            eprintln!(
                "[biblioteca] Se ignora {}: el nombre no es UTF-8",
                entrada.path().display()
            );
            continue;
        }

        if tipo.is_dir() {
            subcarpetas.push(entrada.path());
        } else if tipo.is_file() {
            if es_archivo_de_comic(&nombre) {
                archivos.push(entrada.path());
            } else if comic_core::es_imagen(&nombre) && !es_caratula(&nombre) {
                hay_paginas = true;
            }
        }
    }

    let antes = encontrados.len();
    let mut hay_archivos = !archivos.is_empty();
    encontrados.extend(archivos);
    for subcarpeta in subcarpetas {
        match recorrer(&subcarpeta, false, encontrados) {
            Ok(encontro) => hay_archivos |= encontro,
            Err(e) => eprintln!("[biblioteca] No se pudo leer {}: {e}", subcarpeta.display()),
        }
    }

    // Una carpeta con imágenes sueltas y ningún archivo de cómic, ni en ella ni por
    // debajo, es un cómic en sí misma. Sus subcarpetas (capítulos, por ejemplo) son
    // parte de él: lo que se encontró en ellas se sustituye por la carpeta entera.
    if hay_paginas && !hay_archivos && !es_raiz {
        encontrados.truncate(antes);
        encontrados.push(carpeta.to_path_buf());
    }
    Ok(hay_archivos)
}

fn es_caratula(nombre: &str) -> bool {
    let nombre = Path::new(nombre)
        .file_stem()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    CARATULAS.contains(&nombre.as_str())
}

fn es_archivo_de_comic(nombre: &str) -> bool {
    let extension = Path::new(nombre)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    EXTENSIONES_COMIC.contains(&extension.as_str())
}

/// Tamaño y fecha de modificación (ms desde 1970): si no cambian, el archivo tampoco.
fn sello(ruta: &Path) -> (i64, i64) {
    let Ok(datos) = fs::metadata(ruta) else {
        return (0, 0);
    };
    let modificado = datos
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    // Las carpetas no tienen tamaño útil; basta con la fecha, que cambia al añadir
    // o quitar archivos.
    let tamano = if datos.is_dir() {
        0
    } else {
        datos.len() as i64
    };
    (tamano, modificado)
}

/// Lo que se guarda de cada cómic.
struct Leido {
    huella: String,
    formato: &'static str,
    paginas: usize,
    error: Option<String>,
    metadatos: Metadatos,
}

/// Abre un cómic y saca todo lo que se guarda de él. Si no se puede abrir, devuelve
/// igualmente lo que se pueda (el nombre) junto con el error.
fn leer_comic(ruta: &Path, raiz: &Path) -> Leido {
    let mut leido = match Comic::abrir(ruta) {
        Ok(mut comic) => Leido {
            huella: huella(ruta, comic.paginas()),
            formato: comic.formato().nombre(),
            paginas: comic.paginas().len(),
            error: None,
            metadatos: comic.metadatos(),
        },
        Err(error) => Leido {
            huella: huella(ruta, &[]),
            formato: Formato::detectar(ruta).map_or("?", Formato::nombre),
            paginas: 0,
            error: Some(error),
            metadatos: ruta
                .file_stem()
                .map(|n| metadatos::de_nombre(&n.to_string_lossy()))
                .unwrap_or_default(),
        },
    };

    if leido.paginas == 0 && leido.error.is_none() {
        leido.error = Some("No tiene páginas".to_string());
    }
    if leido.metadatos.serie.is_none() {
        leido.metadatos.serie = Some(serie_por_carpeta(ruta, raiz));
    }
    leido
}

/// La serie de un cómic que no la dice: la carpeta donde está o, si está suelto en
/// la raíz de la biblioteca, su propio nombre.
fn serie_por_carpeta(ruta: &Path, raiz: &Path) -> String {
    let padre = ruta.parent().filter(|p| *p != raiz);
    let nombre = match padre {
        Some(padre) => padre.file_name(),
        None => ruta.file_stem(),
    };
    nombre.map_or("Sin serie".to_string(), |n| {
        n.to_string_lossy().into_owned()
    })
}

/// Identifica un cómic por su contenido, no por su ruta, para reconocerlo si se mueve.
///
/// De un archivo se usan el tamaño, el principio y el final (leerlo entero sería lento
/// con miles de cómics, y dos cómics distintos no coinciden en las tres cosas). De una
/// carpeta, los nombres y tamaños de sus `paginas` (las de [`Comic::paginas`]).
pub fn huella(ruta: &Path, paginas: &[String]) -> String {
    let mut hash = Xxh3::new();

    if ruta.is_dir() {
        for pagina in paginas {
            let tamano = fs::metadata(ruta.join(pagina)).map_or(0, |d| d.len());
            hash.update(pagina.as_bytes());
            hash.update(&tamano.to_le_bytes());
        }
    } else if let Ok(mut archivo) = File::open(ruta) {
        let tamano = archivo.metadata().map_or(0, |d| d.len());
        hash.update(&tamano.to_le_bytes());

        let mut trozo = Vec::new();
        let _ = (&mut archivo).take(TROZO_HUELLA).read_to_end(&mut trozo);
        hash.update(&trozo);
        if tamano > TROZO_HUELLA {
            trozo.clear();
            let desde = tamano.saturating_sub(TROZO_HUELLA).max(TROZO_HUELLA);
            if archivo.seek(SeekFrom::Start(desde)).is_ok() {
                let _ = archivo.read_to_end(&mut trozo);
            }
            hash.update(&trozo);
        }
    }

    format!("{:032x}", hash.digest128())
}

fn guardar(
    transaccion: &Transaction,
    carpeta: i64,
    ruta: &Path,
    tamano: i64,
    modificado: i64,
    leido: &Leido,
) -> Result<(), String> {
    let m = &leido.metadatos;
    transaccion
        .execute(
            "INSERT INTO comics (
                 carpeta, ruta, tamano, modificado, huella, formato, paginas, error,
                 serie, numero, volumen, total, titulo, anio, mes, resumen,
                 guion, dibujo, editorial, generos, idioma, derecha_a_izquierda, portada)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
             ON CONFLICT(ruta) DO UPDATE SET
                 carpeta = excluded.carpeta,
                 tamano = excluded.tamano,
                 modificado = excluded.modificado,
                 huella = excluded.huella,
                 formato = excluded.formato,
                 paginas = excluded.paginas,
                 error = excluded.error,
                 serie = excluded.serie,
                 numero = excluded.numero,
                 volumen = excluded.volumen,
                 total = excluded.total,
                 titulo = excluded.titulo,
                 anio = excluded.anio,
                 mes = excluded.mes,
                 resumen = excluded.resumen,
                 guion = excluded.guion,
                 dibujo = excluded.dibujo,
                 editorial = excluded.editorial,
                 generos = excluded.generos,
                 idioma = excluded.idioma,
                 derecha_a_izquierda = excluded.derecha_a_izquierda,
                 portada = excluded.portada",
            params![
                carpeta,
                texto_de(ruta),
                tamano,
                modificado,
                leido.huella,
                leido.formato,
                leido.paginas as i64,
                leido.error,
                m.serie,
                m.numero,
                m.volumen,
                m.total,
                m.titulo,
                m.anio,
                m.mes,
                m.resumen,
                m.guion,
                m.dibujo,
                m.editorial,
                m.generos,
                m.idioma,
                m.derecha_a_izquierda,
                m.portada.map(|p| p as i64),
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Pasa el progreso de una huella a otra, salvo que la nueva ya tenga el suyo.
fn trasladar_progreso(transaccion: &Transaction, antes: &str, ahora: &str) -> Result<(), String> {
    if antes != ahora {
        transaccion
            .execute(
                "UPDATE OR IGNORE progreso SET huella = ?2 WHERE huella = ?1",
                [antes, ahora],
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
