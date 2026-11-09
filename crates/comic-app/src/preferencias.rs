//! Lo que el usuario elige y se recuerda de una vez para otra, en un archivo de texto
//! con una línea `clave=valor` por preferencia.

use std::fs;
use std::path::{Path, PathBuf};

use crate::controles::Controles;
use crate::filtro::Filtro;

/// Cómo se pasa de una página a otra en el lector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Animacion {
    /// La hoja se levanta y gira como en papel.
    Hoja,
    /// La página nueva entra deslizándose desde un lado.
    Deslizar,
    Ninguna,
}

impl Animacion {
    pub const TODAS: [Animacion; 3] = [Animacion::Hoja, Animacion::Deslizar, Animacion::Ninguna];

    pub fn nombre(self) -> &'static str {
        match self {
            Animacion::Hoja => "Pasar hoja",
            Animacion::Deslizar => "Deslizar",
            Animacion::Ninguna => "Ninguna",
        }
    }

    fn clave(self) -> &'static str {
        match self {
            Animacion::Hoja => "hoja",
            Animacion::Deslizar => "deslizar",
            Animacion::Ninguna => "ninguna",
        }
    }

    fn desde_clave(clave: &str) -> Option<Animacion> {
        Animacion::TODAS.into_iter().find(|a| a.clave() == clave)
    }
}

pub struct Preferencias {
    pub animacion: Animacion,
    pub filtro: Filtro,
    pub controles: Controles,
    archivo: PathBuf,
}

impl Preferencias {
    /// Lee las preferencias de `archivo`. Si no existe o algo no se entiende, se usa el
    /// valor por defecto de eso.
    pub fn cargar(archivo: PathBuf) -> Preferencias {
        let mut preferencias = Preferencias {
            animacion: Animacion::Ninguna,
            filtro: Filtro::default(),
            controles: Controles::default(),
            archivo,
        };
        let Ok(texto) = fs::read_to_string(&preferencias.archivo) else {
            return preferencias;
        };
        for linea in texto.lines() {
            let Some((clave, valor)) = linea.split_once('=') else {
                continue;
            };
            let valor = valor.trim();
            let filtro = &mut preferencias.filtro;
            match clave.trim() {
                "animacion" => match Animacion::desde_clave(valor) {
                    Some(animacion) => preferencias.animacion = animacion,
                    None => eprintln!("[preferencias] Animación desconocida: {valor}"),
                },
                "filtro" => match valor {
                    "si" => filtro.activo = true,
                    "no" => filtro.activo = false,
                    _ => eprintln!("[preferencias] Filtro desconocido: {valor}"),
                },
                "temperatura" => match valor.parse() {
                    Ok(kelvin) => filtro.temperatura = kelvin,
                    Err(_) => eprintln!("[preferencias] Temperatura desconocida: {valor}"),
                },
                "brillo" => match valor.parse::<f32>() {
                    Ok(porcentaje) => filtro.brillo = porcentaje / 100.0,
                    Err(_) => eprintln!("[preferencias] Brillo desconocido: {valor}"),
                },
                otra => {
                    if !preferencias.controles.leer_linea(otra, valor) {
                        eprintln!("[preferencias] Clave desconocida: {otra}");
                    }
                }
            }
        }
        preferencias
    }

    pub fn guardar(&self) {
        let filtro = &self.filtro;
        let texto = format!(
            "animacion={}\nfiltro={}\ntemperatura={:.0}\nbrillo={:.0}\n{}",
            self.animacion.clave(),
            if filtro.activo { "si" } else { "no" },
            filtro.temperatura,
            filtro.brillo * 100.0,
            self.controles.a_lineas(),
        );
        let resultado = crear_carpeta(&self.archivo)
            .and_then(|()| fs::write(&self.archivo, texto).map_err(|e| e.to_string()));
        match resultado {
            Ok(()) => eprintln!("[preferencias] Guardadas en {}", self.archivo.display()),
            Err(e) => eprintln!(
                "[preferencias] No se pudieron guardar en {}: {e}",
                self.archivo.display()
            ),
        }
    }
}

fn crear_carpeta(archivo: &Path) -> Result<(), String> {
    match archivo.parent() {
        Some(carpeta) if !carpeta.as_os_str().is_empty() => {
            fs::create_dir_all(carpeta).map_err(|e| e.to_string())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn se_guardan_y_se_leen() {
        let carpeta = std::env::temp_dir().join(format!("preferencias-{}", std::process::id()));
        let archivo = carpeta.join("preferencias.txt");
        let mut preferencias = Preferencias::cargar(archivo.clone());
        assert_eq!(preferencias.animacion, Animacion::Ninguna);

        preferencias.animacion = Animacion::Deslizar;
        let filtro = Filtro {
            activo: true,
            temperatura: 2700.0,
            brillo: 0.8,
        };
        preferencias.filtro = filtro;
        preferencias.guardar();
        let leidas = Preferencias::cargar(archivo);
        assert_eq!(leidas.animacion, Animacion::Deslizar);
        assert_eq!(leidas.filtro, filtro);
        let _ = fs::remove_dir_all(carpeta);
    }
}
