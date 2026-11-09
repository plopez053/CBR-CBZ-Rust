//! Detecta los recuadros de varias páginas de un cómic y guarda copias de esas páginas
//! con cada recuadro marcado y numerado en el orden de lectura.
//!
//! Uso: `cargo run -p comic-vinetas --example probar -- <comic> <carpeta_salida> [opciones]`
//!
//! Opciones:
//! - `--paginas N`: cuántas páginas probar, repartidas por todo el cómic (16 si no se dice).
//! - `--todas`: todas las páginas.
//! - `--pagina N`: solo esa página (empezando en 1), con los recuadros escritos en la consola.
//! - `--occidental`: leer de izquierda a derecha. Si no, de derecha a izquierda (manga).
//!
//! Además de cada página, deja `hoja-N.jpg`: seis páginas reducidas por hoja, para
//! revisarlas de un vistazo.

use std::path::{Path, PathBuf};
use std::time::Instant;

use comic_vinetas::{Recuadro, Sentido};

/// Colores de los marcos, alternados para distinguir recuadros vecinos.
const COLORES: [[u8; 3]; 4] = [[230, 40, 40], [30, 150, 230], [40, 180, 60], [220, 140, 0]];

fn main() -> Result<(), String> {
    let mut argumentos = std::env::args().skip(1);
    let uso =
        "Uso: probar <comic> <carpeta_salida> [--paginas N] [--todas] [--pagina N] [--occidental]";
    let ruta_comic = PathBuf::from(argumentos.next().ok_or(uso)?);
    let salida = PathBuf::from(argumentos.next().ok_or(uso)?);
    let mut cuantas = Some(16usize);
    let mut sentido = Sentido::DerechaAIzquierda;
    let mut una: Option<usize> = None;
    while let Some(a) = argumentos.next() {
        match a.as_str() {
            "--paginas" => {
                let n = argumentos.next().ok_or(uso)?;
                cuantas = Some(n.parse().map_err(|_| format!("No es un número: {n}"))?);
            }
            "--todas" => cuantas = None,
            "--pagina" => {
                let n = argumentos.next().ok_or(uso)?;
                let n: usize = n.parse().map_err(|_| format!("No es un número: {n}"))?;
                una = Some(n.max(1) - 1);
            }
            "--occidental" => sentido = Sentido::IzquierdaADerecha,
            otro => return Err(format!("Opción desconocida: {otro}\n{uso}")),
        }
    }

    let mut comic = comic_core::Comic::abrir(&ruta_comic)?;
    let total = comic.paginas().len();
    let elegidas: Vec<usize> = match (una, cuantas) {
        (Some(i), _) => vec![i],
        (None, Some(n)) if n < total => (0..n).map(|i| i * total / n).collect(),
        _ => (0..total).collect(),
    };
    std::fs::create_dir_all(&salida).map_err(|e| e.to_string())?;
    let nombre = ruta_comic
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    eprintln!(
        "[vinetas] {nombre}: {total} páginas, se prueban {}",
        elegidas.len()
    );

    let mut tiempo_total = 0.0;
    let mut encogedor = comic_imagen::Encogedor::default();
    let mut reducidas = Vec::new();
    for indice in elegidas {
        let bytes = comic.leer(indice)?;
        let mut imagen = comic_imagen::decodificar(&bytes)?;

        let inicio = Instant::now();
        let recuadros = comic_vinetas::detectar(&imagen.rgba, imagen.ancho, imagen.alto, sentido);
        let ms = inicio.elapsed().as_secs_f64() * 1000.0;
        tiempo_total += ms;

        if una.is_some() {
            for (n, r) in recuadros.iter().enumerate() {
                println!(
                    "{:2}: x {:4}..{:4}  y {:4}..{:4}",
                    n + 1,
                    r.x,
                    r.x + r.ancho,
                    r.y,
                    r.y + r.alto
                );
            }
        }
        pintar(&mut imagen, &recuadros);
        let (ancho, alto) = (imagen.ancho, imagen.alto);
        let archivo = salida.join(format!("{}-p{:03}.jpg", limpiar(&nombre), indice + 1));
        guardar(&imagen, &archivo)?;
        reducidas.push(reducir(&mut encogedor, imagen, indice + 1)?);
        println!(
            "pagina {:3}  {}x{}  {:2} recuadros  {:5.1} ms",
            indice + 1,
            ancho,
            alto,
            recuadros.len(),
            ms
        );
    }
    for (n, grupo) in reducidas.chunks(HOJA.0 * HOJA.1).enumerate() {
        guardar(&hoja(grupo), &salida.join(format!("hoja-{n}.jpg")))?;
    }
    eprintln!("[vinetas] detección: {tiempo_total:.0} ms en total");
    eprintln!("[vinetas] imágenes en {}", salida.display());
    Ok(())
}

fn guardar(imagen: &comic_imagen::Imagen, archivo: &Path) -> Result<(), String> {
    let jpeg = comic_imagen::codificar_jpeg(imagen, 85)?;
    std::fs::write(archivo, jpeg).map_err(|e| e.to_string())
}

