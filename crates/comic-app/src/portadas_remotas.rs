//! Portadas de internet (las de AniList y Metron) y fotos de las noticias, descargadas
//! en hilos aparte.
//!
//! Como las miniaturas de la biblioteca: la primera vez se descargan y se guardan en
//! disco encogidas, y luego se encogen con Lanczos3 al tamaño exacto en píxeles del
//! hueco para dibujarlas 1:1. Solo se piden las que se ven.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use comic_imagen::{Encogedor, Imagen};
use comic_novedades::Cliente;
use eframe::egui::{self, Rect, Stroke, StrokeKind};

use crate::estilo::{self, Colores};
use crate::precarga::{tamano_textura, Tamano};

/// Hilos de descarga. Las imágenes salen de los servidores de imágenes de cada web, no
/// de sus API, y seis a la vez es lo que abre un navegador con cada servidor.
const HILOS: usize = 6;
/// Tamaño de las portadas guardadas en disco, y su calidad (como las miniaturas).
const CAJA_EN_DISCO: Tamano = [480, 720];
const CALIDAD_JPEG: u8 = 92;
/// Las portadas que llevan este tiempo sin usarse se borran del disco al arrancar: las
/// de las novedades de cada semana dejan de hacer falta enseguida.
const CADUCIDAD: Duration = Duration::from_secs(30 * 86_400);
/// Texturas que se guardan en la GPU como mucho.
const MAX_TEXTURAS: usize = 200;

/// Cómo se coloca una imagen en su hueco.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Encaje {
    /// Entera, centrada (las portadas).
    Entera,
    /// Llenando el hueco y recortando lo que sobra (las fotos de las noticias).
    Rellenar,
}

type Clave = (String, Tamano, Encaje);

#[derive(Clone, PartialEq)]
struct Peticion {
    url: String,
    caja: Tamano,
    encaje: Encaje,
}

impl Peticion {
    fn clave(&self) -> Clave {
        (self.url.clone(), self.caja, self.encaje)
    }
}

enum Respuesta {
    Lista {
        clave: Clave,
        imagen: egui::ColorImage,
    },
    Error {
        url: String,
    },
}

struct Cola {
    peticiones: VecDeque<Peticion>,
    en_curso: HashSet<Clave>,
    cerrada: bool,
}

struct Textura {
    textura: egui::TextureHandle,
    usada: u64,
}

pub struct PortadasRemotas {
    cola: Arc<(Mutex<Cola>, Condvar)>,
    resultados: Receiver<Respuesta>,
    texturas: HashMap<Clave, Textura>,
    /// Las que no se pudieron descargar: no se vuelven a pedir en esta sesión.
    fallidas: HashSet<String>,
    /// Lo pedido en este fotograma, y lo último que se mandó a los hilos.
    pedidas: Vec<Peticion>,
    enviadas: Vec<Peticion>,
    fotograma: u64,
}

impl PortadasRemotas {
    pub fn nuevo(carpeta: PathBuf, ctx: egui::Context) -> PortadasRemotas {
        if let Err(e) = fs::create_dir_all(&carpeta) {
            eprintln!("[portadas] No se pudo crear {}: {e}", carpeta.display());
        }
        let limpiar = carpeta.clone();
        thread::spawn(move || borrar_caducadas(&limpiar));

        let cola = Arc::new((
            Mutex::new(Cola {
                peticiones: VecDeque::new(),
                en_curso: HashSet::new(),
                cerrada: false,
            }),
            Condvar::new(),
        ));
        let (enviar, resultados) = mpsc::channel();
        let cliente = Cliente::nuevo();
        for _ in 0..HILOS {
            let trabajador = Trabajador {
                cola: Arc::clone(&cola),
                resultados: enviar.clone(),
                carpeta: carpeta.clone(),
                cliente: cliente.clone(),
                ctx: ctx.clone(),
            };
            thread::spawn(move || trabajador.trabajar());
        }

        PortadasRemotas {
            cola,
            resultados,
            texturas: HashMap::new(),
            fallidas: HashSet::new(),
            pedidas: Vec::new(),
            enviadas: Vec::new(),
            fotograma: 0,
        }
    }

    /// Dibuja la portada de `url` en `hueco`, centrada y con un borde fino; mientras
    /// llega (o si no hay), un hueco vacío. Si no está y se ve, la pide.
    pub fn dibujar(&mut self, ui: &egui::Ui, c: Colores, hueco: Rect, url: Option<&str>) {
        self.dibujar_con(ui, c, hueco, url, Encaje::Entera);
    }

