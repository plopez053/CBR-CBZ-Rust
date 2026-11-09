//! Lo que hace falta para buscar novedades de las series en catálogos de internet: qué
//! tiene el usuario de cada serie y a qué serie del catálogo corresponde.
//!
//! La biblioteca no sabe nada de los catálogos: solo guarda lo que le dicen.

use std::collections::HashMap;

use rusqlite::params;

use crate::serie::nombre_de_grupo;
use crate::{orden_de_lectura, Biblioteca, GRUPO};

/// Lo que tiene el usuario de una serie, para compararlo con lo publicado.
#[derive(Clone, Debug, PartialEq)]
pub struct Coleccion {
    pub nombre: String,
    /// La de [`Serie::clave`](crate::Serie::clave).
    pub clave: String,
    /// Los números que tiene, sin repetir y en orden de lectura.
    pub numeros: Vec<String>,
    /// Los volúmenes o tomos que tiene, sin repetir y de menor a mayor.
    pub volumenes: Vec<u32>,
    /// El año más antiguo de sus cómics.
    pub anio: Option<u32>,
    /// Algún cómic dice que se lee de derecha a izquierda.
    pub manga: bool,
    pub comics: usize,
}

/// En qué quedó la búsqueda de una serie en los catálogos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EstadoVinculo {
    /// Se encontró sola, porque el nombre coincidía.
    Automatico,
    /// La eligió el usuario.
    Elegido,
    /// Se buscó y no salió nada seguro: el usuario puede elegirla a mano.
    SinCoincidencia,
    /// El usuario no quiere que se busque.
    Ignorado,
}

impl EstadoVinculo {
    fn clave(self) -> &'static str {
        match self {
            EstadoVinculo::Automatico => "automatico",
            EstadoVinculo::Elegido => "elegido",
            EstadoVinculo::SinCoincidencia => "sin_coincidencia",
            EstadoVinculo::Ignorado => "ignorado",
        }
    }

    fn desde_clave(clave: &str) -> Option<EstadoVinculo> {
        [
            EstadoVinculo::Automatico,
            EstadoVinculo::Elegido,
            EstadoVinculo::SinCoincidencia,
            EstadoVinculo::Ignorado,
        ]
        .into_iter()
        .find(|e| e.clave() == clave)
    }
}

/// A qué serie de un catálogo corresponde una serie de la biblioteca.
#[derive(Clone, Debug, PartialEq)]
pub struct Vinculo {
    /// La de [`Serie::clave`](crate::Serie::clave).
    pub clave: String,
    /// El catálogo (`anilist`, `metron`...), su identificador de la serie y cómo se llama
    /// allí. Vacíos si no se encontró o se ignora.
    pub externa: Option<SerieExterna>,
    pub estado: EstadoVinculo,
    /// Cuándo se buscó por última vez, en segundos desde 1970.
    pub buscada: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SerieExterna {
    pub fuente: String,
    pub id: String,
    pub nombre: String,
}

impl Biblioteca {
    /// Lo que hay de cada serie, por nombre en orden natural.
    pub fn colecciones(&self) -> Result<Vec<Coleccion>, String> {
        let mut consulta = self
            .db
            .prepare(&format!(
                "SELECT {GRUPO}, c.serie, c.numero, c.volumen, c.anio, c.derecha_a_izquierda,
                        c.ruta
                 FROM comics c
                     LEFT JOIN series_unidas u ON u.clave = c.serie_clave"
            ))
            .map_err(|e| e.to_string())?;

        type Fila = (
            String,
            String,
            Option<String>,
            Option<u32>,
            Option<u32>,
            bool,
            String,
        );
        let filas: Vec<Fila> = consulta
            .query_map([], |f| {
                Ok((
                    f.get(0)?,
                    f.get(1)?,
                    f.get(2)?,
                    f.get(3)?,
                    f.get(4)?,
                    f.get(5)?,
                    f.get(6)?,
                ))
            })
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;

        let mut grupos: HashMap<&str, Vec<&Fila>> = HashMap::new();
        for fila in &filas {
            grupos.entry(&fila.0).or_default().push(fila);
        }

        let mut colecciones: Vec<Coleccion> = grupos
            .into_iter()
            .map(|(clave, mut comics)| {
                comics.sort_by(|a, b| {
                    orden_de_lectura((a.3, a.2.as_deref(), &a.6), (b.3, b.2.as_deref(), &b.6))
                });
                let mut numeros: Vec<String> = Vec::new();
                for numero in comics.iter().filter_map(|c| c.2.clone()) {
                    if !numeros.contains(&numero) {
                        numeros.push(numero);
                    }
                }
                let mut volumenes: Vec<u32> = comics.iter().filter_map(|c| c.3).collect();
                volumenes.sort_unstable();
                volumenes.dedup();
                Coleccion {
                    nombre: nombre_de_grupo(comics.iter().map(|c| c.1.as_str())),
                    clave: clave.to_string(),
                    numeros,
                    volumenes,
                    anio: comics.iter().filter_map(|c| c.4).min(),
                    manga: comics.iter().any(|c| c.5),
                    comics: comics.len(),
                }
            })
            .collect();
        colecciones.sort_by(|a, b| natord::compare_ignore_case(&a.nombre, &b.nombre));
        Ok(colecciones)
    }

    /// Todos los vínculos guardados, por clave de serie.
    pub fn vinculos(&self) -> Result<HashMap<String, Vinculo>, String> {
        let mut consulta = self
            .db
            .prepare("SELECT clave, fuente, externo, nombre, estado, buscada FROM series_externas")
            .map_err(|e| e.to_string())?;
        type Fila = (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            i64,
        );
        let filas: Vec<Fila> = consulta
            .query_map([], |f| {
                Ok((
                    f.get(0)?,
                    f.get(1)?,
                    f.get(2)?,
                    f.get(3)?,
                    f.get(4)?,
                    f.get(5)?,
                ))
            })
            .and_then(Iterator::collect)
            .map_err(|e| e.to_string())?;

        Ok(filas
            .into_iter()
            .filter_map(|(clave, fuente, id, nombre, estado, buscada)| {
                // Un estado que no se conoce sería de una versión más nueva: se ignora y
                // la serie se vuelve a buscar.
                let estado = EstadoVinculo::desde_clave(&estado)?;
                let externa = match (fuente, id) {
                    (Some(fuente), Some(id)) => Some(SerieExterna {
                        fuente,
                        id,
                        nombre: nombre.unwrap_or_default(),
                    }),
                    _ => None,
                };
                Some((
                    clave.clone(),
                    Vinculo {
                        clave,
                        externa,
                        estado,
                        buscada,
                    },
                ))
            })
            .collect())
    }

    /// Guarda (o sustituye) el vínculo de una serie.
    pub fn guardar_vinculo(&self, vinculo: &Vinculo) -> Result<(), String> {
        let externa = vinculo.externa.as_ref();
        self.db
            .execute(
                "INSERT OR REPLACE INTO series_externas
                     (clave, fuente, externo, nombre, estado, buscada)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    vinculo.clave,
                    externa.map(|e| &e.fuente),
                    externa.map(|e| &e.id),
                    externa.map(|e| &e.nombre),
                    vinculo.estado.clave(),
                    vinculo.buscada,
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Olvida el vínculo de una serie: en la próxima comprobación se vuelve a buscar.
    pub fn quitar_vinculo(&self, clave: &str) -> Result<(), String> {
        self.db
            .execute("DELETE FROM series_externas WHERE clave = ?1", [clave])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
