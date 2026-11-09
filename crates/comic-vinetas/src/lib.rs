//! Encuentra los recuadros (viñetas) de una página y los ordena para leerlos uno a uno.
//!
//! Sin IA: se busca el fondo de la página (las calles blancas o negras entre recuadros)
//! rellenando desde los bordes. Lo que el relleno no alcanza es contenido. Cada mancha
//! de contenido es un recuadro o varios unidos por un bocadillo o una onomatopeya que
//! cruza la calle; esos se separan buscando filas o columnas que sean casi todo fondo.
//!
//! El orden sale de cortar el conjunto de recuadros en filas (de arriba abajo) y cada
//! fila en columnas: de derecha a izquierda en manga, al revés en cómic occidental.

/// Lado mayor, en píxeles, de la copia reducida sobre la que se trabaja. Basta para ver
/// las calles y hace que una página tarde pocos milisegundos.
const LADO_TRABAJO: u32 = 1200;
/// Cuánto puede separarse un píxel del color del fondo y seguir contando como fondo.
const TOLERANCIA_FONDO: i32 = 40;
/// Fracción mínima de fondo alcanzado para que una fila o columna cuente como calle.
/// No es 1 porque un bocadillo o una onomatopeya pueden cruzarla.
const CALLE_MINIMA: f32 = 0.85;
/// Hasta cuántas líneas (de la copia reducida) hacia dentro se busca el marco.
const BUSQUEDA_MARCO: u32 = 12;
/// Qué parte de una línea tiene que ser tinta para contar como marco.
const MARCO_MINIMO: f32 = 0.6;
/// Separación máxima, en píxeles de la copia reducida, entre dos trozos de un recuadro.
const HUECO_TROZOS: u32 = 3;
/// Qué parte de un recuadro tiene que quedar dentro de otro mayor para descartarlo.
const DENTRO: f32 = 0.7;
/// Lado mínimo de un recuadro, relativo al lado de la página. Lo más pequeño (números
/// de página, notas al margen, motas del escaneo) se descarta.
const LADO_MINIMO: f32 = 0.06;
/// Superficie mínima de un recuadro, relativa a la página.
const AREA_MINIMA: f32 = 0.008;
/// Cuánto pueden solaparse dos recuadros al ordenarlos sin dejar de estar en filas o
/// columnas distintas, relativo al lado de la página.
const SOLAPE_ORDEN: f32 = 0.02;
/// Lo mismo relativo al propio recuadro: los recuadros inclinados se meten bastante en
/// la fila de al lado sin dejar de ser de otra fila.
const SOLAPE_ORDEN_RELATIVO: f32 = 0.2;

/// En qué orden se leen los recuadros de una fila.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sentido {
    /// Cómic occidental.
    IzquierdaADerecha,
    /// Manga.
    DerechaAIzquierda,
}

/// Un rectángulo en píxeles de la página original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recuadro {
    pub x: u32,
    pub y: u32,
    pub ancho: u32,
    pub alto: u32,
}

/// Los recuadros de una página en RGBA (`ancho * alto * 4` bytes), en orden de lectura.
///
/// Vacío si no se encontró nada que parezca un recuadro: entonces conviene mostrar la
/// página entera.
pub fn detectar(rgba: &[u8], ancho: u32, alto: u32, sentido: Sentido) -> Vec<Recuadro> {
    if ancho == 0 || alto == 0 || rgba.len() < (ancho * alto * 4) as usize {
        return Vec::new();
    }
    let pagina = Reducida::de_rgba(rgba, ancho, alto);
    let (alcanzado, tinta) = pagina.fondo_desde_bordes();

    let mut cajas = Vec::new();
    for mancha in manchas(&alcanzado, pagina.ancho, pagina.alto) {
        partir(&alcanzado, pagina.ancho, mancha, &pagina, &mut cajas);
    }
    unir_trozos(&mut cajas, &tinta, pagina.ancho);
    cajas.retain(|c| pagina.tamano_suficiente(c));
    quitar_contenidas(&mut cajas);

    ordenar(&mut cajas, &pagina, sentido);
    cajas
        .iter()
        .map(|c| pagina.a_original(c, ancho, alto))
        .collect()
}

