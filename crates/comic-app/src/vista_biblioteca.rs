//! Pantalla de la biblioteca: las series en una cuadrícula de portadas, lo que se está
//! leyendo y las carpetas. El escaneo y las miniaturas van en hilos aparte, así que la
//! pantalla no se congela nunca.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use comic_biblioteca::{Biblioteca, Carpeta, Ficha, Portada, Progreso, Resumen, Serie};
use eframe::egui;

use crate::dialogos::{self, Dialogo};
use crate::miniaturas::{self, Miniaturas};
use crate::precarga::Tamano;

/// Tamaño de las tarjetas, en puntos. Las portadas de cómic suelen ser 2:3.
const ANCHO_TARJETA: f32 = 150.0;
const ALTO_PORTADA: f32 = 225.0;
/// Portada, título en dos líneas y una línea de detalle.
const ALTO_TARJETA: f32 = ALTO_PORTADA + 62.0;
const SEPARACION: f32 = 18.0;
const MARGEN: f32 = 20.0;
/// Alto de los títulos de sección ("Seguir leyendo", "Series").
const ALTO_TITULO: f32 = 34.0;

/// Miniaturas que se guardan en la GPU como mucho. Con miles de cómics, tenerlas
/// todas ocuparía gigas; las que hace tiempo que no se ven se sueltan.
const MAX_TEXTURAS: usize = 300;
/// Mientras se escanea, cada cuánto se recargan las listas para ir mostrando lo nuevo.
const RECARGA_DURANTE_ESCANEO: Duration = Duration::from_secs(1);
/// Cuántos cómics se piden para "Seguir leyendo" (se muestran los que quepan en una fila).
const MAX_SEGUIR_LEYENDO: usize = 20;

/// Lo que se ve en una tarjeta y qué pasa al pulsarla.
struct Tarjeta {
    portada: Option<Portada>,
    titulo: String,
    detalle: String,
    /// Texto que aparece al dejar el ratón encima.
    ayuda: String,
    /// Cuánto se ha leído, de 0 a 1.
    progreso: Option<f32>,
    terminado: bool,
    /// `None` si no se puede abrir (un cómic que no se pudo leer).
    al_pulsar: Option<Pulsar>,
}

#[derive(Clone)]
enum Pulsar {
    Serie(String),
    Comic(PathBuf),
}

/// Una miniatura ya en la GPU.
struct Textura {
    textura: egui::TextureHandle,
    /// Para qué tamaño de tarjeta se hizo, en píxeles.
    caja: Tamano,
    /// Último fotograma en que se dibujó, para soltar las que no se ven.
    usada: u64,
}

/// Un escaneo en marcha en otro hilo.
struct Escaneo {
    hechos: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    fin: Receiver<Result<Resumen, String>>,
}

pub struct VistaBiblioteca {
    biblioteca: Biblioteca,
    ruta_db: PathBuf,
    // Lo que se muestra, leído de la base de datos. Se recarga cuando cambia algo, no
    // en cada fotograma.
    carpetas: Vec<Carpeta>,
    series: Vec<Serie>,
    seguir_leyendo: Vec<Ficha>,
    /// La serie que se está viendo y sus cómics, o `None` para ver todas las series.
    serie: Option<(String, Vec<Ficha>)>,
    busqueda: String,

    miniaturas: Miniaturas,
    texturas: HashMap<String, Textura>,
    /// Huellas cuya miniatura no se pudo hacer: no se vuelven a pedir.
    sin_miniatura: std::collections::HashSet<String>,
    /// Lo último que se pidió, para no repetir la misma petición en cada fotograma.
    pedido: Vec<(String, Tamano)>,
    fotograma: u64,

    escaneo: Option<Escaneo>,
    /// Ha cambiado algo durante un escaneo (una carpeta nueva): hay que repetirlo al acabar.
    reescanear: bool,
    ultima_recarga: Instant,

    dialogo: Option<(Dialogo, Receiver<Option<PathBuf>>)>,
    /// Un diálogo que se ha pedido en este fotograma y se abre al final, cuando ya se
    /// tiene la ventana para ponerlo encima.
    dialogo_pedido: Option<Dialogo>,
    ver_carpetas: bool,
    error: Option<String>,
}

