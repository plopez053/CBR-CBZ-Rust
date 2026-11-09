//! Aspecto de la app, según el sistema de diseño de Linear (awesome-design-md): un fondo
//! casi negro, paneles un escalón más claros con una línea fina, un solo color de acento
//! (un ámbar oscuro, en vez del lavanda de Linear) para la acción principal, lo elegido y
//! el progreso, esquinas de 8 px en botones y campos y de 12 px en paneles, y sin sombras.
//! El texto va en Inter; las cifras que cambian (páginas, contadores) en IBM Plex Mono,
//! para que no bailen.

use std::sync::Arc;

use eframe::egui::{self, Color32, FontFamily, FontId, Stroke};

/// Familias de fuentes con nombre: títulos (600) y botones o lo elegido (500).
const FAMILIA_NEGRITA: &str = "negrita";
const FAMILIA_MEDIA: &str = "media";

/// Las fuentes van dentro del programa (licencia OFL, en `assets/fuentes`): así se ve
/// igual en Windows y en Linux, sin depender de lo que haya instalado.
const SANS: &[u8] = include_bytes!("../../../assets/fuentes/Inter-Regular.ttf");
const SANS_MEDIA: &[u8] = include_bytes!("../../../assets/fuentes/Inter-Medium.ttf");
const SANS_NEGRITA: &[u8] = include_bytes!("../../../assets/fuentes/Inter-SemiBold.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fuentes/IBMPlexMono-Regular.ttf");

/// Esquinas: botones y campos, paneles y ventanas, portadas y fotos, y etiquetas.
pub const RADIO_CONTROL: u8 = 8;
pub const RADIO_PANEL: u8 = 12;
pub const RADIO_PORTADA: u8 = 4;
pub const RADIO_ETIQUETA: u8 = 4;

/// Los colores de la interfaz para un tema.
#[derive(Clone, Copy)]
pub struct Colores {
    /// La barra lateral y lo que queda detrás de todo.
    pub lateral: Color32,
    /// La zona de contenido: el mismo fondo que la barra lateral.
    pub contenido: Color32,
    /// Fondo del lector, detrás de las páginas.
    pub lector: Color32,
    /// Huecos de portada vacíos y etiquetas.
    pub tarjeta: Color32,
    /// Paneles sobre el fondo ("Seguir leyendo", las carpetas, las fichas).
    pub panel: Color32,
    /// Fondo de lo que tiene el ratón encima o está elegido en la barra lateral.
    pub resaltado: Color32,
    /// Las líneas finas que separan zonas.
    pub borde: Color32,
    /// Una línea algo más marcada: los controles al pasar el ratón.
    pub borde_fuerte: Color32,
    pub texto: Color32,
    pub texto_secundario: Color32,
    pub acento: Color32,
}

/// La escalera de superficies de Linear sobre su fondo casi negro (con un punto de azul).
const OSCURO: Colores = Colores {
    lateral: Color32::from_rgb(0x01, 0x01, 0x02),
    contenido: Color32::from_rgb(0x01, 0x01, 0x02),
    lector: Color32::from_rgb(0x01, 0x01, 0x02),
    tarjeta: Color32::from_rgb(0x14, 0x15, 0x16),
    panel: Color32::from_rgb(0x0f, 0x10, 0x11),
    resaltado: Color32::from_rgb(0x19, 0x1a, 0x1b),
    borde: Color32::from_rgb(0x23, 0x25, 0x2a),
    borde_fuerte: Color32::from_rgb(0x34, 0x34, 0x3a),
    texto: Color32::from_rgb(0xf7, 0xf8, 0xf8),
    texto_secundario: Color32::from_rgb(0x8a, 0x8f, 0x98),
    // Un ámbar oscuro, más apagado que el amarillo de antes del rediseño.
    acento: Color32::from_rgb(0xb4, 0x88, 0x14),
};

/// Linear no tiene tema claro: el mismo sistema con los grises invertidos.
const CLARO: Colores = Colores {
    lateral: Color32::from_rgb(0xfb, 0xfb, 0xfc),
    contenido: Color32::from_rgb(0xfb, 0xfb, 0xfc),
    lector: Color32::from_rgb(0xec, 0xec, 0xee),
    tarjeta: Color32::from_rgb(0xf0, 0xf1, 0xf3),
    panel: Color32::WHITE,
    resaltado: Color32::from_rgb(0xec, 0xed, 0xf0),
    borde: Color32::from_rgb(0xe4, 0xe5, 0xe9),
    borde_fuerte: Color32::from_rgb(0xd1, 0xd3, 0xd9),
    texto: Color32::from_rgb(0x16, 0x17, 0x1a),
    texto_secundario: Color32::from_rgb(0x5f, 0x63, 0x68),
    acento: Color32::from_rgb(0x9c, 0x72, 0x09),
};

/// Los colores del tema que se está usando.
pub fn colores(ctx: &egui::Context) -> Colores {
    match ctx.theme() {
        egui::Theme::Dark => OSCURO,
        egui::Theme::Light => CLARO,
    }
}

