//! Una carpeta con las imágenes sueltas, incluidas las de sus subcarpetas.

use std::fs;
use std::path::{Path, PathBuf};

use crate::Contenedor;

pub struct Carpeta {
    raiz: PathBuf,
}

pub fn abrir(ruta: &Path) -> Result<(Carpeta, Vec<String>), String> {
    let mut nombres = Vec::new();
    recorrer(ruta, "", &mut nombres)?;

    let carpeta = Carpeta {
        raiz: ruta.to_path_buf(),
    };
    Ok((carpeta, nombres))
}

/// Añade a `nombres` los archivos de `carpeta`, con su ruta desde la raíz (`prefijo`).
fn recorrer(carpeta: &Path, prefijo: &str, nombres: &mut Vec<String>) -> Result<(), String> {
    for entrada in fs::read_dir(carpeta).map_err(|e| e.to_string())? {
        let entrada = entrada.map_err(|e| e.to_string())?;
        let nombre = format!("{prefijo}{}", entrada.file_name().to_string_lossy());
        let tipo = entrada.file_type().map_err(|e| e.to_string())?;

        if tipo.is_dir() {
            recorrer(&entrada.path(), &format!("{nombre}/"), nombres)?;
        } else if tipo.is_file() {
            nombres.push(nombre);
        }
    }
    Ok(())
}

impl Contenedor for Carpeta {
    fn leer(&mut self, nombre: &str) -> Result<Vec<u8>, String> {
        fs::read(self.raiz.join(nombre)).map_err(|e| e.to_string())
    }
}
