//! Núcleo del lector: todo lo que no depende de la interfaz.

mod formatos;
pub mod metadatos;

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub use metadatos::Metadatos;

/// Extensiones que consideramos páginas de un cómic.
const EXTENSIONES_IMAGEN: [&str; 8] = ["jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff"];

/// Dice si el nombre de un archivo dentro de un cómic es una página.
///
/// Ignora la basura que macOS mete en los zips (`__MACOSX/...` y los `._01.jpg`).
pub fn es_imagen(nombre: &str) -> bool {
    let minusculas = nombre.to_lowercase();
    let archivo = minusculas.rsplit(['/', '\\']).next().unwrap_or("");

    if minusculas.contains("__macosx") || archivo.starts_with("._") {
        return false;
    }

    EXTENSIONES_IMAGEN
        .iter()
        .any(|ext| archivo.ends_with(&format!(".{ext}")))
}

fn es_comic_info(nombre: &str) -> bool {
    let archivo = nombre.rsplit('/').next().unwrap_or("");
    archivo.eq_ignore_ascii_case("comicinfo.xml")
}

/// Cómo está guardado un cómic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Formato {
    /// `.cbz` / `.zip`
    Zip,
    /// `.cbr` / `.rar`
    Rar,
    /// `.cb7` / `.7z`
    SieteZip,
    /// `.cbt` / `.tar`
    Tar,
    /// Una carpeta con las imágenes sueltas.
    Carpeta,
}

impl Formato {
    /// Nombre para mostrar.
    pub fn nombre(self) -> &'static str {
        match self {
            Formato::Zip => "ZIP",
            Formato::Rar => "RAR",
            Formato::SieteZip => "7z",
            Formato::Tar => "TAR",
            Formato::Carpeta => "carpeta",
        }
    }

    /// Averigua el formato mirando los primeros bytes del archivo, no la extensión:
    /// hay muchos `.cbr` que en realidad son zips (y al revés).
    pub fn detectar(ruta: &Path) -> Result<Formato, String> {
        if ruta.is_dir() {
            return Ok(Formato::Carpeta);
        }

        // La firma de tar está en el byte 257; con 512 basta para todas.
        let mut cabecera = Vec::with_capacity(512);
        File::open(ruta)
            .and_then(|f| f.take(512).read_to_end(&mut cabecera))
            .map_err(|e| e.to_string())?;

        if cabecera.starts_with(b"PK\x03\x04") || cabecera.starts_with(b"PK\x05\x06") {
            return Ok(Formato::Zip);
        }
        if cabecera.starts_with(b"Rar!\x1a\x07") {
            return Ok(Formato::Rar);
        }
        if cabecera.starts_with(b"7z\xbc\xaf\x27\x1c") {
            return Ok(Formato::SieteZip);
        }
        if cabecera.get(257..262) == Some(b"ustar") {
            return Ok(Formato::Tar);
        }

        // Sin firma conocida: nos fiamos de la extensión (los tar muy antiguos no llevan firma).
        let extension = ruta
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "cbz" | "zip" => Ok(Formato::Zip),
            "cbr" | "rar" => Ok(Formato::Rar),
            "cb7" | "7z" => Ok(Formato::SieteZip),
            "cbt" | "tar" => Ok(Formato::Tar),
            _ => Err("No es un formato de cómic conocido".to_string()),
        }
    }
}

/// Lo que tiene que saber hacer cada formato una vez abierto. Los nombres son los que
/// devolvió al abrirse: rutas dentro del cómic con `/` como separador.
trait Contenedor: Send {
    /// Bytes (sin decodificar) del archivo con ese nombre.
    fn leer(&mut self, nombre: &str) -> Result<Vec<u8>, String>;
}

/// Un cómic abierto, sea del formato que sea. Se queda abierto mientras se lee.
pub struct Comic {
    ruta: PathBuf,
    formato: Formato,
    contenedor: Box<dyn Contenedor>,
    paginas: Vec<String>,
    /// Dónde está su `ComicInfo.xml`, si tiene.
    comic_info: Option<String>,
}

