//! Encontrar cómics que se han movido de sitio y apuntar la biblioteca a su nueva ubicación.
//!
//! Los cómics se reconocen por su contenido (la huella), así que moverlos no pierde el
//! progreso ni los nombres. Lo único que se rompe son las rutas guardadas, que es lo que
//! se arregla aquí. Los archivos nunca se tocan.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::escaneo::{es_archivo_de_comic, huella};
use crate::{texto_de, Biblioteca};

/// Hasta qué profundidad se mira dentro de cada carpeta donde se busca.
const PROFUNDIDAD_MAXIMA: usize = 10;

/// Cuántos archivos del mismo tamaño se prueban, como mucho, para cada cómic perdido.
const CANDIDATOS_POR_COMIC: usize = 8;

/// Qué se arregló al reubicar automáticamente.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Informe {
    /// Carpetas vigiladas que vuelven a apuntar a un sitio que existe (o que se han
    /// fundido con otra que ya cubría esa ubicación).
    pub carpetas: usize,
    /// Cómics sueltos de carpetas propias cuya ruta se ha corregido.
    pub archivos: usize,
    /// Cómics perdidos de los que no se encontró rastro.
    pub sin_encontrar: usize,
}

/// Un cómic cuyo archivo ya no está donde la biblioteca lo guardó.
struct Perdido {
    ruta: PathBuf,
    carpeta: i64,
    propia: bool,
    tamano: u64,
    huella: String,
}

/// Dónde se ha encontrado un cómic perdido.
struct Hallazgo {
    antigua: PathBuf,
    nueva: PathBuf,
    carpeta: i64,
    propia: bool,
}

impl Biblioteca {
    /// Busca, dentro de `raices` y sus subcarpetas, los cómics que la biblioteca no
    /// encuentra donde los dejó, y corrige sus rutas.
    ///
    /// Solo se mira en las carpetas indicadas, nunca en el resto del disco. Un archivo se
    /// da por el mismo cómic solo si coincide su huella, aunque haya cambiado de nombre.
    ///
    /// - Las carpetas vigiladas cuyo directorio ya no existe pasan a la ubicación donde se
    ///   han encontrado la mayoría de sus cómics.
    /// - Los cómics sueltos de las carpetas propias se corrigen uno a uno.
    ///
    /// Después hay que escanear: eso termina de poner al día lo que haya cambiado dentro de
    /// las carpetas reubicadas.
    pub fn reubicar_automaticamente(&self, raices: &[PathBuf]) -> Result<Informe, String> {
        let perdidos = self.comics_perdidos()?;
        let mut informe = Informe::default();
        if perdidos.is_empty() {
            return Ok(informe);
        }

        let encontrados = buscar_perdidos(&perdidos, raices, &self.rutas_conocidas()?);
        informe.sin_encontrar = perdidos.len() - encontrados.len();

        // Carpetas vigiladas: la ubicación nueva es la que más cómics suyos tiene.
        let mut por_carpeta: HashMap<i64, HashMap<PathBuf, usize>> = HashMap::new();
        let carpetas: HashMap<i64, PathBuf> = self
            .todas_las_carpetas()?
            .into_iter()
            .map(|(c, _)| (c.id, c.ruta))
            .collect();
        for h in encontrados.iter().filter(|h| !h.propia) {
            let Some(raiz_antigua) = carpetas.get(&h.carpeta) else {
                continue;
            };
            if let Some(raiz) = raiz_nueva(raiz_antigua, &h.antigua, &h.nueva) {
                *por_carpeta
                    .entry(h.carpeta)
                    .or_default()
                    .entry(raiz)
                    .or_default() += 1;
            }
        }
        let mut ordenadas: Vec<_> = por_carpeta.into_iter().collect();
        ordenadas.sort_by_key(|(id, _)| *id);
        for (id, votos) in ordenadas {
            let Some((nueva, _)) = votos
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            else {
                continue;
            };
            match self.reubicar_carpeta(id, &nueva) {
                Ok(()) => informe.carpetas += 1,
                Err(e) => eprintln!("[biblioteca] No se pudo reubicar la carpeta {id}: {e}"),
            }
        }

        // Cómics sueltos: cada uno por su cuenta.
        for h in encontrados.iter().filter(|h| h.propia) {
            if self.reubicar_archivo(h.carpeta, &h.antigua, &h.nueva)? {
                informe.archivos += 1;
            }
        }

        Ok(informe)
    }

