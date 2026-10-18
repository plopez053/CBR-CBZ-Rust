//! Miniaturas de las portadas, preparadas en hilos aparte.
//!
//! La primera vez se sacan del cómic (lento: hay que abrirlo y decodificar una página
//! entera) y se guardan en disco, en JPEG, a un tamaño de sobra para cualquier
//! pantalla. Después se leen de ahí y se encogen con Lanczos3 al tamaño exacto en
//! píxeles de la tarjeta: así se dibujan 1:1, igual de nítidas que las páginas.

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use comic_biblioteca::Portada;
use comic_core::Comic;
use comic_imagen::{Encogedor, Imagen};
use eframe::egui;

use crate::precarga::{tamano_textura, Tamano};

/// Tamaño máximo de las miniaturas guardadas en disco. Una tarjeta de 150x225 puntos
/// en una pantalla con escala 3 pide 450x675: con esto sobra.
const CAJA_EN_DISCO: Tamano = [480, 720];
/// Calidad del JPEG en disco. Por debajo de 90 se empiezan a ver artefactos en las letras.
const CALIDAD_JPEG: u8 = 92;

/// Una miniatura que se quiere ver: de qué portada y a qué tamaño, en píxeles.
#[derive(Clone)]
pub struct Peticion {
    pub portada: Portada,
    pub caja: Tamano,
}

pub enum Respuesta {
    Lista {
        huella: String,
        caja: Tamano,
        imagen: egui::ColorImage,
    },
    /// No se pudo hacer (cómic roto, página ilegible...). No se vuelve a intentar.
    Error { huella: String },
}

/// Lo que falta por hacer. La interfaz la sustituye entera en cada fotograma por lo
/// que se ve en ese momento, así lo que queda fuera de la pantalla al desplazarse
/// rápido no hace esperar a lo que sí se ve.
struct Cola {
    peticiones: VecDeque<Peticion>,
    /// Las que algún hilo está haciendo ya: no se vuelven a poner en la cola.
    en_curso: HashSet<(String, Tamano)>,
    cerrada: bool,
}

pub struct Miniaturas {
    cola: Arc<(Mutex<Cola>, Condvar)>,
    resultados: Receiver<Respuesta>,
}

impl Miniaturas {
    /// Arranca `hilos` trabajadores que guardan las miniaturas en `carpeta`.
    pub fn nuevo(carpeta: PathBuf, hilos: usize, ctx: egui::Context) -> Miniaturas {
        if let Err(e) = fs::create_dir_all(&carpeta) {
            eprintln!("[miniaturas] No se pudo crear {}: {e}", carpeta.display());
        }

        let cola = Arc::new((
            Mutex::new(Cola {
                peticiones: VecDeque::new(),
                en_curso: HashSet::new(),
                cerrada: false,
            }),
            Condvar::new(),
        ));
        let (enviar, resultados) = mpsc::channel();

        for _ in 0..hilos.max(1) {
            let trabajador = Trabajador {
                cola: Arc::clone(&cola),
                resultados: enviar.clone(),
                carpeta: carpeta.clone(),
                ctx: ctx.clone(),
            };
            thread::spawn(move || trabajador.trabajar());
        }

        Miniaturas { cola, resultados }
    }

    /// Sustituye lo que quedaba por hacer por estas peticiones, en este orden.
    pub fn pedir(&self, peticiones: Vec<Peticion>) {
        let (cola, aviso) = &*self.cola;
        let mut cola = cola.lock().unwrap();
        let en_curso = &cola.en_curso;
        cola.peticiones = peticiones
            .into_iter()
            .filter(|p| !en_curso.contains(&(p.portada.huella.clone(), p.caja)))
            .collect();
        aviso.notify_all();
    }

    /// Recoge las miniaturas que ya estén hechas, sin bloquear.
    pub fn listas(&self) -> impl Iterator<Item = Respuesta> + '_ {
        self.resultados.try_iter()
    }
}

impl Drop for Miniaturas {
    /// Despierta a los hilos para que vean que se ha cerrado y terminen.
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
                        cola.en_curso
                            .insert((peticion.portada.huella.clone(), peticion.caja));
                        break peticion;
                    }
                    cola = aviso.wait(cola).unwrap();
                }
            };

            let huella = peticion.portada.huella.clone();
            let respuesta = match self.preparar(&peticion, &mut encogedor) {
                Ok(imagen) => Respuesta::Lista {
                    huella,
                    caja: peticion.caja,
                    imagen,
                },
                Err(e) => {
                    eprintln!("[miniaturas] {}: {e}", peticion.portada.ruta.display());
                    Respuesta::Error { huella }
                }
            };

            // Primero se envía y luego se quita de `en_curso`: así la interfaz nunca la
            // ve como pendiente y sin hacer a la vez, y no la vuelve a pedir.
            let enviada = self.resultados.send(respuesta).is_ok();
            let (cola, _) = &*self.cola;
            cola.lock()
                .unwrap()
                .en_curso
                .remove(&(peticion.portada.huella, peticion.caja));
            if !enviada {
                return;
            }
            self.ctx.request_repaint();
        }
    }

    fn preparar(
        &self,
        peticion: &Peticion,
        encogedor: &mut Encogedor,
    ) -> Result<egui::ColorImage, String> {
        let archivo = self
            .carpeta
            .join(format!("{}.jpg", peticion.portada.huella));

        let guardada = fs::read(&archivo)
            .ok()
            .and_then(|bytes| comic_imagen::decodificar(&bytes).ok());
        let grande = match guardada {
            Some(imagen) => imagen,
            None => {
                let imagen = sacar_del_comic(&peticion.portada, encogedor)?;
                guardar(&archivo, &imagen);
                imagen
            }
        };

        let [ancho, alto] = tamano_textura([grande.ancho, grande.alto], peticion.caja);
        let imagen = encogedor.encoger(grande, ancho, alto)?;
        Ok(egui::ColorImage::from_rgba_unmultiplied(
            [ancho as usize, alto as usize],
            &imagen.rgba,
        ))
    }
}

/// Abre el cómic, decodifica su portada y la deja al tamaño de disco.
fn sacar_del_comic(portada: &Portada, encogedor: &mut Encogedor) -> Result<Imagen, String> {
    let mut comic = Comic::abrir(&portada.ruta)?;
    let total = comic.paginas().len();
    if total == 0 {
        return Err("No tiene páginas".to_string());
    }
    // Si el ComicInfo.xml apunta fuera del cómic, la primera.
    let pagina = if portada.pagina < total {
        portada.pagina
    } else {
        0
    };

    let original = comic_imagen::decodificar(&comic.leer(pagina)?)?;
    let [ancho, alto] = tamano_textura([original.ancho, original.alto], CAJA_EN_DISCO);
    encogedor.encoger(original, ancho, alto)
}

/// Guarda la miniatura en disco. Se escribe primero en un archivo temporal y luego se
/// renombra: si la app se cierra a medias, no queda un JPEG cortado que se tomaría por bueno.
fn guardar(archivo: &Path, imagen: &Imagen) {
    let temporal = archivo.with_extension("tmp");
    let resultado = comic_imagen::codificar_jpeg(imagen, CALIDAD_JPEG)
        .and_then(|bytes| fs::write(&temporal, bytes).map_err(|e| e.to_string()))
        .and_then(|()| fs::rename(&temporal, archivo).map_err(|e| e.to_string()));
    if let Err(e) = resultado {
        eprintln!("[miniaturas] No se pudo guardar {}: {e}", archivo.display());
        let _ = fs::remove_file(&temporal);
    }
}
