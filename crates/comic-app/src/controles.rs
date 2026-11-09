//! Los controles del lector: qué tecla hace cada cosa y qué hace el ratón sobre la
//! página. Se cambian en Ajustes y se guardan con las preferencias.

use eframe::egui::{self, Key};

/// Lo que se puede hacer con el teclado en el lector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accion {
    Siguiente,
    Anterior,
    Subir,
    Bajar,
    Acercar,
    Alejar,
    AjustarPagina,
    AjustarAncho,
    /// Leer recuadro a recuadro, con zoom a cada uno.
    AjustarVinetas,
    Paginas,
    LuzCalida,
    Tema,
    PantallaCompleta,
    /// Cerrar lo que esté abierto, salir de pantalla completa o volver a la biblioteca.
    Volver,
}

impl Accion {
    pub const TODAS: [Accion; 14] = [
        Accion::Siguiente,
        Accion::Anterior,
        Accion::Subir,
        Accion::Bajar,
        Accion::Acercar,
        Accion::Alejar,
        Accion::AjustarPagina,
        Accion::AjustarAncho,
        Accion::AjustarVinetas,
        Accion::Paginas,
        Accion::LuzCalida,
        Accion::Tema,
        Accion::PantallaCompleta,
        Accion::Volver,
    ];

    pub fn nombre(self) -> &'static str {
        match self {
            Accion::Siguiente => "Página siguiente",
            Accion::Anterior => "Página anterior",
            Accion::Subir => "Desplazar hacia arriba",
            Accion::Bajar => "Desplazar hacia abajo",
            Accion::Acercar => "Acercar",
            Accion::Alejar => "Alejar",
            Accion::AjustarPagina => "Ajustar a la página",
            Accion::AjustarAncho => "Ajustar al ancho",
            Accion::AjustarVinetas => "Leer por viñetas",
            Accion::Paginas => "Ver todas las páginas",
            Accion::LuzCalida => "Luz cálida",
            Accion::Tema => "Tema claro u oscuro",
            Accion::PantallaCompleta => "Pantalla completa",
            Accion::Volver => "Salir de pantalla completa o volver",
        }
    }

    /// Cómo se llama en el archivo de preferencias.
    fn clave(self) -> &'static str {
        match self {
            Accion::Siguiente => "siguiente",
            Accion::Anterior => "anterior",
            Accion::Subir => "subir",
            Accion::Bajar => "bajar",
            Accion::Acercar => "acercar",
            Accion::Alejar => "alejar",
            Accion::AjustarPagina => "ajustar_pagina",
            Accion::AjustarAncho => "ajustar_ancho",
            Accion::AjustarVinetas => "ajustar_vinetas",
            Accion::Paginas => "paginas",
            Accion::LuzCalida => "luz_calida",
            Accion::Tema => "tema",
            Accion::PantallaCompleta => "pantalla_completa",
            Accion::Volver => "volver",
        }
    }

    fn por_defecto(self) -> &'static [Key] {
        match self {
            Accion::Siguiente => &[Key::ArrowRight, Key::Space],
            Accion::Anterior => &[Key::ArrowLeft],
            Accion::Subir => &[Key::ArrowUp],
            Accion::Bajar => &[Key::ArrowDown],
            // En un teclado inglés el + está en la tecla del =.
            Accion::Acercar => &[Key::Plus, Key::Equals],
            Accion::Alejar => &[Key::Minus],
            Accion::AjustarPagina => &[Key::Num0],
            Accion::AjustarAncho => &[Key::W],
            Accion::AjustarVinetas => &[Key::V],
            Accion::Paginas => &[Key::P],
            Accion::LuzCalida => &[Key::N],
            Accion::Tema => &[Key::T],
            Accion::PantallaCompleta => &[Key::F11],
            Accion::Volver => &[Key::Escape],
        }
    }

    fn indice(self) -> usize {
        Accion::TODAS
            .iter()
            .position(|&a| a == self)
            .expect("todas las acciones están en TODAS")
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Controles {
    /// Las teclas de cada acción, en el orden de `Accion::TODAS`. Puede no tener ninguna.
    teclas: Vec<Vec<Key>>,
    /// Un clic en la página pasa a la siguiente.
    pub clic_pasa_pagina: bool,
    /// Un doble clic en la página entra en pantalla completa o sale de ella.
    pub doble_clic_pantalla_completa: bool,
}

impl Default for Controles {
    fn default() -> Controles {
        Controles {
            teclas: Accion::TODAS
                .iter()
                .map(|a| a.por_defecto().to_vec())
                .collect(),
            clic_pasa_pagina: true,
            doble_clic_pantalla_completa: true,
        }
    }
}

impl Controles {
    pub fn teclas(&self, accion: Accion) -> &[Key] {
        &self.teclas[accion.indice()]
    }

    /// Si se ha pulsado en este fotograma alguna tecla de `accion`.
    pub fn pulsada(&self, ctx: &egui::Context, accion: Accion) -> bool {
        ctx.input(|i| self.teclas(accion).iter().any(|&t| i.key_pressed(t)))
    }

    /// Las teclas de `accion` para enseñarlas: `→`, `+ o =`. Vacío si no tiene.
    pub fn texto(&self, accion: Accion) -> String {
        self.teclas(accion)
            .iter()
            .map(|&t| nombre_de_tecla(t))
            .collect::<Vec<_>>()
            .join(" o ")
    }

    /// `accion` también se hace con `tecla`, además de con las que ya tenía. Si otra
    /// acción la tenía, se la quita: una tecla hace una sola cosa, y no se repite.
    pub fn anadir(&mut self, accion: Accion, tecla: Key) {
        for teclas in &mut self.teclas {
            teclas.retain(|&t| t != tecla);
        }
        self.teclas[accion.indice()].push(tecla);
    }

    /// `accion` deja de hacerse con `tecla`.
    pub fn quitar(&mut self, accion: Accion, tecla: Key) {
        self.teclas[accion.indice()].retain(|&t| t != tecla);
    }

    /// Las líneas `clave=valor` para el archivo de preferencias.
    pub fn a_lineas(&self) -> String {
        let mut texto = format!(
            "clic_pasa_pagina={}\ndoble_clic_pantalla_completa={}\n",
            si_no(self.clic_pasa_pagina),
            si_no(self.doble_clic_pantalla_completa),
        );
        for accion in Accion::TODAS {
            let teclas: Vec<&str> = self.teclas(accion).iter().map(|t| t.name()).collect();
            texto.push_str(&format!("tecla.{}={}\n", accion.clave(), teclas.join(",")));
        }
        texto
    }

    /// Lee una línea del archivo de preferencias. Devuelve `false` si no es de los
    /// controles.
    pub fn leer_linea(&mut self, clave: &str, valor: &str) -> bool {
        match clave {
            "clic_pasa_pagina" => self.clic_pasa_pagina = valor != "no",
            "doble_clic_pantalla_completa" => self.doble_clic_pantalla_completa = valor != "no",
            _ => {
                let Some(nombre) = clave.strip_prefix("tecla.") else {
                    return false;
                };
                let Some(accion) = Accion::TODAS.into_iter().find(|a| a.clave() == nombre) else {
                    eprintln!("[preferencias] Acción desconocida: {nombre}");
                    return true;
                };
                let teclas: Vec<Key> = valor
                    .split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .filter_map(|t| {
                        let tecla = Key::from_name(t);
                        if tecla.is_none() {
                            eprintln!("[preferencias] Tecla desconocida: {t}");
                        }
                        tecla
                    })
                    .collect();
                self.teclas[accion.indice()] = teclas;
            }
        }
        true
    }
}

fn si_no(valor: bool) -> &'static str {
    if valor {
        "si"
    } else {
        "no"
    }
}

