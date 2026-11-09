//! El lector: muestra las páginas de un cómic con zoom, desplazamiento y precarga.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use comic_core::Comic;
use eframe::egui;

use crate::controles::{Accion, Controles};
use crate::estilo;
use crate::filtro::{self, Filtro};
use crate::pasar_hoja::{self, Giro};
use crate::precarga::{self, Pagina, Precargador, Respuesta, Tamano, ATRAS, DELANTE};
use crate::preferencias::Animacion;
use crate::selector::Selector;
use crate::vista_biblioteca::{boton_subrayado, boton_texto};

/// Más hilos que páginas en la ventana no sirven de nada: estarían parados.
const MAX_HILOS: usize = ATRAS + 1 + DELANTE;

/// Límites del zoom, relativos al ajuste elegido (1 = la página ajustada justo al hueco).
const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 8.0;
/// Cuánto cambia el zoom con cada pulsación de + o - y con cada muesca de la rueda.
const PASO_ZOOM: f32 = 1.25;
const PASO_ZOOM_RUEDA: f32 = 1.12;
/// Cuánto se desplaza la página con cada pulsación de las flechas arriba/abajo, en puntos.
const PASO_DESPLAZAMIENTO: f32 = 100.0;
/// Puntos que egui cuenta por cada muesca de la rueda en escritorio. Sirve para convertir
/// el desplazamiento continuo de un panel táctil en "muescas".
const PUNTOS_POR_MUESCA: f32 = 40.0;
/// Rapidez con la que la vista alcanza el zoom y la posición pedidos. Con 15, recorre
/// el 95% del camino en unos 200 ms. Más alto = más rápido.
const VELOCIDAD_ANIMACION: f32 = 15.0;
/// Al pasar a la página de al lado, la hoja gira como en papel: cuánto tarda, en segundos.
const DURACION_HOJA: f32 = 0.6;
/// Al saltar lejos (ya no queda la página de antes para hacerla girar), la nueva entra
/// deslizándose desde el lado del que viene mientras aparece: cuánto dura y desde
/// cuántos puntos de distancia.
const DURACION_TRANSICION: f32 = 0.18;
const DISTANCIA_TRANSICION: f32 = 60.0;

/// A qué se ajusta la página cuando el zoom es 1.
#[derive(Clone, Copy, PartialEq)]
enum Ajuste {
    /// La página entera cabe en el hueco.
    Pagina,
    /// La página ocupa todo el ancho y se lee bajando.
    Ancho,
    /// Se lee recuadro a recuadro: la vista se acerca a cada uno y tapa el resto. Pasar
    /// adelante va al siguiente recuadro y, tras el último, a la página siguiente. Con
    /// zoom 1 es como `Pagina`: el zoom de cada recuadro se mide desde la página entera.
    Vinetas,
}

/// Cuánto del hueco ocupa un recuadro al acercarse a él: el resto queda de margen.
const OCUPACION_VINETA: f32 = 0.94;
/// Lo que se deja ver alrededor de un recuadro, en puntos: así no se corta su marco.
const MARGEN_VINETA: f32 = 4.0;

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

/// La entrada de una página nueva. Empieza a contar cuando la página se dibuja por
/// primera vez: si aún se estaba preparando, la animación no se pierde esperándola.
struct Transicion {
    pagina: usize,
    /// 1 si viene de la derecha (hacia delante), -1 si de la izquierda.
    direccion: f32,
    inicio: Option<f64>,
    /// La página que se estaba viendo y dónde estaba, para hacerla girar.
    anterior: Option<(usize, egui::Rect)>,
}

/// Cómo se dibuja la página en este fotograma.
enum Efecto {
    Ninguno,
    Deslizar {
        lado: f32,
        opacidad: f32,
    },
    Hoja {
        progreso: f32,
        anterior: (usize, egui::Rect),
        adelante: bool,
    },
}

