//! Núcleo del lector: todo lo que no depende de la interfaz.

/// Extensiones que consideramos páginas de un cómic.
const EXTENSIONES_IMAGEN: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// Dice si el nombre de un archivo dentro de un CBZ/CBR es una página.
///
/// Ignora la basura que macOS mete en los zips (`__MACOSX/...`).
pub fn es_imagen(nombre: &str) -> bool {
    let minusculas = nombre.to_lowercase();

    if minusculas.contains("__macosx") {
        return false;
    }

    EXTENSIONES_IMAGEN
        .iter()
        .any(|ext| minusculas.ends_with(&format!(".{ext}")))
}

/// Un `.cbz` abierto: el zip se abre una vez y se queda abierto mientras se lee.
pub struct Cbz {
    archive: zip::ZipArchive<std::fs::File>,
    paginas: Vec<String>,
}

impl Cbz {
    /// Abre el archivo y prepara la lista de páginas en orden natural
    /// (`2.jpg` va antes que `10.jpg`).
    pub fn abrir(ruta: &str) -> Result<Cbz, String> {
        let file = std::fs::File::open(ruta).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

        let mut paginas = Vec::new();

        for i in 0..archive.len() {
            let entrada = archive.by_index(i).map_err(|e| e.to_string())?;
            let nombre = entrada.name().map_err(|e| e.to_string())?;

            if !entrada.is_dir() && es_imagen(&nombre) {
                paginas.push(nombre.to_string());
            }
        }

        paginas.sort_by(|a, b| natord::compare(a, b));

        Ok(Cbz { archive, paginas })
    }

    /// Nombres de las páginas, ya ordenados.
    pub fn paginas(&self) -> &[String] {
        &self.paginas
    }

    /// Bytes (sin decodificar) de la página en la posición `indice`.
    pub fn leer(&mut self, indice: usize) -> Result<Vec<u8>, String> {
        let nombre = self
            .paginas
            .get(indice)
            .ok_or(format!("No existe la página {indice}"))?;

        let mut entrada = self.archive.by_name(nombre).map_err(|e| e.to_string())?;

        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entrada, &mut bytes).map_err(|e| e.to_string())?;

        Ok(bytes)
    }
}

/// Abre un `.cbz` y devuelve los nombres de sus páginas (solo imágenes), en orden natural.
pub fn listar_paginas(ruta: &str) -> Result<Vec<String>, String> {
    Ok(Cbz::abrir(ruta)?.paginas)
}

/// Devuelve los bytes de una página concreta de un `.cbz`, buscándola por nombre.
pub fn leer_pagina(ruta: &str, nombre: &str) -> Result<Vec<u8>, String> {
    let mut cbz = Cbz::abrir(ruta)?;

    let indice = cbz
        .paginas()
        .iter()
        .position(|p| p == nombre)
        .ok_or(format!("No existe la página {nombre}"))?;

    cbz.leer(indice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconoce_extensiones_validas() {
        assert!(es_imagen("01.jpg"));
        assert!(es_imagen("PAGINA_02.PNG"));
        assert!(es_imagen("carpeta/03.webp"));
    }

    #[test]
    fn rechaza_otros_archivos() {
        assert!(!es_imagen("ComicInfo.xml"));
        assert!(!es_imagen("notas.txt"));
    }

    #[test]
    fn ignora_basura_de_macos() {
        assert!(!es_imagen("__MACOSX/._01.jpg"));
    }
}
