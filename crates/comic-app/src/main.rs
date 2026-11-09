// En la versión final (`--release`) no se abre la consola negra junto a la ventana.
// En desarrollo sí, para ver los mensajes de `eprintln!`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod captura;
mod controles;
mod dialogos;
mod elegir_portada;
mod estilo;
mod filtro;
mod fondo;
// Opcional: solo existe si hay `src/fuentes.rs` (ver build.rs); no se sube a git.
#[cfg(fuentes)]
mod fuentes;
mod lector;
mod miniaturas;
mod navegador;
mod pasar_hoja;
mod portadas_remotas;
mod precarga;
mod preferencias;
mod selector;
mod vista_ajustes;
mod vista_biblioteca;
mod vista_novedades;

use std::path::{Path, PathBuf};

use captura::Captura;
use comic_core::Comic;
use eframe::egui;
use lector::Lector;
use preferencias::Preferencias;
use vista_biblioteca::VistaBiblioteca;

const TITULO: &str = "Lector de cómics";

/// Un cómic abierto en el lector y lo que hace falta para apuntar por dónde va.
struct Abierto {
    lector: Lector,
    huella: String,
    /// El cómic que sigue a este en su serie, si lo hay.
    siguiente: Option<PathBuf>,
    /// Última página apuntada en la biblioteca, para no guardar en cada fotograma.
    guardada: Option<usize>,
}

/// La aplicación: la biblioteca y, encima, el cómic que se esté leyendo.
struct Aplicacion {
    /// `Err` si no se pudo abrir la base de datos: aun así se puede leer un cómic suelto.
    biblioteca: Result<VistaBiblioteca, String>,
    abierto: Option<Abierto>,
    preferencias: Preferencias,
    /// El modo de captura, si se ha pedido (ver `captura.rs`).
    captura: Option<Captura>,
    /// El tema que se le ha pedido al sistema para la barra de título de la ventana.
    tema_ventana: Option<egui::Theme>,
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

