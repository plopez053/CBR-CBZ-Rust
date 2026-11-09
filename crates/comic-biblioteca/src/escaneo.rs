//! Recorrer las carpetas de la biblioteca y poner la base de datos al día.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::UNIX_EPOCH;

use comic_core::{metadatos, Comic, Formato, Metadatos};
use rusqlite::{params, Connection, Transaction};
use xxhash_rust::xxh3::Xxh3;

use crate::serie::clave_de_serie;
use crate::{texto_de, Biblioteca};

/// Versión del análisis de metadatos (nombres, series...). Al subirla, el siguiente
/// escaneo vuelve a leer todos los cómics aunque sus archivos no hayan cambiado, para
/// que se beneficien de lo que se haya mejorado.
const VERSION_ANALISIS: i64 = 2;

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
    analisis: i64,
}

/// Un cómic que hay que abrir (es nuevo o ha cambiado), con lo que se necesita para guardarlo.
struct Pendiente<'a> {
    carpeta: i64,
    raiz: &'a Path,
    ruta: &'a Path,
    tamano: i64,
    modificado: i64,
    anterior: Option<&'a Conocido>,
}

/// Cuántos cómics se abren a la vez. Abrir uno es sobre todo esperar al disco y
/// listar su contenido, así que más hilos que núcleos tampoco ayudaría.
fn hilos_de_escaneo() -> usize {
    let nucleos = thread::available_parallelism().map_or(1, |n| n.get());
    nucleos.saturating_sub(1).clamp(2, 6)
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
        for (carpeta, _) in self.todas_las_carpetas()? {
            if carpeta.propia {
                // Sin disco que recorrer: solo sus archivos sueltos, si siguen ahí. Sus
                // cómics nunca se dan por borrados (`leidas` no la incluye): se quitan a mano.
                for ruta in self.archivos_propios(carpeta.id)? {
                    if ruta.exists() {
                        // Como raíz, su propia carpeta: la serie sale del nombre del archivo.
                        let raiz = ruta.parent().map(Path::to_path_buf).unwrap_or_default();
                        encontrados.push((carpeta.id, raiz, ruta));
                    }
                }
                continue;
            }
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
        let ignorados = self.huellas_ignoradas()?;
        let total = encontrados.len();

        // Lo que no ha cambiado no se abre.
        let mut pendientes = Vec::new();
        for (carpeta, raiz, ruta) in &encontrados {
            let (tamano, modificado) = sello(ruta);
            let anterior = conocidos.get(&texto_de(ruta));
            match anterior {
                Some(c)
                    if (c.tamano, c.modificado) == (tamano, modificado)
                        && c.analisis >= VERSION_ANALISIS =>
                {
                    resumen.sin_cambios += 1;
                }
                _ => pendientes.push(Pendiente {
                    carpeta: *carpeta,
                    raiz,
                    ruta,
                    tamano,
                    modificado,
                    anterior,
                }),
            }
        }
        let mut hechos = resumen.sin_cambios;
        avance(hechos, total);

        // Varios hilos abren cómics y el actual los guarda. Se guardan de 50 en 50: así
        // la base de datos solo se bloquea unos milisegundos cada vez y la interfaz puede
        // anotar el progreso de lectura mientras se escanea.
        let siguiente = AtomicUsize::new(0);
        let cancelar = AtomicBool::new(false);
        let (enviar, recibir) = mpsc::channel::<(usize, Leido)>();
        let db = &mut self.db;

        thread::scope(|ambito| {
            for _ in 0..hilos_de_escaneo().min(pendientes.len()) {
                let enviar = enviar.clone();
                let (pendientes, siguiente, cancelar) = (&pendientes, &siguiente, &cancelar);
                ambito.spawn(move || {
                    while !cancelar.load(Ordering::Relaxed) {
                        // Cada hilo coge el siguiente número libre: así no se pisan.
                        let indice = siguiente.fetch_add(1, Ordering::Relaxed);
                        let Some(p) = pendientes.get(indice) else {
                            break;
                        };
                        if enviar.send((indice, leer_comic(p.ruta, p.raiz))).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(enviar);

            let mut tanda = Vec::new();
            let mut resultado = Ok(());
            for (indice, leido) in &recibir {
                if let Some(error) = &leido.error {
                    eprintln!(
                        "[biblioteca] {}: {error}",
                        pendientes[indice].ruta.display()
                    );
                    resumen.con_error += 1;
                }
                tanda.push((indice, leido));
                hechos += 1;
                avance(hechos, total);

                if tanda.len() >= POR_TANDA {
                    resultado =
                        guardar_tanda(db, &mut tanda, &pendientes, &ignorados, &mut resumen);
                    if resultado.is_err() {
                        break;
                    }
                }
            }
            if resultado.is_ok() {
                resultado = guardar_tanda(db, &mut tanda, &pendientes, &ignorados, &mut resumen);
            }
            if resultado.is_err() {
                // Que los hilos dejen de abrir cómics que ya nadie va a guardar.
                cancelar.store(true, Ordering::Relaxed);
            }
            resultado
        })?;

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

        series_por_subtitulo(&mut self.db)?;
        series_por_vecindad(&mut self.db)?;
        aplicar_series_forzadas(&self.db)?;
        Ok(resumen)
    }

    /// Los cómics sueltos que se han añadido a mano a una carpeta propia.
    pub(crate) fn archivos_propios(&self, carpeta: i64) -> Result<Vec<PathBuf>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT ruta FROM archivos_propios WHERE carpeta = ?1 ORDER BY ruta")
            .map_err(|e| e.to_string())?;
        consulta
            .query_map([carpeta], |f| Ok(PathBuf::from(f.get::<_, String>(0)?)))
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())
    }

    /// Las huellas de los cómics que el usuario ha quitado de la colección: no se
    /// guardan aunque se encuentren en una carpeta vigilada.
    fn huellas_ignoradas(&self) -> Result<HashSet<String>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT huella FROM comics_ignorados")
            .map_err(|e| e.to_string())?;
        consulta
            .query_map([], |f| f.get(0))
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())
    }

    fn conocidos(&self) -> Result<HashMap<String, Conocido>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT ruta, carpeta, tamano, modificado, huella, analisis FROM comics")
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
                        analisis: f.get(5)?,
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

/// Los cómics que hay en `ruta`: ella misma si es un archivo de cómic; si es una carpeta,
/// los que haya dentro, o ella misma si no tiene ninguno pero sí imágenes (es un cómic).
pub(crate) fn comics_en(ruta: &Path) -> Vec<PathBuf> {
    if ruta.to_str().is_none() {
        return Vec::new();
    }
    if ruta.is_file() {
        let valido = es_archivo_de_comic(&ruta.to_string_lossy());
        return if valido {
            vec![ruta.to_path_buf()]
        } else {
            Vec::new()
        };
    }
    let mut encontrados = buscar_comics(ruta).unwrap_or_default();
    if encontrados.is_empty() && tiene_paginas(ruta) {
        encontrados.push(ruta.to_path_buf());
    }
    encontrados
}

/// Si la carpeta tiene imágenes sueltas que no son una carátula.
fn tiene_paginas(carpeta: &Path) -> bool {
    let Ok(entradas) = fs::read_dir(carpeta) else {
        return false;
    };
    entradas.flatten().any(|entrada| {
        let nombre = entrada.file_name().to_string_lossy().into_owned();
        entrada.file_type().is_ok_and(|t| t.is_file())
            && comic_core::es_imagen(&nombre)
            && !es_caratula(&nombre)
    })
}

fn es_caratula(nombre: &str) -> bool {
    let nombre = Path::new(nombre)
        .file_stem()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    CARATULAS.contains(&nombre.as_str())
}

pub(crate) fn es_archivo_de_comic(nombre: &str) -> bool {
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

/// Un cómic cuyo nombre es `Serie - Subtítulo` (sin número ni tomo que separe una cosa de
/// la otra) sale con la serie entera, subtítulo incluido: `Las Crónicas de Wormwood - El
/// Último Enemigo` no es la misma serie que `Las Crónicas de Wormwood`. Si en su misma
/// carpeta hay un cómic cuya serie es justo lo que va antes del ` - `, el cómic pasa a esa
/// serie. Si encajan varias (`A` y `A - B` para `A - B - C`), no se adivina.
/// Se repite en cada escaneo, así que el orden en que lleguen los archivos da igual.
fn series_por_subtitulo(db: &mut Connection) -> Result<(), String> {
    struct Fila {
        id: i64,
        carpeta: PathBuf,
        serie: String,
    }
    let filas: Vec<Fila> = {
        let mut consulta = db
            .prepare("SELECT id, ruta, serie FROM comics WHERE serie_carpeta IS NULL")
            .map_err(|e| e.to_string())?;
        consulta
            .query_map([], |f| {
                Ok((f.get::<_, i64>(0)?, f.get::<_, String>(1)?, f.get(2)?))
            })
            .and_then(|filas| filas.collect::<rusqlite::Result<Vec<_>>>())
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter_map(|(id, ruta, serie)| {
                let carpeta = PathBuf::from(ruta).parent()?.to_path_buf();
                Some(Fila { id, carpeta, serie })
            })
            .collect()
    };

    // Las series de cada carpeta, por clave.
    let mut por_carpeta: HashMap<&Path, HashMap<String, &str>> = HashMap::new();
    for fila in &filas {
        por_carpeta
            .entry(&fila.carpeta)
            .or_default()
            .insert(clave_de_serie(&fila.serie), &fila.serie);
    }

    let transaccion = db.transaction().map_err(|e| e.to_string())?;
    for fila in &filas {
        let Some(en_carpeta) = por_carpeta.get(fila.carpeta.as_path()) else {
            continue;
        };
        let propia = clave_de_serie(&fila.serie);

        // Cada trozo de antes de un " - ": `A - B - C` da `A` y `A - B`.
        let mut candidatas: HashMap<String, &str> = HashMap::new();
        for (posicion, _) in fila.serie.match_indices(" - ") {
            let clave = clave_de_serie(&fila.serie[..posicion]);
            if clave != propia {
                if let Some(serie) = en_carpeta.get(&clave) {
                    candidatas.insert(clave, serie);
                }
            }
        }
        if candidatas.len() != 1 {
            continue;
        }
        let serie = candidatas.into_values().next().unwrap_or(&fila.serie);
        transaccion
            .execute(
                "UPDATE comics SET serie = ?2, serie_clave = ?3 WHERE id = ?1",
                params![fila.id, serie, clave_de_serie(serie)],
            )
            .map_err(|e| e.to_string())?;
    }
    transaccion.commit().map_err(|e| e.to_string())
}

/// Los cómics que no dicen su serie la sacan de su carpeta, que es una pista débil:
/// `Vinland-T13-16/Tomo 13.cbz` da `Vinland`. Si al lado hay una serie con nombre fiable
/// (de su `ComicInfo.xml` o de su nombre) que empieza igual, como `Vinland Saga` en
/// `Vinland-T01-04`, el cómic pasa a esa serie.
///
/// "Al lado" es la misma carpeta o una vecina (con la misma carpeta madre). Si encajan
/// varias series (`Batman Beyond` y `Batman Year One` para `Batman`), no se adivina.
/// Se repite en cada escaneo, así un cómic suelto se une solo en cuanto llega su serie.
fn series_por_vecindad(db: &mut Connection) -> Result<(), String> {
    struct Fila {
        id: i64,
        ruta: PathBuf,
        serie: String,
        serie_carpeta: Option<String>,
    }
    let filas: Vec<Fila> = {
        let mut consulta = db
            .prepare("SELECT id, ruta, serie, serie_carpeta FROM comics")
            .map_err(|e| e.to_string())?;
        consulta
            .query_map([], |f| {
                Ok(Fila {
                    id: f.get(0)?,
                    ruta: PathBuf::from(f.get::<_, String>(1)?),
                    serie: f.get(2)?,
                    serie_carpeta: f.get(3)?,
                })
            })
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?
    };

    // Series fiables de cada carpeta y de cada carpeta madre (para las vecinas).
    let mut por_carpeta: HashMap<&Path, HashMap<String, &str>> = HashMap::new();
    let mut por_madre: HashMap<&Path, HashMap<String, &str>> = HashMap::new();
    for fila in filas.iter().filter(|f| f.serie_carpeta.is_none()) {
        let Some(carpeta) = fila.ruta.parent() else {
            continue;
        };
        let clave = clave_de_serie(&fila.serie);
        por_carpeta
            .entry(carpeta)
            .or_default()
            .insert(clave.clone(), &fila.serie);
        if let Some(madre) = carpeta.parent() {
            por_madre
                .entry(madre)
                .or_default()
                .insert(clave, &fila.serie);
        }
    }

    let transaccion = db.transaction().map_err(|e| e.to_string())?;
    for fila in &filas {
        let (Some(debil), Some(carpeta)) = (&fila.serie_carpeta, fila.ruta.parent()) else {
            continue;
        };
        let clave_debil = clave_de_serie(debil);
        let encaja =
            |clave: &str| clave == clave_debil || clave.starts_with(&format!("{clave_debil} "));

        let mut candidatas: HashMap<&str, &str> = HashMap::new();
        let cerca = [
            por_carpeta.get(carpeta),
            carpeta.parent().and_then(|m| por_madre.get(m)),
        ];
        for series in cerca.into_iter().flatten() {
            for (clave, serie) in series {
                if encaja(clave) {
                    candidatas.insert(clave, serie);
                }
            }
        }

        let serie = match candidatas.len() {
            1 => candidatas.into_values().next().unwrap_or(debil),
            _ => debil,
        };
        if serie != fila.serie {
            transaccion
                .execute(
                    "UPDATE comics SET serie = ?2, serie_clave = ?3 WHERE id = ?1",
                    params![fila.id, serie, clave_de_serie(serie)],
                )
                .map_err(|e| e.to_string())?;
        }
    }
    transaccion.commit().map_err(|e| e.to_string())
}

/// Los cómics que el usuario ha metido a mano en una serie pasan a ella, digan lo que
/// digan sus metadatos. Va el último en cada escaneo: así gana a todo lo demás, también
/// a lo que se acaba de volver a leer de un archivo cambiado.
fn aplicar_series_forzadas(db: &Connection) -> Result<(), String> {
    db.execute(
        "UPDATE comics SET
             serie = COALESCE(
                 (SELECT x.serie FROM comics x
                  WHERE x.serie_clave =
                            (SELECT f.grupo FROM series_forzadas f WHERE f.ruta = comics.ruta)
                    AND x.ruta NOT IN (SELECT ruta FROM series_forzadas)
                  LIMIT 1),
                 (SELECT f.grupo FROM series_forzadas f WHERE f.ruta = comics.ruta)),
             serie_clave = (SELECT f.grupo FROM series_forzadas f WHERE f.ruta = comics.ruta),
             serie_carpeta = NULL
         WHERE ruta IN (SELECT ruta FROM series_forzadas)",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Lo que se guarda de cada cómic.
struct Leido {
    huella: String,
    serie_clave: String,
    /// La serie sacada de la carpeta, si el cómic no decía la suya.
    serie_carpeta: Option<String>,
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
            serie_clave: String::new(),
            serie_carpeta: None,
            formato: comic.formato().nombre(),
            paginas: comic.paginas().len(),
            error: None,
            metadatos: comic.metadatos(),
        },
        Err(error) => Leido {
            huella: huella(ruta, &[]),
            serie_clave: String::new(),
            serie_carpeta: None,
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
        let serie = serie_por_carpeta(ruta, raiz);
        leido.serie_carpeta = Some(serie.clone());
        leido.metadatos.serie = Some(serie);
    }
    leido.serie_clave = clave_de_serie(leido.metadatos.serie.as_deref().unwrap_or_default());
    leido
}

/// La serie de un cómic que no la dice: la carpeta donde está o, si está suelto en
/// la raíz de la biblioteca, su propio nombre. Sin los rangos de tomos (`T01-04`) ni lo
/// que va entre paréntesis, para que carpetas como `Vinland-T01-04` y `Vinland-T05-08`
/// acaben en la misma serie.
fn serie_por_carpeta(ruta: &Path, raiz: &Path) -> String {
    let padre = ruta.parent().filter(|p| *p != raiz);
    let nombre = match padre {
        Some(padre) => padre.file_name(),
        None => ruta.file_stem(),
    };
    nombre
        .map(|n| metadatos::serie_de_carpeta(&n.to_string_lossy()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Sin serie".to_string())
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

/// Guarda de golpe los cómics ya abiertos y vacía `tanda`.
fn guardar_tanda(
    db: &mut Connection,
    tanda: &mut Vec<(usize, Leido)>,
    pendientes: &[Pendiente],
    ignorados: &HashSet<String>,
    resumen: &mut Resumen,
) -> Result<(), String> {
    if tanda.is_empty() {
        return Ok(());
    }
    let transaccion = db.transaction().map_err(|e| e.to_string())?;
    for (indice, leido) in tanda.iter() {
        let p = &pendientes[*indice];
        if p.anterior.is_none() && ignorados.contains(&leido.huella) {
            // El usuario lo quitó de la colección: no vuelve solo.
            continue;
        }
        guardar(
            &transaccion,
            p.carpeta,
            p.ruta,
            p.tamano,
            p.modificado,
            leido,
        )?;
        if let Some(anterior) = p.anterior {
            // Mismo archivo con otro contenido (por ejemplo, alguien ha editado su
            // ComicInfo.xml): que no se pierda por dónde iba.
            trasladar_progreso(&transaccion, &anterior.huella, &leido.huella)?;
            resumen.actualizados += 1;
        } else {
            resumen.nuevos += 1;
        }
    }
    transaccion.commit().map_err(|e| e.to_string())?;
    tanda.clear();
    Ok(())
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
                 guion, dibujo, editorial, generos, idioma, derecha_a_izquierda, portada,
                 serie_clave, analisis, serie_carpeta)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)
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
                 portada = excluded.portada,
                 serie_clave = excluded.serie_clave,
                 analisis = excluded.analisis,
                 serie_carpeta = excluded.serie_carpeta",
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
                leido.serie_clave,
                VERSION_ANALISIS,
                leido.serie_carpeta,
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
