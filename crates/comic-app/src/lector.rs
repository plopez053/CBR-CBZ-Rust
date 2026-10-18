//! El lector: muestra las páginas de un cómic con zoom, desplazamiento y precarga.

use std::collections::HashMap;
use std::path::Path;

use comic_core::Comic;
use eframe::egui;

use crate::precarga::{self, Pagina, Precargador, Respuesta, Tamano, ATRAS, DELANTE};

/// Más hilos que páginas en la ventana no sirven de nada: estarían parados.
const MAX_HILOS: usize = ATRAS + 1 + DELANTE;

/// Límites del zoom, relativos al ajuste elegido (1 = la página ajustada justo al hueco).
const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 8.0;
/// Cuánto cambia el zoom con cada muesca de la rueda o pulsación de + o -.
const PASO_ZOOM: f32 = 1.25;
/// Cuánto se desplaza la página con cada pulsación de las flechas arriba/abajo, en puntos.
const PASO_DESPLAZAMIENTO: f32 = 100.0;
/// Puntos que egui cuenta por cada muesca de la rueda en escritorio. Sirve para convertir
/// el desplazamiento continuo de un panel táctil en "muescas".
const PUNTOS_POR_MUESCA: f32 = 40.0;
/// Rapidez con la que la vista alcanza el zoom y la posición pedidos. Con 15, recorre
/// el 95% del camino en unos 200 ms. Más alto = más rápido.
const VELOCIDAD_ANIMACION: f32 = 15.0;

/// A qué se ajusta la página cuando el zoom es 1.
#[derive(Clone, Copy, PartialEq)]
enum Ajuste {
    /// La página entera cabe en el hueco.
    Pagina,
    /// La página ocupa todo el ancho y se lee bajando.
    Ancho,
}

/// Una página ya subida a la GPU.
struct Cargada {
    /// Tamaño de la imagen en el archivo: con él se calcula a qué tamaño se dibuja.
    original: Tamano,
    textura: egui::TextureHandle,
}

impl Cargada {
    /// Dice si la textura tiene ya el tamaño que tocaría para esta caja. Si no, se puede
    /// dibujar igualmente (escalada) mientras llega la buena.
    fn sirve_para(&self, caja: Tamano) -> bool {
        let [ancho, alto] = precarga::tamano_textura(self.original, caja);
        self.textura.size() == [ancho as usize, alto as usize]
    }
}

/// Un cómic abierto: en qué página estamos y cómo se ve.
pub struct Lector {
    total: usize,
    actual: usize,
    /// Se ha pedido volver a la biblioteca (Esc o el botón).
    volver: bool,
    precargador: Precargador,
    /// Páginas ya subidas a la GPU.
    texturas: HashMap<usize, Cargada>,
    /// Páginas pedidas a los hilos cuya respuesta aún no ha llegado, y con qué caja.
    pedidas: HashMap<usize, Tamano>,
    errores: HashMap<usize, String>,
    pantalla_completa: bool,
    oscuro: bool,
    ajuste: Ajuste,
    /// Zoom y desplazamiento que se ven ahora mismo. Persiguen a los `_objetivo`
    /// con una animación suave, así el zoom no va a saltos.
    zoom: f32,
    /// Cuánto está movido el centro de la página respecto al centro del hueco, en puntos.
    desplazamiento: egui::Vec2,
    /// Zoom pedido por el usuario. Las páginas se preparan para este zoom, no para los
    /// intermedios de la animación, así los hilos no trabajan en tamaños de paso.
    zoom_objetivo: f32,
    /// Posición pedida. Se recorta en cada fotograma para que la página no se salga;
    /// un valor infinito significa "todo lo que se pueda" (por ejemplo, arriba del todo).
    desplazamiento_objetivo: egui::Vec2,
    /// Hueco para la página en píxeles físicos y escala de la pantalla, del último fotograma.
    hueco_px: egui::Vec2,
    ppp: f32,
    /// Dónde estaba el ratón en la última muesca de zoom y qué punto de la página se
    /// está llevando al centro (de -0,5 a 0,5). Mientras el ratón no se mueva, las
    /// siguientes muescas siguen acercándose a ese mismo punto.
    ancla_zoom: Option<(egui::Vec2, egui::Vec2)>,
    /// Caja del último fotograma, para mostrar el porcentaje de zoom en la barra.
    caja: Tamano,
}

