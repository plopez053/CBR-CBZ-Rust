//! Peticiones a internet, sin pasarse del ritmo que permite cada servicio.

use std::fmt;
use std::io::Read;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use ureq::http::Response;
use ureq::Body;

/// Cuánto se espera como mucho a una respuesta.
const TIEMPO_MAXIMO: Duration = Duration::from_secs(30);
/// Si un servicio pide esperar más que esto, se deja para otra vez.
const ESPERA_MAXIMA: Duration = Duration::from_secs(90);
/// Tamaño máximo de una respuesta (una portada grande o un RSS largo caben de sobra).
const TAMANO_MAXIMO: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorRed {
    /// No se pudo llegar al servidor (sin internet, DNS, tiempo agotado...).
    Conexion(String),
    /// El servidor no acepta el usuario o la contraseña.
    Autenticacion,
    /// El servidor pide que se espere más de la cuenta.
    Limite,
    /// Otra respuesta de error del servidor.
    Http(u16),
    /// La respuesta no se entiende.
    Datos(String),
}

impl fmt::Display for ErrorRed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorRed::Conexion(e) => write!(f, "no se pudo conectar ({e})"),
            ErrorRed::Autenticacion => write!(f, "el usuario o la contraseña no son correctos"),
            ErrorRed::Limite => write!(f, "demasiadas peticiones seguidas; prueba más tarde"),
            ErrorRed::Http(404) => write!(f, "no existe (error 404)"),
            ErrorRed::Http(codigo @ (401 | 403)) => {
                write!(f, "el servidor ha rechazado la petición (error {codigo})")
            }
            ErrorRed::Http(codigo) => write!(f, "el servidor ha respondido con un error {codigo}"),
            ErrorRed::Datos(e) => write!(f, "respuesta que no se entiende ({e})"),
        }
    }
}

/// Deja pasar una petición cada `intervalo` como mucho: si se pide antes, espera. Hay
/// uno por servicio, compartido por todos los hilos (una comprobación y una ficha que se
/// pide a la vez cuentan juntas para el límite).
pub struct Limitador {
    intervalo: Duration,
    /// Cuándo puede salir la siguiente petición.
    siguiente: Mutex<Option<Instant>>,
}

impl Limitador {
    pub const fn nuevo(intervalo: Duration) -> Limitador {
        Limitador {
            intervalo,
            siguiente: Mutex::new(None),
        }
    }

    pub fn esperar(&self) {
        // Se reserva el turno con el candado puesto y se espera ya sin él.
        let turno = {
            let mut siguiente = self.siguiente.lock().unwrap_or_else(|e| e.into_inner());
            let ahora = Instant::now();
            let turno = siguiente.map_or(ahora, |s| s.max(ahora));
            *siguiente = Some(turno + self.intervalo);
            turno
        };
        let espera = turno.saturating_duration_since(Instant::now());
        if !espera.is_zero() {
            thread::sleep(espera);
        }
    }
}

/// Un cliente HTTPS compartido por todas las peticiones (reutiliza las conexiones).
#[derive(Clone)]
pub struct Cliente {
    agente: ureq::Agent,
}

impl Default for Cliente {
    fn default() -> Self {
        Cliente::nuevo()
    }
}

impl Cliente {
    pub fn nuevo() -> Cliente {
        let agente = ureq::Agent::config_builder()
            .timeout_global(Some(TIEMPO_MAXIMO))
            .user_agent(concat!("LectorComics/", env!("CARGO_PKG_VERSION")))
            // Los códigos de error se miran a mano, para distinguir cada caso.
            .http_status_as_error(false)
            .build()
            .new_agent();
        Cliente { agente }
    }