impl VistaBiblioteca {
    /// Abre la biblioteca de `ruta_db` y empieza a escanear sus carpetas en segundo plano.
    pub fn nueva(
        ctx: &egui::Context,
        ruta_db: PathBuf,
        carpeta_miniaturas: PathBuf,
    ) -> Result<VistaBiblioteca, String> {
        let biblioteca = Biblioteca::abrir(&ruta_db)?;
        let nucleos = thread::available_parallelism().map_or(1, |n| n.get());
        let miniaturas = Miniaturas::nuevo(
            carpeta_miniaturas,
            nucleos.saturating_sub(1).clamp(1, 4),
            ctx.clone(),
        );

        let mut vista = VistaBiblioteca {
            biblioteca,
            ruta_db,
            carpetas: Vec::new(),
            series: Vec::new(),
            seguir_leyendo: Vec::new(),
            serie: None,
            busqueda: String::new(),
            miniaturas,
            texturas: HashMap::new(),
            sin_miniatura: Default::default(),
            pedido: Vec::new(),
            fotograma: 0,
            escaneo: None,
            reescanear: false,
            ultima_recarga: Instant::now(),
            dialogo: None,
            dialogo_pedido: None,
            ver_carpetas: false,
            error: None,
        };
        vista.recargar();
        if !vista.carpetas.is_empty() {
            vista.escanear(ctx);
        }
        Ok(vista)
    }

    pub fn progreso(&self, huella: &str) -> Option<Progreso> {
        self.biblioteca.progreso(huella).unwrap_or_else(|e| {
            eprintln!("[biblioteca] No se pudo leer el progreso: {e}");
            None
        })
    }

    pub fn guardar_progreso(&self, huella: &str, pagina: usize, terminado: bool) {
        if let Err(e) = self.biblioteca.guardar_progreso(huella, pagina, terminado) {
            eprintln!("[biblioteca] No se pudo guardar el progreso: {e}");
        }
    }

    pub fn mostrar_error(&mut self, error: String) {
        eprintln!("[ui] {error}");
        self.error = Some(error);
    }

    /// Vuelve a leer de la base de datos todo lo que se muestra.
    pub fn recargar(&mut self) {
        let resultado = (|| -> Result<(), String> {
            self.carpetas = self.biblioteca.carpetas()?;
            self.series = self.biblioteca.series()?;
            self.seguir_leyendo = self.biblioteca.seguir_leyendo(MAX_SEGUIR_LEYENDO)?;
            if let Some((nombre, comics)) = &mut self.serie {
                *comics = self.biblioteca.comics_de_serie(nombre)?;
            }
            Ok(())
        })();
        if let Err(e) = resultado {
            self.mostrar_error(format!("No se pudo leer la biblioteca: {e}"));
        }
        self.ultima_recarga = Instant::now();
    }

    /// Escanea las carpetas en otro hilo, con su propia conexión a la base de datos.
    fn escanear(&mut self, ctx: &egui::Context) {
        if self.escaneo.is_some() {
            self.reescanear = true;
            return;
        }

        let hechos = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let (enviar, fin) = mpsc::channel();
        let ruta_db = self.ruta_db.clone();
        let ctx = ctx.clone();
        let (h, t) = (Arc::clone(&hechos), Arc::clone(&total));

        thread::spawn(move || {
            let resultado = Biblioteca::abrir(&ruta_db).and_then(|mut biblioteca| {
                biblioteca.escanear(|hechos, total| {
                    h.store(hechos, Ordering::Relaxed);
                    t.store(total, Ordering::Relaxed);
                    if hechos % 10 == 0 || hechos == total {
                        ctx.request_repaint();
                    }
                })
            });
            let _ = enviar.send(resultado);
            ctx.request_repaint();
        });

        self.escaneo = Some(Escaneo { hechos, total, fin });
    }

    /// Mira cómo va el escaneo: recarga de vez en cuando y, al acabar, del todo.
    fn seguir_escaneo(&mut self, ctx: &egui::Context) {
        let Some(escaneo) = &self.escaneo else {
            return;
        };

        match escaneo.fin.try_recv() {
            Ok(resultado) => {
                self.escaneo = None;
                match resultado {
                    Ok(resumen) => eprintln!("[biblioteca] Escaneo terminado: {resumen:?}"),
                    Err(e) => self.mostrar_error(format!("El escaneo ha fallado: {e}")),
                }
                self.recargar();
                if std::mem::take(&mut self.reescanear) {
                    self.escanear(ctx);
                }
            }
            Err(_) => {
                if self.ultima_recarga.elapsed() >= RECARGA_DURANTE_ESCANEO {
                    self.recargar();
                }
                ctx.request_repaint_after(RECARGA_DURANTE_ESCANEO);
            }
        }
    }

