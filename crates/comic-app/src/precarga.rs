//! Hilos en segundo plano que leen, decodifican y encogen páginas sin congelar la ventana.

use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use comic_core::Cbz;
use eframe::egui;
use fast_image_resize as fr;

/// Cuántas páginas preparamos por detrás y por delante de la actual.
pub const ATRAS: usize = 1;
pub const DELANTE: usize = 2;

/// Ancho y alto en píxeles.
pub type Tamano = [u32; 2];

/// Lado máximo de una textura que aceptan casi todas las GPU (límite por defecto de wgpu).
const MAX_LADO_TEXTURA: u32 = 8192;

/// Tamaño más grande con el que la página cabe en `caja` sin deformarse.
/// Puede salir mayor que el original: es el tamaño con el que se verá en pantalla.
pub fn encajar([ancho, alto]: Tamano, [caja_ancho, caja_alto]: Tamano) -> Tamano {
    let escala = (caja_ancho as f64 / ancho as f64).min(caja_alto as f64 / alto as f64);
    let ajustar = |lado: u32| ((lado as f64 * escala).round() as u32).max(1);
    [ajustar(ancho), ajustar(alto)]
}

/// Tamaño de la textura que preparan los hilos para esa caja. Igual que [`encajar`],
/// pero nunca mayor que el original (agrandar no añade detalle; eso lo hace la GPU al
/// dibujar) ni que lo que admite la GPU.
pub fn tamano_textura(original: Tamano, caja: Tamano) -> Tamano {
    let [ancho, alto] = encajar(original, caja);
    let tamano = [ancho.min(original[0]), alto.min(original[1])];
    if tamano[0] > MAX_LADO_TEXTURA || tamano[1] > MAX_LADO_TEXTURA {
        encajar(tamano, [MAX_LADO_TEXTURA, MAX_LADO_TEXTURA])
    } else {
        tamano
    }
}

/// Rango de páginas que queremos tener preparadas alrededor de `actual`.
/// La usan tanto la ventana como los hilos, para que ambos estén de acuerdo.
pub fn ventana(actual: usize, total: usize) -> RangeInclusive<usize> {
    let desde = actual.saturating_sub(ATRAS);
    let hasta = (actual + DELANTE).min(total.saturating_sub(1));
    desde..=hasta
}

/// Una página que pedimos y el hueco en el que tiene que caber.
#[derive(Clone, Copy)]
struct Peticion {
    indice: usize,
    caja: Tamano,
}

/// Una página ya preparada para subir a la GPU.
pub struct Pagina {
    pub indice: usize,
    /// Caja con la que se pidió.
    pub caja: Tamano,
    /// Tamaño de la imagen tal como viene en el archivo.
    pub original: Tamano,
    pub imagen: egui::ColorImage,
}

/// Lo que contesta un hilo por cada página que se le pidió.
/// Llevan la caja con la que se pidió, para saber si sigue valiendo.
pub enum Respuesta {
    Lista(Pagina),
    Error(usize, String),
    /// Cuando le tocó, ya no hacía falta (página lejos o ventana redimensionada).
    Descartada(usize, Tamano),
}

/// Lo que se está viendo ahora mismo. Los hilos lo consultan para saltarse
/// las peticiones que se han quedado viejas mientras esperaban en la cola.
#[derive(Default)]
struct Estado {
    actual: AtomicUsize,
    /// La caja empaquetada en un solo número (ancho arriba, alto abajo) para poder
    /// leerla y escribirla de golpe sin `Mutex`.
    caja: AtomicU64,
}

fn empaquetar([ancho, alto]: Tamano) -> u64 {
    (u64::from(ancho) << 32) | u64::from(alto)
}

/// Comunicación con los hilos trabajadores: les pedimos páginas por un canal y
/// nos devuelven las imágenes por otro.
pub struct Precargador {
    peticiones: Sender<Peticion>,
    resultados: Receiver<Respuesta>,
    estado: Arc<Estado>,
}