    /// Cambia de sitio una carpeta vigilada: sus cómics pasan a colgar de `nueva`
    /// conservando la estructura de subcarpetas. Si `nueva` ya está cubierta por otra carpeta
    /// de la biblioteca, esta se quita (sus cómics ya salen por la otra).
    ///
    /// Los archivos no se tocan, y el progreso se conserva porque va por huella.
    pub fn reubicar_carpeta(&self, id: i64, nueva: impl AsRef<Path>) -> Result<(), String> {
        let nueva = std::path::absolute(nueva.as_ref()).map_err(|e| e.to_string())?;
        if !nueva.is_dir() {
            return Err(format!("{} no es una carpeta", nueva.display()));
        }

        let carpetas = self.carpetas()?;
        let Some(actual) = carpetas.iter().find(|c| c.id == id && !c.propia) else {
            return Err("Solo se pueden reubicar las carpetas del disco".to_string());
        };
        let antigua = actual.ruta.clone();

        for otra in carpetas.iter().filter(|c| c.id != id && !c.propia) {
            if nueva.starts_with(&otra.ruta) {
                // Ya la cubre otra: esta sobra.
                return self.quitar_carpeta(id);
            }
            if otra.ruta.starts_with(&nueva) {
                return Err(format!(
                    "{} se solapa con {}, que ya está en la biblioteca",
                    nueva.display(),
                    otra.ruta.display()
                ));
            }
        }

        let mut consulta = self
            .db
            .prepare("SELECT id, ruta FROM comics WHERE carpeta = ?1")
            .map_err(|e| e.to_string())?;
        let comics: Vec<(i64, String)> = consulta
            .query_map([id], |f| Ok((f.get(0)?, f.get(1)?)))
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;
        drop(consulta);

        let transaccion = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        transaccion
            .execute(
                "UPDATE carpetas SET ruta = ?2 WHERE id = ?1",
                params![id, texto_de(&nueva)],
            )
            .map_err(|e| e.to_string())?;
        for (comic, ruta) in comics {
            let Ok(relativa) = Path::new(&ruta).strip_prefix(&antigua) else {
                continue;
            };
            // Si ya hay otro cómic en esa ruta se deja como está: el escaneo lo ordenará.
            transaccion
                .execute(
                    "UPDATE OR IGNORE comics SET ruta = ?2 WHERE id = ?1",
                    params![comic, texto_de(&nueva.join(relativa))],
                )
                .map_err(|e| e.to_string())?;
        }
        transaccion.commit().map_err(|e| e.to_string())
    }

    /// Corrige la ruta de un cómic suelto de una carpeta propia. Devuelve si se cambió.
    fn reubicar_archivo(&self, carpeta: i64, antigua: &Path, nueva: &Path) -> Result<bool, String> {
        let transaccion = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        let cambiados = transaccion
            .execute(
                "UPDATE OR IGNORE archivos_propios SET ruta = ?3 WHERE carpeta = ?1 AND ruta = ?2",
                params![carpeta, texto_de(antigua), texto_de(nueva)],
            )
            .map_err(|e| e.to_string())?;
        if cambiados > 0 {
            transaccion
                .execute(
                    "UPDATE OR IGNORE comics SET ruta = ?2 WHERE ruta = ?1",
                    params![texto_de(antigua), texto_de(nueva)],
                )
                .map_err(|e| e.to_string())?;
        }
        transaccion.commit().map_err(|e| e.to_string())?;
        Ok(cambiados > 0)
    }

    /// Los cómics cuyo archivo no está, y que son de una carpeta vigilada que ya no existe
    /// o de una propia. Los que faltan dentro de una carpeta que sí existe son cosa del
    /// escaneo: ahí se borraron de verdad.
    fn comics_perdidos(&self) -> Result<Vec<Perdido>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT ruta, tamano, huella FROM comics WHERE carpeta = ?1")
            .map_err(|e| e.to_string())?;
        let mut perdidos = Vec::new();
        for (carpeta, _) in self.todas_las_carpetas()? {
            if !carpeta.propia && carpeta.ruta.is_dir() {
                continue;
            }
            let filas: Vec<(String, i64, String)> = consulta
                .query_map([carpeta.id], |f| Ok((f.get(0)?, f.get(1)?, f.get(2)?)))
                .and_then(Iterator::collect)
                .map_err(|e| e.to_string())?;
            for (ruta, tamano, huella) in filas {
                let ruta = PathBuf::from(ruta);
                // Un tamaño 0 es un cómic-carpeta: no hay un archivo que buscar.
                if tamano > 0 && !ruta.exists() {
                    perdidos.push(Perdido {
                        ruta,
                        carpeta: carpeta.id,
                        propia: carpeta.propia,
                        tamano: tamano.max(0) as u64,
                        huella,
                    });
                }
            }
        }
        Ok(perdidos)
    }

    /// Las rutas que la biblioteca ya tiene: un archivo que ya es de otro cómic no puede
    /// ser la nueva ubicación de uno perdido.
    fn rutas_conocidas(&self) -> Result<HashSet<PathBuf>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT ruta FROM comics UNION SELECT ruta FROM archivos_propios")
            .map_err(|e| e.to_string())?;
        consulta
            .query_map([], |f| Ok(PathBuf::from(f.get::<_, String>(0)?)))
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())
    }
}