impl Lector {
    /// Prepara el lector para `comic` y empieza en la página `pagina`.
    pub fn nuevo(ctx: &egui::Context, comic: Comic, ruta: &Path, pagina: usize) -> Lector {
        let total = comic.paginas().len();
        // Uno por núcleo, dejando uno libre para la ventana.
        let nucleos = std::thread::available_parallelism().map_or(1, |n| n.get());
        let hilos = nucleos.saturating_sub(1).clamp(1, MAX_HILOS);
        let precargador = Precargador::nuevo(comic, ruta, hilos, ctx.clone());

        let mut lector = Lector {
            total,
            actual: pagina.min(total.saturating_sub(1)),
            volver: false,
            precargador,
            texturas: HashMap::new(),
            pedidas: HashMap::new(),
            errores: HashMap::new(),
            pantalla_completa: false,
            oscuro: ctx.theme() == egui::Theme::Dark,
            ajuste: Ajuste::Pagina,
            zoom: 1.0,
            desplazamiento: egui::Vec2::ZERO,
            zoom_objetivo: 1.0,
            desplazamiento_objetivo: egui::Vec2::ZERO,
            hueco_px: egui::Vec2::ZERO,
            ppp: 1.0,
            ancla_zoom: None,
            caja: [0, 0],
        };
        lector.ir_arriba();
        lector
    }

    /// Página que se está viendo, empezando en 0.
    pub fn actual(&self) -> usize {
        self.actual
    }

    pub fn total(&self) -> usize {
        self.total
    }

    fn siguiente(&mut self) {
        if self.actual + 1 < self.total {
            self.actual += 1;
            self.ir_arriba();
            eprintln!("[ui] Página siguiente: {}", self.actual + 1);
        }
    }

    fn anterior(&mut self) {
        if self.actual > 0 {
            self.actual -= 1;
            self.ir_arriba();
            eprintln!("[ui] Página anterior: {}", self.actual + 1);
        }
    }

    /// Coloca la vista en la parte de arriba de la página, centrada a lo ancho, sin animar.
    fn ir_arriba(&mut self) {
        self.desplazamiento_objetivo = egui::vec2(0.0, f32::INFINITY);
        self.desplazamiento = self.desplazamiento_objetivo;
        self.ancla_zoom = None;
    }

    fn cambiar_ajuste(&mut self, ajuste: Ajuste) {
        self.ajuste = ajuste;
        self.zoom = 1.0;
        self.zoom_objetivo = 1.0;
        self.ir_arriba();
    }