/// La página en gris y reducida a `LADO_TRABAJO`, haciendo la media de cada bloque.
struct Reducida {
    ancho: u32,
    alto: u32,
    /// Cuántos píxeles originales mide de lado cada píxel de aquí.
    factor: u32,
    gris: Vec<u8>,
}

/// Un rectángulo en píxeles de la página reducida: [x0, x1) x [y0, y1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Caja {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl Caja {
    fn ancho(&self) -> u32 {
        self.x1 - self.x0
    }

    fn alto(&self) -> u32 {
        self.y1 - self.y0
    }
}

impl Reducida {
    fn de_rgba(rgba: &[u8], ancho: u32, alto: u32) -> Reducida {
        let factor = ancho.max(alto).div_ceil(LADO_TRABAJO).max(1);
        let (ra, rh) = (ancho.div_ceil(factor), alto.div_ceil(factor));
        let mut suma = vec![0u32; (ra * rh) as usize];
        let mut cuenta = vec![0u32; (ra * rh) as usize];

        for y in 0..alto {
            let fila = &rgba[(y * ancho * 4) as usize..((y + 1) * ancho * 4) as usize];
            let base = (y / factor * ra) as usize;
            for (x, p) in fila.as_chunks::<4>().0.iter().enumerate() {
                // Luminancia aproximada (pesos de Rec. 601 en enteros).
                let g = (p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8;
                let i = base + x / factor as usize;
                suma[i] += g;
                cuenta[i] += 1;
            }
        }
        let gris = suma
            .iter()
            .zip(&cuenta)
            .map(|(s, c)| (s / c) as u8)
            .collect();
        Reducida {
            ancho: ra,
            alto: rh,
            factor,
            gris,
        }
    }

    /// Color del fondo: la mediana de un marco fino alrededor de la página.
    fn color_fondo(&self) -> i32 {
        let grosor = 3.min(self.ancho / 2).min(self.alto / 2).max(1);
        let mut marco = Vec::new();
        for y in 0..self.alto {
            for x in 0..self.ancho {
                let borde =
                    x < grosor || y < grosor || x >= self.ancho - grosor || y >= self.alto - grosor;
                if borde {
                    marco.push(self.gris[(y * self.ancho + x) as usize]);
                }
            }
        }
        marco.sort_unstable();
        marco[marco.len() / 2] as i32
    }

    /// Qué píxeles son fondo unido a los bordes de la página: las calles y el margen.
    /// Devuelve eso y qué píxeles son tinta (lo que se aparta del color del fondo).
    ///
    /// Antes se engorda un píxel la tinta, para que el relleno no se cuele por un hueco
    /// diminuto en el borde de un recuadro (un trazo que no llega a cerrarse, el JPEG).
    fn fondo_desde_bordes(&self) -> (Vec<bool>, Vec<bool>) {
        let (an, al) = (self.ancho as usize, self.alto as usize);
        let fondo = self.color_fondo();
        let tinta: Vec<bool> = self
            .gris
            .iter()
            .map(|&g| (g as i32 - fondo).abs() > TOLERANCIA_FONDO)
            .collect();

        let mut gruesa = tinta.clone();
        for y in 0..al {
            for x in 0..an {
                if tinta[y * an + x] {
                    for vy in y.saturating_sub(1)..(y + 2).min(al) {
                        for vx in x.saturating_sub(1)..(x + 2).min(an) {
                            gruesa[vy * an + vx] = true;
                        }
                    }
                }
            }
        }

        let mut alcanzado = vec![false; an * al];
        let mut pendientes = Vec::new();
        let sembrar = |i: usize, alcanzado: &mut Vec<bool>, pendientes: &mut Vec<usize>| {
            if !gruesa[i] && !alcanzado[i] {
                alcanzado[i] = true;
                pendientes.push(i);
            }
        };
        for x in 0..an {
            sembrar(x, &mut alcanzado, &mut pendientes);
            sembrar((al - 1) * an + x, &mut alcanzado, &mut pendientes);
        }
        for y in 0..al {
            sembrar(y * an, &mut alcanzado, &mut pendientes);
            sembrar(y * an + an - 1, &mut alcanzado, &mut pendientes);
        }
        while let Some(i) = pendientes.pop() {
            let (x, y) = (i % an, i / an);
            if x > 0 {
                sembrar(i - 1, &mut alcanzado, &mut pendientes);
            }
            if x + 1 < an {
                sembrar(i + 1, &mut alcanzado, &mut pendientes);
            }
            if y > 0 {
                sembrar(i - an, &mut alcanzado, &mut pendientes);
            }
            if y + 1 < al {
                sembrar(i + an, &mut alcanzado, &mut pendientes);
            }
        }
        (alcanzado, tinta)
    }

    fn tamano_suficiente(&self, c: &Caja) -> bool {
        let lado = |medida: u32, total: u32| medida as f32 >= total as f32 * LADO_MINIMO;
        let area = c.ancho() as f32 * c.alto() as f32;
        lado(c.ancho(), self.ancho)
            && lado(c.alto(), self.alto)
            && area >= self.ancho as f32 * self.alto as f32 * AREA_MINIMA
    }

    fn a_original(&self, c: &Caja, ancho: u32, alto: u32) -> Recuadro {
        let x = (c.x0 * self.factor).min(ancho);
        let y = (c.y0 * self.factor).min(alto);
        Recuadro {
            x,
            y,
            ancho: (c.x1 * self.factor).min(ancho) - x,
            alto: (c.y1 * self.factor).min(alto) - y,
        }
    }
}

/// Cajas de cada mancha de contenido (lo no alcanzado por el fondo), uniendo en diagonal.
fn manchas(alcanzado: &[bool], ancho: u32, alto: u32) -> Vec<Caja> {
    let (an, al) = (ancho as usize, alto as usize);
    let mut vista = vec![false; an * al];
    let mut cajas = Vec::new();
    let mut pendientes = Vec::new();

    for inicio in 0..an * al {
        if alcanzado[inicio] || vista[inicio] {
            continue;
        }
        vista[inicio] = true;
        pendientes.push(inicio);
        let (mut x0, mut y0, mut x1, mut y1) = (an, al, 0, 0);
        while let Some(i) = pendientes.pop() {
            let (x, y) = (i % an, i / an);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            for vy in y.saturating_sub(1)..(y + 2).min(al) {
                for vx in x.saturating_sub(1)..(x + 2).min(an) {
                    let v = vy * an + vx;
                    if !alcanzado[v] && !vista[v] {
                        vista[v] = true;
                        pendientes.push(v);
                    }
                }
            }
        }
        cajas.push(Caja {
            x0: x0 as u32,
            y0: y0 as u32,
            x1: x1 as u32,
            y1: y1 as u32,
        });
    }
    cajas
}

/// Separa una mancha en recuadros por las calles que la cruzan, primero en filas y
/// luego cada fila en columnas, y deja cada trozo ceñido a su contenido.
fn partir(alcanzado: &[bool], ancho: u32, caja: Caja, pagina: &Reducida, salida: &mut Vec<Caja>) {
    let Some(caja) = ajustar(alcanzado, ancho, caja) else {
        return;
    };
    for horizontal in [true, false] {
        let trozos = cortar(alcanzado, ancho, caja, pagina, horizontal);
        if trozos.len() > 1 {
            for trozo in trozos {
                partir(alcanzado, ancho, trozo, pagina, salida);
            }
            return;
        }
    }
    salida.push(caja);
}

/// Corta la caja por sus calles horizontales (en filas) o verticales (en columnas).
/// Una calle es una franja de líneas con al menos `CALLE_MINIMA` de fondo. Sin calles,
/// devuelve la caja entera.
fn cortar(
    alcanzado: &[bool],
    ancho: u32,
    caja: Caja,
    pagina: &Reducida,
    horizontal: bool,
) -> Vec<Caja> {
    let fondo = |x: u32, y: u32| alcanzado[(y * ancho + x) as usize];
    // Una calle tiene que medir algo: una sola línea de fondo puede ser casualidad.
    let grosor = (pagina.ancho.max(pagina.alto) / 300).max(2);

    let (calles, largo, minimo): (Vec<bool>, u32, f32) = if horizontal {
        let calles = (caja.y0..caja.y1)
            .map(|y| {
                let n = (caja.x0..caja.x1).filter(|&x| fondo(x, y)).count();
                n as f32 >= caja.ancho() as f32 * CALLE_MINIMA
            })
            .collect();
        (calles, caja.alto(), pagina.alto as f32 * LADO_MINIMO)
    } else {
        let calles = (caja.x0..caja.x1)
            .map(|x| {
                let n = (caja.y0..caja.y1).filter(|&y| fondo(x, y)).count();
                n as f32 >= caja.alto() as f32 * CALLE_MINIMA
            })
            .collect();
        (calles, caja.ancho(), pagina.ancho as f32 * LADO_MINIMO)
    };

    let cortes = buscar_cortes(&calles, grosor, minimo as u32);
    tramos(&cortes, largo)
        .into_iter()
        .map(|(a, b)| {
            if horizontal {
                Caja {
                    y0: caja.y0 + a,
                    y1: caja.y0 + b,
                    ..caja
                }
            } else {
                Caja {
                    x0: caja.x0 + a,
                    x1: caja.x0 + b,
                    ..caja
                }
            }
        })
        .collect()
}

/// Un lado de una caja.
#[derive(Clone, Copy)]
enum Lado {
    Arriba,
    Abajo,
    Izquierda,
    Derecha,
}

/// Cuánto se parece el lado `lado` de la caja a un marco: la mejor línea recta de tinta
/// cerca de ese lado, entre `desde` y `hasta` (columnas para arriba y abajo, filas para
/// izquierda y derecha). Da la parte de esa línea que es tinta: casi 1 si hay marco.
///
/// Se busca hacia dentro y no solo en el borde de la caja, porque un bocadillo que cruza
/// la calle hace que la caja llegue más allá del marco. El borde que deja el fondo al
/// colarse en un recuadro a sangre (cielo, nieve) es irregular y ninguna línea recta lo
/// sigue entera.
fn parecido_a_marco(
    tinta: &[bool],
    ancho: u32,
    c: &Caja,
    lado: Lado,
    desde: u32,
    hasta: u32,
) -> f32 {
    if hasta <= desde {
        return 1.0;
    }
    let es_tinta = |x: u32, y: u32| tinta[(y * ancho + x) as usize];
    // Las líneas a revisar, de fuera hacia dentro, y si son filas (true) o columnas.
    let (lineas, filas): (Vec<u32>, bool) = match lado {
        Lado::Arriba => ((c.y0..c.y1).take(BUSQUEDA_MARCO as usize).collect(), true),
        Lado::Abajo => (
            (c.y0..c.y1).rev().take(BUSQUEDA_MARCO as usize).collect(),
            true,
        ),
        Lado::Izquierda => ((c.x0..c.x1).take(BUSQUEDA_MARCO as usize).collect(), false),
        Lado::Derecha => (
            (c.x0..c.x1).rev().take(BUSQUEDA_MARCO as usize).collect(),
            false,
        ),
    };
    let (primera, ultima) = if filas { (c.y0, c.y1) } else { (c.x0, c.x1) };
    lineas
        .iter()
        .map(|&l| {
            // El marco puede salir algo torcido en el escaneo: vale la línea o sus vecinas.
            let vecinas = l.saturating_sub(1).max(primera)..(l + 2).min(ultima);
            let llenas = (desde..hasta)
                .filter(|&i| {
                    vecinas.clone().any(|v| {
                        if filas {
                            es_tinta(i, v)
                        } else {
                            es_tinta(v, i)
                        }
                    })
                })
                .count();
            llenas as f32 / (hasta - desde) as f32
        })
        .fold(0.0, f32::max)
}

/// Junta los trozos de un mismo recuadro. Cuando un recuadro llega al borde de la
/// página y tiene zonas claras (cielo, nieve), el fondo se cuela dentro y lo parte.
/// Esos trozos quedan pegados (sin calle de verdad entre ellos) y el lado por el que
/// se tocan no tiene marco. Dos recuadros de verdad, aunque estén muy juntos, tienen
/// marco en los dos lados.
fn unir_trozos(cajas: &mut Vec<Caja>, tinta: &[bool], ancho: u32) {
    loop {
        let mut pareja = None;
        'buscar: for i in 0..cajas.len() {
            for j in 0..cajas.len() {
                if i != j && son_trozos(&cajas[i], &cajas[j], tinta, ancho) {
                    pareja = Some((i, j));
                    break 'buscar;
                }
            }
        }
        let Some((i, j)) = pareja else {
            return;
        };
        let (a, b) = (cajas[i], cajas[j]);
        cajas[i] = Caja {
            x0: a.x0.min(b.x0),
            y0: a.y0.min(b.y0),
            x1: a.x1.max(b.x1),
            y1: a.y1.max(b.y1),
        };
        cajas.remove(j);
    }
}