impl Precargador {
    /// Arranca `hilos` trabajadores. El primero se queda con el `Cbz` ya abierto;
    /// el resto abre el suyo desde `ruta`, porque leer del zip necesita `&mut` y
    /// así cada hilo lee a su ritmo sin esperar a los demás.
    pub fn nuevo(cbz: Cbz, ruta: &str, hilos: usize, ctx: egui::Context) -> Precargador {
        let (enviar_peticion, recibir_peticion) = mpsc::channel::<Peticion>();
        let (enviar_resultado, recibir_resultado) = mpsc::channel::<Respuesta>();
        let estado = Arc::new(Estado::default());

        // Un `Receiver` no se puede clonar: los hilos lo comparten tras un `Mutex`
        // y cada petición la coge el primero que esté libre.
        let recibir_peticion = Arc::new(Mutex::new(recibir_peticion));

        let mut primero = Some(cbz);
        for _ in 0..hilos.max(1) {
            let cbz = match primero.take() {
                Some(cbz) => cbz,
                None => match Cbz::abrir(ruta) {
                    Ok(cbz) => cbz,
                    // Si no se puede reabrir, nos apañamos con los hilos que ya hay.
                    Err(_) => break,
                },
            };

            let trabajador = Trabajador {
                cbz,
                peticiones: Arc::clone(&recibir_peticion),
                resultados: enviar_resultado.clone(),
                estado: Arc::clone(&estado),
                ctx: ctx.clone(),
            };
            thread::spawn(move || trabajador.trabajar());
        }

        Precargador {
            peticiones: enviar_peticion,
            resultados: recibir_resultado,
            estado,
        }
    }

    /// Avisa a los hilos de qué página se está viendo y de qué tamaño es el hueco.
    pub fn mover_a(&self, actual: usize, caja: Tamano) {
        self.estado.actual.store(actual, Ordering::Relaxed);
        self.estado.caja.store(empaquetar(caja), Ordering::Relaxed);
    }

    /// Pide una página encogida para caber en `caja`. No espera: la respuesta llega más tarde.
    pub fn pedir(&self, indice: usize, caja: Tamano) {
        let _ = self.peticiones.send(Peticion { indice, caja });
    }

    /// Recoge las respuestas que ya hayan llegado, sin bloquear.
    pub fn listas(&self) -> impl Iterator<Item = Respuesta> + '_ {
        self.resultados.try_iter()
    }
}

/// Lo que necesita cada hilo para trabajar por su cuenta.
struct Trabajador {
    cbz: Cbz,
    peticiones: Arc<Mutex<Receiver<Peticion>>>,
    resultados: Sender<Respuesta>,
    estado: Arc<Estado>,
    ctx: egui::Context,
}

impl Trabajador {
    /// Bucle del hilo. Termina cuando la ventana se cierra y se suelta el `Sender`.
    fn trabajar(mut self) {
        let total = self.cbz.paginas().len();
        let id_hilo = thread::current().id();
        // Guarda búferes internos entre páginas: reutilizarlo evita reservar memoria cada vez.
        let mut redimensionador = fr::Resizer::new();

        loop {
            // El candado se suelta al acabar esta línea: mientras decodificamos,
            // los demás hilos pueden ir cogiendo peticiones.
            let peticion = self.peticiones.lock().unwrap().recv();
            let Ok(Peticion { indice, caja }) = peticion else {
                break;
            };

            // Si el usuario ha saltado lejos o ha cambiado el tamaño de la ventana mientras
            // la petición esperaba en la cola, no gastamos tiempo en algo que se tiraría.
            let actual = self.estado.actual.load(Ordering::Relaxed);
            let caja_actual = self.estado.caja.load(Ordering::Relaxed);
            let lejos = !ventana(actual, total).contains(&indice);
            let redimensionada = empaquetar(caja) != caja_actual;
            let respuesta = if lejos || redimensionada {
                let motivo = if lejos {
                    "queda lejos"
                } else {
                    "cambió el tamaño de la vista"
                };
                eprintln!("[{id_hilo:?}] Página {indice} descartada ({motivo})");
                Respuesta::Descartada(indice, caja)
            } else {
                match self.preparar(indice, caja, &mut redimensionador) {
                    Ok((original, imagen)) => Respuesta::Lista(Pagina {
                        indice,
                        caja,
                        original,
                        imagen,
                    }),
                    Err(e) => {
                        eprintln!("[{id_hilo:?}] Página {indice} error: {e}");
                        Respuesta::Error(indice, e)
                    }
                }
            };

            if self.resultados.send(respuesta).is_err() {
                break;
            }
            // Despierta a la ventana: para dibujar la página nueva o, si se descartó,
            // para que vuelva a pedirla en caso de que haga falta otra vez.
            self.ctx.request_repaint();
        }
    }

