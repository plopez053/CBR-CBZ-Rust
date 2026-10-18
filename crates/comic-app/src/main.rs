mod dialogos;
mod lector;
mod miniaturas;
mod precarga;
mod vista_biblioteca;

use std::path::{Path, PathBuf};

use comic_core::Comic;
use eframe::egui;
use lector::Lector;
use vista_biblioteca::VistaBiblioteca;

const TITULO: &str = "Lector de cómics";

/// Un cómic abierto en el lector y lo que hace falta para apuntar por dónde va.
struct Abierto {
    lector: Lector,
    huella: String,
    /// Última página apuntada en la biblioteca, para no guardar en cada fotograma.
    guardada: Option<usize>,
}

/// La aplicación: la biblioteca y, encima, el cómic que se esté leyendo.
struct Aplicacion {
    /// `Err` si no se pudo abrir la base de datos: aun así se puede leer un cómic suelto.
    biblioteca: Result<VistaBiblioteca, String>,
    abierto: Option<Abierto>,
}

impl Aplicacion {
    fn abrir(&mut self, ctx: &egui::Context, ruta: &Path) {
        let comic = match Comic::abrir(ruta) {
            Ok(comic) => comic,
            Err(e) => {
                let error = format!("No se pudo abrir {}: {e}", ruta.display());
                match &mut self.biblioteca {
                    Ok(vista) => vista.mostrar_error(error),
                    Err(_) => eprintln!("[ui] {error}"),
                }
                return;
            }
        };
        eprintln!(
            "[ui] Abierto como {}: {} páginas",
            comic.formato().nombre(),
            comic.paginas().len()
        );

        // La misma huella que usa la biblioteca: un cómic suelto que también esté en
        // ella comparte el progreso, y uno que no esté lo recuerda igualmente.
        let huella = comic_biblioteca::huella(ruta, comic.paginas());
        // Si se terminó, se vuelve a empezar desde el principio.
        let pagina = match &self.biblioteca {
            Ok(vista) => vista
                .progreso(&huella)
                .filter(|p| !p.terminado)
                .map_or(0, |p| p.pagina),
            Err(_) => 0,
        };

        let titulo = ruta
            .file_stem()
            .map_or(TITULO.into(), |n| n.to_string_lossy());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(titulo.into_owned()));

        self.abierto = Some(Abierto {
            lector: Lector::nuevo(ctx, comic, ruta, pagina),
            huella,
            guardada: None,
        });
    }

    /// Apunta en la biblioteca la página por la que va el lector, si ha cambiado.
    fn guardar_progreso(&mut self) {
        let (Some(abierto), Ok(vista)) = (&mut self.abierto, &self.biblioteca) else {
            return;
        };
        let actual = abierto.lector.actual();
        if abierto.guardada != Some(actual) && abierto.lector.total() > 0 {
            let terminado = actual + 1 >= abierto.lector.total();
            vista.guardar_progreso(&abierto.huella, actual, terminado);
            abierto.guardada = Some(actual);
        }
    }
}

impl eframe::App for Aplicacion {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if let Some(abierto) = &mut self.abierto {
            let volver = abierto.lector.mostrar(ui);
            self.guardar_progreso();
            if volver {
                self.abierto = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(TITULO.to_string()));
                if let Ok(vista) = &mut self.biblioteca {
                    // Lo leído cambia "Seguir leyendo" y lo terminado de cada serie.
                    vista.recargar();
                }
            }
            return;
        }

        let elegido = match &mut self.biblioteca {
            Ok(vista) => vista.mostrar(ui, frame),
            Err(error) => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.heading("No se pudo abrir la biblioteca");
                    ui.label(error.as_str());
                });
                None
            }
        };
        if let Some(ruta) = elegido {
            self.abrir(&ctx, &ruta);
        }
    }
}

/// Dónde se guardan la biblioteca y las miniaturas: las carpetas estándar de cada
/// sistema (en Windows, `AppData`; en Linux, `~/.local/share` y `~/.cache`).
///
/// Con la variable de entorno `LECTOR_COMICS_DATOS` se guarda todo en esa carpeta:
/// sirve para probar con una biblioteca aparte sin tocar la de verdad.
fn rutas() -> (PathBuf, PathBuf) {
    if let Some(carpeta) = std::env::var_os("LECTOR_COMICS_DATOS").map(PathBuf::from) {
        return (carpeta.join("biblioteca.db"), carpeta.join("miniaturas"));
    }
    match directories::ProjectDirs::from("", "", "lector-comics") {
        Some(carpetas) => (
            carpetas.data_dir().join("biblioteca.db"),
            carpetas.cache_dir().join("miniaturas"),
        ),
        None => {
            eprintln!("[ui] No se encontró la carpeta de usuario; se usa la actual");
            (PathBuf::from("biblioteca.db"), PathBuf::from("miniaturas"))
        }
    }
}

fn main() -> eframe::Result {
    // Opcional: un cómic para abrir nada más arrancar.
    let inicial = std::env::args_os().nth(1).map(PathBuf::from);

    let opciones = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITULO)
            // En Linux agrupa las ventanas y asocia el icono del escritorio.
            .with_app_id("lector-comics")
            .with_maximized(true),
        ..Default::default()
    };

    eframe::run_native(
        TITULO,
        opciones,
        Box::new(move |cc| {
            cc.egui_ctx.set_theme(egui::Theme::Dark);

            let (ruta_db, carpeta_miniaturas) = rutas();
            eprintln!("[ui] Biblioteca en {}", ruta_db.display());
            let biblioteca = VistaBiblioteca::nueva(&cc.egui_ctx, ruta_db, carpeta_miniaturas);
            if let Err(e) = &biblioteca {
                eprintln!("[ui] No se pudo abrir la biblioteca: {e}");
            }

            let mut aplicacion = Aplicacion {
                biblioteca,
                abierto: None,
            };
            if let Some(ruta) = inicial {
                aplicacion.abrir(&cc.egui_ctx, &ruta);
            }
            Ok(Box::new(aplicacion))
        }),
    )
}
