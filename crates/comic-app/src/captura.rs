//! Modo de captura, para revisar la interfaz sin tocar la pantalla de nadie: con la
//! variable `LECTOR_COMICS_CAPTURA=<carpeta>`, la ventana se abre fuera de la pantalla y
//! sin coger el foco, recorre las páginas, guarda una imagen de cada una en la carpeta y
//! se cierra. Mejor junto con `LECTOR_COMICS_DATOS`, para no tocar los datos de verdad.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;

/// Lo que se espera en cada página antes de la foto: que lleguen las portadas y lo que
/// se pida a internet.
const ESPERA: Duration = Duration::from_secs(5);

/// Las páginas que se recorren, en orden. Con `claro:` delante, en el tema claro.
const PASOS: [&str; 16] = [
    "biblioteca",
    "serie",
    "leyendo",
    "novedades",
    "novedades:manga",
    "novedades:ficha",
    "novedades:semana",
    "novedades:noticias",
    "novedades:favoritos",
    "carpetas",
    "ajustes",
    "lector",
    "lector:vineta-1",
    "lector:vineta-2",
    "claro:biblioteca",
    "claro:novedades:manga",
];

pub struct Captura {
    carpeta: PathBuf,
    /// El paso que se está fotografiando; `None` antes del primero.
    actual: Option<usize>,
    desde: Instant,
    pedida: bool,
}

impl Captura {
    pub fn desde_entorno() -> Option<Captura> {
        let carpeta = PathBuf::from(std::env::var_os("LECTOR_COMICS_CAPTURA")?);
        if let Err(e) = std::fs::create_dir_all(&carpeta) {
            eprintln!("[captura] No se pudo crear {}: {e}", carpeta.display());
            return None;
        }
        Some(Captura {
            carpeta,
            actual: None,
            desde: Instant::now(),
            pedida: false,
        })
    }

    /// La ventana, fuera de la pantalla y sin coger el foco.
    pub fn ventana(ventana: egui::ViewportBuilder) -> egui::ViewportBuilder {
        ventana
            .with_fullscreen(false)
            .with_maximized(false)
            .with_inner_size([1600.0, 1000.0])
            .with_position([-5000.0, 40.0])
            .with_active(false)
    }

    /// Se llama en cada fotograma, antes de dibujar. Devuelve la página a la que hay que
    /// ir cuando toca cambiar (sin el `claro:`, que se atiende aquí).
    pub fn seguir(&mut self, ctx: &egui::Context) -> Option<String> {
        ctx.request_repaint();
        let Some(actual) = self.actual else {
            return self.empezar(ctx, 0);
        };

        let foto = ctx.input(|i| {
            i.events.iter().find_map(|evento| match evento {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(imagen) = foto {
            self.guardar(actual, &imagen);
            if actual + 1 == PASOS.len() {
                eprintln!("[captura] Hecho: {}", self.carpeta.display());
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return None;
            }
            return self.empezar(ctx, actual + 1);
        }

        if !self.pedida && self.desde.elapsed() >= ESPERA {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.pedida = true;
        }
        None
    }

    fn empezar(&mut self, ctx: &egui::Context, paso: usize) -> Option<String> {
        self.actual = Some(paso);
        self.desde = Instant::now();
        self.pedida = false;
        let (tema, pagina) = match PASOS[paso].strip_prefix("claro:") {
            Some(pagina) => (egui::Theme::Light, pagina),
            None => (egui::Theme::Dark, PASOS[paso]),
        };
        ctx.set_theme(tema);
        Some(pagina.to_string())
    }

    fn guardar(&self, paso: usize, imagen: &egui::ColorImage) {
        let nombre = PASOS[paso].replace(':', "-");
        let archivo = self.carpeta.join(format!("{:02}-{nombre}.jpg", paso + 1));
        let rgba = imagen
            .pixels
            .iter()
            .flat_map(|p| p.to_srgba_unmultiplied())
            .collect();
        let imagen = comic_imagen::Imagen {
            ancho: imagen.size[0] as u32,
            alto: imagen.size[1] as u32,
            rgba,
        };
        let resultado = comic_imagen::codificar_jpeg(&imagen, 92)
            .and_then(|bytes| std::fs::write(&archivo, bytes).map_err(|e| e.to_string()));
        match resultado {
            Ok(()) => eprintln!("[captura] {}", archivo.display()),
            Err(e) => eprintln!("[captura] No se pudo guardar {}: {e}", archivo.display()),
        }
    }
}