/// Si `a` está justo encima o justo a la izquierda de `b` y ninguna de las dos tiene marco
/// en el lado por el que se tocan.
fn son_trozos(a: &Caja, b: &Caja, tinta: &[bool], ancho: u32) -> bool {
    let pegadas = |fin: u32, inicio: u32| fin <= inicio && inicio - fin <= HUECO_TROZOS;
    let sin_marco = |x: f32, y: f32| x.max(y) < MARCO_MINIMO;

    // Encima: comparten columnas y el lado de abajo de `a` toca el de arriba de `b`.
    let (x0, x1) = (a.x0.max(b.x0), a.x1.min(b.x1));
    if x1 > x0 && pegadas(a.y1, b.y0) {
        let abajo = parecido_a_marco(tinta, ancho, a, Lado::Abajo, x0, x1);
        let arriba = parecido_a_marco(tinta, ancho, b, Lado::Arriba, x0, x1);
        return sin_marco(abajo, arriba);
    }
    // A la izquierda: comparten filas.
    let (y0, y1) = (a.y0.max(b.y0), a.y1.min(b.y1));
    if y1 > y0 && pegadas(a.x1, b.x0) {
        let derecha = parecido_a_marco(tinta, ancho, a, Lado::Derecha, y0, y1);
        let izquierda = parecido_a_marco(tinta, ancho, b, Lado::Izquierda, y0, y1);
        return sin_marco(derecha, izquierda);
    }
    false
}

