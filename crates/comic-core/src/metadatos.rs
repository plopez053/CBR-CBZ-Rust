//! Metadatos de un cómic: lo que dice su `ComicInfo.xml` (el formato de ComicRack, que
//! usan casi todas las herramientas) y, para lo que falte, lo que se deduce del nombre.

use std::collections::HashMap;
use std::sync::LazyLock;

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};
use regex::Regex;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadatos {
    pub serie: Option<String>,
    /// Número del ejemplar tal como se escribe ("1", "1.5", "0"): no siempre es un entero.
    pub numero: Option<String>,
    pub volumen: Option<u32>,
    /// Cuántos números tiene la serie en total.
    pub total: Option<u32>,
    pub titulo: Option<String>,
    pub anio: Option<u32>,
    pub mes: Option<u32>,
    pub resumen: Option<String>,
    pub guion: Option<String>,
    pub dibujo: Option<String>,
    pub editorial: Option<String>,
    pub generos: Option<String>,
    pub idioma: Option<String>,
    /// Manga que se lee de derecha a izquierda.
    pub derecha_a_izquierda: bool,
    /// Página que hace de portada, si el `ComicInfo.xml` lo indica.
    pub portada: Option<usize>,
}

impl Metadatos {
    /// Rellena lo que falte con lo que traiga `otros`. Lo que ya hay no se toca.
    pub fn completar(&mut self, otros: Metadatos) {
        let Metadatos {
            serie,
            numero,
            volumen,
            total,
            titulo,
            anio,
            mes,
            resumen,
            guion,
            dibujo,
            editorial,
            generos,
            idioma,
            derecha_a_izquierda,
            portada,
        } = otros;

        self.serie = self.serie.take().or(serie);
        self.numero = self.numero.take().or(numero);
        self.volumen = self.volumen.or(volumen);
        self.total = self.total.or(total);
        self.titulo = self.titulo.take().or(titulo);
        self.anio = self.anio.or(anio);
        self.mes = self.mes.or(mes);
        self.resumen = self.resumen.take().or(resumen);
        self.guion = self.guion.take().or(guion);
        self.dibujo = self.dibujo.take().or(dibujo);
        self.editorial = self.editorial.take().or(editorial);
        self.generos = self.generos.take().or(generos);
        self.idioma = self.idioma.take().or(idioma);
        self.derecha_a_izquierda |= derecha_a_izquierda;
        self.portada = self.portada.or(portada);
    }
}