    /// Pide la portada de `url` para un hueco de `tamano` sin dibujarla, para tenerla lista
    /// antes de enseñar lo que la lleva. Devuelve si ya está lista o si no va a llegar
    /// (no hay, o falló la descarga). Hay que llamarla en cada fotograma mientras se espera.
    pub fn preparar(&mut self, ctx: &egui::Context, tamano: egui::Vec2, url: Option<&str>) -> bool {
        let Some(url) = url.filter(|u| !self.fallidas.contains(*u)) else {
            return true;
        };
        let ppp = ctx.pixels_per_point();
        let caja = [
            (tamano.x * ppp).round() as u32,
            (tamano.y * ppp).round() as u32,
        ];
        let peticion = Peticion {
            url: url.to_string(),
            caja,
            encaje: Encaje::Entera,
        };
        if self.texturas.contains_key(&peticion.clave()) {
            return true;
        }
        self.pedidas.push(peticion);
        false
    }

    /// Como `dibujar`, pero llenando todo el hueco y recortando lo que sobra.
    pub fn dibujar_recortada(&mut self, ui: &egui::Ui, c: Colores, hueco: Rect, url: Option<&str>) {
        self.dibujar_con(ui, c, hueco, url, Encaje::Rellenar);
    }

    fn dibujar_con(
        &mut self,
        ui: &egui::Ui,
        c: Colores,
        hueco: Rect,
        url: Option<&str>,
        encaje: Encaje,
    ) {
        let pintor = ui.painter();
        let borde = Stroke::new(1.0, c.borde);
        // Las portadas, con las esquinas apenas redondeadas; las fotos, algo más.
        let radio = match encaje {
            Encaje::Entera => estilo::RADIO_PORTADA,
            Encaje::Rellenar => estilo::RADIO_CONTROL,
        };
        let Some(url) = url.filter(|u| !self.fallidas.contains(*u)) else {
            pintor.rect(hueco, radio, c.tarjeta, borde, StrokeKind::Inside);
            return;
        };

        let ppp = ui.ctx().pixels_per_point();
        let caja = [
            (hueco.width() * ppp).round() as u32,
            (hueco.height() * ppp).round() as u32,
        ];
        let clave = (url.to_string(), caja, encaje);
        let exacta = self.texturas.contains_key(&clave);
        if !exacta && ui.is_rect_visible(hueco) {
            self.pedidas.push(Peticion {
                url: url.to_string(),
                caja,
                encaje,
            });
        }

        // La del tamaño justo o, mientras llega, la de otro tamaño.
        let fotograma = self.fotograma;
        let textura = match self.texturas.get_mut(&clave) {
            Some(t) => Some(t),
            None => self
                .texturas
                .iter_mut()
                .find(|((u, _, e), _)| u == url && *e == encaje)
                .map(|(_, t)| t),
        };
        let Some(textura) = textura else {
            pintor.rect(hueco, radio, c.tarjeta, borde, StrokeKind::Inside);
            return;
        };
        textura.usada = fotograma;

        if encaje == Encaje::Rellenar {
            // Se recorta por el centro lo que no cabe con la forma del hueco.
            let [ancho, alto] = textura.textura.size();
            let forma_imagen = ancho as f32 / alto.max(1) as f32;
            let forma_hueco = hueco.width() / hueco.height().max(1.0);
            let uv = if forma_imagen > forma_hueco {
                let u = forma_hueco / forma_imagen;
                Rect::from_min_max(
                    egui::pos2((1.0 - u) / 2.0, 0.0),
                    egui::pos2((1.0 + u) / 2.0, 1.0),
                )
            } else {
                let v = forma_imagen / forma_hueco;
                Rect::from_min_max(
                    egui::pos2(0.0, (1.0 - v) / 2.0),
                    egui::pos2(1.0, (1.0 + v) / 2.0),
                )
            };
            let esquina = (hueco.min.to_vec2() * ppp).round() / ppp;
            let rect = Rect::from_min_size(esquina.to_pos2(), hueco.size());
            estilo::imagen(pintor, textura.textura.id(), rect, uv, radio);
            pintor.rect_stroke(rect, radio, borde, StrokeKind::Inside);
            return;
        }

        let tamano = if exacta {
            textura.textura.size_vec2() / ppp
        } else {
            let [ancho, alto] = textura.textura.size();
            let escala = (hueco.width() / ancho as f32).min(hueco.height() / alto as f32);
            egui::vec2(ancho as f32, alto as f32) * escala
        };
        let colocada = Rect::from_center_size(hueco.center(), tamano);
        let esquina = (colocada.min.to_vec2() * ppp).round() / ppp;
        let rect = Rect::from_min_size(esquina.to_pos2(), tamano);
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        estilo::imagen(pintor, textura.textura.id(), rect, uv, radio);
        pintor.rect_stroke(rect, radio, borde, StrokeKind::Outside);
    }

