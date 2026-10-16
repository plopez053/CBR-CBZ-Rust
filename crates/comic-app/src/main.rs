mod precarga;

use std::collections::{HashMap, HashSet};

use comic_core::Cbz;
use eframe::egui;
use precarga::Precargador;

/// Cuántas páginas preparamos por detrás y por delante de la actual.
const ATRAS: usize = 1;
const DELANTE: usize = 2;

/// Estado de la aplicación: qué cómic está abierto y en qué página estamos.
struct Lector {
    total: usize,
    actual: usize,
    precargador: Precargador,
    /// Páginas ya subidas a la GPU, listas para dibujar.
    texturas: HashMap<usize, egui::TextureHandle>,
    /// Páginas pedidas al hilo cuya respuesta aún no ha llegado.
    pedidas: HashSet<usize>,
    errores: HashMap<usize, String>,
    pantalla_completa: bool,
}

impl Lector {
    fn siguiente(&mut self) {
        if self.actual + 1 < self.total {
            self.actual += 1;
        }
    }

    fn anterior(&mut self) {
        self.actual = self.actual.saturating_sub(1);
    }

    /// Rango de páginas que queremos tener preparadas alrededor de la actual.
    fn ventana(&self) -> std::ops::RangeInclusive<usize> {
        let desde = self.actual.saturating_sub(ATRAS);
        let hasta = (self.actual + DELANTE).min(self.total.saturating_sub(1));
        desde..=hasta
    }

    /// Recoge lo que haya terminado el hilo, pide lo que falta y libera lo que queda lejos.
    fn gestionar_precarga(&mut self, ctx: &egui::Context) {
        let listas: Vec<_> = self.precargador.listas().collect();
        for (indice, resultado) in listas {
            self.pedidas.remove(&indice);
            match resultado {
                Ok(imagen) => {
                    // Mipmaps: versiones reducidas de la imagen para que se vea nítida al encogerla.
                    let opciones = egui::TextureOptions::LINEAR
                        .with_mipmap_mode(Some(egui::TextureFilter::Linear));
                    let textura = ctx.load_texture(format!("pagina-{indice}"), imagen, opciones);
                    self.texturas.insert(indice, textura);
                }
                Err(e) => {
                    self.errores.insert(indice, e);
                }
            }
        }

        let ventana = self.ventana();

        // La actual primero, para que sea la que antes llega.
        let orden = std::iter::once(self.actual).chain(ventana.clone());
        for indice in orden {
            let falta = !self.texturas.contains_key(&indice)
                && !self.pedidas.contains(&indice)
                && !self.errores.contains_key(&indice);
            if falta {
                self.pedidas.insert(indice);
                self.precargador.pedir(indice);
            }
        }

        // Cada página decodificada ocupa decenas de MB: soltamos las que quedan lejos.
        self.texturas.retain(|indice, _| ventana.contains(indice));
    }
}

impl eframe::App for Lector {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if ui.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            self.siguiente();
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            self.anterior();
        }
        if ui.input(|i| i.key_pressed(egui::Key::F11)) {
            self.pantalla_completa = !self.pantalla_completa;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.pantalla_completa));
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && self.pantalla_completa {
            self.pantalla_completa = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }

        self.gestionar_precarga(&ctx);

        egui::Panel::bottom("barra").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("⬅").clicked() {
                    self.anterior();
                }
                ui.label(format!("{} / {}", self.actual + 1, self.total));
                if ui.button("➡").clicked() {
                    self.siguiente();
                }
                ui.separator();
                ui.label("F11: pantalla completa");
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if self.total == 0 {
                ui.label("Este cómic no tiene páginas.");
            } else if let Some(textura) = self.texturas.get(&self.actual) {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new(textura).shrink_to_fit());
                });
            } else if let Some(error) = self.errores.get(&self.actual) {
                ui.label(format!("No se pudo leer la página: {error}"));
            } else {
                ui.centered_and_justified(|ui| {
                    ui.spinner();
                });
            }
        });
    }
}

fn main() -> eframe::Result {
    let Some(ruta) = std::env::args().nth(1) else {
        eprintln!("Uso: comic-app <archivo.cbz>");
        return Ok(());
    };

    let cbz = match Cbz::abrir(&ruta) {
        Ok(cbz) => cbz,
        Err(e) => {
            eprintln!("No se pudo abrir {ruta}: {e}");
            return Ok(());
        }
    };

    let opciones = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lector CBZ")
            .with_maximized(true),
        ..Default::default()
    };

    eframe::run_native(
        "Lector CBZ",
        opciones,
        Box::new(|cc| {
            let total = cbz.paginas().len();
            let precargador = Precargador::nuevo(cbz, cc.egui_ctx.clone());

            Ok(Box::new(Lector {
                total,
                actual: 0,
                precargador,
                texturas: HashMap::new(),
                pedidas: HashSet::new(),
                errores: HashMap::new(),
                pantalla_completa: false,
            }))
        }),
    )
}