    /// Pide `url` (con los parámetros `consulta`) y devuelve el texto de la respuesta.
    pub fn texto(
        &self,
        url: &str,
        consulta: &[(&str, String)],
        autorizacion: Option<&str>,
    ) -> Result<String, ErrorRed> {
        let mut respuesta = self
            .enviar(|| {
                let mut peticion = self.agente.get(url);
                for (clave, valor) in consulta {
                    peticion = peticion.query(*clave, valor);
                }
                if let Some(autorizacion) = autorizacion {
                    peticion = peticion.header("Authorization", autorizacion);
                }
                peticion.call()
            })
            .map_err(|e| match e {
                // Solo tiene sentido hablar de usuario y contraseña si se han enviado.
                ErrorRed::Http(401 | 403) if autorizacion.is_some() => ErrorRed::Autenticacion,
                otro => otro,
            })?;
        respuesta
            .body_mut()
            .with_config()
            .limit(TAMANO_MAXIMO)
            .lossy_utf8(true)
            .read_to_string()
            .map_err(|e| ErrorRed::Conexion(e.to_string()))
    }

    /// Envía `cuerpo` (JSON) a `url` con POST y devuelve el texto de la respuesta.
    pub fn enviar_json(&self, url: &str, cuerpo: &str) -> Result<String, ErrorRed> {
        let mut respuesta = self.enviar(|| {
            self.agente
                .post(url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .send(cuerpo)
        })?;
        respuesta
            .body_mut()
            .with_config()
            .limit(TAMANO_MAXIMO)
            .read_to_string()
            .map_err(|e| ErrorRed::Conexion(e.to_string()))
    }

    /// Los primeros `bytes` de una página web, como texto: lo justo para leer su cabecera.
    pub fn principio(&self, url: &str, bytes: u64) -> Result<String, ErrorRed> {
        let mut respuesta = self.enviar(|| self.agente.get(url).call())?;
        let mut datos = Vec::new();
        respuesta
            .body_mut()
            .as_reader()
            .take(bytes)
            .read_to_end(&mut datos)
            .map_err(|e| ErrorRed::Conexion(e.to_string()))?;
        Ok(String::from_utf8_lossy(&datos).into_owned())
    }

    /// Descarga `url` tal cual (una imagen).
    pub fn bytes(&self, url: &str) -> Result<Vec<u8>, ErrorRed> {
        let mut respuesta = self.enviar(|| self.agente.get(url).call())?;
        respuesta
            .body_mut()
            .with_config()
            .limit(TAMANO_MAXIMO)
            .read_to_vec()
            .map_err(|e| ErrorRed::Conexion(e.to_string()))
    }

    /// Hace la petición y mira el código de la respuesta. Si el servidor pide esperar
    /// (429) un rato razonable, espera y lo intenta una vez más.
    fn enviar(
        &self,
        hacer: impl Fn() -> Result<Response<Body>, ureq::Error>,
    ) -> Result<Response<Body>, ErrorRed> {
        for intento in 0..2 {
            let respuesta = hacer().map_err(|e| ErrorRed::Conexion(e.to_string()))?;
            let codigo = respuesta.status().as_u16();
            match codigo {
                200..=299 => return Ok(respuesta),
                429 if intento == 0 => {
                    let espera = respuesta
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .map_or(Duration::from_secs(60), Duration::from_secs);
                    if espera > ESPERA_MAXIMA {
                        return Err(ErrorRed::Limite);
                    }
                    eprintln!("[novedades] El servidor pide esperar {espera:?}");
                    thread::sleep(espera + Duration::from_secs(1));
                }
                429 => return Err(ErrorRed::Limite),
                _ => return Err(ErrorRed::Http(codigo)),
            }
        }
        Err(ErrorRed::Limite)
    }
}

/// La cabecera `Authorization` de HTTP Basic.
pub fn autorizacion_basica(usuario: &str, contrasena: &str) -> String {
    format!(
        "Basic {}",
        base64(format!("{usuario}:{contrasena}").as_bytes())
    )
}

fn base64(datos: &[u8]) -> String {
    const ALFABETO: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut texto = String::with_capacity(datos.len().div_ceil(3) * 4);
    for trozo in datos.chunks(3) {
        let b = [
            trozo[0],
            trozo.get(1).copied().unwrap_or(0),
            trozo.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= trozo.len() {
                texto.push(ALFABETO[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                texto.push('=');
            }
        }
    }
    texto
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_como_el_estandar() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(
            autorizacion_basica("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }
}
