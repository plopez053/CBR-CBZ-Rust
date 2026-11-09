//! Elegir portadas desde la biblioteca, en una ventana con miniaturas de las que se pulsa
//! la que se quiere:
//! - la de un cómic: todas las páginas del cómic ([`ElegirPortada`]);
//! - la de una serie: las portadas de todos sus cómics ([`ElegirPortadaDeSerie`]).
//!
//! Las miniaturas se preparan en hilos aparte solo para lo que se ve, y se quedan en
//! memoria hasta que se cierra la ventana.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use comic_biblioteca::{Ficha, Portada};
use eframe::egui::{self, Color32, FontId, Rect, Stroke, StrokeKind};

use crate::estilo::{self, Colores};
use crate::miniaturas::{self, Miniaturas};
use crate::precarga::Tamano;
use crate::selector::Cargador;
use crate::vista_biblioteca::{boton_texto, texto_ajustado};

const COLUMNAS: usize = 5;
/// Lo que ocupa cada miniatura en la ventana, en puntos. Se preparan para este tamaño y se
/// dibujan 1:1.
const ANCHO_MINI: f32 = 124.0;
const ALTO_MINI: f32 = 186.0;
/// Sitio debajo de cada miniatura para su número o su nombre.
const ALTO_ETIQUETA: f32 = 20.0;
const SEPARACION: f32 = 12.0;
/// Alto máximo de la lista; si hay más, se desplaza.
const ALTO_LISTA: f32 = 470.0;
/// Filas por encima y por debajo de las que se ven cuyas miniaturas también se preparan.
const FILAS_DE_MARGEN: usize = 2;

pub enum Resultado {
    /// Sigue abierta.
    Abierta,
    /// Se ha pulsado una miniatura (empezando en 0).
    Elegida(usize),
    /// Se ha cerrado sin elegir.
    Cerrada,
}

/// La ventana, con el marco de los paneles. Se cierra con Esc o pulsando fuera.
fn ventana(id: &'static str, c: Colores) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id)).frame(
        egui::Frame::new()
            .fill(c.panel)
            .stroke(Stroke::new(1.0, c.borde))
            .corner_radius(estilo::RADIO_PANEL)
            .inner_margin(egui::Margin::same(24)),
    )
}

/// El contenido de la ventana: un título, la rejilla y "Cancelar". Devuelve la miniatura
/// pulsada, cuáles se ven (y las de alrededor, para prepararlas) y si se ha pulsado
/// "Cancelar".
fn contenido(
    ui: &mut egui::Ui,
    c: Colores,
    titulo: &str,
    total: usize,
    mut celda: impl FnMut(&mut egui::Ui, usize) -> bool,
) -> (Option<usize>, Vec<usize>, bool) {
    // Un poco más ancha que las columnas: cabe la barra de desplazamiento.
    ui.set_width(COLUMNAS as f32 * (ANCHO_MINI + SEPARACION) + 8.0);
    ui.add(egui::Label::new(estilo::titulo(titulo, 18.0, c.texto)).truncate());
    ui.add_space(12.0);

    let mut elegida = None;
    let mut visibles = Vec::new();
    egui::ScrollArea::vertical()
        .max_height(ALTO_LISTA)
        .auto_shrink([false, true])
        .show_rows(
            ui,
            ALTO_MINI + ALTO_ETIQUETA,
            total.div_ceil(COLUMNAS),
            |ui, filas| {
                ui.spacing_mut().item_spacing = egui::vec2(SEPARACION, SEPARACION);
                for fila in filas.clone() {
                    ui.horizontal(|ui| {
                        for i in fila * COLUMNAS..((fila + 1) * COLUMNAS).min(total) {
                            if celda(ui, i) {
                                elegida = Some(i);
                            }
                        }
                    });
                }
                let desde = filas.start.saturating_sub(FILAS_DE_MARGEN) * COLUMNAS;
                let hasta = ((filas.end + FILAS_DE_MARGEN) * COLUMNAS).min(total);
                visibles = (desde..hasta).collect();
            },
        );

    ui.add_space(12.0);
    let cancelar = ui.add(boton_texto(c, "Cancelar")).clicked();
    (elegida, visibles, cancelar)
}

