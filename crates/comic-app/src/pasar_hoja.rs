//! La animación de pasar página como en un cómic de papel, con el modelo de los lectores
//! de libros electrónicos: la deformación cónica de Hong, Carmichael y otros ("Turning
//! Pages of 3D Electronic Books", IEEE 3DUI 2006).
//!
//! Una hoja de papel se puede doblar pero no estirar, y las únicas formas que cumplen
//! eso son trozos de cilindro o de cono. Al coger una hoja por la esquina de abajo, se
//! enrolla sobre un cono cuyo vértice queda en la línea del lomo, por debajo de la hoja:
//! cerca del vértice (la esquina de abajo) se curva mucho, y lejos (arriba) casi nada.
//! Mientras tanto, la hoja entera gira sobre el lomo. La animación mueve tres cosas a la
//! vez: dónde está el vértice, lo cerrado que está el cono y cuánto ha girado la hoja.
//!
//! La hoja es una malla con la imagen de la página. Cada punto se enrolla en el cono, se
//! gira sobre el lomo y se proyecta con perspectiva (lo que se acerca se ve más grande).
//! La luz se calcula con la inclinación real de cada punto, con un brillo en la curva, y
//! la hoja levantada echa su sombra sobre la página de debajo. Por detrás se ve el papel,
//! con la página transparentándose del revés.

use std::cmp::Ordering;
use std::f32::consts::PI;

use eframe::egui::{self, epaint::Vertex, Color32, Pos2, Rect, TextureId};

/// Puntos de la malla a lo ancho y a lo alto. Con menos se notan aristas en la curva.
const COLUMNAS: usize = 36;
const FILAS: usize = 48;
/// Distancia del ojo a la pantalla, en altos del hueco. Menos = perspectiva más exagerada.
const DISTANCIA_OJO: f32 = 2.4;
/// Cuánto se transparenta la página a través del papel, vista por detrás.
const TRANSPARENCIA: f32 = 0.06;
/// Entre qué ángulos de giro (en medias vueltas) se desvanece la hoja al pasar al otro
/// lado del lomo: ahí ya queda fuera de la página y solo estorbaría.
const DESVANECER_DESDE: f32 = 0.45;
const DESVANECER_HASTA: f32 = 0.8;
/// Color del dorso de la hoja.
const PAPEL: Color32 = Color32::from_rgb(0xe6, 0xe2, 0xd8);
/// De dónde viene la luz: de arriba a la izquierda y por delante (x a la derecha, y hacia
/// arriba, z hacia quien mira).
const LUZ: [f32; 3] = [-0.5, 0.45, 0.75];
/// Luz que llega aunque la hoja esté de canto.
const AMBIENTE: f32 = 0.35;
/// Fuerza y dureza del brillo del papel en la curva.
const BRILLO: f32 = 0.35;
const DUREZA_BRILLO: i32 = 40;
/// Oscuridad máxima de la sombra que la hoja echa sobre la de debajo.
const SOMBRA: f32 = 0.5;

/// Las tres fases de la animación, con los valores del artículo. El vértice del cono va
/// en altos de página por debajo del borde inferior; el ángulo es lo abierto que está
/// el cono (90° es plano).
///
/// 1. Hasta el 15%: la esquina se despega y el cono se va cerrando.
/// 2. Hasta el 40%: la hoja, ya enrollada, se levanta casi sin cambiar de forma.
/// 3. Hasta el final: el cono se abre otra vez y la hoja cae plana al otro lado.
const VERTICE: [f32; 3] = [-15.0, -2.5, -3.5];
const ANGULO: [f32; 3] = [90.0, 8.0, 6.0];
const FIN_FASE_1: f32 = 0.15;
const FIN_FASE_2: f32 = 0.4;
/// Lo deprisa que se cierra el cono al despegar. En el artículo es 0,05 (casi de golpe),
/// pensado para seguir al dedo; en una animación sola queda brusco.
const CIERRE_INICIAL: f32 = 0.5;
/// Reparto del tiempo: más de 1 = despega despacio y acaba rápido, que es cuando la hoja
/// ya está fuera de la página.
const RITMO: f32 = 1.5;

/// Una hoja girando sobre otra que queda debajo.
pub struct Giro {
    /// La que gira y dónde está cuando está plana.
    pub girando: (TextureId, Rect),
    /// La que queda debajo, quieta.
    pub debajo: (TextureId, Rect),
    /// De 0 (al empezar) a 1 (al acabar).
    pub progreso: f32,
    /// Hacia delante, la hoja de encima se levanta y se va a la izquierda. Hacia atrás,
    /// la hoja llega desde la izquierda y se posa encima (la misma animación al revés).
    pub adelante: bool,
    /// Color por el que se multiplican las hojas (el del filtro de luz cálida).
    pub tinte: Color32,
}

