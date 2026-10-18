//! CBR: un rar. La librería de RARLAB solo sabe recorrerlo de principio a fin,
//! así que guardamos por dónde vamos y seguimos desde ahí en la siguiente lectura.
//!
//! Importa sobre todo en los rar "sólidos" (comprimidos como un único bloque): en
//! ellos, saltarse un archivo también obliga a descomprimirlo. Volviendo a empezar
//! cada vez, leer la página 100 costaría descomprimir las 99 anteriores.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use unrar::{Archive, CursorBeforeHeader, OpenArchive, Process};

use crate::Contenedor;

// La librería de RARLAB usa funciones de advapi32 (registro y permisos de Windows),
// pero `unrar_sys` no pide enlazarla y el enlazador falla sin esto.
#[cfg(windows)]
#[link(name = "advapi32")]
extern "system" {}

pub struct Cbr {
    ruta: PathBuf,
    /// Posición de cada archivo dentro del rar, para saber si queda por delante del cursor.
    orden: HashMap<String, usize>,
    cursor: Option<Cursor>,
}

/// El rar abierto justo antes de la cabecera del archivo número `posicion`.
struct Cursor {
    archivo: OpenArchive<Process, CursorBeforeHeader>,
    posicion: usize,
}

// SAFETY: `OpenArchive` guarda un puntero a la estructura de la librería de C, y por eso
// Rust no lo deja pasar a otro hilo. Ese puntero solo lo usa este `Cursor` (nunca se
// comparte) y la librería no lo ata al hilo que lo creó, así que moverlo es seguro.
unsafe impl Send for Cursor {}

pub fn abrir(ruta: &Path) -> Result<(Cbr, Vec<String>), String> {
    // Listar solo lee las cabeceras: no descomprime nada, ni en los sólidos.
    let lista = Archive::new(ruta)
        .open_for_listing()
        .map_err(|e| e.to_string())?;

    let mut nombres = Vec::new();
    let mut orden = HashMap::new();
    // Las carpetas también cuentan para la posición: al leer aparecen igual.
    for (posicion, entrada) in lista.enumerate() {
        let entrada = entrada.map_err(|e| e.to_string())?;
        if entrada.is_file() {
            let nombre = nombre_de(&entrada.filename);
            orden.insert(nombre.clone(), posicion);
            nombres.push(nombre);
        }
    }

    let cbr = Cbr {
        ruta: ruta.to_path_buf(),
        orden,
        cursor: None,
    };
    Ok((cbr, nombres))
}

/// Los rar hechos en Windows usan `\`; los dejamos todos con `/`, como los zip.
fn nombre_de(ruta: &Path) -> String {
    ruta.to_string_lossy().replace('\\', "/")
}

impl Contenedor for Cbr {
    fn leer(&mut self, nombre: &str) -> Result<Vec<u8>, String> {
        let objetivo = *self
            .orden
            .get(nombre)
            .ok_or(format!("No existe {nombre} en el rar"))?;

        // Hacia atrás no se puede ir: si ya lo hemos pasado, hay que volver a abrir.
        // Si algo falla a medias, el cursor se pierde y la siguiente lectura reabre.
        let Cursor {
            mut archivo,
            mut posicion,
        } = match self.cursor.take() {
            Some(cursor) if cursor.posicion <= objetivo => cursor,
            _ => Cursor {
                archivo: Archive::new(&self.ruta)
                    .open_for_processing()
                    .map_err(|e| e.to_string())?,
                posicion: 0,
            },
        };

        loop {
            let cabecera = archivo
                .read_header()
                .map_err(|e| e.to_string())?
                .ok_or(format!("No existe {nombre} en el rar"))?;
            posicion += 1;

            // Se compara el nombre y no solo la posición, por si el recuento se desviara.
            if nombre_de(&cabecera.entry().filename) == nombre {
                let (bytes, resto) = cabecera.read().map_err(|e| e.to_string())?;
                self.cursor = Some(Cursor {
                    archivo: resto,
                    posicion,
                });
                return Ok(bytes);
            }
            archivo = cabecera.skip().map_err(|e| e.to_string())?;
        }
    }
}