/// Nombre de archivo sin caracteres raros.
fn limpiar(nombre: &str) -> String {
    nombre
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Marca cada recuadro con un marco de color y su número en la esquina superior izquierda.
fn pintar(imagen: &mut comic_imagen::Imagen, recuadros: &[Recuadro]) {
    let grosor = (imagen.ancho.max(imagen.alto) / 250).max(3);
    for (n, r) in recuadros.iter().enumerate() {
        let color = COLORES[n % COLORES.len()];
        let (x1, y1) = (r.x + r.ancho, r.y + r.alto);
        rellenar(imagen, r.x, r.y, x1, (r.y + grosor).min(y1), color);
        rellenar(imagen, r.x, y1.saturating_sub(grosor), x1, y1, color);
        rellenar(imagen, r.x, r.y, (r.x + grosor).min(x1), y1, color);
        rellenar(imagen, x1.saturating_sub(grosor), r.y, x1, y1, color);

        // El número, en blanco sobre un recuadro del color del marco.
        let escala = grosor * 2;
        let texto = (n + 1).to_string();
        let ancho_texto = texto.len() as u32 * 4 * escala + escala;
        let alto_texto = 7 * escala;
        rellenar(imagen, r.x, r.y, r.x + ancho_texto, r.y + alto_texto, color);
        for (i, cifra) in texto.bytes().enumerate() {
            let x = r.x + escala + i as u32 * 4 * escala;
            pintar_cifra(imagen, cifra - b'0', x, r.y + escala, escala);
        }
    }
}

/// Cifras de 3 x 5 puntos, una fila por número de 3 bits.
const CIFRAS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

fn pintar_cifra(imagen: &mut comic_imagen::Imagen, cifra: u8, x: u32, y: u32, escala: u32) {
    for (fila, bits) in CIFRAS[cifra as usize].iter().enumerate() {
        for columna in 0..3 {
            if bits & (4 >> columna) != 0 {
                let px = x + columna * escala;
                let py = y + fila as u32 * escala;
                rellenar(imagen, px, py, px + escala, py + escala, [255, 255, 255]);
            }
        }
    }
}

fn rellenar(imagen: &mut comic_imagen::Imagen, x0: u32, y0: u32, x1: u32, y1: u32, color: [u8; 3]) {
    for y in y0..y1.min(imagen.alto) {
        for x in x0..x1.min(imagen.ancho) {
            let i = ((y * imagen.ancho + x) * 4) as usize;
            imagen.rgba[i..i + 3].copy_from_slice(&color);
        }
    }
}

/// Columnas y filas de cada hoja, y lo que mide cada página en ella.
const HOJA: (usize, usize) = (3, 2);
const CELDA: (u32, u32) = (620, 900);

/// La página reducida para la hoja, con su número abajo a la derecha.
fn reducir(
    encogedor: &mut comic_imagen::Encogedor,
    imagen: comic_imagen::Imagen,
    pagina: usize,
) -> Result<comic_imagen::Imagen, String> {
    let escala = (CELDA.0 as f32 / imagen.ancho as f32).min(CELDA.1 as f32 / imagen.alto as f32);
    let ancho = ((imagen.ancho as f32 * escala) as u32).max(1);
    let alto = ((imagen.alto as f32 * escala) as u32).max(1);
    let mut reducida = encogedor.encoger(imagen, ancho, alto)?;

    let texto = pagina.to_string();
    let punto = 6;
    let ancho_texto = texto.len() as u32 * 4 * punto + punto;
    let (x, y) = (
        ancho.saturating_sub(ancho_texto),
        alto.saturating_sub(7 * punto),
    );
    rellenar(&mut reducida, x, y, ancho, alto, [0, 0, 0]);
    for (i, cifra) in texto.bytes().enumerate() {
        pintar_cifra(
            &mut reducida,
            cifra - b'0',
            x + punto + i as u32 * 4 * punto,
            y + punto,
            punto,
        );
    }
    Ok(reducida)
}

/// Junta las páginas reducidas en una hoja gris.
fn hoja(paginas: &[comic_imagen::Imagen]) -> comic_imagen::Imagen {
    let (an, al) = (CELDA.0 * HOJA.0 as u32, CELDA.1 * HOJA.1 as u32);
    let mut hoja = comic_imagen::Imagen {
        ancho: an,
        alto: al,
        rgba: [90, 90, 90, 255].repeat((an * al) as usize),
    };
    for (i, p) in paginas.iter().enumerate() {
        let x0 = (i % HOJA.0) as u32 * CELDA.0;
        let y0 = (i / HOJA.0) as u32 * CELDA.1;
        for y in 0..p.alto {
            let origen = &p.rgba[(y * p.ancho * 4) as usize..((y + 1) * p.ancho * 4) as usize];
            let inicio = (((y0 + y) * an + x0) * 4) as usize;
            hoja.rgba[inicio..inicio + origen.len()].copy_from_slice(origen);
        }
    }
    hoja
}
