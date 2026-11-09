//! El fondo de la biblioteca: la portada del cómic que se está leyendo, en gris, algo
//! difuminada y muy tenue, detrás del contenido. Se va desvaneciendo hacia abajo hasta
//! el color de fondo, para que no estorbe a lo que hay encima.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Instant;

use comic_biblioteca::Portada;
use comic_imagen::Encogedor;
use eframe::egui::{self, Color32, Rect};

use crate::miniaturas;
use crate::precarga::{tamano_textura, Tamano};

/// Tamaño al que se reduce la portada antes de difuminarla. Al estirarla a toda la zona,
/// el filtro lineal y el difuminado la dejan suave: más grande no se notaría.
const CAJA: Tamano = [240, 360];
/// Cuánto se difumina: desviación de la campana de Gauss, en píxeles de esa imagen
/// reducida, con decimales. 0 es nítida; con 1,4 aún se reconoce el dibujo; por encima de
/// 6 queda una mancha.
const SIGMA: f32 = 1.4;
/// Cuánto se ve la portada sobre el fondo (de 0 a 1) en la parte de arriba.
const INTENSIDAD_OSCURO: f32 = 0.16;
const INTENSIDAD_CLARO: f32 = 0.12;
/// Hasta dónde se ve entera (fracción del alto); de ahí hacia abajo se desvanece.
const ENTERA_HASTA: f32 = 0.3;
/// Segundos que tarda en aparecer al cambiar de cómic.
const APARECER: f32 = 0.8;

struct Capa {
    textura: egui::TextureHandle,
    llegada: Instant,
}

pub struct Fondo {
    /// Donde están las miniaturas en disco: la portada sale de ahí, sin abrir el cómic.
    carpeta: PathBuf,
    /// La portada que se quiere de fondo, por su huella (la que hay o la que se prepara).
    huella: Option<String>,
    capa: Option<Capa>,
    /// La de antes, mientras aparece la nueva encima.
    anterior: Option<Capa>,
    preparando: Option<Receiver<Option<egui::ColorImage>>>,
}

impl Fondo {
    pub fn nuevo(carpeta: PathBuf) -> Fondo {
        Fondo {
            carpeta,
            huella: None,
            capa: None,
            anterior: None,
            preparando: None,
        }
    }

    /// Olvida la portada que tiene: la siguiente vez que se pida, se prepara otra vez.
    pub fn olvidar(&mut self) {
        self.huella = None;
        self.preparando = None;
        self.capa = None;
        self.anterior = None;
    }

