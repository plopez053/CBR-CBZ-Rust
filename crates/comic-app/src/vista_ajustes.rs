//! La página de ajustes: cómo se pasa de página, qué hace el ratón en el lector y qué
//! tecla hace cada cosa.

use eframe::egui::{self, FontId, Rect, Sense, Stroke};

use crate::controles::{nombre_de_tecla, Accion, Controles};
use crate::estilo::{self, Colores};
use crate::preferencias::{Animacion, Preferencias};
use crate::vista_biblioteca::{
    area_de_desplazamiento, boton_subrayado, boton_texto, panel, MARGEN,
};
use crate::vista_novedades::pildora;

/// Alto de cada fila de los paneles.
const ALTO_FILA: f32 = 36.0;

#[derive(Default)]
pub struct VistaAjustes {
    /// La acción cuya tecla se está cambiando: la próxima tecla que se pulse será la suya.
    esperando: Option<Accion>,
}

impl VistaAjustes {
    /// Dibuja la página. Lo que se cambie se guarda en `preferencias` al momento.
    pub fn mostrar(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        preferencias: &mut Preferencias,
        arriba: bool,
    ) {
        let antes = (preferencias.animacion, preferencias.controles.clone());

        // Con una tecla pendiente, la siguiente que se pulse es la nueva (Esc incluida:
        // también se puede asignar). Para dejarlo como estaba, se pulsa en otro sitio.
        if let Some(accion) = self.esperando {
            let pulsada = ui.input(|i| {
                i.events.iter().find_map(|evento| match evento {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        ..
                    } => Some(*key),
                    _ => None,
                })
            });
            if let Some(tecla) = pulsada {
                preferencias.controles.anadir(accion, tecla);
                self.esperando = None;
            }
        }

        let mut tecla_pulsada = false;
        area_de_desplazamiento(arriba).show(ui, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::same(MARGEN as i8))
                .show(ui, |ui| {
                    ui.set_max_width(640.0);
                    ui.label(estilo::titulo("Ajustes", 24.0, c.texto));
                    ui.add_space(22.0);

                    seccion(ui, c, "Lectura");
                    panel(c).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        self.lectura(ui, c, preferencias);
                    });
                    ui.add_space(26.0);

                    seccion(ui, c, "Teclas");
                    panel(c).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        tecla_pulsada = self.teclas(ui, c, &mut preferencias.controles);
                    });
                    if preferencias.controles != Controles::default() {
                        ui.add_space(12.0);
                        let restablecer = ui.add(boton_subrayado(c, "Restablecer controles"));
                        if restablecer.clicked() {
                            preferencias.controles = Controles::default();
                            self.esperando = None;
                        }
                    }
                });
        });

        // Un clic fuera de las teclas deja la que se estaba cambiando como estaba.
        if self.esperando.is_some() && !tecla_pulsada && ui.input(|i| i.pointer.any_click()) {
            self.esperando = None;
        }

        if (preferencias.animacion, preferencias.controles.clone()) != antes {
            preferencias.guardar();
        }
    }

    fn lectura(&mut self, ui: &mut egui::Ui, c: Colores, preferencias: &mut Preferencias) {
        fila(ui, c, "Al pasar de página", |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // De derecha a izquierda: se ponen al revés para que se lean en orden.
            for animacion in Animacion::TODAS.into_iter().rev() {
                let elegida = preferencias.animacion == animacion;
                if pildora(ui, c, animacion.nombre(), elegida).clicked() {
                    preferencias.animacion = animacion;
                }
            }
        });
        separador(ui, c);
        let controles = &mut preferencias.controles;
        fila(ui, c, "Un clic en la página pasa a la siguiente", |ui| {
            interruptor(ui, c, &mut controles.clic_pasa_pagina);
        });
        separador(ui, c);
        fila(ui, c, "Doble clic para la pantalla completa", |ui| {
            interruptor(ui, c, &mut controles.doble_clic_pantalla_completa);
        });
    }

    /// Una fila por acción, con su tecla. Devuelve si se ha pulsado alguna tecla.
    fn teclas(&mut self, ui: &mut egui::Ui, c: Colores, controles: &mut Controles) -> bool {
        let mut pulsada = false;
        for (i, accion) in Accion::TODAS.into_iter().enumerate() {
            if i > 0 {
                separador(ui, c);
            }
            let esperando = self.esperando == Some(accion);
            fila(ui, c, accion.nombre(), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                // De derecha a izquierda: primero "Añadir" otra tecla, luego las
                // teclas al revés para que se lean en orden. Pulsar una la quita.
                let anadir = if esperando {
                    tecla(ui, c, "Pulsa una tecla", true)
                } else {
                    ui.add(boton_texto(c, "Añadir"))
                };
                if anadir.clicked() {
                    pulsada = true;
                    self.esperando = if esperando { None } else { Some(accion) };
                }
                let mut quitar = None;
                for &t in controles.teclas(accion).iter().rev() {
                    if tecla(ui, c, nombre_de_tecla(t), false)
                        .on_hover_text("Quitar esta tecla")
                        .clicked()
                    {
                        quitar = Some(t);
                    }
                }
                if let Some(t) = quitar {
                    controles.quitar(accion, t);
                    pulsada = true;
                }
            });
        }
        pulsada
    }
}

