//! Filtro para leer más a gusto, sobre todo de noche: tiñe las páginas de un tono cálido
//! (con menos azul) y puede oscurecerlas.
//!
//! No toca las imágenes: la GPU multiplica cada página por un color al dibujarla, así que
//! no cuesta nada y la nitidez del encogido sigue igual.

use eframe::egui::Color32;

/// Temperatura del blanco normal de la pantalla (sRGB), en kelvin: con ella no se tiñe nada.
pub const TEMPERATURA_NEUTRA: f32 = 6500.0;
/// Lo más cálido que se deja elegir, parecido a la luz de una vela.
pub const TEMPERATURA_MIN: f32 = 2000.0;
/// Lo más oscuro que se deja elegir: por debajo la página ya casi no se lee.
pub const BRILLO_MIN: f32 = 0.3;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Filtro {
    pub activo: bool,
    /// Temperatura de color del blanco de la página, en kelvin. Menos = más cálido.
    pub temperatura: f32,
    /// Cuánta luz de la página se deja pasar, de `BRILLO_MIN` a 1.
    pub brillo: f32,
}

impl Default for Filtro {
    /// Apagado, y al encenderlo, un tono como el de una bombilla halógena sin oscurecer.
    fn default() -> Filtro {
        Filtro {
            activo: false,
            temperatura: 3400.0,
            brillo: 1.0,
        }
    }
}

impl Filtro {
    /// El color por el que se multiplica la página al dibujarla. Blanco si está apagado.
    pub fn tinte(&self) -> Color32 {
        if !self.activo {
            return Color32::WHITE;
        }
        let neutro = blanco_a(TEMPERATURA_NEUTRA);
        let calido = blanco_a(self.temperatura.clamp(TEMPERATURA_MIN, TEMPERATURA_NEUTRA));
        let brillo = self.brillo.clamp(BRILLO_MIN, 1.0);
        // Relativo al blanco neutro, para que a 6500 K quede blanco del todo y no un pelín
        // amarillo como sale de la fórmula.
        let [r, g, b] = [0, 1, 2].map(|i| {
            let factor = (calido[i] / neutro[i]).min(1.0) * brillo;
            (factor * 255.0).round() as u8
        });
        Color32::from_rgb(r, g, b)
    }
}

/// Color de la luz de un cuerpo negro a `temperatura` kelvin, en sRGB de 0 a 255.
///
/// Es la aproximación de Tanner Helland a los datos de Mitchell Charity: no sirve para
/// medir, pero va bien de 1000 K a 40000 K y es la que usan muchos filtros de pantalla.
/// <https://tannerhelland.com/2012/09/18/convert-temperature-rgb-algorithm-code.html>
fn blanco_a(temperatura: f32) -> [f32; 3] {
    let t = temperatura / 100.0;
    let rojo = if t <= 66.0 {
        255.0
    } else {
        329.698_73 * (t - 60.0).powf(-0.133_204_76)
    };
    let verde = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_17 * (t - 60.0).powf(-0.075_514_85)
    };
    let azul = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    [rojo, verde, azul].map(|c| c.clamp(0.0, 255.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apagado_o_neutro_no_tine() {
        let apagado = Filtro {
            activo: false,
            temperatura: 2000.0,
            brillo: 0.5,
        };
        assert_eq!(apagado.tinte(), Color32::WHITE);
        let neutro = Filtro {
            activo: true,
            temperatura: TEMPERATURA_NEUTRA,
            brillo: 1.0,
        };
        assert_eq!(neutro.tinte(), Color32::WHITE);
    }

    #[test]
    fn coincide_con_los_valores_de_helland() {
        // Valores de comprobación de la versión en Rust de la fórmula (crate tempergb).
        let [r, g, b] = blanco_a(2500.0).map(|c| c.round() as u8);
        assert_eq!((r, g, b), (255, 159, 70));
    }

    #[test]
    fn mas_calido_quita_mas_azul() {
        let tinte = |temperatura| {
            Filtro {
                activo: true,
                temperatura,
                brillo: 1.0,
            }
            .tinte()
        };
        let (templado, calido) = (tinte(4500.0), tinte(2500.0));
        assert_eq!(calido.r(), 255);
        assert!(calido.b() < calido.g() && calido.g() < calido.r());
        assert!(calido.b() < templado.b() && calido.g() < templado.g());
    }

    #[test]
    fn el_brillo_oscurece_por_igual() {
        let filtro = Filtro {
            activo: true,
            temperatura: TEMPERATURA_NEUTRA,
            brillo: 0.5,
        };
        assert_eq!(filtro.tinte(), Color32::from_gray(128));
    }
}
