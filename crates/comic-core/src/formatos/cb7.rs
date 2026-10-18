//! CB7: un 7z.
//!
//! Los 7z suelen ser sólidos: para leer un archivo hay que descomprimir los que van
//! antes en su bloque. Con cómics normales es tolerable, pero las últimas páginas de
//! uno muy grande tardan más en salir.

use std::fs::File;
use std::path::Path;

use sevenz_rust2::{ArchiveReader, Password};

use crate::Contenedor;

pub struct Cb7 {
    lector: ArchiveReader<File>,
}

pub fn abrir(ruta: &Path) -> Result<(Cb7, Vec<String>), String> {
    let mut lector = ArchiveReader::open(ruta, Password::empty()).map_err(|e| e.to_string())?;
    // Ya hay un hilo por página en el precargador: más hilos aquí solo se pisarían.
    lector.set_thread_count(1);

    let nombres = lector
        .archive()
        .files
        .iter()
        .filter(|entrada| !entrada.is_directory && entrada.has_stream)
        .map(|entrada| entrada.name.clone())
        .collect();

    Ok((Cb7 { lector }, nombres))
}

impl Contenedor for Cb7 {
    fn leer(&mut self, nombre: &str) -> Result<Vec<u8>, String> {
        self.lector.read_file(nombre).map_err(|e| e.to_string())
    }
}