/// Un punto de la hoja ya deformado: dónde cae en pantalla y dónde está en el espacio
/// (en anchos de página, con el lomo en x = 0 y el borde de abajo en y = 0).
#[derive(Clone, Copy)]
struct Punto {
    pantalla: Pos2,
    espacio: [f32; 3],
}

/// Un trozo de la malla, listo para pintar.
struct Trozo {
    /// Índices de sus cuatro esquinas en la malla, en el sentido de las agujas del reloj.
    esquinas: [usize; 4],
    profundidad: f32,
    de_frente: bool,
}

pub fn dibujar(pintor: &egui::Painter, hueco: Rect, giro: &Giro) {
    let t = giro.progreso.clamp(0.0, 1.0).powf(RITMO);
    let t = if giro.adelante { t } else { 1.0 - t };
    let rect_girando = giro.girando.1;
    let alto = rect_girando.height() / rect_girando.width().max(1.0);
    let (vertice, angulo, rotacion) = forma(t, alto);
    let levantada = rotacion.sin();

    // La de debajo, algo más oscura mientras la hoja la tapa.
    let uv_entero = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let (textura_debajo, rect_debajo) = giro.debajo;
    pintor.image(
        textura_debajo,
        rect_debajo,
        uv_entero,
        Color32::from_gray((255.0 * (1.0 - 0.2 * levantada)) as u8) * giro.tinte,
    );

    let (textura, rect) = giro.girando;
    let puntos = deformar(rect, hueco, vertice, angulo, rotacion);
    let trozos = ordenar(&puntos);

    // Pasada la vertical cae fuera de la página: se desvanece antes de tumbarse.
    let opacidad = ((DESVANECER_HASTA * PI - rotacion)
        / ((DESVANECER_HASTA - DESVANECER_DESDE) * PI))
        .clamp(0.0, 1.0);

    let sombra = pintor.with_clip_rect(rect_debajo.intersect(pintor.clip_rect()));
    pintar_sombra(&sombra, &puntos, &trozos, rect, hueco, opacidad);
    pintar_hoja(pintor, textura, &puntos, &trozos, opacidad, giro.tinte);
}

/// Dónde está el vértice del cono (en anchos de página), lo abierto que está y cuánto ha
/// girado la hoja sobre el lomo, en el momento `t` (de 0 a 1). `alto` es el alto de la
/// página en anchos.
fn forma(t: f32, alto: f32) -> (f32, f32, f32) {
    let mezclar = |f: f32, a: f32, b: f32| a + (b - a) * f;
    // Arranques y frenadas suaves, como en el artículo.
    let curva = |x: f32, exponente: f32| (PI * x.powf(exponente) / 2.0).sin();
    let [v1, v2, v3] = VERTICE.map(|v| v * alto);
    let [a1, a2, a3] = ANGULO.map(f32::to_radians);

    let (angulo, vertice) = if t <= FIN_FASE_1 {
        let x = t / FIN_FASE_1;
        (
            mezclar(curva(x, CIERRE_INICIAL), a1, a2),
            mezclar(curva(x, 0.5), v1, v2),
        )
    } else if t <= FIN_FASE_2 {
        let x = (t - FIN_FASE_1) / (FIN_FASE_2 - FIN_FASE_1);
        (mezclar(x, a2, a3), mezclar(x, v2, v3))
    } else {
        let x = (t - FIN_FASE_2) / (1.0 - FIN_FASE_2);
        (
            mezclar(curva(x, 10.0), a3, a1),
            mezclar(curva(x, 2.0), v3, v1),
        )
    };
    (vertice, angulo, t * PI)
}

/// Enrolla en el cono el punto (x, y) de la hoja plana y lo gira sobre el lomo.
fn enrollar(x: f32, y: f32, vertice: f32, angulo: f32, rotacion: f32) -> [f32; 3] {
    // Distancia al vértice: es el radio de la circunferencia del cono que pasa por el
    // punto, medido sobre la hoja; en el cono, esa circunferencia mide `radio`.
    let distancia = (x * x + (y - vertice).powi(2)).sqrt();
    let seno = angulo.sin();
    let radio = distancia * seno;
    // Cuánto da la vuelta alrededor del eje del cono. La longitud del arco se conserva:
    // el papel no se estira.
    let vuelta = (x / distancia).asin() / seno;
    let cx = radio * vuelta.sin();
    let cy = distancia + vertice - radio * (1.0 - vuelta.cos()) * seno;
    let cz = radio * (1.0 - vuelta.cos()) * angulo.cos();
    // Giro sobre el lomo (el eje y), levantándose hacia quien mira.
    let (s, c) = rotacion.sin_cos();
    [cx * c - cz * s, cy, cx * s + cz * c]
}