/// Un cómic abierto: en qué página estamos y cómo se ve.
pub struct Lector {
    total: usize,
    actual: usize,
    /// Se ha pedido volver a la biblioteca (Esc o el botón).
    volver: bool,
    /// La barra flotante de pantalla completa: hasta cuándo se ve y dónde está.
    barra_visible_hasta: Option<Instant>,
    barra_rect: egui::Rect,
    precargador: Precargador,
    /// Páginas ya subidas a la GPU.
    texturas: HashMap<usize, Cargada>,
    /// Páginas pedidas a los hilos cuya respuesta aún no ha llegado, y con qué caja.
    pedidas: HashMap<usize, Tamano>,
    errores: HashMap<usize, String>,
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
    transicion: Option<Transicion>,
    /// Cómo se pasa de página. Se puede cambiar desde la barra.
    animacion: Animacion,
    /// Tono cálido y brillo de las páginas. Se puede cambiar desde la barra.
    filtro: Filtro,
    /// Qué página se dibujó en el último fotograma y dónde.
    ultima_dibujada: Option<(usize, egui::Rect)>,
    selector: Selector,
    /// Qué tecla hace cada cosa y qué hace el ratón sobre la página.
    controles: Controles,
    /// Cuándo se hizo un clic en la página que aún no ha pasado de página: se espera por
    /// si es el primero de un doble clic (pantalla completa).
    clic_pendiente: Option<Instant>,
    /// Cómo se llama el cómic que sigue a este en su serie, si lo hay.
    siguiente: Option<String>,
    /// Se está ofreciendo pasar al siguiente cómic (se pidió avanzar en la última página).
    fin_abierto: bool,
    /// Se ha elegido pasar al siguiente cómic; la aplicación lo recoge y lo abre.
    pedir_siguiente: bool,
    /// Los recuadros de las páginas ya buscadas, en orden de lectura y en fracciones de
    /// la página (de 0 a 1). Vacío si no se encontró ninguno: se lee la página entera.
    /// Ocupan poco, así que no se sueltan al alejarse de la página.
    recuadros: HashMap<usize, Vec<egui::Rect>>,
    /// Leyendo por viñetas, qué recuadro de la página actual se está viendo.
    vineta: usize,
    /// Al volver a la página anterior se empieza por su último recuadro, que puede no
    /// conocerse todavía.
    vineta_al_final: bool,
    /// El último encuadre hecho: página, recuadro (ninguno = página entera) y hueco. Si
    /// cambia algo de eso, se vuelve a encuadrar.
    encuadre: Option<(usize, Option<usize>, [u32; 2])>,
    /// La parte de la página que se ve sin tapar, en fracciones de la página. Persigue a
    /// `marco_objetivo` con la misma animación que el zoom.
    marco_visto: Option<egui::Rect>,
    marco_objetivo: Option<egui::Rect>,
}

impl Lector {
    /// Prepara el lector para `comic` y empieza en la página `pagina`.
    pub fn nuevo(
        ctx: &egui::Context,
        comic: Comic,
        ruta: &Path,
        pagina: usize,
        animacion: Animacion,
        filtro: Filtro,
        controles: Controles,
    ) -> Lector {
        let total = comic.paginas().len();
        // Uno por núcleo, dejando uno libre para la ventana.
        let nucleos = std::thread::available_parallelism().map_or(1, |n| n.get());
        let hilos = nucleos.saturating_sub(1).clamp(1, MAX_HILOS);
        let precargador = Precargador::nuevo(comic, ruta, hilos, ctx.clone());

        let mut lector = Lector {
            total,
            actual: pagina.min(total.saturating_sub(1)),
            volver: false,
            barra_visible_hasta: None,
            barra_rect: egui::Rect::NOTHING,
            precargador,
            texturas: HashMap::new(),
            pedidas: HashMap::new(),
            errores: HashMap::new(),
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
            transicion: None,
            animacion,
            filtro,
            ultima_dibujada: None,
            selector: Selector::nuevo(ruta, total),
            controles,
            clic_pendiente: None,
            siguiente: None,
            fin_abierto: false,
            pedir_siguiente: false,
            recuadros: HashMap::new(),
            vineta: 0,
            vineta_al_final: false,
            encuadre: None,
            marco_visto: None,
            marco_objetivo: None,
        };
        lector.ir_arriba();
        lector
    }

    /// Dice cómo se llama el cómic que viene después de este, o que no hay ninguno.
    pub fn poner_siguiente(&mut self, titulo: Option<String>) {
        self.siguiente = titulo;
    }

    /// Si el usuario ha elegido pasar al siguiente cómic desde la última vez que se preguntó.
    pub fn pide_siguiente(&mut self) -> bool {
        std::mem::take(&mut self.pedir_siguiente)
    }

    /// La página que el usuario ha elegido como portada desde la última vez que se preguntó.
    pub fn portada_elegida(&mut self) -> Option<usize> {
        self.selector.portada_elegida()
    }

    /// Para el modo de captura: leer por viñetas desde el recuadro `vineta` (desde 0).
    pub(crate) fn vinetas_para_captura(&mut self, vineta: usize) {
        self.cambiar_ajuste(Ajuste::Vinetas);
        self.vineta = vineta;
    }

    /// Página que se está viendo, empezando en 0.
    pub fn actual(&self) -> usize {
        self.actual
    }

    pub fn total(&self) -> usize {
        self.total
    }

    /// La animación elegida, para recordarla para el próximo cómic.
    pub fn animacion(&self) -> Animacion {
        self.animacion
    }

    /// El filtro elegido, para recordarlo para el próximo cómic.
    pub fn filtro(&self) -> Filtro {
        self.filtro
    }