    /// Multiplica el zoom pedido por `factor`.
    ///
    /// Con `foco` (un punto de la pantalla medido desde el centro del hueco, normalmente
    /// el ratón), el punto de la página que hay debajo acaba en el centro de la vista.
    /// Sin él (teclado y botones), se queda en el centro lo que ya estaba en el centro.
    fn zoom_hacia(&mut self, factor: f32, foco: Option<egui::Vec2>) {
        let nuevo = (self.zoom_objetivo * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        let original = self.texturas.get(&self.actual).map(|c| c.original);

        match (foco, original) {
            (Some(foco), Some(original)) if self.desplazamiento.is_finite() => {
                // Punto de la página que irá al centro, de -0,5 a 0,5 desde su centro.
                // Si el ratón no se ha movido desde la muesca anterior, es el mismo de antes:
                // la página ya se está desplazando y lo que hay ahora bajo el cursor es otra cosa.
                let punto = match self.ancla_zoom {
                    Some((foco_anterior, punto)) if (foco - foco_anterior).length() < 2.0 => punto,
                    _ => {
                        (foco - self.desplazamiento) / self.tamano_en_pantalla(original, self.zoom)
                    }
                };
                self.ancla_zoom = Some((foco, punto));
                self.desplazamiento_objetivo = -punto * self.tamano_en_pantalla(original, nuevo);
            }
            _ if self.desplazamiento_objetivo.is_finite() => {
                self.desplazamiento_objetivo *= nuevo / self.zoom_objetivo;
            }
            _ => {}
        }
        self.zoom_objetivo = nuevo;
    }

    /// Acerca el zoom y la posición que se ven a los pedidos. Devuelve si aún falta camino.
    fn animar(&mut self, dt: f32) -> bool {
        if !self.desplazamiento.is_finite() || !self.desplazamiento_objetivo.is_finite() {
            self.desplazamiento = self.desplazamiento_objetivo;
        }
        // Cada fotograma recorre la misma fracción del camino que queda: empieza rápido y
        // frena al llegar. El zoom se interpola en logaritmos para que acercar de x1 a x2
        // se sienta igual de rápido que de x4 a x8.
        let avance = 1.0 - (-dt * VELOCIDAD_ANIMACION).exp();
        let (desde, hasta) = (self.zoom.ln(), self.zoom_objetivo.ln());
        self.zoom = (desde + (hasta - desde) * avance).exp();
        self.desplazamiento += (self.desplazamiento_objetivo - self.desplazamiento) * avance;

        let cerca = (self.zoom / self.zoom_objetivo - 1.0).abs() < 1e-3
            && (self.desplazamiento_objetivo - self.desplazamiento).length() < 0.25;
        if cerca {
            // Exactamente en el objetivo: así la página acaba dibujándose 1:1, nítida.
            self.zoom = self.zoom_objetivo;
            self.desplazamiento = self.desplazamiento_objetivo;
        }
        !cerca
    }

    fn cambiar_tema(&mut self, ctx: &egui::Context) {
        self.oscuro = !self.oscuro;
        ctx.set_theme(if self.oscuro {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
    }

    /// Caja en la que tiene que caber la página, en píxeles físicos, según el hueco
    /// disponible, el ajuste y el zoom. Ajustar al ancho es una caja sin límite de alto.
    fn caja_para(&self, zoom: f32) -> Tamano {
        let ancho = (self.hueco_px.x * zoom) as u32;
        let alto = match self.ajuste {
            Ajuste::Pagina => (self.hueco_px.y * zoom) as u32,
            Ajuste::Ancho => u32::MAX,
        };
        [ancho, alto]
    }

    /// Tamaño en puntos con el que se dibuja una página con ese zoom.
    fn tamano_en_pantalla(&self, original: Tamano, zoom: f32) -> egui::Vec2 {
        let [ancho, alto] = precarga::encajar(original, self.caja_para(zoom));
        egui::vec2(ancho as f32, alto as f32) / self.ppp
    }

    /// Zoom respecto al tamaño original de la página actual, para la barra.
    fn porcentaje(&self) -> Option<f32> {
        let cargada = self.texturas.get(&self.actual)?;
        let [ancho, _] = precarga::encajar(cargada.original, self.caja);
        Some(ancho as f32 / cargada.original[0] as f32 * 100.0)
    }

    /// Rango de páginas que queremos tener preparadas alrededor de la actual.
    fn ventana(&self) -> std::ops::RangeInclusive<usize> {
        precarga::ventana(self.actual, self.total)
    }

    /// Recoge lo que hayan terminado los hilos, pide lo que falta y libera lo que queda lejos.
    /// `caja` es donde tiene que caber la página, en píxeles físicos.
    fn gestionar_precarga(&mut self, ctx: &egui::Context, caja: Tamano) {
        // Lo primero, para que los hilos dejen de trabajar en lo que ya no toca.
        self.precargador.mover_a(self.actual, caja);

        let listas: Vec<_> = self.precargador.listas().collect();
        for respuesta in listas {
            match respuesta {
                Respuesta::Lista(pagina) => self.recibir(ctx, pagina, caja),
                Respuesta::Error(indice, e) => {
                    self.pedidas.remove(&indice);
                    self.errores.insert(indice, e);
                }
                // Si sigue haciendo falta, se volverá a pedir más abajo.
                Respuesta::Descartada(indice, caja_pedida) => {
                    if self.pedidas.get(&indice) == Some(&caja_pedida) {
                        self.pedidas.remove(&indice);
                    }
                }
            }
        }

        let ventana = self.ventana();

        // Con la ventana minimizada no hay hueco: no pedimos nada.
        if caja[0] > 0 && caja[1] > 0 {
            // La actual primero, para que sea la que antes llega.
            let orden = std::iter::once(self.actual).chain(ventana.clone());
            let mut pedidas_nuevas = 0;
            for indice in orden {
                let lista = self
                    .texturas
                    .get(&indice)
                    .is_some_and(|c| c.sirve_para(caja));
                let falta = !lista
                    && self.pedidas.get(&indice) != Some(&caja)
                    && !self.errores.contains_key(&indice);
                if falta {
                    self.pedidas.insert(indice, caja);
                    self.precargador.pedir(indice, caja);
                    pedidas_nuevas += 1;
                }
            }
            if pedidas_nuevas > 0 {
                let alto = match caja[1] {
                    u32::MAX => "sin límite".to_string(),
                    alto => alto.to_string(),
                };
                eprintln!(
                    "[ui] Se pidieron {pedidas_nuevas} páginas (ventana: {ventana:?}, caja: {}x{alto})",
                    caja[0]
                );
            }
        }

        // Soltamos las páginas que quedan lejos para no acumular memoria de GPU.
        self.texturas.retain(|indice, _| ventana.contains(indice));
    }

    /// Sube a la GPU una página que acaba de llegar de los hilos.
    fn recibir(&mut self, ctx: &egui::Context, pagina: Pagina, caja: Tamano) {
        let Pagina {
            indice,
            caja: caja_pedida,
            original,
            imagen,
        } = pagina;

        if self.pedidas.get(&indice) == Some(&caja_pedida) {
            self.pedidas.remove(&indice);
        }
        // Una respuesta vieja no debe pisar una textura que ya tiene el tamaño bueno.
        let ya_buena = self
            .texturas
            .get(&indice)
            .is_some_and(|c| c.sirve_para(caja));
        let [ancho, alto] = precarga::tamano_textura(original, caja);
        if ya_buena && imagen.size != [ancho as usize, alto as usize] {
            return;
        }

        let rol = if indice == self.actual {
            "actual"
        } else {
            "precarga"
        };
        eprintln!("[ui] Página {indice} subida a GPU ({rol})");
        // Normalmente se dibuja 1:1 y el filtro no interviene. Se nota al pasar del 100%
        // del original (suaviza en vez de pixelar) y un instante tras cambiar el zoom.
        let textura = ctx.load_texture(
            format!("pagina-{indice}"),
            imagen,
            egui::TextureOptions::LINEAR,
        );
        self.texturas.insert(indice, Cargada { original, textura });
    }

    fn atajos_de_teclado(&mut self, ctx: &egui::Context) {
        let pulsada = |tecla| ctx.input(|i| i.key_pressed(tecla));

        if pulsada(egui::Key::ArrowRight) {
            self.siguiente();
        }
        if pulsada(egui::Key::ArrowLeft) {
            self.anterior();
        }
        if pulsada(egui::Key::ArrowUp) {
            self.desplazamiento_objetivo.y += PASO_DESPLAZAMIENTO;
        }
        if pulsada(egui::Key::ArrowDown) {
            self.desplazamiento_objetivo.y -= PASO_DESPLAZAMIENTO;
        }
        if pulsada(egui::Key::Plus) || pulsada(egui::Key::Equals) {
            self.zoom_hacia(PASO_ZOOM, None);
        }
        if pulsada(egui::Key::Minus) {
            self.zoom_hacia(1.0 / PASO_ZOOM, None);
        }
        if pulsada(egui::Key::Num0) {
            self.cambiar_ajuste(Ajuste::Pagina);
        }
        if pulsada(egui::Key::W) {
            self.cambiar_ajuste(Ajuste::Ancho);
        }
        if pulsada(egui::Key::T) {
            self.cambiar_tema(ctx);
        }
        if pulsada(egui::Key::F11) {
            self.pantalla_completa = !self.pantalla_completa;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.pantalla_completa));
        }
        // Esc primero sale de pantalla completa; si no lo está, vuelve a la biblioteca.
        if pulsada(egui::Key::Escape) {
            if self.pantalla_completa {
                self.pantalla_completa = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            } else {
                self.volver = true;
            }
        }
    }

    fn barra(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("Biblioteca")
                .on_hover_text("Volver a la biblioteca (Esc)")
                .clicked()
            {
                self.volver = true;
            }

            ui.separator();
            if ui
                .button("⬅")
                .on_hover_text("Página anterior (←)")
                .clicked()
            {
                self.anterior();
            }
            ui.label(format!("{} / {}", self.actual + 1, self.total));
            if ui
                .button("➡")
                .on_hover_text("Página siguiente (→)")
                .clicked()
            {
                self.siguiente();
            }

            ui.separator();
            let pagina = ui
                .selectable_label(self.ajuste == Ajuste::Pagina, "Página")
                .on_hover_text("Ajustar la página entera (0)");
            if pagina.clicked() {
                self.cambiar_ajuste(Ajuste::Pagina);
            }
            let ancho = ui
                .selectable_label(self.ajuste == Ajuste::Ancho, "Ancho")
                .on_hover_text("Ajustar al ancho (W)");
            if ancho.clicked() {
                self.cambiar_ajuste(Ajuste::Ancho);
            }

            ui.separator();
            if ui.button("−").on_hover_text("Alejar (-)").clicked() {
                self.zoom_hacia(1.0 / PASO_ZOOM, None);
            }
            let porcentaje = self.porcentaje().map_or("—".into(), |p| format!("{p:.0}%"));
            ui.label(porcentaje)
                .on_hover_text("Tamaño respecto al original. Rueda del ratón para hacer zoom");
            if ui.button("+").on_hover_text("Acercar (+)").clicked() {
                self.zoom_hacia(PASO_ZOOM, None);
            }

            ui.separator();
            let tema = if self.oscuro { "Claro" } else { "Oscuro" };
            if ui
                .button(tema)
                .on_hover_text("Cambiar el tema (T)")
                .clicked()
            {
                self.cambiar_tema(ui.ctx());
            }
            ui.label("F11: pantalla completa");
        });
    }

    /// Dibuja la página actual y atiende el ratón sobre ella: arrastrar mueve la página
    /// y la rueda hace zoom hacia el punto donde está el cursor.
    fn vista(&mut self, ui: &mut egui::Ui, hueco: egui::Rect) {
        let respuesta = ui.interact(hueco, ui.id().with("pagina"), egui::Sense::drag());
        if respuesta.dragged() {
            // Arrastrar tiene que seguir a la mano sin retraso: movemos lo que se ve y el objetivo.
            self.desplazamiento += respuesta.drag_delta();
            self.desplazamiento_objetivo += respuesta.drag_delta();
            self.ancla_zoom = None;
        }
        if respuesta.hovered() {
            let (muescas, raton) = ui.input(|i| (muescas_de_rueda(i), i.pointer.hover_pos()));
            if muescas != 0.0 {
                let foco = raton.map(|p| p - hueco.center());
                self.zoom_hacia(PASO_ZOOM.powf(muescas), foco);
            }
        }

        if self.total == 0 {
            ui.label("Este cómic no tiene páginas.");
        } else if let Some(cargada) = self.texturas.get(&self.actual) {
            let original = cargada.original;
            let destino = self.tamano_en_pantalla(original, self.zoom_objetivo);

            // Lo que la página sobresale del hueco por cada lado: hasta ahí se puede mover
            // normalmente. "Arriba del todo" (infinito) se resuelve con este límite.
            let holgura = ((destino - hueco.size()) / 2.0).max(egui::Vec2::ZERO);
            let mut objetivo = self.desplazamiento_objetivo;
            if !objetivo.is_finite() {
                objetivo = objetivo.clamp(-holgura, holgura);
            }
            // Con zoom, la página se puede mover en cada eje mientras no deje hueco negro
            // (si es más grande que la pantalla) ni se salga de ella (si es más pequeña).
            // Así lo que señala el ratón va al centro siempre que los bordes lo permiten.
            let limite = if self.zoom_objetivo > 1.0 {
                ((destino - hueco.size()) / 2.0).abs()
            } else {
                holgura
            };
            self.desplazamiento_objetivo = objetivo.clamp(-limite, limite);

            if self.animar(ui.input(|i| i.stable_dt).min(0.1)) {
                ui.ctx().request_repaint();
            }

            if limite != egui::Vec2::ZERO && respuesta.hovered() {
                let icono = if respuesta.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                };
                ui.ctx().set_cursor_icon(icono);
            }

            let tamano = self.tamano_en_pantalla(original, self.zoom);
            let cargada = &self.texturas[&self.actual];
            dibujar_pagina(ui, hueco, &cargada.textura, tamano, self.desplazamiento);
        } else if let Some(error) = self.errores.get(&self.actual) {
            ui.label(format!("No se pudo leer la página: {error}"));
        } else {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
        }
    }
}