/// Lee un `ComicInfo.xml`. Es tolerante: ignora las etiquetas que no conoce y los
/// valores que no tienen sentido (ComicRack, por ejemplo, escribe `-1` en los años
/// desconocidos). Solo falla si el XML está roto.
pub fn de_comic_info(xml: &str) -> Result<Metadatos, String> {
    let mut lector = Reader::from_str(xml.trim_start_matches('\u{feff}'));

    // Texto de cada etiqueta hija de <ComicInfo>, con el nombre en minúsculas.
    let mut campos: HashMap<String, String> = HashMap::new();
    let mut portada = None;
    let mut profundidad = 0;
    let mut campo: Option<String> = None;
    let mut texto = String::new();

    loop {
        let evento = lector.read_event().map_err(|e| e.to_string())?;
        match &evento {
            Event::Start(etiqueta) | Event::Empty(etiqueta) => {
                let nombre = etiqueta.local_name().as_ref().to_lowercase();
                // <Pages><Page Image="0" Type="FrontCover" /></Pages>
                if nombre == "page" && portada.is_none() {
                    let atributo = |clave: &str| {
                        etiqueta
                            .try_get_attribute(clave)
                            .ok()
                            .flatten()
                            .and_then(|a| {
                                a.normalized_value(XmlVersion::Implicit1_0)
                                    .ok()
                                    .map(|v| v.into_owned())
                            })
                    };
                    if atributo("Type").is_some_and(|t| t.contains("FrontCover")) {
                        portada = atributo("Image").and_then(|i| i.trim().parse().ok());
                    }
                }
                if matches!(evento, Event::Start(_)) {
                    profundidad += 1;
                    if profundidad == 2 {
                        campo = Some(nombre);
                        texto.clear();
                    }
                }
            }
            Event::Text(t) if profundidad == 2 => texto.push_str(&t.xml10_content()),
            Event::CData(t) if profundidad == 2 => texto.push_str(&t.xml10_content()),
            // Las entidades (`&amp;`, `&#233;`...) llegan aparte del texto que las rodea.
            Event::GeneralRef(r) if profundidad == 2 => {
                let caracter = match r.resolve_char_ref() {
                    Ok(Some(c)) => Some(c),
                    _ => match r.as_ref() {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    },
                };
                texto.extend(caracter);
            }
            Event::End(_) => {
                if profundidad == 2 {
                    if let Some(nombre) = campo.take() {
                        campos.insert(nombre, texto.trim().to_string());
                    }
                }
                profundidad -= 1;
            }
            Event::Eof => break,
            _ => {}
        }
    }

    let mut texto = |clave: &str| campos.remove(clave).filter(|v| !v.is_empty());
    let entero = |valor: Option<String>| valor.and_then(|v| v.parse::<u32>().ok());

    Ok(Metadatos {
        serie: texto("series"),
        numero: texto("number").map(|n| normalizar_numero(&n)),
        volumen: entero(texto("volume")),
        total: entero(texto("count")),
        titulo: texto("title"),
        anio: entero(texto("year")),
        mes: entero(texto("month")).filter(|m| (1..=12).contains(m)),
        resumen: texto("summary"),
        guion: texto("writer"),
        dibujo: texto("penciller"),
        editorial: texto("publisher"),
        generos: texto("genre"),
        idioma: texto("languageiso"),
        derecha_a_izquierda: texto("manga").is_some_and(|m| m == "YesAndRightToLeft"),
        portada,
    })
}

/// Deduce serie, número, volumen, título y año del nombre de un cómic (sin extensión).
///
/// Entiende los nombres que se ven en la práctica:
/// - `Bleeding Hearts 001 (2026) (Digital) (Pyrate-DCP)`
/// - `Batman v3 050 (2018)` y `Dragon Ball Tomo 05`
/// - `Saga #54`, `Spider-Man 2099 001`
/// - `Serie 001 - Título` y `Serie - 001 - Título`
pub fn de_nombre(nombre: &str) -> Metadatos {
    static ANIO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(((?:19|20)\d\d)\)").unwrap());
    static VOLUMEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:^|\s)(?:v|vol\.?|volume|volumen|tomo)\s*(\d+)(?:\s|$)").unwrap()
    });
    static NUMERO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#?(\d+(?:\.\d+)?)$").unwrap());
    static NUMERO_AL_FINAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(.*?)\s+#?(\d+(?:\.\d+)?)$").unwrap());

    let nombre = nombre.replace('_', " ");
    let mut datos = Metadatos {
        anio: ANIO.captures(&nombre).and_then(|c| c[1].parse().ok()),
        ..Metadatos::default()
    };

    // Lo que va entre paréntesis o corchetes es año, fuente, grupo...: no forma parte del título.
    let fin = nombre.find(['(', '[']).unwrap_or(nombre.len());
    let mut principal = nombre[..fin].trim().to_string();

    if let Some(c) = VOLUMEN.captures(&principal) {
        datos.volumen = c[1].parse().ok();
        let rango = c.get(0).unwrap().range();
        principal.replace_range(rango, " ");
    }
    let principal = espacios_simples(&principal);

    let partes: Vec<&str> = principal.split(" - ").map(str::trim).collect();
    // `Serie - 001 - Título`: una parte que es solo un número separa serie y título.
    let separador = partes
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, parte)| NUMERO.is_match(parte));

    if let Some((i, parte)) = separador {
        datos.serie = unir(&partes[..i]);
        datos.numero = Some(normalizar_numero(parte));
        datos.titulo = unir(&partes[i + 1..]);
    } else if let Some(c) = NUMERO.captures(partes[0]) {
        // El nombre entero es un número: `001.cbz`.
        datos.numero = Some(normalizar_numero(&c[1]));
        datos.titulo = unir(&partes[1..]);
    } else if let Some(c) = NUMERO_AL_FINAL.captures(partes[0]) {
        datos.serie = Some(c[1].trim().to_string()).filter(|s| !s.is_empty());
        datos.numero = Some(normalizar_numero(&c[2]));
        datos.titulo = unir(&partes[1..]);
    } else {
        datos.serie = unir(&partes);
    }

    // `Capítulo 001` o `Issue 5` no dicen la serie: mejor que la decida quien llama
    // (la biblioteca usa el nombre de la carpeta).
    if datos
        .serie
        .as_deref()
        .is_some_and(|s| PALABRAS_SIN_SERIE.contains(&s.to_lowercase().as_str()))
    {
        datos.serie = None;
    }

    datos
}