    /// Pasa a la página siguiente. En la última, si hay otro cómic después, lo ofrece.
    /// Leyendo por viñetas, antes recorre los recuadros de la página.
    fn siguiente(&mut self) {
        if self.ajuste == Ajuste::Vinetas {
            let cuantos = self.recuadros.get(&self.actual).map_or(0, Vec::len);
            if self.vineta + 1 < cuantos {
                self.vineta += 1;
                return;
            }
        }
        if self.actual + 1 < self.total {
            self.ir_a(self.actual + 1);
        } else if self.total > 0 && self.siguiente.is_some() {
            self.fin_abierto = true;
            eprintln!("[ui] Fin del cómic: se ofrece el siguiente");
        }
    }

    fn anterior(&mut self) {
        if self.ajuste == Ajuste::Vinetas && self.vineta > 0 {
            self.vineta -= 1;
            return;
        }
        if self.actual > 0 {
            self.ir_a(self.actual - 1);
            self.vineta_al_final = self.ajuste == Ajuste::Vinetas;
        }
    }

    /// Salta a otra página, con la animación de entrada.
    fn ir_a(&mut self, pagina: usize) {
        if pagina == self.actual || pagina >= self.total {
            return;
        }
        let direccion = if pagina > self.actual { 1.0 } else { -1.0 };
        // La hoja que gira es la que se estaba viendo; deslizando no hace falta.
        let anterior = self
            .ultima_dibujada
            .filter(|(indice, _)| *indice == self.actual && self.animacion == Animacion::Hoja);
        self.actual = pagina;
        self.vineta = 0;
        self.vineta_al_final = false;
        self.ir_arriba();
        // Leyendo por viñetas la página no se ve entera: el giro o el deslizamiento de la
        // página quedarían raros. Ahí lo que se anima es el paso de un recuadro a otro.
        let animar = self.animacion != Animacion::Ninguna && self.ajuste != Ajuste::Vinetas;
        self.transicion = animar.then_some(Transicion {
            pagina,
            direccion,
            inicio: None,
            anterior,
        });
        eprintln!("[ui] Página {}", self.actual + 1);
    }

