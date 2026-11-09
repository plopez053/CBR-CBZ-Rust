//! El selector de páginas: un panel flotante, sin bordes, con las páginas en fila. La
//! del centro se ve grande y las vecinas, a los lados, cada vez más pequeñas y oscuras.
//! La rueda las hace pasar y un clic salta a esa página.
//!
//! Se abre con el botón "Páginas" de la barra, con la tecla P o dejando el ratón un
//! momento en la esquina inferior derecha de la página.
//!
//! Las miniaturas las preparan unos hilos propios, con su propio `Comic` abierto, para
//! no hacer esperar a las páginas grandes del lector. Se guardan solo en memoria y solo
//! las de alrededor de lo que se ve.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use comic_core::Comic;
use comic_imagen::Encogedor;
use eframe::egui::{self, Color32, Rect, Stroke, StrokeKind, Vec2};

use crate::estilo::{self, Colores};
use crate::lector::muescas_de_rueda;
use crate::pasar_hoja::degradado;
use crate::precarga::{tamano_textura, Tamano};

/// Alto máximo de una página dentro del selector, en puntos. Las miniaturas se preparan
/// para este tamaño, así valen para cualquier alto del panel.
const ALTO_HOJA_MAX: f32 = 260.0;
/// Ancho entre alto de una página que aún no ha llegado (el de un cómic americano).
const PROPORCION_HOJA: f32 = 0.66;
/// Ancho máximo del panel, en puntos.
const ANCHO_PANEL_MAX: f32 = 760.0;
const MARGEN: f32 = 14.0;
/// Sitio encima de las páginas para el título y debajo para su número.
const ALTO_TITULO: f32 = 22.0;
const ALTO_NUMERO: f32 = 22.0;
/// Muescas de la rueda que hay que girar para pasar una página. Más = menos sensible.
const MUESCAS_POR_PAGINA: f32 = 1.0;
/// Lado de la zona de la esquina inferior derecha que abre el selector.
const LADO_ESQUINA: f32 = 56.0;
/// Desde qué distancia a la esquina empieza a asomar la pista (la punta doblada).
const ALCANCE_PISTA: f32 = 140.0;
/// Segundos con el ratón en la esquina antes de abrirse, para que no se abra sin querer.
const ESPERA_ESQUINA: f32 = 0.3;
/// Segundos con el ratón fuera antes de cerrarse, si se abrió con la esquina.
const ESPERA_CIERRE: f32 = 0.4;
/// Duración de la animación de abrir y cerrar.
const DURACION_APERTURA: f32 = 0.2;
/// Rapidez con la que las páginas se deslizan hasta la elegida (como el zoom del lector).
const VELOCIDAD: f32 = 14.0;
/// Páginas a cada lado de la del centro que se dibujan.
const VECINAS_VISIBLES: usize = 4;
/// Páginas a cada lado cuyas miniaturas se preparan y se guardan.
const VECINAS_PRECARGA: usize = 10;
const HILOS: usize = 2;

/// Cómo se abrió: lo que se abre al pasar el ratón se cierra al quitarlo.
#[derive(Clone, Copy, PartialEq)]
enum Modo {
    Raton,
    Fijo,
}

pub struct Selector {
    ruta: PathBuf,
    total: usize,
    abierto: Option<Modo>,
    /// Fotograma en que se abrió: el clic que lo abrió no debe contar como "clic fuera".
    abierto_en: u64,
    /// Página que se ve en el centro (con decimales mientras se desliza) y la pedida.
    posicion: f32,
    objetivo: f32,
    /// Rueda girada que aún no llega a una página entera (paneles táctiles).
    rueda: f32,
    /// Segundos que lleva el ratón en la esquina, o fuera del panel.
    en_esquina: f32,
    fuera: f32,
    /// Tras cerrarse, la esquina no lo vuelve a abrir hasta que el ratón salga de ella.
    esquina_armada: bool,
    /// Se arranca la primera vez que se abre: muchos cómics se leen sin abrirlo nunca.
    cargador: Option<Cargador>,
    hojas: HashMap<usize, egui::TextureHandle>,
    fallidas: HashSet<usize>,
    /// Lo último que se pidió a los hilos, para no repetir la misma petición cada fotograma.
    pedidas: Vec<usize>,
    /// La página sobre la que se ha abierto el menú del botón derecho.
    menu_pagina: Option<usize>,
    /// La página que se ha elegido como portada en ese menú y aún no se ha recogido.
    portada_elegida: Option<usize>,
}