    /// Pone de fondo `portada` (o ninguno). Se puede llamar en cada fotograma: solo
    /// prepara la imagen, en un hilo aparte, cuando cambia.
    pub fn pedir(&mut self, ctx: &egui::Context, portada: Option<&Portada>) {
        let huella = portada.map(|p| p.huella.clone());
        if huella != self.huella {
            self.huella = huella;
            self.preparando = None;
            match portada {
                None => {
                    self.capa = None;
                    self.anterior = None;
                }
                Some(portada) => {
                    let (enviar, recibir) = mpsc::channel();
                    let carpeta = self.carpeta.clone();
                    let portada = portada.clone();
                    let ctx = ctx.clone();
                    thread::spawn(move || {
                        let imagen = preparar(&carpeta, &portada)
                            .map_err(|e| {
                                eprintln!("[fondo] {}: {e}", portada.ruta.display());
                            })
                            .ok();
                        // Si ya se ha pedido otra, nadie lo recoge: no pasa nada.
                        let _ = enviar.send(imagen);
                        ctx.request_repaint();
                    });
                    self.preparando = Some(recibir);
                }
            }
        }

        let Some(recibir) = &self.preparando else {
            return;
        };
        match recibir.try_recv() {
            Ok(Some(imagen)) => {
                let textura = ctx.load_texture("fondo", imagen, egui::TextureOptions::LINEAR);
                self.anterior = self.capa.take();
                self.capa = Some(Capa {
                    textura,
                    llegada: Instant::now(),
                });
                self.preparando = None;
            }
            Ok(None) | Err(TryRecvError::Disconnected) => {
                self.capa = None;
                self.anterior = None;
                self.preparando = None;
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Dibuja el fondo en `zona`, encima del color de fondo y debajo de todo lo demás.
    pub fn pintar(&mut self, ui: &egui::Ui, zona: Rect) {
        let Some(capa) = &self.capa else {
            return;
        };
        let t = (capa.llegada.elapsed().as_secs_f32() / APARECER).min(1.0);
        if t < 1.0 {
            ui.ctx().request_repaint();
        } else {
            self.anterior = None;
        }
        // Que empiece y acabe despacio.
        let t = t * t * (3.0 - 2.0 * t);

        let intensidad = match ui.ctx().theme() {
            egui::Theme::Dark => INTENSIDAD_OSCURO,
            egui::Theme::Light => INTENSIDAD_CLARO,
        };
        let pintor = ui.painter_at(zona);
        if let Some(anterior) = &self.anterior {
            dibujar(&pintor, &anterior.textura, zona, intensidad * (1.0 - t));
        }
        if let Some(capa) = &self.capa {
            dibujar(&pintor, &capa.textura, zona, intensidad * t);
        }
    }
}

/// La portada cubriendo toda la zona (recortando lo que sobre, centrada en horizontal y
/// algo por encima del centro en vertical), entera arriba y desvaneciéndose hacia abajo.
fn dibujar(pintor: &egui::Painter, textura: &egui::TextureHandle, zona: Rect, intensidad: f32) {
    let alfa = (intensidad * 255.0).round() as u8;
    if alfa == 0 || zona.width() <= 0.0 || zona.height() <= 0.0 {
        return;
    }
    let [ancho, alto] = textura.size().map(|n| n as f32);
    let escala = (zona.width() / ancho).max(zona.height() / alto);
    let visible = egui::vec2(
        zona.width() / (ancho * escala),
        zona.height() / (alto * escala),
    );
    let u = (1.0 - visible.x) / 2.0;
    let v = (1.0 - visible.y) * 0.3;

    let mut malla = egui::Mesh::with_texture(textura.id());
    let filas = [
        (0.0, Color32::from_white_alpha(alfa)),
        (ENTERA_HASTA, Color32::from_white_alpha(alfa)),
        (1.0, Color32::TRANSPARENT),
    ];
    for (fraccion, color) in filas {
        let y = zona.top() + zona.height() * fraccion;
        let uv_y = v + visible.y * fraccion;
        for (x, uv_x) in [(zona.left(), u), (zona.right(), u + visible.x)] {
            malla.vertices.push(egui::epaint::Vertex {
                pos: egui::pos2(x, y),
                uv: egui::pos2(uv_x, uv_y),
                color,
            });
        }
    }
    for fila in 0..2u32 {
        let i = fila * 2;
        malla.add_triangle(i, i + 1, i + 2);
        malla.add_triangle(i + 1, i + 3, i + 2);
    }
    pintor.add(malla);
}

/// La portada reducida, en gris y difuminada.
fn preparar(carpeta: &Path, portada: &Portada) -> Result<egui::ColorImage, String> {
    let mut encogedor = Encogedor::default();
    let grande = miniaturas::portada_en_disco(carpeta, portada, &mut encogedor)?;
    let [ancho, alto] = tamano_textura([grande.ancho, grande.alto], CAJA);
    let imagen = encogedor.encoger(grande, ancho, alto)?;
    let (ancho, alto) = (ancho as usize, alto as usize);

    let mut gris: Vec<f32> = imagen
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32)
        .collect();
    difuminar(&mut gris, ancho, alto, SIGMA);
    Ok(egui::ColorImage::from_gray_iter(
        [ancho, alto],
        gris.into_iter().map(|g| g.round().clamp(0.0, 255.0) as u8),
    ))
}

/// Difumina con una campana de Gauss de desviación `sigma` (en píxeles, admite
/// decimales), en horizontal y luego en vertical. En los bordes se repite el último píxel.
fn difuminar(gris: &mut [f32], ancho: usize, alto: usize, sigma: f32) {
    if sigma <= 0.0 {
        return;
    }
    let alcance = (sigma * 3.0).ceil() as isize;
    let mut nucleo: Vec<f32> = (-alcance..=alcance)
        .map(|d| (-((d * d) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f32 = nucleo.iter().sum();
    nucleo.iter_mut().for_each(|p| *p /= total);

    let mut horizontal = vec![0.0; gris.len()];
    for y in 0..alto {
        for x in 0..ancho {
            horizontal[y * ancho + x] = nucleo
                .iter()
                .zip(-alcance..=alcance)
                .map(|(peso, d)| {
                    let xx = (x as isize + d).clamp(0, ancho as isize - 1) as usize;
                    peso * gris[y * ancho + xx]
                })
                .sum();
        }
    }
    for y in 0..alto {
        for x in 0..ancho {
            gris[y * ancho + x] = nucleo
                .iter()
                .zip(-alcance..=alcance)
                .map(|(peso, d)| {
                    let yy = (y as isize + d).clamp(0, alto as isize - 1) as usize;
                    peso * horizontal[yy * ancho + x]
                })
                .sum();
        }
    }
}