    /// Cómo va la animación de cambio de página en este fotograma.
    fn efecto_transicion(&mut self, ctx: &egui::Context) -> Efecto {
        let Some(transicion) = &mut self.transicion else {
            return Efecto::Ninguno;
        };
        if transicion.pagina != self.actual {
            self.transicion = None;
            return Efecto::Ninguno;
        }
        // La hoja solo puede girar si la página de antes sigue en la GPU: al saltar lejos
        // ya se ha soltado.
        let anterior = transicion
            .anterior
            .filter(|(indice, _)| self.texturas.contains_key(indice));
        let ahora = ctx.input(|i| i.time);
        let inicio = *transicion.inicio.get_or_insert(ahora);
        let duracion = if anterior.is_some() {
            DURACION_HOJA
        } else {
            DURACION_TRANSICION
        };
        let avance = ((ahora - inicio) as f32 / duracion).min(1.0);
        if avance >= 1.0 {
            self.transicion = None;
            return Efecto::Ninguno;
        }
        ctx.request_repaint();
        match anterior {
            Some(anterior) => Efecto::Hoja {
                progreso: avance,
                anterior,
                adelante: transicion.direccion > 0.0,
            },
            None => {
                // Rápida al principio y frenando al final.
                let suave = 1.0 - (1.0 - avance).powi(3);
                Efecto::Deslizar {
                    lado: transicion.direccion * DISTANCIA_TRANSICION * (1.0 - suave),
                    opacidad: suave,
                }
            }
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
        self.vineta = 0;
        self.vineta_al_final = false;
        self.encuadre = None;
        self.marco_visto = None;
        self.marco_objetivo = None;
    }

    /// Leyendo por viñetas, coloca la vista sobre el recuadro que toca: zoom para que
    /// llene el hueco y desplazamiento para que quede en el centro. Solo cuando cambia
    /// la página, el recuadro o el hueco, así entre medias se puede mover o hacer zoom a
    /// mano. Al cambiar de recuadro se anima; al llegar a otra página, se salta directo.
    fn encuadrar(&mut self, original: Tamano, hueco: egui::Vec2) {
        let recuadro = match self.recuadros.get(&self.actual) {
            Some(lista) if !lista.is_empty() => {
                if self.vineta_al_final {
                    self.vineta = lista.len() - 1;
                    self.vineta_al_final = false;
                }
                self.vineta = self.vineta.min(lista.len() - 1);
                Some((self.vineta, lista[self.vineta]))
            }
            _ => None,
        };
        let px = [self.hueco_px.x as u32, self.hueco_px.y as u32];
        let clave = (self.actual, recuadro.map(|(i, _)| i), px);
        if self.encuadre == Some(clave) {
            return;
        }
        let otra_pagina = self
            .encuadre
            .is_none_or(|(pagina, _, _)| pagina != self.actual);
        self.encuadre = Some(clave);

        let entera = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        let (zoom, marco) = match recuadro {
            Some((_, marco)) => {
                let base = self.tamano_en_pantalla(original, 1.0);
                let zoom = (hueco.x * OCUPACION_VINETA / (marco.width() * base.x))
                    .min(hueco.y * OCUPACION_VINETA / (marco.height() * base.y));
                // Nunca más pequeño que la página entera.
                (zoom.clamp(1.0, ZOOM_MAX), marco)
            }
            // Mientras se buscan sus recuadros, o si no tiene, se ve la página entera.
            None => (1.0, entera),
        };
        let tamano = self.tamano_en_pantalla(original, zoom);
        self.zoom_objetivo = zoom;
        self.desplazamiento_objetivo = -(marco.center() - egui::pos2(0.5, 0.5)) * tamano;
        self.marco_objetivo = Some(marco);
        self.ancla_zoom = None;
        if otra_pagina {
            self.zoom = zoom;
            self.desplazamiento = self.desplazamiento_objetivo;
            self.marco_visto = Some(marco);
        }
        match recuadro {
            Some((i, _)) => eprintln!("[ui] Viñeta {} de la página {}", i + 1, self.actual + 1),
            None => eprintln!("[ui] Página {} entera (sin recuadros)", self.actual + 1),
        }
    }

    /// Tapa con el color de fondo todo lo que queda fuera del recuadro que se lee.
    /// `pagina` es dónde está dibujada la página.
    fn tapar_fuera_del_recuadro(
        &self,
        pintor: &egui::Painter,
        hueco: egui::Rect,
        pagina: egui::Rect,
    ) {
        let Some(marco) = self.marco_visto else {
            return;
        };
        let visible = egui::Rect::from_min_size(
            pagina.min + marco.min.to_vec2() * pagina.size(),
            marco.size() * pagina.size(),
        )
        .expand(MARGEN_VINETA);
        let color = estilo::colores(pintor.ctx()).lector * self.filtro.tinte();
        let (izquierda, derecha) = (
            visible.left().max(hueco.left()),
            visible.right().min(hueco.right()),
        );
        let tapas = [
            egui::Rect::from_min_max(hueco.min, egui::pos2(hueco.right(), visible.top())),
            egui::Rect::from_min_max(egui::pos2(hueco.left(), visible.bottom()), hueco.max),
            egui::Rect::from_min_max(
                egui::pos2(hueco.left(), visible.top()),
                egui::pos2(izquierda, visible.bottom()),
            ),
            egui::Rect::from_min_max(
                egui::pos2(derecha, visible.top()),
                egui::pos2(hueco.right(), visible.bottom()),
            ),
        ];
        for tapa in tapas {
            if tapa.is_positive() {
                pintor.rect_filled(tapa, 0.0, color);
            }
        }
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
        let mut marco_cerca = true;
        if let (Some(visto), Some(objetivo)) = (self.marco_visto, self.marco_objetivo) {
            let mover = |a: egui::Pos2, b: egui::Pos2| a + (b - a) * avance;
            let nuevo = egui::Rect::from_min_max(
                mover(visto.min, objetivo.min),
                mover(visto.max, objetivo.max),
            );
            marco_cerca =
                (nuevo.min - objetivo.min).length() + (nuevo.max - objetivo.max).length() < 1e-4;
            self.marco_visto = Some(if marco_cerca { objetivo } else { nuevo });
        } else {
            self.marco_visto = self.marco_objetivo;
        }

        let cerca = (self.zoom / self.zoom_objetivo - 1.0).abs() < 1e-3
            && (self.desplazamiento_objetivo - self.desplazamiento).length() < 0.25;
        if cerca {
            // Exactamente en el objetivo: así la página acaba dibujándose 1:1, nítida.
            self.zoom = self.zoom_objetivo;
            self.desplazamiento = self.desplazamiento_objetivo;
        }
        !cerca || !marco_cerca
    }

    fn alternar_filtro(&mut self) {
        self.filtro.activo = !self.filtro.activo;
        let estado = if self.filtro.activo {
            "activado"
        } else {
            "desactivado"
        };
        eprintln!("[ui] Filtro de luz cálida {estado}");
    }

    /// Lo que hay dentro del menú "Luz cálida" de la barra.
    fn menu_filtro(&mut self, ui: &mut egui::Ui) {
        let mut activo = self.filtro.activo;
        if ui.checkbox(&mut activo, "Activado (N)").changed() {
            self.alternar_filtro();
        }
        ui.add_enabled_ui(self.filtro.activo, |ui| {
            ui.add(
                egui::Slider::new(
                    &mut self.filtro.temperatura,
                    filtro::TEMPERATURA_MIN..=filtro::TEMPERATURA_NEUTRA,
                )
                .step_by(100.0)
                .suffix(" K")
                .text("Temperatura"),
            )
            .on_hover_text("Menos kelvin = más cálido, con menos azul. 6500 K es el blanco normal");
            let mut porcentaje = self.filtro.brillo * 100.0;
            let brillo = ui.add(
                egui::Slider::new(&mut porcentaje, filtro::BRILLO_MIN * 100.0..=100.0)
                    .step_by(1.0)
                    .suffix(" %")
                    .text("Brillo"),
            );
            if brillo.changed() {
                self.filtro.brillo = porcentaje / 100.0;
            }
            brillo.on_hover_text(
                "Oscurece la página. Si el monitor aún puede bajar el brillo, es mejor \
                 bajarlo allí: oscurecer aquí no hace más negros los negros",
            );
            if ui.button("Valores por defecto").clicked() {
                self.filtro = Filtro {
                    activo: true,
                    ..Filtro::default()
                };
            }
        });
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
            Ajuste::Pagina | Ajuste::Vinetas => (self.hueco_px.y * zoom) as u32,
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
                // Leyendo por viñetas también hacen falta sus recuadros: si la textura ya
                // está pero ellos no (se acaba de cambiar de ajuste), se vuelve a pedir.
                let sin_recuadros =
                    self.ajuste == Ajuste::Vinetas && !self.recuadros.contains_key(&indice);
                let falta = (!lista || sin_recuadros)
                    && self.pedidas.get(&indice) != Some(&caja)
                    && !self.errores.contains_key(&indice);
                if falta {
                    self.pedidas.insert(indice, caja);
                    self.precargador.pedir(indice, caja, sin_recuadros);
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
            recuadros,
        } = pagina;

        if self.pedidas.get(&indice) == Some(&caja_pedida) {
            self.pedidas.remove(&indice);
        }
        if let Some(recuadros) = recuadros {
            self.recuadros.insert(indice, recuadros);
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
        // Con el aviso del siguiente cómic abierto, las teclas son solo suyas: seguir,
        // o quedarse en la última página.
        if self.fin_abierto {
            let seguir = ctx.input(|i| i.key_pressed(egui::Key::Enter))
                || self.controles.pulsada(ctx, Accion::Siguiente);
            if seguir {
                self.fin_abierto = false;
                self.pedir_siguiente = true;
            } else if self.controles.pulsada(ctx, Accion::Anterior)
                || self.controles.pulsada(ctx, Accion::Volver)
            {
                self.fin_abierto = false;
            }
            return;
        }
        let pulsada = |accion| self.controles.pulsada(ctx, accion);
        let [siguiente, anterior, subir, bajar, acercar, alejar, pagina, ancho, vinetas, paginas, luz, tema, completa, volver] =
            Accion::TODAS.map(pulsada);
        if siguiente {
            self.siguiente();
        }
        if anterior {
            self.anterior();
        }
        if subir {
            self.desplazamiento_objetivo.y += PASO_DESPLAZAMIENTO;
        }
        if bajar {
            self.desplazamiento_objetivo.y -= PASO_DESPLAZAMIENTO;
        }
        if acercar {
            self.zoom_hacia(PASO_ZOOM, None);
        }
        if alejar {
            self.zoom_hacia(1.0 / PASO_ZOOM, None);
        }
        if pagina {
            self.cambiar_ajuste(Ajuste::Pagina);
        }
        if ancho {
            self.cambiar_ajuste(Ajuste::Ancho);
        }
        if vinetas {
            self.cambiar_ajuste(Ajuste::Vinetas);
        }
        if tema {
            self.cambiar_tema(ctx);
        }
        if paginas {
            self.selector.alternar(ctx, self.actual);
        }
        if luz {
            self.alternar_filtro();
        }
        if completa {
            self.alternar_pantalla_completa(ctx);
        }
        // Primero cierra el selector, luego sale de pantalla completa y, si no hay nada
        // de eso, vuelve a la biblioteca.
        if volver {
            if self.selector.abierto() {
                self.selector.cerrar();
            } else if en_pantalla_completa(ctx) {
                poner_pantalla_completa(ctx, false);
            } else {
                self.volver = true;
            }
        }
    }

    fn alternar_pantalla_completa(&mut self, ctx: &egui::Context) {
        // La app arranca en pantalla completa: se mira el estado real de la ventana, no
        // una cuenta propia, para que el primer doble clic salga de ella.
        poner_pantalla_completa(ctx, !en_pantalla_completa(ctx));
    }

    /// El texto de ayuda de un botón, con sus teclas si tiene: `Página anterior (←)`.
    fn ayuda(&self, texto: &str, accion: Accion) -> String {
        let teclas = self.controles.texto(accion);
        if teclas.is_empty() {
            texto.to_string()
        } else {
            format!("{texto} ({teclas})")
        }
    }

    /// Un clic en la página pasa a la siguiente y un doble clic cambia la pantalla
    /// completa. Si el doble clic está activado, el clic espera lo que tarda un doble clic
    /// en llegar, para no pasar de página al hacerlo.
    fn atender_clics(&mut self, ctx: &egui::Context, respuesta: &egui::Response) {
        let doble = self.controles.doble_clic_pantalla_completa;
        if doble && respuesta.double_clicked() {
            self.clic_pendiente = None;
            self.alternar_pantalla_completa(ctx);
            return;
        }
        if self.controles.clic_pasa_pagina && respuesta.clicked() {
            if doble {
                self.clic_pendiente = Some(Instant::now());
            } else {
                self.siguiente();
            }
        }
        if let Some(cuando) = self.clic_pendiente {
            let espera =
                Duration::from_secs_f64(ctx.options(|o| o.input_options.max_double_click_delay));
            let pasado = cuando.elapsed();
            if pasado >= espera {
                self.clic_pendiente = None;
                self.siguiente();
            } else {
                ctx.request_repaint_after(espera - pasado);
            }
        }
    }

    fn barra(&mut self, ui: &mut egui::Ui) {
        let c = estilo::colores(ui.ctx());
        ui.horizontal(|ui| {
            estilo::sin_marco(ui, c);
            if ui
                .button("Biblioteca")
                .on_hover_text(self.ayuda("Volver a la biblioteca", Accion::Volver))
                .clicked()
            {
                self.volver = true;
            }

            ui.separator();
            if ui
                .button("←")
                .on_hover_text(self.ayuda("Página anterior", Accion::Anterior))
                .clicked()
            {
                self.anterior();
            }
            // Las cifras en monoespaciada: el contador no cambia de ancho al pasar página.
            let ancho_total = self.total.to_string().len();
            ui.label(
                egui::RichText::new(format!(
                    "{:>ancho_total$} / {}",
                    self.actual + 1,
                    self.total
                ))
                .monospace(),
            );
            if ui
                .button("→")
                .on_hover_text(self.ayuda("Página siguiente", Accion::Siguiente))
                .clicked()
            {
                self.siguiente();
            }
            let ayuda = format!(
                "{}, o deja el ratón en la esquina de abajo a la derecha",
                self.ayuda("Ver todas las páginas", Accion::Paginas)
            );
            let paginas = ui
                .selectable_label(self.selector.abierto(), "Páginas")
                .on_hover_text(ayuda);
            if paginas.clicked() {
                self.selector.alternar(ui.ctx(), self.actual);
            }

            ui.separator();
            let pagina = ui
                .selectable_label(self.ajuste == Ajuste::Pagina, "Página")
                .on_hover_text(self.ayuda("Ajustar la página entera", Accion::AjustarPagina));
            if pagina.clicked() {
                self.cambiar_ajuste(Ajuste::Pagina);
            }
            let ancho = ui
                .selectable_label(self.ajuste == Ajuste::Ancho, "Ancho")
                .on_hover_text(self.ayuda("Ajustar al ancho", Accion::AjustarAncho));
            if ancho.clicked() {
                self.cambiar_ajuste(Ajuste::Ancho);
            }
            let vinetas = ui
                .selectable_label(self.ajuste == Ajuste::Vinetas, "Viñetas")
                .on_hover_text(self.ayuda(
                    "Leer recuadro a recuadro, de derecha a izquierda",
                    Accion::AjustarVinetas,
                ));
            if vinetas.clicked() {
                self.cambiar_ajuste(Ajuste::Vinetas);
            }

            ui.separator();
            let ayuda = self.ayuda("Alejar", Accion::Alejar);
            if ui.button("−").on_hover_text(ayuda).clicked() {
                self.zoom_hacia(1.0 / PASO_ZOOM, None);
            }
            let porcentaje = self
                .porcentaje()
                .map_or("—".into(), |p| format!("{p:>3.0}%"));
            ui.label(egui::RichText::new(porcentaje).monospace())
                .on_hover_text("Tamaño respecto al original. Rueda del ratón para hacer zoom");
            let ayuda = self.ayuda("Acercar", Accion::Acercar);
            if ui.button("+").on_hover_text(ayuda).clicked() {
                self.zoom_hacia(PASO_ZOOM, None);
            }

            ui.separator();
            egui::ComboBox::from_id_salt("animacion")
                .selected_text(self.animacion.nombre())
                .show_ui(ui, |ui| {
                    for animacion in Animacion::TODAS {
                        ui.selectable_value(&mut self.animacion, animacion, animacion.nombre());
                    }
                })
                .response
                .on_hover_text("Cómo se pasa de una página a otra");

            ui.separator();
            let ayuda_luz = self.ayuda(
                "Tono cálido y brillo de las páginas para descansar la vista",
                Accion::LuzCalida,
            );
            // El menú no se cierra al tocar dentro: así se pueden mover los deslizadores.
            let boton = egui::Button::selectable(self.filtro.activo, "Luz cálida");
            let config = egui::containers::menu::MenuConfig::new()
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
            egui::containers::menu::MenuButton::from_button(boton)
                .config(config)
                .ui(ui, |ui| self.menu_filtro(ui))
                .0
                .on_hover_text(ayuda_luz);

            ui.separator();
            let tema = if self.oscuro { "Claro" } else { "Oscuro" };
            let ayuda = self.ayuda("Cambiar el tema", Accion::Tema);
            if ui.button(tema).on_hover_text(ayuda).clicked() {
                self.cambiar_tema(ui.ctx());
            }
            let teclas = self.controles.texto(Accion::PantallaCompleta);
            let pista = match (
                self.controles.doble_clic_pantalla_completa,
                teclas.is_empty(),
            ) {
                (true, true) => "Doble clic: pantalla completa".to_string(),
                (true, false) => format!("Doble clic o {teclas}: pantalla completa"),
                (false, false) => format!("{teclas}: pantalla completa"),
                (false, true) => String::new(),
            };
            if !pista.is_empty() {
                ui.weak(pista);
            }
        });
    }

    /// La barra sobre la página, en pantalla completa. Aparece con el ratón en la franja de
    /// abajo (menos la esquina del selector de páginas) y se queda mientras esté encima, o
    /// haya un menú abierto, y un momento después.
    fn barra_flotante(&mut self, ctx: &egui::Context) {
        let pantalla = ctx.content_rect();
        let raton = ctx.input(|i| i.pointer.hover_pos());
        let en_franja = raton.is_some_and(|p| {
            p.y >= pantalla.bottom() - ALTO_FRANJA_BARRA
                && p.x <= pantalla.right() - ANCHO_ESQUINA_LIBRE
        });
        let encima = raton.is_some_and(|p| self.barra_rect.contains(p));
        let con_menu = egui::Popup::is_any_open(ctx);
        let ahora = Instant::now();
        if en_franja || (encima && self.barra_visible_hasta.is_some()) || con_menu {
            self.barra_visible_hasta = Some(ahora + ESPERA_OCULTAR_BARRA);
        }
        let Some(hasta) = self.barra_visible_hasta.filter(|h| *h > ahora) else {
            self.barra_visible_hasta = None;
            self.barra_rect = egui::Rect::NOTHING;
            return;
        };
        ctx.request_repaint_after(hasta - ahora);

        let c = estilo::colores(ctx);
        let respuesta = egui::Area::new(egui::Id::new("barra_flotante"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::LEFT_BOTTOM, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(c.panel)
                    .stroke(egui::Stroke::new(1.0, c.borde))
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.set_width(pantalla.width() - 18.0);
                        self.barra(ui);
                    });
            });
        self.barra_rect = respuesta.response.rect;
    }