/// Una miniatura con su etiqueta debajo. `marcada` es la que ya es la portada. Devuelve si
/// se ha pulsado.
fn celda(
    ui: &mut egui::Ui,
    c: Colores,
    ppp: f32,
    textura: Option<&egui::TextureHandle>,
    etiqueta: &str,
    fuente: FontId,
    marcada: bool,
) -> bool {
    let (rect, respuesta) = ui.allocate_exact_size(
        egui::vec2(ANCHO_MINI, ALTO_MINI + ALTO_ETIQUETA),
        egui::Sense::click(),
    );
    let hueco = Rect::from_min_size(rect.min, egui::vec2(ANCHO_MINI, ALTO_MINI));
    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

    let imagen = match textura {
        Some(textura) => {
            // Centrada en su hueco y alineada con los píxeles de la pantalla.
            let tamano = textura.size_vec2() / ppp;
            let colocada = Rect::from_center_size(hueco.center(), tamano);
            let esquina = (colocada.min.to_vec2() * ppp).round() / ppp;
            let rect = Rect::from_min_size(esquina.to_pos2(), tamano);
            estilo::imagen(ui.painter(), textura.id(), rect, uv, estilo::RADIO_PORTADA);
            rect
        }
        None => {
            ui.painter()
                .rect_filled(hueco, estilo::RADIO_PORTADA, c.tarjeta);
            hueco
        }
    };

    let pintor = ui.painter();
    let borde = if marcada {
        Stroke::new(2.0, c.acento)
    } else {
        Stroke::new(1.0, c.borde)
    };
    pintor.rect_stroke(imagen, estilo::RADIO_PORTADA, borde, StrokeKind::Outside);
    if respuesta.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        pintor.rect_filled(imagen, estilo::RADIO_PORTADA, Color32::from_white_alpha(28));
    }
    let color = if marcada {
        c.acento
    } else {
        c.texto_secundario
    };
    let texto = texto_ajustado(ui, etiqueta, fuente, color, ANCHO_MINI, 1);
    let posicion = egui::pos2(hueco.center().x - texto.size().x / 2.0, hueco.max.y + 5.0);
    ui.painter().galley(posicion, texto, color);
    respuesta.clicked()
}

/// La portada de un cómic: se elige una de sus páginas.
pub struct ElegirPortada {
    /// El cómic al que se le elige la portada.
    pub huella: String,
    titulo: String,
    ruta: PathBuf,
    total: usize,
    /// La página que es ahora su portada, para marcarla.
    actual: usize,
    /// Se arranca al dibujar la primera vez, cuando se sabe la escala de la pantalla.
    cargador: Option<Cargador>,
    hojas: HashMap<usize, egui::TextureHandle>,
    fallidas: HashSet<usize>,
    /// Lo último que se pidió a los hilos, para no repetirlo en cada fotograma.
    pedidas: Vec<usize>,
}

impl ElegirPortada {
    pub fn nuevo(ficha: &Ficha, titulo: String) -> ElegirPortada {
        let total = ficha.paginas;
        ElegirPortada {
            huella: ficha.huella.clone(),
            titulo,
            ruta: ficha.ruta.clone(),
            total,
            actual: ficha.portada().pagina.min(total.saturating_sub(1)),
            cargador: None,
            hojas: HashMap::new(),
            fallidas: HashSet::new(),
            pedidas: Vec::new(),
        }
    }

    /// Dibuja la ventana y dice qué ha pasado en ella. `Elegida` trae la página (desde 0).
    pub fn mostrar(&mut self, ctx: &egui::Context) -> Resultado {
        let c = estilo::colores(ctx);
        let ppp = ctx.pixels_per_point();
        self.recoger(ctx);

        let titulo = format!("Portada de {}", self.titulo);
        let mut resultado = (None, Vec::new(), false);
        let modal = ventana("elegir_portada", c).show(ctx, |ui| {
            resultado = contenido(ui, c, &titulo, self.total, |ui, pagina| {
                let textura = self.hojas.get(&pagina);
                let etiqueta = (pagina + 1).to_string();
                let fuente = estilo::cifras(11.0);
                celda(
                    ui,
                    c,
                    ppp,
                    textura,
                    &etiqueta,
                    fuente,
                    pagina == self.actual,
                )
            });
        });
        let (elegida, visibles, cancelar) = resultado;

        self.pedir(ctx, ppp, visibles);

        if let Some(pagina) = elegida {
            eprintln!("[ui] Portada elegida: página {}", pagina + 1);
            Resultado::Elegida(pagina)
        } else if cancelar || modal.should_close() {
            Resultado::Cerrada
        } else {
            Resultado::Abierta
        }
    }

    /// Sube a la GPU las miniaturas que hayan terminado los hilos.
    fn recoger(&mut self, ctx: &egui::Context) {
        let Some(cargador) = &self.cargador else {
            return;
        };
        for (pagina, resultado) in cargador.listas() {
            match resultado {
                Ok(imagen) => {
                    let textura = ctx.load_texture(
                        format!("portada-{pagina}"),
                        imagen,
                        egui::TextureOptions::LINEAR,
                    );
                    self.hojas.insert(pagina, textura);
                }
                Err(e) => {
                    eprintln!("[ui] Portada, página {}: {e}", pagina + 1);
                    self.fallidas.insert(pagina);
                }
            }
        }
    }