/// El nombre de una tecla como se ve en el teclado, en español donde cambia.
pub fn nombre_de_tecla(tecla: Key) -> &'static str {
    match tecla {
        Key::ArrowRight => "→",
        Key::ArrowLeft => "←",
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::Escape => "Esc",
        Key::Space => "Espacio",
        Key::Enter => "Intro",
        Key::Backspace => "Retroceso",
        Key::Delete => "Supr",
        Key::Insert => "Insert",
        Key::Home => "Inicio",
        Key::End => "Fin",
        Key::PageUp => "Re Pág",
        Key::PageDown => "Av Pág",
        Key::Tab => "Tabulador",
        otra => otra.symbol_or_name(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_tecla_hace_una_sola_cosa() {
        let mut controles = Controles::default();
        controles.anadir(Accion::Anterior, Key::Space);
        // La barra espaciadora ya no pasa a la siguiente: ahora es de "anterior".
        assert_eq!(controles.teclas(Accion::Siguiente), [Key::ArrowRight]);
        assert_eq!(
            controles.teclas(Accion::Anterior),
            [Key::ArrowLeft, Key::Space]
        );
    }

    #[test]
    fn una_accion_puede_tener_varias_teclas() {
        let mut controles = Controles::default();
        assert_eq!(
            controles.teclas(Accion::Siguiente),
            [Key::ArrowRight, Key::Space]
        );
        controles.anadir(Accion::Bajar, Key::S);
        assert_eq!(controles.teclas(Accion::Bajar), [Key::ArrowDown, Key::S]);
        // Una tecla ya usada pasa a la nueva acción, sin repetirse.
        controles.anadir(Accion::Bajar, Key::Space);
        assert_eq!(
            controles.teclas(Accion::Bajar),
            [Key::ArrowDown, Key::S, Key::Space]
        );
        assert_eq!(controles.teclas(Accion::Siguiente), [Key::ArrowRight]);
        controles.quitar(Accion::Bajar, Key::S);
        assert_eq!(
            controles.teclas(Accion::Bajar),
            [Key::ArrowDown, Key::Space]
        );
    }

    #[test]
    fn se_guardan_y_se_leen() {
        let mut controles = Controles::default();
        controles.anadir(Accion::Siguiente, Key::D);
        controles.anadir(Accion::PantallaCompleta, Key::F);
        controles.clic_pasa_pagina = false;

        let mut leidos = Controles::default();
        for linea in controles.a_lineas().lines() {
            let (clave, valor) = linea.split_once('=').unwrap();
            assert!(leidos.leer_linea(clave, valor));
        }
        assert_eq!(leidos, controles);
    }
}