/// Calcula cada punto de la malla. Van por filas, de arriba abajo.
fn deformar(rect: Rect, hueco: Rect, vertice: f32, angulo: f32, rotacion: f32) -> Vec<Punto> {
    let ancho = rect.width().max(1.0);
    let alto = rect.height() / ancho;
    let ojo = DISTANCIA_OJO * hueco.height().max(1.0);
    let centro = hueco.center();

    let mut puntos = Vec::with_capacity((FILAS + 1) * (COLUMNAS + 1));
    for fila in 0..=FILAS {
        let v = fila as f32 / FILAS as f32;
        for columna in 0..=COLUMNAS {
            let u = columna as f32 / COLUMNAS as f32;
            let espacio = enrollar(u, (1.0 - v) * alto, vertice, angulo, rotacion);
            puntos.push(Punto {
                pantalla: proyectar(rect, espacio, ojo, centro),
                espacio,
            });
        }
    }
    puntos
}

/// De anchos de página a la pantalla, con perspectiva: z sale hacia quien mira, y lo que
/// se acerca se ve más grande.
fn proyectar(rect: Rect, [x, y, z]: [f32; 3], ojo: f32, centro: Pos2) -> Pos2 {
    let ancho = rect.width();
    let plano = egui::pos2(rect.min.x + x * ancho, rect.max.y - y * ancho);
    let escala = ojo / (ojo - z * ancho).max(ojo * 0.2);
    centro + (plano - centro) * escala
}

fn indice(fila: usize, columna: usize) -> usize {
    fila * (COLUMNAS + 1) + columna
}

/// Los trozos de la malla, del más lejano al más cercano: el orden en que hay que
/// pintarlos para que lo de delante tape a lo de detrás.
fn ordenar(puntos: &[Punto]) -> Vec<Trozo> {
    let mut trozos = Vec::with_capacity(FILAS * COLUMNAS);
    for fila in 0..FILAS {
        for columna in 0..COLUMNAS {
            let esquinas = [
                indice(fila, columna),
                indice(fila, columna + 1),
                indice(fila + 1, columna + 1),
                indice(fila + 1, columna),
            ];
            let [a, b, _, d] = esquinas.map(|i| puntos[i].pantalla);
            // Si al proyectarlo queda del revés, lo que se ve es el dorso.
            let (lado_u, lado_v) = (b - a, d - a);
            let de_frente = lado_u.x * lado_v.y - lado_u.y * lado_v.x >= 0.0;
            let profundidad = esquinas.iter().map(|&i| puntos[i].espacio[2]).sum::<f32>();
            trozos.push(Trozo {
                esquinas,
                profundidad,
                de_frente,
            });
        }
    }
    trozos.sort_by(|a, b| {
        a.profundidad
            .partial_cmp(&b.profundidad)
            .unwrap_or(Ordering::Equal)
    });
    trozos
}

fn normalizar([x, y, z]: [f32; 3]) -> [f32; 3] {
    let largo = (x * x + y * y + z * z).sqrt().max(1e-6);
    [x / largo, y / largo, z / largo]
}