    /// Pide a los hilos las miniaturas que faltan de las páginas que se ven.
    fn pedir(&mut self, ctx: &egui::Context, ppp: f32, visibles: Vec<usize>) {
        let faltan: Vec<usize> = visibles
            .into_iter()
            .filter(|p| !self.hojas.contains_key(p) && !self.fallidas.contains(p))
            .collect();
        if faltan == self.pedidas {
            return;
        }
        let cargador = self
            .cargador
            .get_or_insert_with(|| Cargador::nuevo(&self.ruta, caja(ppp), ctx.clone()));
        cargador.pedir(faltan.clone());
        self.pedidas = faltan;
    }
}

/// La portada de una serie: se elige la de uno de sus cómics.
pub struct ElegirPortadaDeSerie {
    /// La serie a la que se le elige la portada.
    pub clave: String,
    nombre: String,
    /// Cada cómic de la serie: su portada y cómo se llama.
    comics: Vec<(Portada, String)>,
    /// El cómic cuya portada es ahora la de la serie, para marcarla.
    actual: String,
    carpeta: PathBuf,
    /// Se arranca al dibujar la primera vez, cuando se sabe la escala de la pantalla.
    miniaturas: Option<Miniaturas>,
    texturas: HashMap<String, egui::TextureHandle>,
}

impl ElegirPortadaDeSerie {
    /// `comics` son los que se pueden abrir, con su nombre; `actual` es la huella del cómic
    /// cuya portada es ahora la de la serie y `carpeta`, donde están las miniaturas.
    pub fn nuevo(
        clave: String,
        nombre: String,
        comics: Vec<(Portada, String)>,
        actual: String,
        carpeta: PathBuf,
    ) -> ElegirPortadaDeSerie {
        ElegirPortadaDeSerie {
            clave,
            nombre,
            comics,
            actual,
            carpeta,
            miniaturas: None,
            texturas: HashMap::new(),
        }
    }

    /// La huella del cómic de la posición `i` de lo elegido.
    pub fn huella_de(&self, i: usize) -> &str {
        &self.comics[i].0.huella
    }

    /// Dibuja la ventana y dice qué ha pasado en ella. `Elegida` trae la posición del cómic.
    pub fn mostrar(&mut self, ctx: &egui::Context) -> Resultado {
        let c = estilo::colores(ctx);
        let ppp = ctx.pixels_per_point();
        self.preparar(ctx, ppp);

        let titulo = format!("Portada de {}", self.nombre);
        let mut resultado = (None, Vec::new(), false);
        let modal = ventana("elegir_portada_de_serie", c).show(ctx, |ui| {
            resultado = contenido(ui, c, &titulo, self.comics.len(), |ui, i| {
                let (portada, nombre) = &self.comics[i];
                let textura = self.texturas.get(&portada.huella);
                let fuente = FontId::proportional(12.0);
                let marcada = portada.huella == self.actual;
                celda(ui, c, ppp, textura, nombre, fuente, marcada)
            });
        });
        let (elegida, _, cancelar) = resultado;

        if let Some(i) = elegida {
            eprintln!("[ui] Portada de la serie: {}", self.comics[i].1);
            Resultado::Elegida(i)
        } else if cancelar || modal.should_close() {
            Resultado::Cerrada
        } else {
            Resultado::Abierta
        }
    }

    /// Pide todas las miniaturas la primera vez (suelen estar ya en disco, de la
    /// biblioteca) y sube a la GPU las que vayan llegando.
    fn preparar(&mut self, ctx: &egui::Context, ppp: f32) {
        let miniaturas = self.miniaturas.get_or_insert_with(|| {
            let miniaturas = Miniaturas::nuevo(self.carpeta.clone(), 2, ctx.clone());
            miniaturas.pedir(
                self.comics
                    .iter()
                    .map(|(portada, _)| miniaturas::Peticion {
                        portada: portada.clone(),
                        caja: caja(ppp),
                    })
                    .collect(),
            );
            miniaturas
        });
        for respuesta in miniaturas.listas() {
            match respuesta {
                miniaturas::Respuesta::Lista { huella, imagen, .. } => {
                    let textura = ctx.load_texture(
                        format!("portada-serie-{huella}"),
                        imagen,
                        egui::TextureOptions::LINEAR,
                    );
                    self.texturas.insert(huella, textura);
                }
                // Sin miniatura se queda el hueco vacío.
                miniaturas::Respuesta::Error { .. } => {}
            }
        }
    }
}

/// La caja de una miniatura en píxeles de la pantalla.
fn caja(ppp: f32) -> Tamano {
    [
        (ANCHO_MINI * ppp).ceil() as u32,
        (ALTO_MINI * ppp).ceil() as u32,
    ]
}
