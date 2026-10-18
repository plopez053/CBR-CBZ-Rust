//! CBZ: un zip. Permite saltar directamente a cualquier archivo.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::Contenedor;

pub struct Cbz {
    archivo: zip::ZipArchive<File>,
}

pub fn abrir(ruta: &Path) -> Result<(Cbz, Vec<String>), String> {
    let file = File::open(ruta).map_err(|e| e.to_string())?;
    let mut archivo = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    let mut nombres = Vec::new();
    for i in 0..archivo.len() {
        let entrada = archivo.by_index(i).map_err(|e| e.to_string())?;
        let nombre = entrada.name().map_err(|e| e.to_string())?;

        if !entrada.is_dir() {
            nombres.push(nombre.to_string());
        }
    }

    Ok((Cbz { archivo }, nombres))
}

impl Contenedor for Cbz {
    fn leer(&mut self, nombre: &str) -> Result<Vec<u8>, String> {
        let mut entrada = self.archivo.by_name(nombre).map_err(|e| e.to_string())?;

        let mut bytes = Vec::new();
        entrada.read_to_end(&mut bytes).map_err(|e| e.to_string())?;

        Ok(bytes)
    }
}