    fn agregar_carpeta(&mut self, ctx: &egui::Context, ruta: &Path) {
        match self.biblioteca.agregar_carpeta(ruta) {
            Ok(carpeta) => {
                eprintln!("[biblioteca] Carpeta añadida: {}", carpeta.ruta.display());
                self.recargar();
                self.escanear(ctx);
            }
            Err(e) => self.mostrar_error(e),
        }
    }

    fn quitar_carpeta(&mut self, id: i64) {
        if let Err(e) = self.biblioteca.quitar_carpeta(id) {
            self.mostrar_error(format!("No se pudo quitar la carpeta: {e}"));
        }
        self.serie = None;
        self.recargar();
    }

    fn abrir_serie(&mut self, nombre: String) {
        match self.biblioteca.comics_de_serie(&nombre) {
            Ok(comics) => self.serie = Some((nombre, comics)),
            Err(e) => self.mostrar_error(format!("No se pudo leer la serie: {e}")),
        }
    }

    /// Dibuja la pantalla. Devuelve el cómic que hay que abrir, si se ha elegido uno.
    pub fn mostrar(&mut self, ui: &mut egui::Ui, ventana: &eframe::Frame) -> Option<PathBuf> {
        let ctx = ui.ctx().clone();
        self.fotograma += 1;
        self.seguir_escaneo(&ctx);
        self.recibir_miniaturas(&ctx);

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.serie.is_some() {
            self.serie = None;
        }

        egui::Panel::top("barra_biblioteca").show(ui, |ui| self.barra(ui));

        let fondo = egui::Frame::central_panel(&ctx.global_style())
            .fill(ctx.global_style().visuals.extreme_bg_color)
            .inner_margin(0.0);
        let pulsada = egui::CentralPanel::default()
            .frame(fondo)
            .show(ui, |ui| self.contenido(ui))
            .inner;

        self.ventana_carpetas(&ctx);
        self.ventana_error(&ctx);
        self.soltar_texturas_viejas();

        let mut abrir = match pulsada {
            Some(Pulsar::Serie(nombre)) => {
                self.abrir_serie(nombre);
                None
            }
            Some(Pulsar::Comic(ruta)) => Some(ruta),
            None => None,
        };
        abrir = abrir.or(self.atender_dialogos(&ctx, ventana));
        abrir
    }