/// La carpeta que hace ahora de raíz, suponiendo que `antigua` (en la raíz `raiz_antigua`)
/// es la misma que `nueva`: si conserva la ruta relativa, se quita de `nueva`; si no
/// (se reorganizó), vale la carpeta donde está.
fn raiz_nueva(raiz_antigua: &Path, antigua: &Path, nueva: &Path) -> Option<PathBuf> {
    let relativa = antigua.strip_prefix(raiz_antigua).ok()?;
    if nueva.ends_with(relativa) {
        let niveles = relativa.components().count();
        nueva.ancestors().nth(niveles).map(Path::to_path_buf)
    } else {
        nueva.parent().map(Path::to_path_buf)
    }
}

/// Recorre `raices` y empareja cada cómic perdido con un archivo que tenga su huella.
fn buscar_perdidos(
    perdidos: &[Perdido],
    raices: &[PathBuf],
    conocidas: &HashSet<PathBuf>,
) -> Vec<Hallazgo> {
    let tamanos: HashSet<u64> = perdidos.iter().map(|p| p.tamano).collect();
    let mut por_tamano: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for raiz in raices {
        recorrer(raiz, 0, &tamanos, conocidas, &mut por_tamano);
    }

    let mut huellas: HashMap<PathBuf, String> = HashMap::new();
    let mut usados: HashSet<PathBuf> = HashSet::new();
    let mut encontrados = Vec::new();

    for perdido in perdidos {
        let Some(candidatos) = por_tamano.get(&perdido.tamano) else {
            continue;
        };
        let nombre = perdido.ruta.file_name();
        // Primero los que se llaman igual: casi siempre es el bueno.
        let mut ordenados: Vec<&PathBuf> =
            candidatos.iter().filter(|c| !usados.contains(*c)).collect();
        ordenados.sort_by_key(|c| c.file_name() != nombre);

        for candidato in ordenados.into_iter().take(CANDIDATOS_POR_COMIC) {
            let huella_candidato = huellas
                .entry(candidato.clone())
                .or_insert_with(|| huella(candidato, &[]));
            if *huella_candidato == perdido.huella {
                usados.insert(candidato.clone());
                encontrados.push(Hallazgo {
                    antigua: perdido.ruta.clone(),
                    nueva: candidato.clone(),
                    carpeta: perdido.carpeta,
                    propia: perdido.propia,
                });
                break;
            }
        }
    }
    encontrados
}

/// Guarda en `por_tamano` los archivos de cómic de `carpeta` y sus subcarpetas cuyo
/// tamaño es uno de los buscados.
fn recorrer(
    carpeta: &Path,
    profundidad: usize,
    tamanos: &HashSet<u64>,
    conocidas: &HashSet<PathBuf>,
    por_tamano: &mut HashMap<u64, Vec<PathBuf>>,
) {
    if profundidad > PROFUNDIDAD_MAXIMA {
        return;
    }
    let Ok(entradas) = fs::read_dir(carpeta) else {
        return;
    };
    for entrada in entradas.flatten() {
        let nombre = entrada.file_name().to_string_lossy().into_owned();
        if nombre.starts_with('.') {
            continue;
        }
        let Ok(tipo) = entrada.file_type() else {
            continue;
        };
        let ruta = entrada.path();
        if tipo.is_dir() {
            // Las carpetas de compilación de Rust tienen decenas de miles de archivos.
            if nombre == "target" && carpeta.join("Cargo.toml").exists() || nombre == "node_modules"
            {
                continue;
            }
            recorrer(&ruta, profundidad + 1, tamanos, conocidas, por_tamano);
        } else if tipo.is_file() && es_archivo_de_comic(&nombre) {
            let Ok(datos) = entrada.metadata() else {
                continue;
            };
            if tamanos.contains(&datos.len()) && !conocidas.contains(&ruta) {
                por_tamano.entry(datos.len()).or_default().push(ruta);
            }
        }
    }
}