        // Al acabar este, se ofrece el siguiente de su serie (si está en la biblioteca).
        let siguiente = match &self.biblioteca {
            Ok(vista) => vista.siguiente_de(&huella),
            Err(_) => None,
        };
        let mut lector = Lector::nuevo(
            ctx,
            comic,
            ruta,
            pagina,
            self.preferencias.animacion,
            self.preferencias.filtro,
            self.preferencias.controles.clone(),
        );
        lector.poner_siguiente(siguiente.as_ref().map(|(_, titulo)| titulo.clone()));
        self.abierto = Some(Abierto {
            lector,
            huella,
            siguiente: siguiente.map(|(ruta, _)| ruta),
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

        // La barra de título (con los botones de cerrar y minimizar) es del sistema: se le
        // pide que vaya con el tema de la app, oscura en el oscuro, para que no salga blanca.
        let tema = ctx.theme();
        if self.tema_ventana != Some(tema) {
            let sistema = match tema {
                egui::Theme::Dark => egui::SystemTheme::Dark,
                egui::Theme::Light => egui::SystemTheme::Light,
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(sistema));
            self.tema_ventana = Some(tema);
        }

        if let (Some(captura), Ok(vista)) = (&mut self.captura, &mut self.biblioteca) {
            if let Some(pagina) = captura.seguir(&ctx) {
                self.abierto = None;
                // "lector:vineta-2": el lector leyendo por viñetas, en el segundo recuadro.
                let (paso, vineta) = match pagina.strip_prefix("lector:vineta-") {
                    Some(n) => ("lector", n.parse::<usize>().ok()),
                    None => (pagina.as_str(), None),
                };
                if let Some(ruta) = vista.ir_a_para_captura(&ctx, paso) {
                    self.abrir(&ctx, &ruta);
                    if let (Some(n), Some(abierto)) = (vineta, &mut self.abierto) {
                        abierto.lector.vinetas_para_captura(n.saturating_sub(1));
                    }
                }
            }
        }

        if let Some(abierto) = &mut self.abierto {
            let volver = abierto.lector.mostrar(ui);
            let portada = abierto.lector.portada_elegida();
            let al_siguiente = abierto
                .lector
                .pide_siguiente()
                .then(|| abierto.siguiente.clone())
                .flatten();
            if let (Some(pagina), Ok(vista)) = (portada, &mut self.biblioteca) {
                vista.elegir_portada(&abierto.huella, pagina);
            }
            // Mientras se arrastra un deslizador del filtro cambia en cada fotograma: se
            // guarda al soltar el ratón, no cien veces seguidas.
            let soltado = !ctx.input(|i| i.pointer.any_down());
            let cambiadas = abierto.lector.animacion() != self.preferencias.animacion
                || abierto.lector.filtro() != self.preferencias.filtro;
            if cambiadas && soltado {
                self.preferencias.animacion = abierto.lector.animacion();
                self.preferencias.filtro = abierto.lector.filtro();
                self.preferencias.guardar();
            }
            self.guardar_progreso();
            if let Some(ruta) = al_siguiente {
                self.abrir(&ctx, &ruta);
                return;
            }
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

        // Esc también saca de pantalla completa en la biblioteca, antes que cualquier otra
        // cosa que haga con esa tecla.
        if lector::en_pantalla_completa(&ctx)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            lector::poner_pantalla_completa(&ctx, false);
        }

        let elegido = match &mut self.biblioteca {
            Ok(vista) => vista.mostrar(ui, frame, &mut self.preferencias),
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

/// Dónde se guardan las cosas de la app.
struct Rutas {
    biblioteca: PathBuf,
    miniaturas: PathBuf,
    preferencias: PathBuf,
    /// Ajustes de las novedades (cuenta de Metron, RSS).
    novedades: PathBuf,
    /// Lo último que se comprobó y las portadas descargadas.
    cache_novedades: PathBuf,
}

/// Las carpetas estándar de cada sistema (en Windows, `AppData`; en Linux,
/// `~/.local/share`, `~/.cache` y `~/.config`).
///
/// Con la variable de entorno `LECTOR_COMICS_DATOS` se guarda todo en esa carpeta:
/// sirve para probar con una biblioteca aparte sin tocar la de verdad.
fn rutas() -> Rutas {
    if let Some(carpeta) = std::env::var_os("LECTOR_COMICS_DATOS").map(PathBuf::from) {
        return Rutas {
            biblioteca: carpeta.join("biblioteca.db"),
            miniaturas: carpeta.join("miniaturas"),
            preferencias: carpeta.join("preferencias.txt"),
            novedades: carpeta.join("novedades.txt"),
            cache_novedades: carpeta.join("novedades"),
        };
    }
    match directories::ProjectDirs::from("", "", "lector-comics") {
        Some(carpetas) => Rutas {
            biblioteca: carpetas.data_dir().join("biblioteca.db"),
            miniaturas: carpetas.cache_dir().join("miniaturas"),
            preferencias: carpetas.config_dir().join("preferencias.txt"),
            novedades: carpetas.config_dir().join("novedades.txt"),
            cache_novedades: carpetas.cache_dir().join("novedades"),
        },
        None => {
            eprintln!("[ui] No se encontró la carpeta de usuario; se usa la actual");
            Rutas {
                biblioteca: PathBuf::from("biblioteca.db"),
                miniaturas: PathBuf::from("miniaturas"),
                preferencias: PathBuf::from("preferencias.txt"),
                novedades: PathBuf::from("novedades.txt"),
                cache_novedades: PathBuf::from("novedades"),
            }
        }
    }
}

fn main() -> eframe::Result {
    // Opcional: un cómic para abrir nada más arrancar.
    let inicial = std::env::args_os().nth(1).map(PathBuf::from);

    let mut ventana = egui::ViewportBuilder::default()
        .with_title(TITULO)
        // En Linux agrupa las ventanas y asocia el icono del escritorio.
        .with_app_id("lector-comics")
        .with_fullscreen(true);
    match eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icono.png")) {
        Ok(icono) => ventana = ventana.with_icon(icono),
        Err(e) => eprintln!("[ui] No se pudo cargar el icono: {e}"),
    }
    let captura = Captura::desde_entorno();
    if captura.is_some() {
        ventana = Captura::ventana(ventana);
    }
    let opciones = eframe::NativeOptions {
        viewport: ventana,
        ..Default::default()
    };

    eframe::run_native(
        TITULO,
        opciones,
        Box::new(move |cc| {
            estilo::aplicar(&cc.egui_ctx);
            cc.egui_ctx.set_theme(egui::Theme::Dark);

            let rutas = rutas();
            eprintln!("[ui] Biblioteca en {}", rutas.biblioteca.display());
            let biblioteca = VistaBiblioteca::nueva(
                &cc.egui_ctx,
                rutas.biblioteca,
                rutas.miniaturas,
                rutas.novedades,
                rutas.cache_novedades,
            );
            if let Err(e) = &biblioteca {
                eprintln!("[ui] No se pudo abrir la biblioteca: {e}");
            }

            let mut aplicacion = Aplicacion {
                biblioteca,
                abierto: None,
                preferencias: Preferencias::cargar(rutas.preferencias),
                captura,
                tema_ventana: None,
            };
            if let Some(ruta) = inicial {
                aplicacion.abrir(&cc.egui_ctx, &ruta);
            }
            Ok(Box::new(aplicacion))
        }),
    )
}