    /// Abre el diálogo que se haya pedido y recoge lo que se elija en él.
    fn atender_dialogos(
        &mut self,
        ctx: &egui::Context,
        ventana: &eframe::Frame,
    ) -> Option<PathBuf> {
        if let Some(dialogo) = self.dialogo_pedido.take() {
            // Uno cada vez: en Linux el anterior puede seguir abierto.
            if self.dialogo.is_none() {
                self.dialogo = Some((dialogo, dialogos::abrir(dialogo, ventana)));
            }
        }

        let (dialogo, respuesta) = self.dialogo.as_ref()?;
        let elegido = match respuesta.try_recv() {
            Ok(elegido) => elegido,
            Err(mpsc::TryRecvError::Empty) => {
                // En Linux llega desde otro hilo: hay que seguir mirando.
                ctx.request_repaint_after(Duration::from_millis(100));
                return None;
            }
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        let dialogo = *dialogo;
        self.dialogo = None;

        let ruta = elegido?;
        match dialogo {
            Dialogo::CarpetaDeBiblioteca => {
                self.agregar_carpeta(ctx, &ruta);
                None
            }
            Dialogo::Comic | Dialogo::CarpetaDeImagenes => Some(ruta),
        }
    }

    fn barra(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.serie.is_some() {
                if ui
                    .button("⬅ Series")
                    .on_hover_text("Volver a todas las series (Esc)")
                    .clicked()
                {
                    self.serie = None;
                }
                ui.separator();
            }

            if ui
                .button("Añadir carpeta")
                .on_hover_text("Añadir a la biblioteca una carpeta con cómics")
                .clicked()
            {
                self.dialogo_pedido = Some(Dialogo::CarpetaDeBiblioteca);
            }
            ui.menu_button("Abrir", |ui| {
                if ui.button("Cómic…").clicked() {
                    self.dialogo_pedido = Some(Dialogo::Comic);
                    ui.close();
                }
                if ui.button("Carpeta de imágenes…").clicked() {
                    self.dialogo_pedido = Some(Dialogo::CarpetaDeImagenes);
                    ui.close();
                }
            })
            .response
            .on_hover_text("Leer un cómic sin añadirlo a la biblioteca");
            if ui
                .selectable_label(self.ver_carpetas, "Carpetas")
                .on_hover_text("Ver y quitar las carpetas de la biblioteca")
                .clicked()
            {
                self.ver_carpetas = !self.ver_carpetas;
            }

            if self.serie.is_none() {
                ui.separator();
                ui.add(
                    egui::TextEdit::singleline(&mut self.busqueda)
                        .hint_text("Buscar serie")
                        .desired_width(220.0),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let oscuro = ui.ctx().theme() == egui::Theme::Dark;
                if ui
                    .button(if oscuro { "Claro" } else { "Oscuro" })
                    .on_hover_text("Cambiar el tema")
                    .clicked()
                {
                    ui.ctx().set_theme(if oscuro {
                        egui::Theme::Light
                    } else {
                        egui::Theme::Dark
                    });
                }
                ui.separator();

                if let Some(escaneo) = &self.escaneo {
                    let hechos = escaneo.hechos.load(Ordering::Relaxed);
                    let total = escaneo.total.load(Ordering::Relaxed);
                    if total > 0 {
                        ui.label(format!("Escaneando {hechos} de {total}"));
                    } else {
                        ui.label("Buscando cómics");
                    }
                    ui.spinner();
                } else {
                    let comics: usize = self.series.iter().map(|s| s.comics).sum();
                    ui.weak(format!(
                        "{} {}, {} {}",
                        self.series.len(),
                        plural(self.series.len(), "serie", "series"),
                        comics,
                        plural(comics, "cómic", "cómics"),
                    ));
                }
            });
        });
    }

    /// Lo que va en el centro: la biblioteca vacía, las series o una serie.
    fn contenido(&mut self, ui: &mut egui::Ui) -> Option<Pulsar> {
        if self.carpetas.is_empty() {
            self.biblioteca_vacia(ui);
            return None;
        }

        if let Some((nombre, comics)) = &self.serie {
            let titulo = format!(
                "{nombre}  ·  {} {}",
                comics.len(),
                plural(comics.len(), "cómic", "cómics")
            );
            let tarjetas = comics.iter().map(tarjeta_de_comic).collect();
            return self.cuadricula(ui, None, Some(titulo), tarjetas);
        }

        let busqueda = self.busqueda.trim().to_lowercase();
        let series: Vec<Tarjeta> = self
            .series
            .iter()
            .filter(|s| busqueda.is_empty() || s.nombre.to_lowercase().contains(&busqueda))
            .map(tarjeta_de_serie)
            .collect();

        // "Seguir leyendo" solo sin búsqueda: al buscar, lo que importa es lo que se busca.
        let seguir = if busqueda.is_empty() && !self.seguir_leyendo.is_empty() {
            let tarjetas = self.seguir_leyendo.iter().map(tarjeta_de_seguir).collect();
            Some(("Seguir leyendo".to_string(), tarjetas))
        } else {
            None
        };

        if series.is_empty() {
            ui.add_space(MARGEN);
            ui.vertical_centered(|ui| {
                if busqueda.is_empty() {
                    ui.weak("Todavía no hay cómics. Si se está escaneando, irán apareciendo aquí.");
                } else {
                    ui.weak(format!(
                        "Ninguna serie contiene \u{201c}{}\u{201d}.",
                        self.busqueda.trim()
                    ));
                }
            });
            return None;
        }
        let titulo = seguir.as_ref().map(|_| "Series".to_string());
        self.cuadricula(ui, seguir, titulo, series)
    }

    fn biblioteca_vacia(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.heading("Tu biblioteca está vacía");
            ui.add_space(8.0);
            ui.label("Añade la carpeta donde guardas tus cómics.");
            ui.weak("La biblioteca solo los lee: nunca mueve ni cambia tus archivos.");
            ui.add_space(16.0);
            if ui.button("Añadir carpeta").clicked() {
                self.dialogo_pedido = Some(Dialogo::CarpetaDeBiblioteca);
            }
        });
    }

    /// Una cuadrícula de tarjetas que se desplaza en vertical, con una fila opcional
    /// arriba (`seccion`). Solo se dibujan las filas que se ven, así va igual de fluida
    /// con diez series que con diez mil.
    fn cuadricula(
        &mut self,
        ui: &mut egui::Ui,
        seccion: Option<(String, Vec<Tarjeta>)>,
        titulo: Option<String>,
        tarjetas: Vec<Tarjeta>,
    ) -> Option<Pulsar> {
        let ppp = ui.ctx().pixels_per_point();
        // Tamaño exacto en píxeles de la portada, para que la miniatura se dibuje 1:1.
        let caja = [
            (ANCHO_TARJETA * ppp).round() as u32,
            (ALTO_PORTADA * ppp).round() as u32,
        ];
        let paso = egui::vec2(ANCHO_TARJETA + SEPARACION, ALTO_TARJETA + SEPARACION);
        let tamano = egui::vec2(ANCHO_TARJETA, ALTO_TARJETA);

        let mut pulsada = None;
        // Primero las que se ven y luego las de la fila de más que se prepara por
        // arriba y por abajo, para que al desplazarse ya estén.
        let mut peticiones = (Vec::new(), Vec::new());

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show_viewport(ui, |ui, vista| {
                let ancho = ui.available_width() - 2.0 * MARGEN;
                let columnas = (((ancho + SEPARACION) / paso.x).floor() as usize).max(1);
                let origen = ui.max_rect().min + egui::vec2(MARGEN, MARGEN);
                let mut y = 0.0;

                if let Some((nombre, fila)) = &seccion {
                    titulo_de_seccion(ui, origen + egui::vec2(0.0, y), nombre);
                    y += ALTO_TITULO;
                    for (columna, tarjeta) in fila.iter().take(columnas).enumerate() {
                        let rect = egui::Rect::from_min_size(
                            origen + egui::vec2(columna as f32 * paso.x, y),
                            tamano,
                        );
                        let id = ui.id().with(("seguir", columna));
                        if self.tarjeta(ui, rect, id, tarjeta, caja, &mut peticiones) {
                            pulsada = tarjeta.al_pulsar.clone();
                        }
                    }
                    y += paso.y + SEPARACION;
                }
                if let Some(titulo) = &titulo {
                    titulo_de_seccion(ui, origen + egui::vec2(0.0, y), titulo);
                    y += ALTO_TITULO;
                }

                let filas = tarjetas.len().div_ceil(columnas);
                ui.set_height(y + filas as f32 * paso.y + 2.0 * MARGEN);

                // Filas visibles, más una por arriba y otra por abajo.
                let desde = (vista.min.y - MARGEN - y) / paso.y;
                let hasta = (vista.max.y - MARGEN - y) / paso.y;
                let primera = (desde.floor().max(0.0) as usize).saturating_sub(1);
                let ultima = ((hasta.ceil().max(0.0) as usize) + 1).min(filas);

                for fila in primera..ultima {
                    for columna in 0..columnas {
                        let Some(tarjeta) = tarjetas.get(fila * columnas + columna) else {
                            break;
                        };
                        let rect = egui::Rect::from_min_size(
                            origen + egui::vec2(columna as f32 * paso.x, y + fila as f32 * paso.y),
                            tamano,
                        );
                        let id = ui.id().with(("tarjeta", fila * columnas + columna));
                        if self.tarjeta(ui, rect, id, tarjeta, caja, &mut peticiones) {
                            pulsada = tarjeta.al_pulsar.clone();
                        }
                    }
                }
            });

        let (mut visibles, cercanas) = peticiones;
        visibles.extend(cercanas);
        self.pedir_miniaturas(visibles);
        pulsada
    }

    /// Dibuja una tarjeta. Devuelve si se ha pulsado. Si le falta la miniatura, la añade
    /// a `peticiones` (a las visibles o a las cercanas).
    fn tarjeta(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        id: egui::Id,
        tarjeta: &Tarjeta,
        caja: Tamano,
        peticiones: &mut (Vec<miniaturas::Peticion>, Vec<miniaturas::Peticion>),
    ) -> bool {
        let sentido = if tarjeta.al_pulsar.is_some() {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let respuesta = ui.interact(rect, id, sentido).on_hover_text(&tarjeta.ayuda);
        let encima = respuesta.hovered() && tarjeta.al_pulsar.is_some();
        if encima {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let visuales = ui.visuals().clone();
        let pintor = ui.painter();
        let hueco = egui::Rect::from_min_size(rect.min, egui::vec2(ANCHO_TARJETA, ALTO_PORTADA));
        pintor.rect_filled(hueco, 4.0, visuales.faint_bg_color);

        // La miniatura, si está; si no (o es de otro tamaño), se pide.
        let mut dibujada = false;
        if let Some(portada) = &tarjeta.portada {
            if let Some(textura) = self.texturas.get_mut(&portada.huella) {
                textura.usada = self.fotograma;
                dibujar_miniatura(ui, hueco, &textura.textura);
                dibujada = true;
            }
            let lista = self
                .texturas
                .get(&portada.huella)
                .is_some_and(|t| t.caja == caja);
            if !lista && !self.sin_miniatura.contains(&portada.huella) {
                let peticion = miniaturas::Peticion {
                    portada: portada.clone(),
                    caja,
                };
                if ui.clip_rect().intersects(rect) {
                    peticiones.0.push(peticion);
                } else {
                    peticiones.1.push(peticion);
                }
            }
        }
        let pintor = ui.painter();
        // Sin miniatura: o no se puede abrir, o no se pudo sacar la portada. Mientras
        // se está haciendo, el hueco se queda vacío (suele tardar un instante).
        let sin_portada = tarjeta
            .portada
            .as_ref()
            .is_none_or(|p| self.sin_miniatura.contains(&p.huella));
        if !dibujada && (tarjeta.al_pulsar.is_none() || sin_portada) {
            let aviso = if tarjeta.al_pulsar.is_none() {
                "No se puede abrir"
            } else {
                "Sin portada"
            };
            pintor.text(
                hueco.center(),
                egui::Align2::CENTER_CENTER,
                aviso,
                egui::FontId::proportional(13.0),
                visuales.weak_text_color(),
            );
        }

        if let Some(progreso) = tarjeta.progreso.filter(|_| !tarjeta.terminado) {
            let barra =
                egui::Rect::from_min_max(egui::pos2(hueco.min.x, hueco.max.y - 4.0), hueco.max);
            pintor.rect_filled(barra, 0.0, egui::Color32::from_black_alpha(160));
            let hecho = egui::Rect::from_min_size(
                barra.min,
                egui::vec2(barra.width() * progreso.clamp(0.0, 1.0), barra.height()),
            );
            pintor.rect_filled(hecho, 0.0, visuales.selection.bg_fill);
        }
        if tarjeta.terminado {
            let galeria = pintor.layout_no_wrap(
                "Leído".to_string(),
                egui::FontId::proportional(12.0),
                egui::Color32::WHITE,
            );
            let fondo = egui::Rect::from_min_size(
                egui::pos2(hueco.max.x - galeria.size().x - 14.0, hueco.min.y + 6.0),
                galeria.size() + egui::vec2(8.0, 4.0),
            );
            pintor.rect_filled(fondo, 3.0, egui::Color32::from_black_alpha(190));
            pintor.galley(
                fondo.min + egui::vec2(4.0, 2.0),
                galeria,
                egui::Color32::WHITE,
            );
        }
        if encima {
            pintor.rect_stroke(
                hueco,
                4.0,
                egui::Stroke::new(2.0, visuales.selection.stroke.color),
                egui::StrokeKind::Outside,
            );
        }

        let titulo = texto_ajustado(
            ui,
            &tarjeta.titulo,
            egui::FontId::proportional(14.0),
            visuales.strong_text_color(),
            2,
        );
        let alto_titulo = titulo.size().y;
        pintor.galley(
            hueco.left_bottom() + egui::vec2(0.0, 6.0),
            titulo,
            egui::Color32::WHITE,
        );
        let detalle = texto_ajustado(
            ui,
            &tarjeta.detalle,
            egui::FontId::proportional(12.0),
            visuales.weak_text_color(),
            1,
        );
        pintor.galley(
            hueco.left_bottom() + egui::vec2(0.0, 8.0 + alto_titulo),
            detalle,
            egui::Color32::WHITE,
        );

        respuesta.clicked()
    }

    fn pedir_miniaturas(&mut self, peticiones: Vec<miniaturas::Peticion>) {
        let pedido: Vec<(String, Tamano)> = peticiones
            .iter()
            .map(|p| (p.portada.huella.clone(), p.caja))
            .collect();
        if pedido != self.pedido {
            self.miniaturas.pedir(peticiones);
            self.pedido = pedido;
        }
    }

    /// Sube a la GPU las miniaturas que hayan terminado los hilos.
    fn recibir_miniaturas(&mut self, ctx: &egui::Context) {
        let listas: Vec<_> = self.miniaturas.listas().collect();
        for respuesta in listas {
            match respuesta {
                miniaturas::Respuesta::Lista {
                    huella,
                    caja,
                    imagen,
                } => {
                    // Como las páginas: se dibujan 1:1, y el filtro solo actúa si la
                    // ventana cambia de escala hasta que llega la nueva.
                    let textura = ctx.load_texture(
                        format!("miniatura-{huella}"),
                        imagen,
                        egui::TextureOptions::LINEAR,
                    );
                    self.texturas.insert(
                        huella,
                        Textura {
                            textura,
                            caja,
                            usada: self.fotograma,
                        },
                    );
                }
                miniaturas::Respuesta::Error { huella } => {
                    self.sin_miniatura.insert(huella);
                }
            }
        }
    }

    fn soltar_texturas_viejas(&mut self) {
        if self.texturas.len() <= MAX_TEXTURAS {
            return;
        }
        let mut usadas: Vec<u64> = self.texturas.values().map(|t| t.usada).collect();
        usadas.sort_unstable();
        // Se quedan las dos terceras partes más recientes.
        let limite = usadas[usadas.len() - MAX_TEXTURAS * 2 / 3];
        self.texturas.retain(|_, t| t.usada >= limite);
    }

    fn ventana_carpetas(&mut self, ctx: &egui::Context) {
        if !self.ver_carpetas {
            return;
        }
        let mut abierta = true;
        let mut quitar = None;
        let mut reescanear = false;
        let escaneando = self.escaneo.is_some();

        egui::Window::new("Carpetas de la biblioteca")
            .open(&mut abierta)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.weak(
                    "La biblioteca solo lee estas carpetas: nunca mueve ni cambia tus archivos.",
                );
                ui.add_space(6.0);
                if self.carpetas.is_empty() {
                    ui.label("No hay ninguna.");
                }
                for carpeta in &self.carpetas {
                    ui.horizontal(|ui| {
                        let quitar_boton = ui
                            .add_enabled(!escaneando, egui::Button::new("Quitar"))
                            .on_hover_text(
                                "Quita sus cómics de la biblioteca. Los archivos no se tocan, \
                                 y si la vuelves a añadir, cada cómic sigue por donde ibas.",
                            )
                            .on_disabled_hover_text("Espera a que termine el escaneo");
                        if quitar_boton.clicked() {
                            quitar = Some(carpeta.id);
                        }
                        ui.label(carpeta.ruta.display().to_string());
                    });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Añadir carpeta").clicked() {
                        self.dialogo_pedido = Some(Dialogo::CarpetaDeBiblioteca);
                    }
                    if ui
                        .add_enabled(!escaneando, egui::Button::new("Volver a escanear"))
                        .on_hover_text("Buscar cómics nuevos, cambiados o borrados")
                        .clicked()
                    {
                        reescanear = true;
                    }
                });
            });

        self.ver_carpetas = abierta;
        if let Some(id) = quitar {
            self.quitar_carpeta(id);
        }
        if reescanear {
            self.escanear(ctx);
        }
    }

    fn ventana_error(&mut self, ctx: &egui::Context) {
        let Some(error) = &self.error else {
            return;
        };
        let mut cerrar = false;
        egui::Window::new("Algo ha fallado")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(error);
                ui.add_space(8.0);
                if ui.button("Aceptar").clicked() {
                    cerrar = true;
                }
            });
        if cerrar {
            self.error = None;
        }
    }
}

