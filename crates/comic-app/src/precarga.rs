//! Hilo en segundo plano que lee y decodifica páginas sin congelar la ventana.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use comic_core::Cbz;
use eframe::egui;

/// Lo que devuelve el hilo: la posición de la página y su imagen ya decodificada (o el error).
pub type Resultado = (usize, Result<egui::ColorImage, String>);

/// Comunicación con el hilo trabajador: le pedimos páginas por un canal y
/// nos devuelve las imágenes por otro.
pub struct Precargador {
    peticiones: Sender<usize>,
    resultados: Receiver<Resultado>,
}

impl Precargador {
    /// Arranca el hilo. El `Cbz` se mueve dentro del hilo: a partir de aquí solo él lo usa.
    pub fn nuevo(mut cbz: Cbz, ctx: egui::Context) -> Precargador {
        let (enviar_peticion, recibir_peticion) = mpsc::channel::<usize>();
        let (enviar_resultado, recibir_resultado) = mpsc::channel::<Resultado>();

        thread::spawn(move || {
            // El bucle termina solo cuando la ventana se cierra y se suelta el `Sender`.
            for indice in recibir_peticion {
                let imagen = cbz.leer(indice).and_then(|bytes| decodificar(&bytes));

                if enviar_resultado.send((indice, imagen)).is_err() {
                    break;
                }
                // Despierta a la ventana para que dibuje la página nueva.
                ctx.request_repaint();
            }
        });

        Precargador {
            peticiones: enviar_peticion,
            resultados: recibir_resultado,
        }
    }

    /// Pide una página al hilo. No espera: la respuesta llega más tarde.
    pub fn pedir(&self, indice: usize) {
        let _ = self.peticiones.send(indice);
    }

    /// Recoge las páginas que ya estén listas, sin bloquear.
    pub fn listas(&self) -> impl Iterator<Item = Resultado> + '_ {
        self.resultados.try_iter()
    }
}

/// Convierte los bytes de un JPG/PNG/WebP en píxeles que egui puede subir a la GPU.
fn decodificar(bytes: &[u8]) -> Result<egui::ColorImage, String> {
    let imagen = image::load_from_memory(bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();

    let tamano = [imagen.width() as usize, imagen.height() as usize];

    Ok(egui::ColorImage::from_rgba_unmultiplied(tamano, imagen.as_raw()))
}