/// Descarta los recuadros que caen casi enteros dentro de otro mayor: trozos de un
/// recuadro que el fondo ha partido sin llegar a separarlo.
fn quitar_contenidas(cajas: &mut Vec<Caja>) {
    let area = |c: &Caja| c.ancho() as u64 * c.alto() as u64;
    let comun = |a: &Caja, b: &Caja| {
        let ancho = a.x1.min(b.x1).saturating_sub(a.x0.max(b.x0)) as u64;
        let alto = a.y1.min(b.y1).saturating_sub(a.y0.max(b.y0)) as u64;
        ancho * alto
    };
    let copia = cajas.clone();
    let mut indice = 0;
    cajas.retain(|c| {
        let yo = indice;
        indice += 1;
        // Entre dos iguales (salen repetidas a veces) se queda la primera.
        let mayor = |j: usize, otra: &Caja| area(otra) > area(c) || (otra == c && j < yo);
        !copia.iter().enumerate().any(|(j, otra)| {
            j != yo && mayor(j, otra) && comun(c, otra) as f32 >= area(c) as f32 * DENTRO
        })
    });
}

/// Encoge la caja hasta el contenido que tiene dentro. `None` si no tiene nada.
fn ajustar(alcanzado: &[bool], ancho: u32, caja: Caja) -> Option<Caja> {
    let contenido = |x: u32, y: u32| !alcanzado[(y * ancho + x) as usize];
    let fila = |y: u32| (caja.x0..caja.x1).any(|x| contenido(x, y));
    let columna = |x: u32| (caja.y0..caja.y1).any(|y| contenido(x, y));

    let y0 = (caja.y0..caja.y1).find(|&y| fila(y))?;
    let y1 = (caja.y0..caja.y1).rev().find(|&y| fila(y))? + 1;
    let x0 = (caja.x0..caja.x1).find(|&x| columna(x))?;
    let x1 = (caja.x0..caja.x1).rev().find(|&x| columna(x))? + 1;
    Some(Caja { x0, y0, x1, y1 })
}