    /// Dibuja la página actual y atiende el ratón sobre ella: arrastrar mueve la página
    /// y la rueda hace zoom hacia el punto donde está el cursor.
    fn vista(&mut self, ui: &mut egui::Ui, hueco: egui::Rect) {
        let respuesta = ui.interact(hueco, ui.id().with("pagina"), egui::Sense::click_and_drag());
        self.atender_clics(ui.ctx(), &respuesta);
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
                self.zoom_hacia(PASO_ZOOM_RUEDA.powf(muescas), foco);
            }
        }

        if self.total == 0 {
            ui.label("Este cómic no tiene páginas.");
        } else if let Some(cargada) = self.texturas.get(&self.actual) {
            let original = cargada.original;
            let vinetas = self.ajuste == Ajuste::Vinetas;
            if vinetas {
                self.encuadrar(original, hueco.size());
            }
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
            // Leyendo por viñetas el recuadro va al centro aunque esté en una esquina de la
            // página: lo que queda fuera de la página se tapa igual que el resto.
            if !vinetas {
                self.desplazamiento_objetivo = objetivo.clamp(-limite, limite);
            }

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
            let rect = colocar_pagina(ui.ctx(), hueco, tamano, self.desplazamiento);
            let efecto = self.efecto_transicion(ui.ctx());
            // Recortado al hueco, para que una página con zoom no tape la barra.
            let pintor = ui.painter_at(hueco);
            let textura = self.texturas[&self.actual].textura.id();
            let tinte = self.filtro.tinte();
            match efecto {
                Efecto::Ninguno => dibujar_pagina(&pintor, textura, rect, tinte),
                Efecto::Deslizar { lado, opacidad } => dibujar_pagina(
                    &pintor,
                    textura,
                    rect.translate(egui::vec2(lado, 0.0)),
                    tinte.gamma_multiply(opacidad),
                ),
                Efecto::Hoja {
                    progreso,
                    anterior: (indice, rect_anterior),
                    adelante,
                } => {
                    let anterior = (self.texturas[&indice].textura.id(), rect_anterior);
                    let nueva = (textura, rect);
                    // Hacia delante se levanta la de antes; hacia atrás llega la nueva.
                    let (girando, debajo) = if adelante {
                        (anterior, nueva)
                    } else {
                        (nueva, anterior)
                    };
                    let giro = Giro {
                        girando,
                        debajo,
                        progreso,
                        adelante,
                        tinte,
                    };
                    pasar_hoja::dibujar(&pintor, hueco, &giro);
                }
            }
            if vinetas {
                self.tapar_fuera_del_recuadro(&pintor, hueco, rect);
            }
            self.ultima_dibujada = Some((self.actual, rect));
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
pub(crate) fn muescas_de_rueda(entrada: &egui::InputState) -> f32 {
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

/// Dónde va la página a `tamano` (en puntos), centrada en `hueco` y movida `desplazamiento`.
///
/// La esquina se alinea con los píxeles de la pantalla: cuando la textura mide justo
/// lo que se dibuja, la GPU no la vuelve a filtrar y se ve tan nítida como salió del hilo.
fn colocar_pagina(
    ctx: &egui::Context,
    hueco: egui::Rect,
    tamano: egui::Vec2,
    desplazamiento: egui::Vec2,
) -> egui::Rect {
    let ppp = ctx.pixels_per_point();
    let colocada = egui::Rect::from_center_size(hueco.center() + desplazamiento, tamano);
    let esquina = (colocada.min.to_vec2() * ppp).round() / ppp;
    egui::Rect::from_min_size(esquina.to_pos2(), tamano)
}

/// Dibuja la página en `rect` multiplicada por `tinte`: el del filtro, y con alfa si se
/// está desvaneciendo.
fn dibujar_pagina(
    pintor: &egui::Painter,
    textura: egui::TextureId,
    rect: egui::Rect,
    tinte: egui::Color32,
) {
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    pintor.image(textura, rect, uv, tinte);
}

pub(crate) fn en_pantalla_completa(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().fullscreen).unwrap_or(false)
}

/// Al salir de pantalla completa la ventana queda maximizada, con su barra de título y la
/// del sistema, y no en el tamaño pequeño que tendría por defecto.
pub(crate) fn poner_pantalla_completa(ctx: &egui::Context, completa: bool) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(completa));
    if !completa {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }
}