fn seccion(ui: &mut egui::Ui, c: Colores, texto: &str) {
    ui.label(
        egui::RichText::new(texto)
            .font(estilo::negrita(16.0))
            .color(c.texto),
    );
    ui.add_space(8.0);
}

/// Una fila de un panel: el nombre a la izquierda y el control a la derecha.
fn fila(ui: &mut egui::Ui, c: Colores, nombre: &str, control: impl FnOnce(&mut egui::Ui)) {
    let (zona, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ALTO_FILA), Sense::hover());
    ui.painter().text(
        zona.left_center(),
        egui::Align2::LEFT_CENTER,
        nombre,
        FontId::proportional(14.0),
        c.texto,
    );
    let mut derecha = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(zona)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    control(&mut derecha);
}

fn separador(ui: &mut egui::Ui, c: Colores) {
    let (zona, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    let y = ui.painter().round_to_pixel_center(zona.center().y);
    ui.painter()
        .hline(zona.x_range(), y, Stroke::new(1.0, c.borde));
}

/// Una tecla como en el teclado: el nombre en un recuadro. Mientras se espera la nueva,
/// el texto va en el color de acento.
fn tecla(ui: &mut egui::Ui, c: Colores, texto: &str, esperando: bool) -> egui::Response {
    let (texto, fuente, color) = if esperando {
        (texto, estilo::media(13.0), c.acento)
    } else {
        (texto, estilo::media(13.0), c.texto)
    };
    let galeria = ui
        .painter()
        .layout_no_wrap(texto.to_string(), fuente, color);
    let tamano = egui::vec2((galeria.size().x + 20.0).max(40.0), 26.0);
    let (rect, respuesta) = ui.allocate_exact_size(tamano, Sense::click());
    let respuesta = respuesta.on_hover_cursor(egui::CursorIcon::PointingHand);
    let encima = respuesta.hovered() || esperando;
    let pintor = ui.painter();
    pintor.rect(
        rect,
        estilo::RADIO_CONTROL,
        if encima { c.resaltado } else { c.tarjeta },
        Stroke::new(1.0, if encima { c.borde_fuerte } else { c.borde }),
        egui::StrokeKind::Inside,
    );
    pintor.galley(rect.center() - galeria.size() / 2.0, galeria, color);
    respuesta
}

/// Un interruptor de encendido y apagado: una píldora con un círculo que se mueve, en el
/// color de acento cuando está encendido.
fn interruptor(ui: &mut egui::Ui, c: Colores, encendido: &mut bool) -> egui::Response {
    let (rect, mut respuesta) = ui.allocate_exact_size(egui::vec2(34.0, 20.0), Sense::click());
    if respuesta.clicked() {
        *encendido = !*encendido;
        respuesta.mark_changed();
    }
    let respuesta = respuesta.on_hover_cursor(egui::CursorIcon::PointingHand);
    let t = ui
        .ctx()
        .animate_bool_with_time(respuesta.id, *encendido, 0.12);
    let pintor = ui.painter();
    let fondo = if *encendido { c.acento } else { c.borde_fuerte };
    pintor.rect_filled(rect, rect.height() / 2.0, fondo);
    let radio = rect.height() / 2.0 - 3.0;
    let x = egui::lerp(
        (rect.left() + radio + 3.0)..=(rect.right() - radio - 3.0),
        t,
    );
    let circulo = Rect::from_center_size(
        egui::pos2(x, rect.center().y),
        egui::Vec2::splat(radio * 2.0),
    );
    pintor.circle_filled(circulo.center(), radio, egui::Color32::WHITE);
    respuesta
}
