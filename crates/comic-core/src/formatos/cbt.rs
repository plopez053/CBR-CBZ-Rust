//! CBT: un tar. No está comprimido, así que al abrirlo apuntamos dónde empieza y cuánto
//! mide cada archivo, y luego leemos directamente esos bytes.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::Contenedor;

pub struct Cbt {
    archivo: File,
    /// Para cada nombre: dónde empiezan sus bytes dentro del tar y cuántos son.
    posiciones: HashMap<String, (u64, u64)>,
}

pub fn abrir(ruta: &Path) -> Result<(Cbt, Vec<String>), String> {
    let file = File::open(ruta).map_err(|e| e.to_string())?;
    let mut tar = tar::Archive::new(file);

    let mut nombres = Vec::new();
    let mut posiciones = HashMap::new();
    // `with_seek` salta el contenido de cada archivo en vez de leerlo.
    for entrada in tar.entries_with_seek().map_err(|e| e.to_string())? {
        let entrada = entrada.map_err(|e| e.to_string())?;
        if entrada.header().entry_type().is_file() {
            let ruta = entrada.path().map_err(|e| e.to_string())?;
            let nombre = ruta.to_string_lossy().replace('\\', "/");
            posiciones.insert(
                nombre.clone(),
                (entrada.raw_file_position(), entrada.size()),
            );
            nombres.push(nombre);
        }
    }

    let cbt = Cbt {
        archivo: tar.into_inner(),
        posiciones,
    };
    Ok((cbt, nombres))
}

impl Contenedor for Cbt {
    fn leer(&mut self, nombre: &str) -> Result<Vec<u8>, String> {
        let &(inicio, tamano) = self
            .posiciones
            .get(nombre)
            .ok_or(format!("No existe {nombre} en el tar"))?;

        let mut bytes = vec![0; tamano as usize];
        self.archivo
            .seek(SeekFrom::Start(inicio))
            .and_then(|_| self.archivo.read_exact(&mut bytes))
            .map_err(|e| e.to_string())?;

        Ok(bytes)
    }
}