    /// Al final de cada fotograma: manda a los hilos lo que se ha pedido, recoge lo que
    /// hayan terminado y suelta las texturas que hace tiempo que no se ven.
    pub fn fin_de_fotograma(&mut self, ctx: &egui::Context) {
        self.fotograma += 1;
        let pedidas = std::mem::take(&mut self.pedidas);
        if pedidas != self.enviadas {
            let (cola, aviso) = &*self.cola;
            let mut cola = cola.lock().unwrap();
            let en_curso = &cola.en_curso;
            cola.peticiones = pedidas
                .iter()
                .filter(|p| !en_curso.contains(&p.clave()))
                .cloned()
                .collect();
            aviso.notify_all();
            self.enviadas = pedidas;
        }

        let listas: Vec<Respuesta> = self.resultados.try_iter().collect();
        for respuesta in listas {
            match respuesta {
                Respuesta::Lista { clave, imagen } => {
                    let (url, caja, encaje) = &clave;
                    let textura = ctx.load_texture(
                        format!("portada-{url}-{}x{}-{encaje:?}", caja[0], caja[1]),
                        imagen,
                        egui::TextureOptions::LINEAR,
                    );
                    self.texturas.insert(
                        clave,
                        Textura {
                            textura,
                            usada: self.fotograma,
                        },
                    );
                }
                Respuesta::Error { url } => {
                    self.fallidas.insert(url);
                }
            }
        }

        if self.texturas.len() > MAX_TEXTURAS {
            let mut usadas: Vec<u64> = self.texturas.values().map(|t| t.usada).collect();
            usadas.sort_unstable();
            let limite = usadas[usadas.len() - MAX_TEXTURAS * 2 / 3];
            self.texturas.retain(|_, t| t.usada >= limite);
        }
    }
}

impl Drop for PortadasRemotas {
    fn drop(&mut self) {
        let (cola, aviso) = &*self.cola;
        cola.lock().unwrap().cerrada = true;
        aviso.notify_all();
    }
}

struct Trabajador {
    cola: Arc<(Mutex<Cola>, Condvar)>,
    resultados: Sender<Respuesta>,
    carpeta: PathBuf,
    cliente: Cliente,
    ctx: egui::Context,
}

impl Trabajador {
    fn trabajar(self) {
        let mut encogedor = Encogedor::default();
        loop {
            let peticion = {
                let (cola, aviso) = &*self.cola;
                let mut cola = cola.lock().unwrap();
                loop {
                    if cola.cerrada {
                        return;
                    }
                    if let Some(peticion) = cola.peticiones.pop_front() {
                        cola.en_curso.insert(peticion.clave());
                        break peticion;
                    }
                    cola = aviso.wait(cola).unwrap();
                }
            };

            let respuesta = match self.preparar(&peticion, &mut encogedor) {
                Ok(imagen) => Respuesta::Lista {
                    clave: peticion.clave(),
                    imagen,
                },
                Err(e) => {
                    eprintln!("[portadas] {}: {e}", peticion.url);
                    Respuesta::Error {
                        url: peticion.url.clone(),
                    }
                }
            };
            let enviada = self.resultados.send(respuesta).is_ok();
            let (cola, _) = &*self.cola;
            cola.lock().unwrap().en_curso.remove(&peticion.clave());
            if !enviada {
                return;
            }
            self.ctx.request_repaint();
        }
    }

    fn descargar(&self, url: &str) -> Result<Imagen, String> {
        let bytes = self.cliente.bytes(url).map_err(|e| e.to_string())?;
        comic_imagen::decodificar(&bytes)
    }