    /// Lee, decodifica y encoge una página para que quepa en `caja`.
    /// Devuelve también el tamaño original, que la ventana necesita para el zoom.
    fn preparar(
        &mut self,
        indice: usize,
        caja: Tamano,
        redimensionador: &mut fr::Resizer,
    ) -> Result<(Tamano, egui::ColorImage), String> {
        let inicio = Instant::now();
        let bytes = self.cbz.leer(indice)?;
        let original = image::load_from_memory(&bytes)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        let decodificada = inicio.elapsed();

        let (ancho, alto) = original.dimensions();
        let [destino_ancho, destino_alto] = tamano_textura([ancho, alto], caja);

        let pixeles = if [destino_ancho, destino_alto] == [ancho, alto] {
            // Se va a ver a su tamaño o más grande: la subimos tal cual.
            original.into_raw()
        } else {
            let origen = fr::images::Image::from_vec_u8(
                ancho,
                alto,
                original.into_raw(),
                fr::PixelType::U8x4,
            )
            .map_err(|e| e.to_string())?;
            let mut destino =
                fr::images::Image::new(destino_ancho, destino_alto, fr::PixelType::U8x4);
            // Lanczos3 directamente sobre sRGB. Comparado con Mitchell, Magic Kernel Sharp y
            // con encoger en luz lineal, es lo que mejor conserva el trazo de la tinta y las
            // letras: en luz lineal las líneas finas salen más grises y delgadas.
            let opciones = fr::ResizeOptions::new()
                .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3));
            redimensionador
                .resize(&origen, &mut destino, &opciones)
                .map_err(|e| e.to_string())?;
            destino.into_vec()
        };

        eprintln!(
            "[{:?}] Página {indice} lista: {ancho}x{alto} -> {destino_ancho}x{destino_alto} \
             (decodificar {:.1}ms, encoger {:.1}ms)",
            thread::current().id(),
            decodificada.as_secs_f64() * 1000.0,
            (inicio.elapsed() - decodificada).as_secs_f64() * 1000.0,
        );

        let tamano = [destino_ancho as usize, destino_alto as usize];
        let imagen = egui::ColorImage::from_rgba_unmultiplied(tamano, &pixeles);
        Ok(([ancho, alto], imagen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoge_manteniendo_la_proporcion() {
        assert_eq!(encajar([2000, 3000], [1920, 1000]), [667, 1000]);
    }

    #[test]
    fn ajustar_al_ancho_es_una_caja_sin_limite_de_alto() {
        assert_eq!(encajar([2000, 3000], [1000, u32::MAX]), [1000, 1500]);
    }

    #[test]
    fn en_pantalla_puede_agrandar_pero_la_textura_no() {
        assert_eq!(encajar([800, 1200], [1920, 1500]), [1000, 1500]);
        assert_eq!(tamano_textura([800, 1200], [1920, 1500]), [800, 1200]);
    }

    #[test]
    fn la_textura_no_pasa_del_limite_de_la_gpu() {
        assert_eq!(tamano_textura([10000, 5000], [20000, 20000]), [8192, 4096]);
    }
}
