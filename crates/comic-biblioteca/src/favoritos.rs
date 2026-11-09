//! Favoritos: obras y números de los catálogos de novedades que el usuario se guarda para
//! buscarlos más adelante. No son cómics de la biblioteca: no tienen archivo.

use rusqlite::params;

use crate::Biblioteca;

#[derive(Clone, Debug, PartialEq)]
pub struct Favorito {
    /// Qué es y en qué catálogo, para volver a pedir su ficha (la pone quien lo guarda).
    pub clave: String,
    pub titulo: String,
    /// La dirección de la portada en el catálogo.
    pub portada: Option<String>,
    /// Un resumen en una línea, para la tarjeta.
    pub linea: String,
    /// Cuándo se guardó, en segundos desde 1970. Los más nuevos van primero.
    pub anadido: i64,
}

impl Biblioteca {
    /// Los favoritos, del más nuevo al más antiguo.
    pub fn favoritos(&self) -> Result<Vec<Favorito>, String> {
        let mut consulta = self
            .db
            .prepare(
                "SELECT clave, titulo, portada, linea, anadido FROM favoritos
                 ORDER BY anadido DESC, titulo",
            )
            .map_err(|e| e.to_string())?;
        let filas = consulta
            .query_map([], |f| {
                Ok(Favorito {
                    clave: f.get(0)?,
                    titulo: f.get(1)?,
                    portada: f.get(2)?,
                    linea: f.get(3)?,
                    anadido: f.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        filas.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// Guarda `favorito`, o lo pone al día si ya estaba (sin cambiar cuándo se añadió).
    pub fn anadir_favorito(&self, favorito: &Favorito) -> Result<(), String> {
        self.db
            .execute(
                "INSERT INTO favoritos (clave, titulo, portada, linea, anadido)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (clave) DO UPDATE SET
                     titulo = excluded.titulo,
                     portada = excluded.portada,
                     linea = excluded.linea",
                params![
                    favorito.clave,
                    favorito.titulo,
                    favorito.portada,
                    favorito.linea,
                    favorito.anadido,
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn quitar_favorito(&self, clave: &str) -> Result<(), String> {
        self.db
            .execute("DELETE FROM favoritos WHERE clave = ?1", [clave])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
