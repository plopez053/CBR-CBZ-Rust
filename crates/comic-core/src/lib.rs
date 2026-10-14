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

/// Abre un `.cbz` y devuelve los nombres de sus páginas (solo imágenes).
///
/// Las páginas salen en orden natural: `2.jpg` va antes que `10.jpg`.
pub fn listar_paginas(ruta: &str) -> Result<Vec<String>, String> {
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

    Ok(paginas)
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