/// Alto de la franja de abajo que hace aparecer la barra en pantalla completa.
const ALTO_FRANJA_BARRA: f32 = 40.0;
/// Lo que se deja libre a la derecha de esa franja: es la esquina del selector de páginas.
const ANCHO_ESQUINA_LIBRE: f32 = 160.0;
/// Cuánto tarda en esconderse la barra tras quitar el ratón.
const ESPERA_OCULTAR_BARRA: Duration = Duration::from_millis(700);

impl Lector {
    /// Dibuja el lector. Devuelve `true` cuando el usuario quiere volver a la biblioteca.
    pub fn mostrar(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        self.atajos_de_teclado(&ctx);

        // En pantalla completa la barra no ocupa sitio: asoma sobre la página al acercar el
        // ratón al borde de abajo y se esconde sola.
        if en_pantalla_completa(&ctx) {
            self.barra_flotante(&ctx);
        } else {
            egui::Panel::bottom("barra").show(ui, |ui| self.barra(ui));
        }

        // Fondo casi negro (o gris claro en el tema claro) y sin márgenes: todo el sitio
        // para la página. Se tiñe como las páginas, si no, el gris claro deslumbraría.
        let fondo = egui::Frame::central_panel(&ctx.global_style())
            .fill(estilo::colores(&ctx).lector * self.filtro.tinte())
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
            if let Some(pagina) = self.selector.mostrar(ui, hueco, self.actual) {
                self.ir_a(pagina);
                ctx.request_repaint();
            }
        });

        if self.fin_abierto {
            self.aviso_siguiente(&ctx);
        }

        std::mem::take(&mut self.volver)
    }

    /// El aviso del final del cómic: ofrece seguir con el siguiente sin salir a la
    /// biblioteca. Se cierra al pulsar fuera o con Esc, y se queda en la última página.
    fn aviso_siguiente(&mut self, ctx: &egui::Context) {
        let Some(titulo) = self.siguiente.clone() else {
            self.fin_abierto = false;
            return;
        };
        let c = estilo::colores(ctx);
        let modal = egui::Modal::new(egui::Id::new("siguiente_comic"))
            .frame(
                egui::Frame::new()
                    .fill(c.panel)
                    .stroke(egui::Stroke::new(1.0, c.borde))
                    .corner_radius(estilo::RADIO_PANEL)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(
                    egui::RichText::new("Siguiente")
                        .size(12.0)
                        .color(c.texto_secundario),
                );
                ui.add_space(4.0);
                ui.add(egui::Label::new(estilo::titulo(&titulo, 18.0, c.texto)).wrap());
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui.add(boton_subrayado(c, "Leer ahora")).clicked() {
                        self.fin_abierto = false;
                        self.pedir_siguiente = true;
                    }
                    if ui.add(boton_texto(c, "Biblioteca")).clicked() {
                        self.fin_abierto = false;
                        self.volver = true;
                    }
                });
            });
        if modal.should_close() {
            self.fin_abierto = false;
        }
    }
}