/// Fuente para títulos (Inter SemiBold).
pub fn negrita(tamano: f32) -> FontId {
    FontId::new(tamano, FontFamily::Name(FAMILIA_NEGRITA.into()))
}

/// Fuente para botones y lo elegido (Inter Medium).
pub fn media(tamano: f32) -> FontId {
    FontId::new(tamano, FontFamily::Name(FAMILIA_MEDIA.into()))
}

/// Fuente para cifras que cambian (IBM Plex Mono): los números no bailan al cambiar.
pub fn cifras(tamano: f32) -> FontId {
    FontId::monospace(tamano)
}

/// Un título de página: Inter SemiBold con el interletrado algo cerrado, como en Linear.
pub fn titulo(texto: &str, tamano: f32, color: Color32) -> egui::RichText {
    egui::RichText::new(texto)
        .font(negrita(tamano))
        .color(color)
        .extra_letter_spacing(-tamano * 0.02)
}

/// Dibuja una textura en `rect` con las esquinas redondeadas.
pub fn imagen(
    pintor: &egui::Painter,
    textura: egui::TextureId,
    rect: egui::Rect,
    uv: egui::Rect,
    radio: u8,
) {
    pintor.add(
        egui::epaint::RectShape::filled(rect, radio, Color32::WHITE).with_texture(textura, uv),
    );
}

/// Aplica fuentes y colores a los dos temas. Se llama una vez al arrancar.
pub fn aplicar(ctx: &egui::Context) {
    cargar_fuentes(ctx);
    ctx.style_mut_of(egui::Theme::Dark, |estilo| ajustar(estilo, OSCURO, true));
    ctx.style_mut_of(egui::Theme::Light, |estilo| ajustar(estilo, CLARO, false));
}

fn cargar_fuentes(ctx: &egui::Context) {
    let mut definiciones = egui::FontDefinitions::default();
    for (nombre, datos) in [
        ("sans", SANS),
        ("sans-media", SANS_MEDIA),
        ("sans-negrita", SANS_NEGRITA),
        ("mono", MONO),
    ] {
        definiciones
            .font_data
            .insert(nombre.into(), Arc::new(egui::FontData::from_static(datos)));
    }

    // Las de egui se quedan detrás, para los símbolos que no tenga Inter.
    let respaldo = definiciones
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (familia, fuente) in [
        (FAMILIA_NEGRITA, "sans-negrita"),
        (FAMILIA_MEDIA, "sans-media"),
    ] {
        let mut lista = respaldo.clone();
        lista.insert(0, fuente.into());
        definiciones
            .families
            .insert(FontFamily::Name(familia.into()), lista);
    }
    if let Some(familia) = definiciones.families.get_mut(&FontFamily::Proportional) {
        familia.insert(0, "sans".into());
    }
    if let Some(familia) = definiciones.families.get_mut(&FontFamily::Monospace) {
        familia.insert(0, "mono".into());
    }

    ctx.set_fonts(definiciones);
}

/// Quita el marco a los botones de `ui` (y de lo que se dibuje dentro): texto gris que se
/// enciende al pasar el ratón, con una píldora gris detrás; lo elegido, con el texto claro
/// y la píldora fija. Como la barra lateral y `boton_texto`, para controles de egui que no
/// se pueden sustituir (selectores, menús desplegables).
pub fn sin_marco(ui: &mut egui::Ui, c: Colores) {
    let v = ui.visuals_mut();
    v.selection.bg_fill = c.resaltado;
    v.selection.stroke = Stroke::new(1.0, c.texto);
    let w = &mut v.widgets;
    for (estado, fondo, texto) in [
        (&mut w.inactive, Color32::TRANSPARENT, c.texto_secundario),
        (&mut w.hovered, c.resaltado, c.texto),
        (&mut w.active, c.borde, c.texto),
        (&mut w.open, c.resaltado, c.texto),
    ] {
        estado.weak_bg_fill = fondo;
        estado.bg_fill = fondo;
        estado.bg_stroke = Stroke::NONE;
        estado.fg_stroke = Stroke::new(1.0, texto);
    }
}