impl Selector {
    pub fn nuevo(ruta: &Path, total: usize) -> Selector {
        Selector {
            ruta: ruta.to_path_buf(),
            total,
            abierto: None,
            abierto_en: 0,
            posicion: 0.0,
            objetivo: 0.0,
            rueda: 0.0,
            en_esquina: 0.0,
            fuera: 0.0,
            esquina_armada: true,
            cargador: None,
            hojas: HashMap::new(),
            fallidas: HashSet::new(),
            pedidas: Vec::new(),
            menu_pagina: None,
            portada_elegida: None,
        }
    }

    pub fn abierto(&self) -> bool {
        self.abierto.is_some()
    }

    /// La página que el usuario ha elegido como portada del cómic, si lo ha hecho desde
    /// la última vez que se preguntó.
    pub fn portada_elegida(&mut self) -> Option<usize> {
        self.portada_elegida.take()
    }

    /// Lo abre (sin que se cierre al quitar el ratón) o lo cierra.
    pub fn alternar(&mut self, ctx: &egui::Context, actual: usize) {
        if self.abierto() {
            self.cerrar();
        } else {
            self.abrir(ctx, Modo::Fijo, actual);
        }
    }

    pub fn cerrar(&mut self) {
        if self.abierto.take().is_some() {
            eprintln!("[selector] Cerrado");
        }
        self.esquina_armada = false;
        self.en_esquina = 0.0;
    }

    fn abrir(&mut self, ctx: &egui::Context, modo: Modo, actual: usize) {
        if self.total == 0 {
            return;
        }
        self.abierto = Some(modo);
        self.abierto_en = ctx.cumulative_frame_nr();
        // Se abre ya colocado en la página que se está leyendo, sin deslizarse hasta ella.
        self.objetivo = actual as f32;
        self.posicion = self.objetivo;
        self.rueda = 0.0;
        self.fuera = 0.0;
        eprintln!("[selector] Abierto en la página {}", actual + 1);
    }

    /// Dónde va el panel: abajo a la derecha del hueco de la página.
    fn rect_panel(hueco: Rect) -> Rect {
        let ancho = (hueco.width() * 0.55)
            .clamp(360.0, ANCHO_PANEL_MAX)
            .min(hueco.width() - 2.0 * MARGEN)
            .max(0.0);
        let alto = (hueco.height() * 0.45)
            .clamp(
                220.0,
                ALTO_HOJA_MAX + 2.0 * MARGEN + ALTO_TITULO + ALTO_NUMERO,
            )
            .min(hueco.height() - 2.0 * MARGEN)
            .max(0.0);
        Rect::from_min_size(
            hueco.max - egui::vec2(ancho + MARGEN, alto + MARGEN),
            egui::vec2(ancho, alto),
        )
    }