fn plural(n: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if n == 1 {
        singular
    } else {
        plural
    }
}

fn tarjeta_de_serie(serie: &Serie) -> Tarjeta {
    let mut detalle = format!(
        "{} {}",
        serie.comics,
        plural(serie.comics, "cómic", "cómics")
    );
    if serie.terminados > 0 {
        detalle.push_str(&format!(
            " · {} {}",
            serie.terminados,
            plural(serie.terminados, "leído", "leídos")
        ));
    }
    Tarjeta {
        portada: Some(serie.portada.clone()),
        titulo: serie.nombre.clone(),
        ayuda: format!("{}\n{detalle}", serie.nombre),
        detalle,
        progreso: (serie.terminados > 0).then(|| serie.terminados as f32 / serie.comics as f32),
        terminado: serie.terminados == serie.comics,
        al_pulsar: Some(Pulsar::Serie(serie.nombre.clone())),
    }
}

/// Cómo se llama un cómic dentro de su serie: `#1 · Título`, `Vol. 2`, o el nombre
/// del archivo si no hay nada mejor.
fn nombre_de_comic(ficha: &Ficha) -> String {
    let m = &ficha.metadatos;
    let numero = match (&m.numero, m.volumen) {
        (Some(numero), _) => Some(format!("#{numero}")),
        (None, Some(volumen)) => Some(format!("Vol. {volumen}")),
        (None, None) => None,
    };
    match (numero, &m.titulo) {
        (Some(numero), Some(titulo)) => format!("{numero} · {titulo}"),
        (Some(numero), None) => numero,
        (None, Some(titulo)) => titulo.clone(),
        (None, None) => ficha
            .ruta
            .file_stem()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned()),
    }
}