fn ajustar(estilo: &mut egui::Style, c: Colores, oscuro: bool) {
    use egui::{FontFamily::Proportional, TextStyle};

    estilo.text_styles = [
        (TextStyle::Small, FontId::new(12.0, Proportional)),
        (TextStyle::Body, FontId::new(14.0, Proportional)),
        (TextStyle::Button, media(14.0)),
        (TextStyle::Monospace, cifras(13.0)),
        (TextStyle::Heading, negrita(24.0)),
    ]
    .into();

    estilo.spacing.item_spacing = egui::vec2(8.0, 6.0);
    // 8 x 14 de relleno, como los botones de Linear.
    estilo.spacing.button_padding = egui::vec2(14.0, 6.0);
    estilo.spacing.interact_size.y = 32.0;
    estilo.spacing.window_margin = egui::Margin::same(20);
    estilo.spacing.menu_margin = egui::Margin::same(6);
    // Barras de desplazamiento finas que no tapan el contenido hasta acercar el ratón.
    estilo.spacing.scroll = egui::style::ScrollStyle::thin();

    let v = &mut estilo.visuals;
    v.panel_fill = c.lateral;
    v.window_fill = c.panel;
    // Las ventanas y menús se separan del fondo con una línea, no con una sombra.
    v.window_stroke = Stroke::new(1.0, c.borde);
    v.window_corner_radius = RADIO_PANEL.into();
    v.menu_corner_radius = RADIO_CONTROL.into();
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    // Los campos de texto, en el primer escalón sobre el fondo.
    v.extreme_bg_color = c.panel;
    v.text_edit_bg_color = Some(c.panel);
    v.faint_bg_color = c.tarjeta;
    // Lo seleccionado, con un velo del color de acento.
    v.selection.bg_fill = c
        .resaltado
        .lerp_to_gamma(c.acento, if oscuro { 0.45 } else { 0.3 });
    v.selection.stroke = Stroke::new(1.0, c.texto);
    v.hyperlink_color = c.acento;
    v.override_text_color = None;
    v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };
    v.slider_trailing_fill = true;

    // Botones: el primer escalón con una línea fina; al pasar el ratón, un escalón más y
    // la línea algo más marcada.
    let w = &mut v.widgets;
    for (estado, fondo, linea) in [
        (&mut w.inactive, c.panel, c.borde),
        (&mut w.hovered, c.tarjeta, c.borde_fuerte),
        (&mut w.active, c.resaltado, c.borde_fuerte),
        (&mut w.open, c.tarjeta, c.borde_fuerte),
    ] {
        estado.corner_radius = RADIO_CONTROL.into();
        estado.weak_bg_fill = fondo;
        estado.bg_fill = fondo;
        estado.bg_stroke = Stroke::new(1.0, linea);
        estado.fg_stroke = Stroke::new(1.0, c.texto);
        estado.expansion = 0.0;
    }
    // Al escribir, el campo enseña el anillo del color de acento.
    w.active.bg_stroke = Stroke::new(1.0, c.acento);
    w.noninteractive.corner_radius = RADIO_CONTROL.into();
    w.noninteractive.bg_stroke = Stroke::new(1.0, c.borde);
    w.noninteractive.fg_stroke = Stroke::new(1.0, c.texto);
    w.noninteractive.bg_fill = c.contenido;
    w.noninteractive.weak_bg_fill = c.contenido;
}

/// Para llamar en el menú principal tras dibujar un submenú abierto: si el ratón sale del
/// submenú y de su botón `boton`, cierra solo el submenú (el principal sigue abierto)
/// tras un momento de gracia mínimo.
pub fn cerrar_submenu_al_alejarse(ui: &egui::Ui, submenu: egui::Rect, boton: egui::Rect) {
    const MARGEN: f32 = 0.0;
    const GRACIA: f64 = 0.10;

    let ctx = ui.ctx().clone();
    let id = egui::Id::new("submenu_fuera_desde");
    let zona = submenu.union(boton).expand(MARGEN);
    let fuera = ctx.pointer_hover_pos().is_some_and(|p| !zona.contains(p));
    if !fuera {
        ctx.data_mut(|d| d.remove::<f64>(id));
        return;
    }
    let ahora = ctx.input(|i| i.time);
    let desde = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(id, || ahora));
    if ahora - desde > GRACIA {
        ctx.data_mut(|d| d.remove::<f64>(id));
        egui::containers::menu::MenuState::from_ui(ui, |estado, _| estado.open_item = None);
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(GRACIA));
    }
}

/// Para llamar al final de un menú del botón derecho: lo cierra si el ratón se aleja de
/// él (y de `submenu`, si hay uno abierto) o si se desplaza la vista. Deja un margen y
/// un momento de gracia, para que un movimiento torpe camino del submenú no lo cierre.
pub fn cerrar_menu_al_alejarse(ui: &mut egui::Ui, submenu: Option<egui::Rect>) {
    const MARGEN: f32 = 2.0;
    const GRACIA: f64 = 0.50;

    let ctx = ui.ctx().clone();
    // Solo hay un menú del botón derecho abierto a la vez: basta con un estado.
    let id = egui::Id::new("menu_fuera_desde");
    let zona = submenu
        .map_or(ui.min_rect(), |s| s.union(ui.min_rect()))
        .expand(MARGEN);
    let fuera = ctx.pointer_hover_pos().is_some_and(|p| !zona.contains(p));
    let ahora = ctx.input(|i| i.time);

    let mut cerrar = ctx.input(|i| {
        i.raw
            .events
            .iter()
            .any(|e| matches!(e, egui::Event::MouseWheel { .. }))
    });
    if fuera {
        let desde = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(id, || ahora));
        if ahora - desde > GRACIA {
            cerrar = true;
        } else {
            // Sin esto, si el ratón se queda quieto fuera, nadie vuelve a mirar.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(GRACIA));
        }
    } else {
        ctx.data_mut(|d| d.remove::<f64>(id));
    }
    if cerrar {
        ctx.data_mut(|d| d.remove::<f64>(id));
        ui.close();
    }
}