    /// Atiende la esquina, abre y cierra, y dibuja el panel si está abierto (o cerrándose).
    /// Devuelve la página que se ha elegido con un clic.
    pub fn mostrar(&mut self, ui: &egui::Ui, hueco: Rect, actual: usize) -> Option<usize> {
        let ctx = ui.ctx().clone();
        let c = estilo::colores(&ctx);
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let (raton, pulsado, clic, pos_clic) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.any_down(),
                i.pointer.primary_clicked(),
                i.pointer.interact_pos(),
            )
        });
        let panel = Self::rect_panel(hueco);
        let esquina = Rect::from_min_max(hueco.max - Vec2::splat(LADO_ESQUINA), hueco.max);
        let en_esquina = raton.is_some_and(|p| esquina.contains(p));
        if !en_esquina {
            self.esquina_armada = true;
        }

        match self.abierto {
            None => {
                // Mientras se arrastra la página no cuenta: se abriría sin querer.
                if en_esquina && !pulsado && self.esquina_armada {
                    self.en_esquina += dt;
                    if self.en_esquina >= ESPERA_ESQUINA {
                        self.abrir(&ctx, Modo::Raton, actual);
                    } else {
                        ctx.request_repaint();
                    }
                } else {
                    self.en_esquina = 0.0;
                }
                if !self.abierto() {
                    self.dibujar_pista(ui, hueco, raton, pulsado, c);
                }
            }
            Some(modo) => {
                // Con el menú del botón derecho abierto, el ratón puede salirse del panel.
                let con_menu = egui::Popup::is_any_open(&ctx);
                let dentro =
                    con_menu || en_esquina || raton.is_some_and(|p| panel.expand(16.0).contains(p));
                if modo == Modo::Raton && !dentro && !pulsado {
                    self.fuera += dt;
                    if self.fuera >= ESPERA_CIERRE {
                        self.cerrar();
                    } else {
                        ctx.request_repaint();
                    }
                } else {
                    self.fuera = 0.0;
                }
                let clic_fuera = clic && !con_menu && pos_clic.is_some_and(|p| !panel.contains(p));
                if clic_fuera && ctx.cumulative_frame_nr() != self.abierto_en {
                    self.cerrar();
                }
            }
        }

        let apertura = ctx.animate_bool_with_time_and_easing(
            egui::Id::new("selector-paginas"),
            self.abierto(),
            DURACION_APERTURA,
            egui::emath::easing::cubic_out,
        );
        if apertura <= 0.0 || panel.width() < 100.0 {
            return None;
        }

        // Cada fotograma recorre la misma fracción de lo que falta: empieza rápido y frena.
        let avance = 1.0 - (-dt * VELOCIDAD).exp();
        self.posicion += (self.objetivo - self.posicion) * avance;
        if (self.objetivo - self.posicion).abs() < 0.001 {
            self.posicion = self.objetivo;
        } else {
            ctx.request_repaint();
        }
        self.gestionar_miniaturas(&ctx);

        // Sale de la esquina: se desliza un poco hacia arriba y a la izquierda mientras aparece.
        let desvio = Vec2::splat(28.0 * (1.0 - apertura));
        let mut elegida = None;
        egui::Area::new(egui::Id::new("selector-paginas-area"))
            .order(egui::Order::Foreground)
            .fixed_pos(panel.min + desvio)
            .fade_in(false)
            .show(&ctx, |ui| {
                ui.multiply_opacity(apertura);
                let (rect, respuesta) = ui.allocate_exact_size(panel.size(), egui::Sense::click());
                if self.abierto() && respuesta.hovered() {
                    self.girar_rueda(ui);
                }
                let encima = self.dibujar_panel(ui, rect, actual, c);
                // Botón derecho sobre una página: usarla como portada del cómic.
                if respuesta.secondary_clicked() {
                    self.menu_pagina = encima;
                }
                if let Some(pagina) = self.menu_pagina.filter(|_| self.abierto()) {
                    respuesta.context_menu(|ui| {
                        if ui.button("Usar como portada").clicked() {
                            eprintln!("[selector] Portada: página {}", pagina + 1);
                            self.portada_elegida = Some(pagina);
                            ui.close();
                        }
                        estilo::cerrar_menu_al_alejarse(ui, None);
                    });
                }
                if let Some(pagina) = encima.filter(|_| self.abierto()) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    if respuesta.clicked() {
                        eprintln!("[selector] Elegida la página {}", pagina + 1);
                        elegida = Some(pagina);
                        self.cerrar();
                    }
                }
            });
        elegida
    }

    /// Rueda hacia delante (hacia la pantalla), páginas hacia delante.
    fn girar_rueda(&mut self, ui: &egui::Ui) {
        self.rueda += ui.input(muescas_de_rueda) / MUESCAS_POR_PAGINA;
        let pasos = self.rueda.trunc();
        if pasos != 0.0 {
            self.rueda -= pasos;
            let ultima = self.total.saturating_sub(1) as f32;
            self.objetivo = (self.objetivo + pasos).clamp(0.0, ultima);
        }
    }

    /// Una punta de página doblada en la esquina, que asoma al acercar el ratón y crece
    /// mientras se espera a que se abra: así se sabe que ahí hay algo.
    fn dibujar_pista(
        &self,
        ui: &egui::Ui,
        hueco: Rect,
        raton: Option<egui::Pos2>,
        pulsado: bool,
        c: Colores,
    ) {
        let Some(raton) = raton.filter(|p| hueco.contains(*p) && !pulsado) else {
            return;
        };
        let cerca = 1.0 - ((hueco.max - raton).length() / ALCANCE_PISTA).min(1.0);
        if cerca <= 0.0 {
            return;
        }
        let carga = (self.en_esquina / ESPERA_ESQUINA).min(1.0);
        let lado = 16.0 + 10.0 * cerca + 14.0 * carga;
        let esquina = hueco.max;
        let punta = vec![
            esquina - egui::vec2(lado, 0.0),
            esquina - egui::vec2(0.0, lado),
            esquina,
        ];
        let pintor = ui.painter_at(hueco);
        pintor.add(egui::Shape::convex_polygon(
            punta,
            c.acento.gamma_multiply(0.35 + 0.65 * cerca.max(carga)),
            Stroke::NONE,
        ));
        // El doblez: una línea más oscura en la diagonal.
        pintor.line_segment(
            [
                esquina - egui::vec2(lado, 0.0),
                esquina - egui::vec2(0.0, lado),
            ],
            Stroke::new(1.5, Color32::from_black_alpha((90.0 * cerca) as u8)),
        );
    }

    /// Dibuja el fondo, el título y las páginas. Devuelve la que tiene el ratón encima.
    fn dibujar_panel(&self, ui: &egui::Ui, rect: Rect, actual: usize, c: Colores) -> Option<usize> {
        let fondo = ui.painter();
        fondo.rect_filled(rect, 0, c.lector);
        fondo.rect_stroke(rect, 0, Stroke::new(1.0, c.borde), StrokeKind::Inside);

        let interior = rect.shrink(MARGEN);
        let pintor = ui.painter_at(rect.shrink(1.0));
        let alto_hoja = (interior.height() - ALTO_TITULO - ALTO_NUMERO).min(ALTO_HOJA_MAX);
        let ancho_hoja = alto_hoja * PROPORCION_HOJA;
        let centro_y = interior.min.y + ALTO_TITULO + alto_hoja / 2.0;

        // De la más lejana a la del centro, para que las de delante tapen a las de detrás.
        let central = self.posicion.round() as isize;
        let ultima = self.total as isize - 1;
        let mut paginas: Vec<usize> = (central - VECINAS_VISIBLES as isize
            ..=central + VECINAS_VISIBLES as isize)
            .filter(|&p| (0..=ultima).contains(&p))
            .map(|p| p as usize)
            .collect();
        paginas.sort_by(|a, b| {
            let da = (*a as f32 - self.posicion).abs();
            let db = (*b as f32 - self.posicion).abs();
            db.total_cmp(&da)
        });

        // Cada página que se ve, con su distancia al centro, para saber cuál está encima.
        let mut zonas: Vec<(Rect, usize, f32)> = Vec::new();
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        for pagina in paginas {
            let d = pagina as f32 - self.posicion;
            let lejos = d.abs();
            let escala = (1.0 - 0.16 * lejos).max(0.3);
            // Más juntas cuanto más lejos, como si se fueran hacia el fondo.
            let x = interior.center().x + ancho_hoja * 1.08 * d * (1.0 - 0.08 * lejos).max(0.5);
            let caja = Rect::from_center_size(
                egui::pos2(x, centro_y),
                egui::vec2(ancho_hoja, alto_hoja) * escala,
            );
            if !caja.intersects(rect) {
                continue;
            }
            // Encajada en su hueco sin deformarse: las páginas dobles salen más bajas.
            let hoja = self.hojas.get(&pagina);
            let pagina_rect = match hoja {
                Some(textura) => {
                    let [ancho, alto] = textura.size().map(|l| l as f32);
                    let ajuste = (caja.width() / ancho).min(caja.height() / alto);
                    Rect::from_center_size(caja.center(), egui::vec2(ancho, alto) * ajuste)
                }
                None => caja,
            };
            // Las lejanas, más oscuras (no transparentes: se verían las de detrás).
            let brillo = (1.0 - 0.2 * lejos).clamp(0.3, 1.0);

            let sombra = egui::Shadow {
                offset: [0, 4],
                blur: 12,
                spread: 0,
                color: Color32::from_black_alpha((110.0 * brillo) as u8),
            };
            pintor.add(sombra.as_shape(pagina_rect, 0));
            match hoja {
                Some(textura) => {
                    let tinte = Color32::from_gray((255.0 * brillo) as u8);
                    pintor.image(textura.id(), pagina_rect, uv, tinte);
                }
                None => {
                    pintor.rect_filled(pagina_rect, 0, oscurecer(c.tarjeta, brillo));
                }
            }

            // La que se está leyendo, marcada con el color de acento.
            let es_actual = pagina == actual;
            if es_actual {
                pintor.rect_stroke(
                    pagina_rect,
                    0,
                    Stroke::new(2.0, oscurecer(c.acento, brillo)),
                    StrokeKind::Outside,
                );
            }
            let tamano_letra = (12.0 * escala.max(0.75)).round();
            pintor.text(
                egui::pos2(caja.center().x, caja.max.y + 5.0),
                egui::Align2::CENTER_TOP,
                (pagina + 1).to_string(),
                estilo::cifras(tamano_letra),
                if es_actual {
                    c.acento
                } else {
                    c.texto_secundario
                }
                .gamma_multiply(brillo),
            );
            zonas.push((pagina_rect, pagina, lejos));
        }

        // Los lados se funden con el fondo, para que las páginas no se corten de golpe.
        let ancho_fundido = (rect.width() * 0.08).min(50.0);
        degradado(
            &pintor,
            Rect::from_min_size(rect.min, egui::vec2(ancho_fundido, rect.height())),
            c.lector,
            Color32::TRANSPARENT,
        );
        degradado(
            &pintor,
            Rect::from_min_max(egui::pos2(rect.max.x - ancho_fundido, rect.min.y), rect.max),
            Color32::TRANSPARENT,
            c.lector,
        );

        let encima = ui.input(|i| i.pointer.hover_pos()).and_then(|raton| {
            zonas
                .iter()
                .filter(|(zona, _, _)| zona.contains(raton))
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .map(|(zona, pagina, _)| (*zona, *pagina))
        });
        if let Some((zona, _)) = encima {
            pintor.rect_filled(zona, 0, Color32::from_white_alpha(28));
        }

        // Arriba, qué página se está leyendo o a cuál se va a saltar.
        let titulo = match encima {
            Some((_, pagina)) if pagina != actual => format!("Ir a la página {}", pagina + 1),
            _ => format!("Página {} de {}", actual + 1, self.total),
        };
        pintor.text(
            interior.min,
            egui::Align2::LEFT_TOP,
            titulo,
            estilo::negrita(13.0),
            c.texto,
        );

        encima.map(|(_, pagina)| pagina)
    }

    /// Recoge las miniaturas hechas, pide las que faltan alrededor de la página a la que
    /// se va y suelta las que quedan lejos.
    fn gestionar_miniaturas(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        // Lo bastante ancha para que una página doble también salga nítida.
        let lado = (ALTO_HOJA_MAX * ppp).ceil() as u32;
        let caja: Tamano = [lado, lado];
        let cargador = self
            .cargador
            .get_or_insert_with(|| Cargador::nuevo(&self.ruta, caja, ctx.clone()));

        for (pagina, resultado) in cargador.listas() {
            match resultado {
                Ok(imagen) => {
                    let textura = ctx.load_texture(
                        format!("hoja-{pagina}"),
                        imagen,
                        egui::TextureOptions::LINEAR,
                    );
                    self.hojas.insert(pagina, textura);
                }
                Err(e) => {
                    eprintln!("[selector] Página {}: {e}", pagina + 1);
                    self.fallidas.insert(pagina);
                }
            }
        }

        // De la página a la que se va hacia fuera, alternando lados: lo que se ve, primero.
        let centro = self.objetivo.round() as isize;
        let ultima = self.total as isize - 1;
        let mut faltan = Vec::new();
        for distancia in 0..=VECINAS_PRECARGA as isize {
            for pagina in [centro + distancia, centro - distancia] {
                if !(0..=ultima).contains(&pagina) {
                    continue;
                }
                let pagina = pagina as usize;
                if !faltan.contains(&pagina)
                    && !self.hojas.contains_key(&pagina)
                    && !self.fallidas.contains(&pagina)
                {
                    faltan.push(pagina);
                }
            }
        }
        if faltan != self.pedidas {
            cargador.pedir(faltan.clone());
            self.pedidas = faltan;
        }
        // Unas cuantas de margen, para no soltar lo que se acaba de dejar atrás.
        let margen = VECINAS_PRECARGA as isize + 4;
        self.hojas
            .retain(|pagina, _| (*pagina as isize - centro).abs() <= margen);
    }
}