impl Comic {
    /// Abre el cómic y prepara la lista de páginas en orden natural
    /// (`2.jpg` va antes que `10.jpg`).
    pub fn abrir(ruta: impl AsRef<Path>) -> Result<Comic, String> {
        let ruta = ruta.as_ref();
        let formato = Formato::detectar(ruta)?;

        let (contenedor, nombres): (Box<dyn Contenedor>, Vec<String>) = match formato {
            Formato::Zip => caja(formatos::cbz::abrir(ruta)?),
            Formato::Rar => caja(formatos::cbr::abrir(ruta)?),
            Formato::SieteZip => caja(formatos::cb7::abrir(ruta)?),
            Formato::Tar => caja(formatos::cbt::abrir(ruta)?),
            Formato::Carpeta => caja(formatos::carpeta::abrir(ruta)?),
        };

        // Normalmente está en la raíz; si hubiera varios, el menos profundo.
        let comic_info = nombres
            .iter()
            .filter(|n| es_comic_info(n))
            .min_by_key(|n| n.matches('/').count())
            .cloned();

        let mut paginas: Vec<String> = nombres.into_iter().filter(|n| es_imagen(n)).collect();
        paginas.sort_by(|a, b| natord::compare(a, b));

        Ok(Comic {
            ruta: ruta.to_path_buf(),
            formato,
            contenedor,
            paginas,
            comic_info,
        })
    }

    pub fn formato(&self) -> Formato {
        self.formato
    }

    /// Nombres de las páginas, ya ordenados.
    pub fn paginas(&self) -> &[String] {
        &self.paginas
    }

    /// Serie, número, autores... Primero lo que diga su `ComicInfo.xml` y, para lo que
    /// falte, lo que se deduce del nombre. Nunca falla: un `ComicInfo.xml` roto o
    /// ilegible se ignora y queda lo del nombre.
    pub fn metadatos(&mut self) -> Metadatos {
        let mut datos = self
            .comic_info
            .clone()
            .and_then(|nombre| self.contenedor.leer(&nombre).ok())
            .and_then(|bytes| metadatos::de_comic_info(&String::from_utf8_lossy(&bytes)).ok())
            .unwrap_or_default();

        // `file_stem` cortaría el nombre de una carpeta como `Akira Vol. 1` en el punto.
        let nombre = match self.formato {
            Formato::Carpeta => self.ruta.file_name(),
            _ => self.ruta.file_stem(),
        };
        if let Some(nombre) = nombre {
            datos.completar(metadatos::de_nombre(&nombre.to_string_lossy()));
        }
        datos
    }

    /// Bytes (sin decodificar) de la página en la posición `indice`.
    pub fn leer(&mut self, indice: usize) -> Result<Vec<u8>, String> {
        let nombre = self
            .paginas
            .get(indice)
            .ok_or(format!("No existe la página {indice}"))?;

        self.contenedor.leer(nombre)
    }
}

/// Mete en una caja el contenedor que devuelve el `abrir` de cada formato.
fn caja<C: Contenedor + 'static>(
    (contenedor, nombres): (C, Vec<String>),
) -> (Box<dyn Contenedor>, Vec<String>) {
    (Box::new(contenedor), nombres)
}

/// Abre un cómic y devuelve los nombres de sus páginas (solo imágenes), en orden natural.
pub fn listar_paginas(ruta: impl AsRef<Path>) -> Result<Vec<String>, String> {
    Ok(Comic::abrir(ruta)?.paginas)
}

/// Devuelve los bytes de una página concreta de un cómic, buscándola por nombre.
pub fn leer_pagina(ruta: impl AsRef<Path>, nombre: &str) -> Result<Vec<u8>, String> {
    let mut comic = Comic::abrir(ruta)?;

    let indice = comic
        .paginas()
        .iter()
        .position(|p| p == nombre)
        .ok_or(format!("No existe la página {nombre}"))?;

    comic.leer(indice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconoce_extensiones_validas() {
        assert!(es_imagen("01.jpg"));
        assert!(es_imagen("PAGINA_02.PNG"));
        assert!(es_imagen("carpeta/03.webp"));
        assert!(es_imagen("04.gif"));
    }

    #[test]
    fn rechaza_otros_archivos() {
        assert!(!es_imagen("ComicInfo.xml"));
        assert!(!es_imagen("notas.txt"));
        assert!(!es_imagen("jpg"));
    }

    #[test]
    fn ignora_basura_de_macos() {
        assert!(!es_imagen("__MACOSX/._01.jpg"));
        assert!(!es_imagen("capitulo/._01.jpg"));
    }
}