fn progreso_de(ficha: &Ficha) -> (Option<f32>, bool) {
    match ficha.progreso {
        Some(p) if ficha.paginas > 0 => (
            Some((p.pagina + 1) as f32 / ficha.paginas as f32),
            p.terminado,
        ),
        _ => (None, false),
    }
}

fn tarjeta_de_comic(ficha: &Ficha) -> Tarjeta {
    let (progreso, terminado) = progreso_de(ficha);
    let m = &ficha.metadatos;
    let detalle = match &ficha.error {
        Some(error) => error.clone(),
        None => {
            let paginas = format!(
                "{} {}",
                ficha.paginas,
                plural(ficha.paginas, "página", "páginas")
            );
            match m.anio {
                Some(anio) => format!("{anio} · {paginas}"),
                None => paginas,
            }
        }
    };
    let titulo = nombre_de_comic(ficha);
    let mut ayuda = format!("{}\n{titulo}\n{}", ficha.serie(), ficha.ruta.display());
    if let Some(autor) = &m.guion {
        ayuda.push_str(&format!("\nGuion: {autor}"));
    }
    if let Some(autor) = &m.dibujo {
        ayuda.push_str(&format!("\nDibujo: {autor}"));
    }
    Tarjeta {
        portada: ficha.error.is_none().then(|| ficha.portada()),
        titulo,
        detalle,
        ayuda,
        progreso,
        terminado,
        al_pulsar: ficha
            .error
            .is_none()
            .then(|| Pulsar::Comic(ficha.ruta.clone())),
    }
}