fn oscurecer(color: Color32, brillo: f32) -> Color32 {
    let [r, g, b, a] = color.to_array();
    let f = |canal: u8| (canal as f32 * brillo) as u8;
    Color32::from_rgba_premultiplied(f(r), f(g), f(b), a)
}

/// Lo que falta por hacer. La interfaz la sustituye entera cuando cambia lo que se ve,
/// así al pasar páginas deprisa no se hace esperar a lo que ya está en pantalla.
struct Cola {
    paginas: VecDeque<usize>,
    /// Las que algún hilo está haciendo ya: no se vuelven a poner en la cola.
    en_curso: HashSet<usize>,
    cerrada: bool,
}

pub(crate) type Resultado = (usize, Result<egui::ColorImage, String>);

/// Hilos que leen páginas del cómic y las dejan del tamaño de una miniatura.
pub(crate) struct Cargador {
    cola: Arc<(Mutex<Cola>, Condvar)>,
    resultados: Receiver<Resultado>,
}

impl Cargador {
    pub(crate) fn nuevo(ruta: &Path, caja: Tamano, ctx: egui::Context) -> Cargador {
        let cola = Arc::new((
            Mutex::new(Cola {
                paginas: VecDeque::new(),
                en_curso: HashSet::new(),
                cerrada: false,
            }),
            Condvar::new(),
        ));
        let (enviar, resultados) = mpsc::channel();
        for _ in 0..HILOS {
            let trabajador = Trabajador {
                ruta: ruta.to_path_buf(),
                caja,
                cola: Arc::clone(&cola),
                resultados: enviar.clone(),
                ctx: ctx.clone(),
            };
            thread::spawn(move || trabajador.trabajar());
        }
        eprintln!(
            "[selector] {HILOS} hilos de miniaturas (caja: {}x{})",
            caja[0], caja[1]
        );
        Cargador { cola, resultados }
    }

