//! La ficha de una obra o de un número: lo que se enseña al pulsar su portada. Es igual
//! venga de AniList o de Metron, para que la interfaz no tenga que distinguirlas.

use serde::{Deserialize, Serialize};

use crate::rss::entidades;
use crate::Fuente;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ficha {
    pub fuente: Fuente,
    /// El identificador en el catálogo.
    pub id: String,
    pub titulo: String,
    /// Otro título (el inglés de un manga) o el de las historias de un número.
    pub subtitulo: Option<String>,
    /// Un resumen en una línea: `Manga, Japón, 2021, en publicación`.
    pub linea: String,
    /// La portada para las cuadrículas y otra más grande para la ficha.
    pub portada: Option<String>,
    pub portada_grande: Option<String>,
    /// Su página en la web del catálogo.
    pub url: Option<String>,
    /// Pares de nombre y valor: `("Guion", "Scott Snyder")`.
    pub datos: Vec<(String, String)>,
    /// Ya en español.
    pub generos: Vec<String>,
    /// Texto plano con los párrafos separados por una línea en blanco. Los catálogos
    /// las tienen en inglés.
    pub sinopsis: Option<String>,
    /// Lo que recomiendan los lectores a quien le gusta esta obra, de más a menos votada.
    /// Solo en AniList, y solo cuando se ha pedido la ficha entera.
    #[serde(default)]
    pub recomendaciones: Vec<Recomendada>,
}

/// Una obra recomendada: lo justo para enseñarla y pedir su ficha al pulsarla.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recomendada {
    /// El identificador en AniList.
    pub id: String,
    pub titulo: String,
    pub portada: Option<String>,
}

/// Los géneros de AniList y de Metron, en español. Los que no están se dejan como vienen.
pub fn genero(nombre: &str) -> String {
    let traducido = match nombre {
        "Action" => "Acción",
        "Adventure" => "Aventura",
        "Comedy" | "Humor" => "Humor",
        "Drama" => "Drama",
        "Ecchi" => "Ecchi",
        "Fantasy" => "Fantasía",
        "Horror" => "Terror",
        "Mahou Shoujo" => "Chicas mágicas",
        "Mecha" => "Mecha",
        "Music" => "Música",
        "Mystery" => "Misterio",
        "Psychological" => "Psicológico",
        "Romance" => "Romance",
        "Sci-Fi" | "Science Fiction" => "Ciencia ficción",
        "Slice of Life" => "Costumbrista",
        "Sports" | "Sport" => "Deportes",
        "Supernatural" => "Sobrenatural",
        "Thriller" => "Suspense",
        "Crime" => "Policiaco",
        "Espionage" => "Espionaje",
        "Historical" => "Histórico",
        "Super-Hero" => "Superhéroes",
        "War" => "Bélico",
        "Western" => "Wéstern",
        "Adult" => "Adultos",
        "Parody" => "Parodia",
        "Teen" => "Juvenil",
        "Childrens" => "Infantil",
        "Animal" => "Animales",
        "Martial Arts" => "Artes marciales",
        _ => return nombre.to_string(),
    };
    traducido.to_string()
}

/// El papel de un autor de manga en AniList. Solo los que se enseñan: ayudantes,
/// traductores, editores y demás se dejan fuera.
pub fn papel_anilist(papel: &str) -> Option<&'static str> {
    match papel {
        "Story & Art" => Some("Guion y dibujo"),
        "Story" => Some("Guion"),
        "Art" => Some("Dibujo"),
        "Original Story" | "Original Creator" => Some("Obra original"),
        _ => None,
    }
}

/// El papel de un autor de cómic en Metron, en el orden en que se enseñan.
pub const PAPELES_METRON: [(&str, &str); 9] = [
    ("Writer", "Guion"),
    ("Plot", "Argumento"),
    ("Artist", "Dibujo"),
    ("Penciller", "Lápiz"),
    ("Inker", "Tinta"),
    ("Colorist", "Color"),
    ("Letterer", "Rotulación"),
    ("Cover", "Portada"),
    ("Translator", "Traducción"),
];

pub fn pais(codigo: &str) -> Option<&'static str> {
    match codigo {
        "JP" => Some("Japón"),
        "KR" => Some("Corea del Sur"),
        "CN" => Some("China"),
        "TW" => Some("Taiwán"),
        _ => None,
    }
}

/// El tipo de una serie de Metron.
pub fn tipo_de_serie(tipo: &str) -> String {
    let traducido = match tipo {
        "Single Issue" | "Ongoing Series" => "Grapa",
        "Limited Series" => "Serie limitada",
        "Mini-Series" => "Miniserie",
        "One-Shot" => "Número único",
        "Annual Series" | "Annual" => "Anual",
        "Trade Paperback" => "Tomo recopilatorio",
        "Hardcover" => "Tapa dura",
        "Omnibus" => "Ómnibus",
        "Graphic Novel" => "Novela gráfica",
        "Digital Chapter" => "Capítulo digital",
        "Magazine" => "Revista",
        _ => return tipo.to_string(),
    };
    traducido.to_string()
}

/// El estado de una serie de Metron.
pub fn estado_de_serie(estado: &str) -> String {
    let traducido = match estado {
        "Ongoing" => "En publicación",
        "Completed" => "Terminada",
        "Cancelled" => "Cancelada",
        "Hiatus" => "En pausa",
        _ => return estado.to_string(),
    };
    traducido.to_string()
}

/// Quita el HTML de una sinopsis pero conserva los párrafos: `<br>`, `</p>` y los saltos
/// de línea separan párrafos, y entre dos párrafos queda una línea en blanco.
pub fn texto_con_parrafos(html: &str) -> Option<String> {
    let mut texto = String::with_capacity(html.len());
    let mut resto = html;
    while let Some(inicio) = resto.find('<') {
        texto.push_str(&resto[..inicio]);
        let Some(fin) = resto[inicio..].find('>') else {
            // Un `<` suelto: no es una etiqueta.
            texto.push_str(&resto[inicio..]);
            resto = "";
            break;
        };
        let etiqueta = resto[inicio + 1..inicio + fin].trim().to_ascii_lowercase();
        let nombre = etiqueta
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if matches!(nombre, "br" | "p" | "div" | "li") {
            texto.push('\n');
        }
        resto = &resto[inicio + fin + 1..];
    }
    texto.push_str(resto);

    let texto = entidades(&texto);
    let parrafos: Vec<String> = texto
        .split('\n')
        .map(|linea| linea.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|linea| !linea.is_empty())
        .collect();
    (!parrafos.is_empty()).then(|| parrafos.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_sinopsis_conserva_los_parrafos() {
        assert_eq!(
            texto_con_parrafos("Uno <i>dos</i><br><br>\n\nTres&nbsp;&amp; cuatro<br/>(Source: X)")
                .as_deref(),
            Some("Uno dos\n\nTres & cuatro\n\n(Source: X)")
        );
        assert_eq!(texto_con_parrafos("  <br> "), None);
        assert_eq!(texto_con_parrafos("a < b").as_deref(), Some("a < b"));
    }

    #[test]
    fn traduce_lo_que_conoce() {
        assert_eq!(genero("Sci-Fi"), "Ciencia ficción");
        assert_eq!(genero("Isekai"), "Isekai");
        assert_eq!(papel_anilist("Assistant"), None);
        assert_eq!(papel_anilist("Story & Art"), Some("Guion y dibujo"));
    }
}