fn tarjeta_de_seguir(ficha: &Ficha) -> Tarjeta {
    let (progreso, terminado) = progreso_de(ficha);
    let pagina = ficha.progreso.map_or(0, |p| p.pagina) + 1;
    Tarjeta {
        portada: Some(ficha.portada()),
        titulo: format!("{} {}", ficha.serie(), nombre_de_comic(ficha)),
        detalle: format!("Página {pagina} de {}", ficha.paginas),
        ayuda: format!("Seguir por la página {pagina}\n{}", ficha.ruta.display()),
        progreso,
        terminado,
        al_pulsar: Some(Pulsar::Comic(ficha.ruta.clone())),
    }
}

fn titulo_de_seccion(ui: &egui::Ui, posicion: egui::Pos2, texto: &str) {
    ui.painter().text(
        posicion,
        egui::Align2::LEFT_TOP,
        texto,
        egui::FontId::proportional(20.0),
        ui.visuals().strong_text_color(),
    );
}

/// Texto partido en líneas al ancho de la tarjeta, con `…` si no cabe en `filas`.
fn texto_ajustado(
    ui: &egui::Ui,
    texto: &str,
    fuente: egui::FontId,
    color: egui::Color32,
    filas: usize,
) -> Arc<egui::Galley> {
    let mut trabajo = egui::text::LayoutJob::single_section(
        texto.to_string(),
        egui::TextFormat::simple(fuente, color),
    );
    trabajo.wrap = egui::text::TextWrapping {
        max_width: ANCHO_TARJETA,
        max_rows: filas,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(trabajo)
}

/// Dibuja la miniatura centrada en `hueco`, alineada con los píxeles de la pantalla
/// para que no se vuelva a filtrar (igual que las páginas).
fn dibujar_miniatura(ui: &egui::Ui, hueco: egui::Rect, textura: &egui::TextureHandle) {
    let ppp = ui.ctx().pixels_per_point();
    let tamano = textura.size_vec2() / ppp;
    let colocada = egui::Rect::from_center_size(hueco.center(), tamano);
    let esquina = (colocada.min.to_vec2() * ppp).round() / ppp;
    let rect = egui::Rect::from_min_size(esquina.to_pos2(), tamano);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter()
        .image(textura.id(), rect, uv, egui::Color32::WHITE);
}