/// Palabras que a veces van delante del número en lugar del nombre de la serie.
const PALABRAS_SIN_SERIE: [&str; 13] = [
    "capitulo",
    "capítulo",
    "cap",
    "cap.",
    "chapter",
    "ch",
    "ch.",
    "issue",
    "numero",
    "número",
    "episodio",
    "episode",
    "ep",
];

/// Junta las partes con ` - `, o nada si no hay partes.
fn unir(partes: &[&str]) -> Option<String> {
    Some(partes.join(" - ")).filter(|s| !s.is_empty())
}

fn espacios_simples(texto: &str) -> String {
    texto.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `001` -> `1`, `#54` -> `54`, `007.5` -> `7.5`. Lo que no empieza por cifra se deja igual.
fn normalizar_numero(numero: &str) -> String {
    let numero = numero.trim().trim_start_matches('#');
    if !numero.starts_with(|c: char| c.is_ascii_digit()) {
        return numero.to_string();
    }
    let sin_ceros = numero.trim_start_matches('0');
    if sin_ceros.is_empty() || sin_ceros.starts_with('.') {
        format!("0{sin_ceros}")
    } else {
        sin_ceros.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lo que se deduce de un nombre: serie, número, volumen, título y año.
    type Deducido = (
        Option<String>,
        Option<String>,
        Option<u32>,
        Option<String>,
        Option<u32>,
    );

    fn nombre(n: &str) -> Deducido {
        let d = de_nombre(n);
        (d.serie, d.numero, d.volumen, d.titulo, d.anio)
    }

    fn s(texto: &str) -> Option<String> {
        Some(texto.to_string())
    }

    #[test]
    fn nombre_tipico_de_grupo_digital() {
        assert_eq!(
            nombre("Bleeding Hearts 001 (2026) (Digital) (Pyrate-DCP)"),
            (s("Bleeding Hearts"), s("1"), None, None, Some(2026))
        );
    }

    #[test]
    fn volumen_y_numero() {
        assert_eq!(
            nombre("Batman v3 050 (2018)"),
            (s("Batman"), s("50"), Some(3), None, Some(2018))
        );
        assert_eq!(
            nombre("Dragon Ball Tomo 05"),
            (s("Dragon Ball"), None, Some(5), None, None)
        );
        assert_eq!(
            nombre("One Piece Vol. 12"),
            (s("One Piece"), None, Some(12), None, None)
        );
    }

    #[test]
    fn numero_con_almohadilla_o_decimales() {
        assert_eq!(nombre("Saga #54"), (s("Saga"), s("54"), None, None, None));
        assert_eq!(
            nombre("Invincible 007.5"),
            (s("Invincible"), s("7.5"), None, None, None)
        );
        assert_eq!(nombre("Spawn 000"), (s("Spawn"), s("0"), None, None, None));
    }

    #[test]
    fn el_numero_es_el_ultimo_de_la_serie() {
        assert_eq!(
            nombre("Spider-Man 2099 001 (1992)"),
            (s("Spider-Man 2099"), s("1"), None, None, Some(1992))
        );
    }

    #[test]
    fn titulo_tras_guion() {
        assert_eq!(
            nombre("Hellboy 001 - Semilla de destruccion"),
            (
                s("Hellboy"),
                s("1"),
                None,
                s("Semilla de destruccion"),
                None
            )
        );
        assert_eq!(
            nombre("Hellboy - 001 - Semilla de destruccion"),
            (
                s("Hellboy"),
                s("1"),
                None,
                s("Semilla de destruccion"),
                None
            )
        );
    }

    #[test]
    fn guiones_bajos_y_sin_numero() {
        assert_eq!(
            nombre("Bleeding_Hearts_001_(2026)"),
            (s("Bleeding Hearts"), s("1"), None, None, Some(2026))
        );
        assert_eq!(nombre("Maus"), (s("Maus"), None, None, None, None));
        assert_eq!(nombre("001"), (None, s("1"), None, None, None));
    }

    #[test]
    fn sin_serie_si_solo_dice_capitulo() {
        assert_eq!(nombre("Capítulo 001"), (None, s("1"), None, None, None));
        assert_eq!(nombre("Chapter 12"), (None, s("12"), None, None, None));
        assert_eq!(nombre("Vol. 01"), (None, None, Some(1), None, None));
    }

    #[test]
    fn comic_info_completo() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <Title>El principio</Title>
  <Series>Bleeding Hearts</Series>
  <Number>001</Number>
  <Count>6</Count>
  <Volume>2026</Volume>
  <Summary>Una historia de amor &amp; sangre.</Summary>
  <Year>2026</Year>
  <Month>3</Month>
  <Writer>Ana &#xC9;lez</Writer>
  <Penciller>Luis</Penciller>
  <Publisher>Image</Publisher>
  <Genre>Horror, Romance</Genre>
  <LanguageISO>en</LanguageISO>
  <Manga>No</Manga>
  <Pages>
    <Page Image="0" Type="InnerCover" />
    <Page Image="1" Type="FrontCover" ImageWidth="1988" />
  </Pages>
</ComicInfo>"#;
        let datos = de_comic_info(xml).unwrap();
        assert_eq!(datos.serie, s("Bleeding Hearts"));
        assert_eq!(datos.numero, s("1"));
        assert_eq!(datos.total, Some(6));
        assert_eq!(datos.volumen, Some(2026));
        assert_eq!(datos.titulo, s("El principio"));
        assert_eq!(datos.resumen, s("Una historia de amor & sangre."));
        assert_eq!(datos.anio, Some(2026));
        assert_eq!(datos.mes, Some(3));
        assert_eq!(datos.guion, s("Ana Élez"));
        assert_eq!(datos.dibujo, s("Luis"));
        assert_eq!(datos.editorial, s("Image"));
        assert_eq!(datos.generos, s("Horror, Romance"));
        assert_eq!(datos.idioma, s("en"));
        assert!(!datos.derecha_a_izquierda);
        assert_eq!(datos.portada, Some(1));
    }

    #[test]
    fn comic_info_con_valores_raros() {
        // BOM al principio, años "-1" de ComicRack, etiquetas vacías y desconocidas.
        let xml = "\u{feff}<ComicInfo><Series>Akira</Series><Year>-1</Year><Title></Title>\
                   <Inventada>x</Inventada><Manga>YesAndRightToLeft</Manga></ComicInfo>";
        let datos = de_comic_info(xml).unwrap();
        assert_eq!(datos.serie, s("Akira"));
        assert_eq!(datos.anio, None);
        assert_eq!(datos.titulo, None);
        assert!(datos.derecha_a_izquierda);
    }

    #[test]
    fn comic_info_roto_da_error() {
        assert!(de_comic_info("<ComicInfo><Series>Akira</Title></ComicInfo>").is_err());
    }

    #[test]
    fn completar_no_pisa_lo_que_hay() {
        let mut datos = Metadatos {
            serie: s("Del XML"),
            ..Metadatos::default()
        };
        datos.completar(de_nombre("Del nombre 003 (1999)"));
        assert_eq!(datos.serie, s("Del XML"));
        assert_eq!(datos.numero, s("3"));
        assert_eq!(datos.anio, Some(1999));
    }
}