    /// Sustituye lo que quedaba por hacer por estas páginas, en este orden.
    pub(crate) fn pedir(&self, paginas: Vec<usize>) {
        let (cola, aviso) = &*self.cola;
        let mut cola = cola.lock().unwrap();
        let en_curso = &cola.en_curso;
        cola.paginas = paginas
            .into_iter()
            .filter(|p| !en_curso.contains(p))
            .collect();
        aviso.notify_all();
    }

    pub(crate) fn listas(&self) -> impl Iterator<Item = Resultado> + '_ {
        self.resultados.try_iter()
    }
}

impl Drop for Cargador {
    /// Despierta a los hilos para que vean que se ha cerrado y terminen.
    fn drop(&mut self) {
        let (cola, aviso) = &*self.cola;
        cola.lock().unwrap().cerrada = true;
        aviso.notify_all();
    }
}

struct Trabajador {
    ruta: PathBuf,
    caja: Tamano,
    cola: Arc<(Mutex<Cola>, Condvar)>,
    resultados: Sender<Resultado>,
    ctx: egui::Context,
}

impl Trabajador {
    fn trabajar(self) {
        // Leer del archivo necesita `&mut`: cada hilo abre el suyo.
        let mut comic = match Comic::abrir(&self.ruta) {
            Ok(comic) => comic,
            Err(e) => {
                eprintln!("[selector] No se pudo abrir {}: {e}", self.ruta.display());
                return;
            }
        };
        let mut encogedor = Encogedor::default();

        loop {
            let pagina = {
                let (cola, aviso) = &*self.cola;
                let mut cola = cola.lock().unwrap();
                loop {
                    if cola.cerrada {
                        return;
                    }
                    if let Some(pagina) = cola.paginas.pop_front() {
                        cola.en_curso.insert(pagina);
                        break pagina;
                    }
                    cola = aviso.wait(cola).unwrap();
                }
            };

            let resultado = preparar(&mut comic, pagina, self.caja, &mut encogedor);
            // Primero se envía y luego se quita de `en_curso`: así la interfaz nunca la
            // ve como pendiente y sin hacer a la vez, y no la vuelve a pedir.
            let enviada = self.resultados.send((pagina, resultado)).is_ok();
            let (cola, _) = &*self.cola;
            cola.lock().unwrap().en_curso.remove(&pagina);
            if !enviada {
                return;
            }
            self.ctx.request_repaint();
        }
    }
}

fn preparar(
    comic: &mut Comic,
    pagina: usize,
    caja: Tamano,
    encogedor: &mut Encogedor,
) -> Result<egui::ColorImage, String> {
    let original = comic_imagen::decodificar(&comic.leer(pagina)?)?;
    let [ancho, alto] = tamano_textura([original.ancho, original.alto], caja);
    let imagen = encogedor.encoger(original, ancho, alto)?;
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [ancho as usize, alto as usize],
        &imagen.rgba,
    ))
}