fn restar(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn producto(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Hacia dónde mira la cara de delante de la hoja en cada punto, con los vecinos.
fn normales(puntos: &[Punto]) -> Vec<[f32; 3]> {
    let mut normales = Vec::with_capacity(puntos.len());
    for fila in 0..=FILAS {
        for columna in 0..=COLUMNAS {
            let p = |f: usize, c: usize| puntos[indice(f, c)].espacio;
            // A lo ancho, hacia el borde libre; a lo alto, hacia arriba (las filas van de
            // arriba abajo).
            let a_lo_ancho = restar(
                p(fila, (columna + 1).min(COLUMNAS)),
                p(fila, columna.saturating_sub(1)),
            );
            let a_lo_alto = restar(
                p(fila.saturating_sub(1), columna),
                p((fila + 1).min(FILAS), columna),
            );
            let [ax, ay, az] = a_lo_ancho;
            let [bx, by, bz] = a_lo_alto;
            normales.push(normalizar([
                ay * bz - az * by,
                az * bx - ax * bz,
                ax * by - ay * bx,
            ]));
        }
    }
    normales
}

/// Luz difusa y brillo en un punto, vista la cara de delante (o la de detrás).
fn iluminar(normal: [f32; 3], de_frente: bool) -> (f32, f32) {
    let luz = normalizar(LUZ);
    let medio = normalizar([luz[0], luz[1], luz[2] + 1.0]);
    let signo = if de_frente { 1.0 } else { -1.0 };
    let difusa = (signo * producto(normal, luz)).max(0.0);
    // Normalizada para que la hoja plana quede con su color tal cual.
    let plana = AMBIENTE + (1.0 - AMBIENTE) * luz[2];
    let difusa = ((AMBIENTE + (1.0 - AMBIENTE) * difusa) / plana).min(1.0);
    let plano_medio = medio[2].powi(DUREZA_BRILLO);
    let brillo = ((signo * producto(normal, medio))
        .max(0.0)
        .powi(DUREZA_BRILLO)
        - plano_medio)
        .max(0.0)
        * BRILLO;
    (difusa, brillo)
}

/// La sombra de la hoja levantada sobre la de debajo: cada punto se lleva al plano de la
/// página siguiendo la luz, más claro cuanto más alto está.
fn pintar_sombra(
    pintor: &egui::Painter,
    puntos: &[Punto],
    trozos: &[Trozo],
    rect: Rect,
    hueco: Rect,
    opacidad: f32,
) {
    let ojo = DISTANCIA_OJO * hueco.height().max(1.0);
    let centro = hueco.center();
    let luz = normalizar(LUZ);
    let mut malla = egui::Mesh::default();
    let sombras: Vec<(Pos2, Color32)> = puntos
        .iter()
        .map(|p| {
            let [x, y, z] = p.espacio;
            let z = z.max(0.0);
            let en_el_plano = [x - z * luz[0] / luz[2], y - z * luz[1] / luz[2], 0.0];
            let fuerza = SOMBRA * (1.0 - z / 1.2).clamp(0.0, 1.0) * opacidad;
            (
                proyectar(rect, en_el_plano, ojo, centro),
                Color32::from_black_alpha((255.0 * fuerza) as u8),
            )
        })
        .collect();
    // Solo los trozos de cara a la luz: así la hoja enrollada no se sombrea dos veces.
    for trozo in trozos.iter().filter(|t| t.de_frente) {
        let base = malla.vertices.len() as u32;
        for &i in &trozo.esquinas {
            malla.colored_vertex(sombras[i].0, sombras[i].1);
        }
        malla.add_triangle(base, base + 1, base + 2);
        malla.add_triangle(base, base + 2, base + 3);
    }
    pintor.add(malla);
}

/// Pinta los trozos en orden. Los seguidos del mismo lado van en las mismas mallas.
fn pintar_hoja(
    pintor: &egui::Painter,
    textura: TextureId,
    puntos: &[Punto],
    trozos: &[Trozo],
    opacidad: f32,
    tinte: Color32,
) {
    let normales = normales(puntos);
    let uv = |i: usize| {
        egui::pos2(
            (i % (COLUMNAS + 1)) as f32 / COLUMNAS as f32,
            (i / (COLUMNAS + 1)) as f32 / FILAS as f32,
        )
    };

    // Delante, la página; detrás, el papel y la página transparentándose; y encima de
    // las dos, el brillo (se suma a lo que hay debajo: alfa 0 en egui es aditivo).
    let mut frente = egui::Mesh::with_texture(textura);
    let mut papel = egui::Mesh::default();
    let mut transparencia = egui::Mesh::with_texture(textura);
    let mut brillos = egui::Mesh::default();
    let mut lado_actual = None;

    let vaciar = |mallas: [&mut egui::Mesh; 4]| {
        for malla in mallas {
            if !malla.is_empty() {
                let textura = malla.texture_id;
                pintor.add(std::mem::replace(malla, egui::Mesh::with_texture(textura)));
            }
        }
    };

    for trozo in trozos {
        if lado_actual.is_some_and(|lado| lado != trozo.de_frente) {
            vaciar([&mut frente, &mut papel, &mut transparencia, &mut brillos]);
        }
        lado_actual = Some(trozo.de_frente);

        let base = |malla: &egui::Mesh| malla.vertices.len() as u32;
        let (b_frente, b_papel, b_transparencia, b_brillos) = (
            base(&frente),
            base(&papel),
            base(&transparencia),
            base(&brillos),
        );
        for &i in &trozo.esquinas {
            let pos = puntos[i].pantalla;
            let (luz, brillo) = iluminar(normales[i], trozo.de_frente);
            let gris = |f: f32| (255.0 * f) as u8;
            // Luz de la escena teñida por el filtro: también el dorso y el brillo, para
            // que no asome blanco azulado al girar.
            let con_tinte = Color32::from_gray(gris(luz)) * tinte;
            if trozo.de_frente {
                frente.vertices.push(Vertex {
                    pos,
                    uv: uv(i),
                    color: con_tinte.gamma_multiply(opacidad),
                });
            } else {
                let papel_iluminado = Color32::from_rgb(
                    (PAPEL.r() as f32 * luz) as u8,
                    (PAPEL.g() as f32 * luz) as u8,
                    (PAPEL.b() as f32 * luz) as u8,
                );
                papel.colored_vertex(pos, (papel_iluminado * tinte).gamma_multiply(opacidad));
                // Al verse desde detrás, la imagen ya sale del revés sin hacer nada.
                transparencia.vertices.push(Vertex {
                    pos,
                    uv: uv(i),
                    color: con_tinte.gamma_multiply(TRANSPARENCIA * opacidad),
                });
            }
            let suma = Color32::from_gray(gris(brillo * opacidad)) * tinte;
            brillos.colored_vertex(
                pos,
                Color32::from_rgba_premultiplied(suma.r(), suma.g(), suma.b(), 0),
            );
        }
        let mallas: [(&mut egui::Mesh, u32); 4] = [
            (&mut frente, b_frente),
            (&mut papel, b_papel),
            (&mut transparencia, b_transparencia),
            (&mut brillos, b_brillos),
        ];
        for (malla, base) in mallas {
            if malla.vertices.len() as u32 == base + 4 {
                malla.add_triangle(base, base + 1, base + 2);
                malla.add_triangle(base, base + 2, base + 3);
            }
        }
    }
    vaciar([&mut frente, &mut papel, &mut transparencia, &mut brillos]);
}

/// Un rectángulo que pasa de un color a otro de izquierda a derecha.
pub fn degradado(pintor: &egui::Painter, rect: Rect, izquierda: Color32, derecha: Color32) {
    let mut malla = egui::Mesh::default();
    malla.colored_vertex(rect.left_top(), izquierda);
    malla.colored_vertex(rect.right_top(), derecha);
    malla.colored_vertex(rect.right_bottom(), derecha);
    malla.colored_vertex(rect.left_bottom(), izquierda);
    malla.add_triangle(0, 1, 2);
    malla.add_triangle(0, 2, 3);
    pintor.add(malla);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empieza_y_acaba_plana() {
        let (v, a, r) = forma(0.0, 1.5);
        let p = enrollar(0.7, 1.2, v, a, r);
        assert!((p[0] - 0.7).abs() < 1e-4 && (p[1] - 1.2).abs() < 1e-4 && p[2].abs() < 1e-4);

        // Al acabar, tumbada al otro lado del lomo.
        let (v, a, r) = forma(1.0, 1.5);
        let p = enrollar(0.7, 1.2, v, a, r);
        assert!((p[0] + 0.7).abs() < 1e-3 && (p[1] - 1.2).abs() < 1e-3 && p[2].abs() < 1e-3);
    }

    #[test]
    fn el_lomo_no_se_mueve() {
        for paso in 0..=20 {
            let (v, a, r) = forma(paso as f32 / 20.0, 1.5);
            let p = enrollar(0.0, 0.8, v, a, r);
            assert!(p[0].abs() < 1e-4 && (p[1] - 0.8).abs() < 1e-4 && p[2].abs() < 1e-4);
        }
    }

    #[test]
    fn el_papel_no_se_estira() {
        // La distancia entre dos puntos vecinos de una fila se mantiene al enrollarla.
        let (v, a, r) = forma(0.3, 1.5);
        let paso = 0.01;
        let mut largo = 0.0;
        let mut anterior = enrollar(0.0, 0.5, v, a, r);
        let mut x = paso;
        while x <= 1.0 + 1e-6 {
            let p = enrollar(x, 0.5, v, a, r);
            let d = restar(p, anterior);
            largo += producto(d, d).sqrt();
            anterior = p;
            x += paso;
        }
        // En la hoja plana la fila mide 1, y enrollada tiene que seguir midiendo 1.
        assert!((largo - 1.0).abs() < 0.01, "largo {largo}");
    }
}
