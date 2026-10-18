//! Decodificar y encoger páginas. Lo usan el lector y, más adelante, las miniaturas.
//!
//! Está en un crate aparte por rendimiento: el código genérico de `fast_image_resize`
//! se compila en el crate que lo llama, no en la dependencia. Aquí se llama, y este
//! crate se optimiza también en desarrollo (ver el `Cargo.toml` del workspace).
//! Para que eso funcione, nada de lo público puede ser genérico ni `#[inline]`:
//! si lo fuera, se volvería a compilar en quien lo use, sin optimizar.

use fast_image_resize as fr;

/// Una imagen en RGBA de 8 bits, sin premultiplicar.
pub struct Imagen {
    pub ancho: u32,
    pub alto: u32,
    /// `ancho * alto * 4` bytes, fila a fila.
    pub rgba: Vec<u8>,
}

/// Decodifica una página (JPG, PNG, WebP...). El formato se deduce de los bytes.
pub fn decodificar(bytes: &[u8]) -> Result<Imagen, String> {
    let imagen = image::load_from_memory(bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let (ancho, alto) = imagen.dimensions();
    Ok(Imagen {
        ancho,
        alto,
        rgba: imagen.into_raw(),
    })
}

/// Comprime una imagen en JPEG con `calidad` de 1 a 100. Se pierde la transparencia,
/// que en una portada no hay.
pub fn codificar_jpeg(imagen: &Imagen, calidad: u8) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = imagen
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, _]| [r, g, b])
        .collect();

    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, calidad)
        .encode(
            &rgb,
            imagen.ancho,
            imagen.alto,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// Encoge imágenes. Guarda búferes internos entre llamadas: reutilizarlo para varias
/// páginas evita reservar memoria cada vez.
pub struct Encogedor {
    redimensionador: fr::Resizer,
}

impl Default for Encogedor {
    fn default() -> Self {
        Encogedor {
            redimensionador: fr::Resizer::new(),
        }
    }
}

impl Encogedor {
    /// Devuelve `imagen` a `ancho` x `alto`. Si ya mide eso, la devuelve tal cual.
    pub fn encoger(&mut self, imagen: Imagen, ancho: u32, alto: u32) -> Result<Imagen, String> {
        if (imagen.ancho, imagen.alto) == (ancho, alto) {
            return Ok(imagen);
        }

        let origen = fr::images::Image::from_vec_u8(
            imagen.ancho,
            imagen.alto,
            imagen.rgba,
            fr::PixelType::U8x4,
        )
        .map_err(|e| e.to_string())?;
        let mut destino = fr::images::Image::new(ancho, alto, fr::PixelType::U8x4);
        // Lanczos3 directamente sobre sRGB. Comparado con Mitchell, Magic Kernel Sharp y
        // con encoger en luz lineal, es lo que mejor conserva el trazo de la tinta y las
        // letras: en luz lineal las líneas finas salen más grises y delgadas.
        let opciones = fr::ResizeOptions::new()
            .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3));
        self.redimensionador
            .resize(&origen, &mut destino, &opciones)
            .map_err(|e| e.to_string())?;

        Ok(Imagen {
            ancho,
            alto,
            rgba: destino.into_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gris(ancho: u32, alto: u32) -> Imagen {
        Imagen {
            ancho,
            alto,
            rgba: vec![128; (ancho * alto * 4) as usize],
        }
    }

    #[test]
    fn encoge_al_tamano_pedido() {
        let encogida = Encogedor::default()
            .encoger(gris(200, 300), 100, 150)
            .unwrap();
        assert_eq!((encogida.ancho, encogida.alto), (100, 150));
        assert_eq!(encogida.rgba.len(), 100 * 150 * 4);
        // Un color liso sigue igual de liso al encogerlo.
        assert!(encogida.rgba.iter().all(|&b| b == 128));
    }

    #[test]
    fn mismo_tamano_la_devuelve_tal_cual() {
        let imagen = gris(10, 10);
        let puntero = imagen.rgba.as_ptr();
        let igual = Encogedor::default().encoger(imagen, 10, 10).unwrap();
        assert_eq!(igual.rgba.as_ptr(), puntero);
    }

    #[test]
    fn un_jpeg_se_vuelve_a_leer_igual_de_grande() {
        let jpeg = codificar_jpeg(&gris(40, 60), 90).unwrap();
        let leida = decodificar(&jpeg).unwrap();
        assert_eq!((leida.ancho, leida.alto), (40, 60));
        // El JPEG pierde algo, pero un gris liso tiene que seguir casi igual.
        assert!(leida.rgba.iter().step_by(4).all(|&r| r.abs_diff(128) <= 2));
    }

    #[test]
    fn rechaza_lo_que_no_es_una_imagen() {
        assert!(decodificar(b"contenido falso de 1.jpg").is_err());
    }
}