/// Cuántas muescas ha girado la rueda en este fotograma: positivo hacia delante (acercar).
/// Se leen los eventos crudos y no `smooth_scroll_delta`, porque el suavizado lo hace la
/// animación del zoom. Ctrl + rueda y el pellizco de un panel táctil llegan igual.
fn muescas_de_rueda(entrada: &egui::InputState) -> f32 {
    entrada
        .events
        .iter()
        .map(|evento| match evento {
            egui::Event::MouseWheel { unit, delta, .. } => match unit {
                egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => delta.y,
                egui::MouseWheelUnit::Point => delta.y / PUNTOS_POR_MUESCA,
            },
            _ => 0.0,
        })
        .sum()
}

/// Dibuja la página a `tamano` (en puntos), centrada en `hueco` y movida `desplazamiento`.
///
/// La esquina se alinea con los píxeles de la pantalla: cuando la textura mide justo
/// lo que se dibuja, la GPU no la vuelve a filtrar y se ve tan nítida como salió del hilo.
fn dibujar_pagina(
    ui: &egui::Ui,
    hueco: egui::Rect,
    textura: &egui::TextureHandle,
    tamano: egui::Vec2,
    desplazamiento: egui::Vec2,
) {
    let ppp = ui.ctx().pixels_per_point();
    let colocada = egui::Rect::from_center_size(hueco.center() + desplazamiento, tamano);
    let esquina = (colocada.min.to_vec2() * ppp).round() / ppp;
    let rect = egui::Rect::from_min_size(esquina.to_pos2(), tamano);

    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    // Recortado al hueco, para que una página con zoom no tape la barra.
    ui.painter_at(hueco)
        .image(textura.id(), rect, uv, egui::Color32::WHITE);
}

impl Lector {
    /// Dibuja el lector. Devuelve `true` cuando el usuario quiere volver a la biblioteca.
    pub fn mostrar(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        self.atajos_de_teclado(&ctx);

        egui::Panel::bottom("barra").show(ui, |ui| self.barra(ui));

        // Fondo casi negro (o blanco en el tema claro) y sin márgenes: todo el sitio para la página.
        let fondo = egui::Frame::central_panel(&ctx.global_style())
            .fill(ctx.global_style().visuals.extreme_bg_color)
            .inner_margin(0.0);
        egui::CentralPanel::default().frame(fondo).show(ui, |ui| {
            // Hasta aquí no sabemos cuánto sitio queda para la página (la barra va antes).
            let hueco = ui.available_rect_before_wrap();
            self.ppp = ctx.pixels_per_point();
            self.hueco_px = (hueco.size() * self.ppp).floor();
            // Se pide el tamaño del zoom final, no el del paso de la animación.
            let caja = self.caja_para(self.zoom_objetivo);
            self.caja = caja;
            self.gestionar_precarga(&ctx, caja);
            self.vista(ui, hueco);
        });

        std::mem::take(&mut self.volver)
    }
}