/// Posiciones (centro de cada calle) donde cortar una lista de filas o columnas en la
/// que `calle[i]` dice si esa línea es casi todo fondo. Solo valen las calles de al
/// menos `grosor` líneas que dejan a cada lado un trozo de `minimo` líneas o más.
fn buscar_cortes(calle: &[bool], grosor: u32, minimo: u32) -> Vec<u32> {
    let mut candidatos = Vec::new();
    let mut i = 0;
    while i < calle.len() {
        if !calle[i] {
            i += 1;
            continue;
        }
        let inicio = i;
        while i < calle.len() && calle[i] {
            i += 1;
        }
        if (i - inicio) as u32 >= grosor {
            candidatos.push(((inicio + i) / 2) as u32);
        }
    }

    // Se quitan los cortes que dejarían un trozo demasiado pequeño, sin separarlo.
    let total = calle.len() as u32;
    let mut cortes: Vec<u32> = Vec::new();
    for c in candidatos {
        let anterior = cortes.last().copied().unwrap_or(0);
        if c - anterior >= minimo && total - c >= minimo {
            cortes.push(c);
        }
    }
    cortes
}

/// Los tramos [a, b) que quedan entre los cortes.
fn tramos(cortes: &[u32], total: u32) -> Vec<(u32, u32)> {
    let mut bordes = vec![0];
    bordes.extend_from_slice(cortes);
    bordes.push(total);
    bordes.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Ordena los recuadros para leerlos: en filas de arriba abajo y, dentro de cada fila,
/// en columnas según `sentido`. Cada fila o columna se ordena igual por dentro.
fn ordenar(cajas: &mut Vec<Caja>, pagina: &Reducida, sentido: Sentido) {
    let solape_y = (pagina.alto as f32 * SOLAPE_ORDEN) as u32;
    let solape_x = (pagina.ancho as f32 * SOLAPE_ORDEN) as u32;
    *cajas = ordenar_grupo(std::mem::take(cajas), solape_x, solape_y, sentido);
}

fn ordenar_grupo(
    mut cajas: Vec<Caja>,
    solape_x: u32,
    solape_y: u32,
    sentido: Sentido,
) -> Vec<Caja> {
    if cajas.len() <= 1 {
        return cajas;
    }

    // Filas: una caja empieza fila nueva si queda por debajo de todo lo anterior.
    cajas.sort_by_key(|c| (c.y0, c.x0));
    let filas = agrupar(&cajas, |c| c.y0, |c| c.y1, solape_y);
    if filas.len() > 1 {
        return filas
            .into_iter()
            .flat_map(|fila| ordenar_grupo(fila, solape_x, solape_y, sentido))
            .collect();
    }

    // Columnas, empezando por el lado por el que se lee.
    let columnas = match sentido {
        Sentido::IzquierdaADerecha => {
            cajas.sort_by_key(|c| (c.x0, c.y0));
            agrupar(&cajas, |c| c.x0, |c| c.x1, solape_x)
        }
        Sentido::DerechaAIzquierda => {
            // Con las coordenadas dadas la vuelta, "de derecha a izquierda" es
            // "de menor a mayor", igual que en el otro sentido.
            cajas.sort_by_key(|c| (u32::MAX - c.x1, c.y0));
            agrupar(&cajas, |c| u32::MAX - c.x1, |c| u32::MAX - c.x0, solape_x)
        }
    };
    if columnas.len() > 1 {
        return columnas
            .into_iter()
            .flat_map(|columna| ordenar_grupo(columna, solape_x, solape_y, sentido))
            .collect();
    }

    // Recuadros entrelazados que no se dejan cortar ni en filas ni en columnas: por
    // dónde empiezan, de arriba abajo y luego en el sentido de lectura.
    cajas.sort_by_key(|c| match sentido {
        Sentido::IzquierdaADerecha => (c.y0, c.x0),
        Sentido::DerechaAIzquierda => (c.y0, u32::MAX - c.x1),
    });
    cajas
}

/// Reparte cajas ya ordenadas por `inicio` en grupos que no se solapan en esa dirección,
/// salvo un poco: `solape` píxeles o una parte del propio recuadro, lo que sea mayor.
fn agrupar(
    cajas: &[Caja],
    inicio: impl Fn(&Caja) -> u32,
    fin: impl Fn(&Caja) -> u32,
    solape: u32,
) -> Vec<Vec<Caja>> {
    let mut grupos: Vec<Vec<Caja>> = Vec::new();
    let mut fin_grupo: u32 = 0;
    for c in cajas {
        let largo = fin(c) - inicio(c);
        let permitido = solape.max((largo as f32 * SOLAPE_ORDEN_RELATIVO) as u32);
        let metido = fin_grupo.saturating_sub(inicio(c));
        let nuevo = grupos.is_empty() || metido <= permitido;
        if nuevo {
            grupos.push(Vec::new());
            fin_grupo = fin(c);
        } else {
            fin_grupo = fin_grupo.max(fin(c));
        }
        grupos.last_mut().unwrap().push(*c);
    }
    grupos
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Página blanca de 1000 x 1400 con un marco negro de 4 px por cada rectángulo.
    fn pagina(recuadros: &[(u32, u32, u32, u32)]) -> Vec<u8> {
        let (an, al) = (1000u32, 1400u32);
        let mut rgba = vec![255u8; (an * al * 4) as usize];
        for &(x0, y0, x1, y1) in recuadros {
            for y in y0..y1 {
                for x in x0..x1 {
                    let borde = x < x0 + 4 || x >= x1 - 4 || y < y0 + 4 || y >= y1 - 4;
                    if borde {
                        let i = ((y * an + x) * 4) as usize;
                        rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
                    }
                }
            }
        }
        rgba
    }

    fn esquinas(r: &[Recuadro]) -> Vec<(u32, u32)> {
        // Redondeado a decenas: el borde sale algún píxel antes (se engorda la tinta).
        r.iter()
            .map(|r| ((r.x + 5) / 10 * 10, (r.y + 5) / 10 * 10))
            .collect()
    }

    const REJILLA: [(u32, u32, u32, u32); 4] = [
        (50, 50, 480, 680),
        (520, 50, 950, 680),
        (50, 720, 480, 1350),
        (520, 720, 950, 1350),
    ];

    #[test]
    fn manga_de_derecha_a_izquierda() {
        let r = detectar(&pagina(&REJILLA), 1000, 1400, Sentido::DerechaAIzquierda);
        assert_eq!(esquinas(&r), [(520, 50), (50, 50), (520, 720), (50, 720)]);
    }

    #[test]
    fn occidental_de_izquierda_a_derecha() {
        let r = detectar(&pagina(&REJILLA), 1000, 1400, Sentido::IzquierdaADerecha);
        assert_eq!(esquinas(&r), [(50, 50), (520, 50), (50, 720), (520, 720)]);
    }

    #[test]
    fn columna_alta_a_la_derecha_va_primero() {
        // Un recuadro alto a la derecha y dos apilados a la izquierda: en manga se lee
        // el alto y luego los dos de la izquierda de arriba abajo.
        let recuadros = [
            (600, 50, 950, 1350),
            (50, 50, 560, 680),
            (50, 720, 560, 1350),
        ];
        let r = detectar(&pagina(&recuadros), 1000, 1400, Sentido::DerechaAIzquierda);
        assert_eq!(esquinas(&r), [(600, 50), (50, 50), (50, 720)]);
    }

    #[test]
    fn un_bocadillo_que_cruza_la_calle_no_une_los_recuadros() {
        let mut rgba = pagina(&REJILLA);
        // Una mancha negra pequeña que tapa la calle entre los dos de arriba.
        for y in 300..340 {
            for x in 470..530 {
                let i = ((y * 1000 + x) * 4) as usize;
                rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let r = detectar(&rgba, 1000, 1400, Sentido::DerechaAIzquierda);
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn pagina_en_blanco_no_tiene_recuadros() {
        assert!(detectar(&pagina(&[]), 1000, 1400, Sentido::DerechaAIzquierda).is_empty());
    }

    #[test]
    fn se_ignoran_las_motas() {
        let r = detectar(
            &pagina(&[(500, 1370, 520, 1390)]),
            1000,
            1400,
            Sentido::DerechaAIzquierda,
        );
        assert!(r.is_empty());
    }
}