    fn preparar(
        &self,
        peticion: &Peticion,
        encogedor: &mut Encogedor,
    ) -> Result<egui::ColorImage, String> {
        let archivo = self
            .carpeta
            .join(format!("{:016x}.jpg", huella_de_url(&peticion.url)));
        let guardada = fs::read(&archivo)
            .ok()
            .and_then(|bytes| comic_imagen::decodificar(&bytes).ok());
        let grande = match guardada {
            Some(imagen) => {
                // Se toca para que no caduque mientras se siga usando.
                if let Ok(abierto) = fs::File::options().append(true).open(&archivo) {
                    let _ = abierto.set_modified(SystemTime::now());
                }
                imagen
            }
            None => {
                if !peticion.url.starts_with("https://") {
                    return Err("solo se descargan portadas por HTTPS".to_string());
                }
                let original = self.descargar(&peticion.url).or_else(|error| {
                    // Hay webs que sirven la foto en un formato que no se sabe leer
                    // (AVIF) en un dominio y la original en el otro: Ramen Para Dos da
                    // AVIF en ramenparados.com y JPEG en www.ramenparados.com.
                    let otra = con_o_sin_www(&peticion.url).ok_or(error.clone())?;
                    self.descargar(&otra).map_err(|_| error)
                })?;
                let [ancho, alto] = tamano_textura([original.ancho, original.alto], CAJA_EN_DISCO);
                let imagen = encogedor.encoger(original, ancho, alto)?;
                guardar(&archivo, &imagen);
                imagen
            }
        };

        let [ancho, alto] = match peticion.encaje {
            Encaje::Entera => tamano_textura([grande.ancho, grande.alto], peticion.caja),
            Encaje::Rellenar => tamano_relleno([grande.ancho, grande.alto], peticion.caja),
        };
        let imagen = encogedor.encoger(grande, ancho, alto)?;
        Ok(egui::ColorImage::from_rgba_unmultiplied(
            [ancho as usize, alto as usize],
            &imagen.rgba,
        ))
    }
}

/// El tamaño al que hay que encoger una imagen para que llene `caja` entera (lo que
/// sobra se recorta al dibujarla). Nunca más grande que la original.
fn tamano_relleno(original: Tamano, caja: Tamano) -> Tamano {
    let [ancho, alto] = [original[0].max(1) as f64, original[1].max(1) as f64];
    let escala = (caja[0] as f64 / ancho).max(caja[1] as f64 / alto).min(1.0);
    [
        ((ancho * escala).round() as u32).max(1),
        ((alto * escala).round() as u32).max(1),
    ]
}

/// `https://www.a.com/x.jpg` da `https://a.com/x.jpg`, y al revés.
fn con_o_sin_www(url: &str) -> Option<String> {
    let resto = url.strip_prefix("https://")?;
    Some(match resto.strip_prefix("www.") {
        Some(sin) => format!("https://{sin}"),
        None => format!("https://www.{resto}"),
    })
}

/// FNV-1a de 64 bits: el nombre del archivo de cada portada. Tiene que dar lo mismo en
/// cada versión del programa, así que no vale el `Hasher` de la biblioteca estándar.
fn huella_de_url(url: &str) -> u64 {
    url.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn guardar(archivo: &Path, imagen: &Imagen) {
    let temporal = archivo.with_extension("tmp");
    let resultado = comic_imagen::codificar_jpeg(imagen, CALIDAD_JPEG)
        .and_then(|bytes| fs::write(&temporal, bytes).map_err(|e| e.to_string()))
        .and_then(|()| fs::rename(&temporal, archivo).map_err(|e| e.to_string()));
    if let Err(e) = resultado {
        eprintln!("[portadas] No se pudo guardar {}: {e}", archivo.display());
        let _ = fs::remove_file(&temporal);
    }
}

fn borrar_caducadas(carpeta: &Path) {
    let Ok(entradas) = fs::read_dir(carpeta) else {
        return;
    };
    let mut borradas = 0;
    for entrada in entradas.flatten() {
        let caducada = entrada
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|edad| edad > CADUCIDAD);
        if caducada && fs::remove_file(entrada.path()).is_ok() {
            borradas += 1;
        }
    }
    if borradas > 0 {
        eprintln!("[portadas] Borradas {borradas} portadas sin usar");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rellenar_cubre_la_caja_sin_agrandar() {
        assert_eq!(tamano_relleno([1200, 630], [200, 112]), [213, 112]);
        assert_eq!(tamano_relleno([600, 900], [200, 112]), [200, 300]);
        assert_eq!(tamano_relleno([100, 50], [200, 112]), [100, 50]);
    }

    #[test]
    fn el_otro_dominio() {
        assert_eq!(
            con_o_sin_www("https://a.com/x.jpg").as_deref(),
            Some("https://www.a.com/x.jpg")
        );
        assert_eq!(
            con_o_sin_www("https://www.a.com/x.jpg").as_deref(),
            Some("https://a.com/x.jpg")
        );
        assert_eq!(con_o_sin_www("http://a.com/x.jpg"), None);
    }

    #[test]
    fn la_huella_no_cambia() {
        // Valores de referencia de FNV-1a.
        assert_eq!(huella_de_url(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(huella_de_url("a"), 0xaf63_dc4c_8601_ec8c);
    }
}
