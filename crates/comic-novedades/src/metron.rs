//! Metron (metron.cloud): base de datos abierta de cómic, sobre todo americano, con la
//! fecha de salida de cada número. Hace falta una cuenta gratuita. Límite: 20 peticiones
//! por minuto y 5000 al día.

use std::time::Duration;

use serde::Deserialize;

use crate::anilist::plural;
use crate::fecha::Fecha;
use crate::ficha::{
    estado_de_serie, genero, texto_con_parrafos, tipo_de_serie, Ficha, PAPELES_METRON,
};
use crate::red::{autorizacion_basica, Cliente, ErrorRed, Limitador};
use crate::{Candidato, Cuenta, Fuente, Numero};

const API: &str = "https://metron.cloud/api/";
/// Algo más de 3 s entre peticiones: no se pasa nunca de las 20 por minuto.
static LIMITADOR: Limitador = Limitador::nuevo(Duration::from_millis(3100));
/// Páginas que se siguen como mucho en un listado (una serie larga o una semana con
/// muchas novedades), para no gastar el límite diario en una sola consulta.
const MAX_PAGINAS: usize = 30;

pub struct Metron {
    cliente: Cliente,
    autorizacion: String,
}

#[derive(Deserialize)]
struct Pagina<T> {
    next: Option<String>,
    results: Vec<T>,
}

#[derive(Deserialize)]
struct SerieMetron {
    id: u64,
    /// El nombre con el año: `Saga (2012)`.
    series: String,
    year_began: Option<u32>,
    issue_count: Option<u32>,
    #[serde(default)]
    publisher: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct NumeroMetron {
    id: Option<u64>,
    number: String,
    series: Option<SerieBasica>,
    cover_date: Option<String>,
    store_date: Option<String>,
    image: Option<String>,
}

#[derive(Deserialize)]
struct SerieBasica {
    name: String,
    year_began: Option<u32>,
}

/// Algo con nombre: una editorial, un género, un papel...
#[derive(Deserialize)]
struct ConNombre {
    name: String,
}

/// La ficha entera de un número.
#[derive(Deserialize)]
struct NumeroDetalle {
    id: u64,
    publisher: Option<ConNombre>,
    imprint: Option<ConNombre>,
    series: Option<SerieDeNumero>,
    number: String,
    #[serde(default)]
    title: Option<String>,
    /// Los títulos de las historias que trae.
    #[serde(default)]
    name: Vec<String>,
    cover_date: Option<String>,
    store_date: Option<String>,
    price: Option<String>,
    price_currency: Option<String>,
    page: Option<u32>,
    desc: Option<String>,
    image: Option<String>,
    #[serde(default)]
    arcs: Vec<ConNombre>,
    #[serde(default)]
    credits: Vec<Credito>,
    resource_url: Option<String>,
}

#[derive(Deserialize)]
struct SerieDeNumero {
    name: String,
    year_began: Option<u32>,
    series_type: Option<ConNombre>,
    #[serde(default)]
    genres: Vec<ConNombre>,
}

#[derive(Deserialize)]
struct Credito {
    creator: String,
    #[serde(default)]
    role: Vec<ConNombre>,
}

/// La ficha entera de una serie.
#[derive(Deserialize)]
struct SerieDetalle {
    id: u64,
    name: String,
    publisher: Option<ConNombre>,
    imprint: Option<ConNombre>,
    series_type: Option<ConNombre>,
    status: Option<String>,
    year_began: Option<u32>,
    year_end: Option<u32>,
    issue_count: Option<u32>,
    desc: Option<String>,
    #[serde(default)]
    genres: Vec<ConNombre>,
    resource_url: Option<String>,
}

impl Metron {
    pub fn nuevo(cliente: Cliente, cuenta: &Cuenta) -> Metron {
        Metron {
            cliente,
            autorizacion: autorizacion_basica(&cuenta.usuario, &cuenta.contrasena),
        }
    }

    /// Las series cuyo nombre contiene `texto`.
    pub fn buscar(&self, texto: &str) -> Result<Vec<Candidato>, ErrorRed> {
        let pagina: Pagina<SerieMetron> =
            self.pedir(&format!("{API}series/"), &[("name", texto.to_string())])?;
        Ok(pagina.results.into_iter().map(candidato_de).collect())
    }

    /// Todos los números publicados (o anunciados) de una serie.
    pub fn numeros(&self, serie: &str) -> Result<Vec<Numero>, ErrorRed> {
        self.listado(&[("series_id", serie.to_string())])
    }

    /// Los números que salen a la venta entre `desde` y `hasta`, los dos incluidos.
    pub fn salen_entre(&self, desde: Fecha, hasta: Fecha) -> Result<Vec<Numero>, ErrorRed> {
        self.listado(&[
            ("store_date_range_after", desde.iso()),
            ("store_date_range_before", hasta.iso()),
        ])
    }

    /// La ficha de un número: sinopsis, autores, editorial, páginas, precio...
    pub fn ficha_de_numero(&self, id: u64) -> Result<Ficha, ErrorRed> {
        let numero: NumeroDetalle = self.pedir(&format!("{API}issue/{id}/"), &[])?;
        Ok(ficha_de_numero(numero))
    }

    /// La ficha de una serie: descripción, editorial, estado, géneros...
    pub fn ficha_de_serie(&self, id: &str) -> Result<Ficha, ErrorRed> {
        let serie: SerieDetalle = self.pedir(&format!("{API}series/{id}/"), &[])?;
        Ok(ficha_de_serie(serie))
    }

    fn listado(&self, consulta: &[(&str, String)]) -> Result<Vec<Numero>, ErrorRed> {
        let mut numeros = Vec::new();
        let mut pagina: Pagina<NumeroMetron> = self.pedir(&format!("{API}issue/"), consulta)?;
        for _ in 0..MAX_PAGINAS {
            numeros.extend(pagina.results.into_iter().map(numero_de));
            // `next` ya lleva la consulta entera.
            match pagina.next {
                Some(siguiente) => pagina = self.pedir(&siguiente, &[])?,
                None => break,
            }
        }
        Ok(numeros)
    }

    fn pedir<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        consulta: &[(&str, String)],
    ) -> Result<T, ErrorRed> {
        LIMITADOR.esperar();
        let texto = self
            .cliente
            .texto(url, consulta, Some(&self.autorizacion))?;
        serde_json::from_str(&texto).map_err(|e| ErrorRed::Datos(e.to_string()))
    }
}

/// `Saga (2012)` da `Saga`.
pub fn sin_anio(nombre: &str) -> &str {
    let nombre = nombre.trim();
    match nombre.rfind(" (") {
        Some(i)
            if nombre.ends_with(')')
                && nombre[i + 2..nombre.len() - 1]
                    .chars()
                    .all(|c| c.is_ascii_digit()) =>
        {
            nombre[..i].trim_end()
        }
        _ => nombre,
    }
}

fn candidato_de(serie: SerieMetron) -> Candidato {
    let editorial = serie.publisher.as_ref().and_then(|p| match p {
        serde_json::Value::Object(o) => o.get("name").and_then(|n| n.as_str()).map(str::to_string),
        _ => None,
    });
    let mut detalle = Vec::new();
    detalle.extend(serie.year_began.map(|a| a.to_string()));
    detalle.extend(serie.issue_count.map(|n| plural(n, "número", "números")));
    detalle.extend(editorial);

    let base = sin_anio(&serie.series).to_string();
    Candidato {
        fuente: Fuente::Metron,
        id: serie.id.to_string(),
        otros_titulos: if base != serie.series {
            vec![base]
        } else {
            Vec::new()
        },
        titulo: serie.series,
        anio: serie.year_began,
        detalle: detalle.join(", "),
        portada: None,
        url: None,
        formato: None,
    }
}

fn numero_de(numero: NumeroMetron) -> Numero {
    let serie = match &numero.series {
        Some(s) => match s.year_began {
            Some(anio) => format!("{} ({anio})", s.name),
            None => s.name.clone(),
        },
        None => String::new(),
    };
    Numero {
        id: numero.id,
        numero: numero.number,
        serie,
        fecha: numero
            .store_date
            .as_deref()
            .and_then(Fecha::leer)
            .or_else(|| numero.cover_date.as_deref().and_then(Fecha::leer)),
        portada: numero.image,
    }
}

/// `DC Comics` o `DC Comics / Black Label`.
fn editorial(publisher: Option<ConNombre>, imprint: Option<ConNombre>) -> Option<String> {
    match (publisher, imprint) {
        (Some(p), Some(i)) => Some(format!("{} / {}", p.name, i.name)),
        (Some(p), None) => Some(p.name),
        (None, Some(i)) => Some(i.name),
        (None, None) => None,
    }
}

fn ficha_de_numero(numero: NumeroDetalle) -> Ficha {
    let serie = numero.series;
    let nombre_serie = serie.as_ref().map_or("", |s| s.name.as_str());
    let editorial = editorial(numero.publisher, numero.imprint);
    let tipo = serie
        .as_ref()
        .and_then(|s| s.series_type.as_ref())
        .map(|t| tipo_de_serie(&t.name));
    let venta = numero.store_date.as_deref().and_then(Fecha::leer);
    let portada = numero.cover_date.as_deref().and_then(Fecha::leer);

    let mut linea = Vec::new();
    linea.extend(editorial.clone());
    linea.extend(tipo);
    linea.extend(venta.or(portada).map(|f| f.anio.to_string()));

    let mut datos: Vec<(String, String)> = Vec::new();
    // Los autores, juntos por papel y en el orden de PAPELES_METRON.
    for (papel, en_espanol) in PAPELES_METRON {
        let nombres: Vec<&str> = numero
            .credits
            .iter()
            .filter(|c| c.role.iter().any(|r| r.name == papel))
            .map(|c| c.creator.as_str())
            .collect();
        if !nombres.is_empty() {
            datos.push((en_espanol.to_string(), nombres.join(", ")));
        }
    }
    if let Some(serie) = &serie {
        let nombre = match serie.year_began {
            Some(anio) => format!("{} ({anio})", serie.name),
            None => serie.name.clone(),
        };
        datos.push(("Serie".to_string(), nombre));
    }
    datos.extend(editorial.map(|e| ("Editorial".to_string(), e)));
    datos.extend(venta.map(|f| ("A la venta".to_string(), f.to_string())));
    if let Some(portada) = portada {
        datos.push(("Fecha de portada".to_string(), portada.to_string()));
    }
    datos.extend(
        numero
            .page
            .filter(|&p| p > 0)
            .map(|p| ("Páginas".to_string(), p.to_string())),
    );
    if let Some(precio) = numero
        .price
        .filter(|p| p.parse::<f64>().is_ok_and(|v| v > 0.0))
    {
        let moneda = numero.price_currency.unwrap_or_default();
        datos.push((
            "Precio".to_string(),
            format!("{precio} {moneda}").trim().to_string(),
        ));
    }
    let arcos: Vec<String> = numero.arcs.into_iter().map(|a| a.name).collect();
    if !arcos.is_empty() {
        datos.push(("Arco".to_string(), arcos.join(", ")));
    }

    let historias: Vec<String> = numero
        .title
        .into_iter()
        .chain(numero.name)
        .filter(|t| !t.trim().is_empty())
        .collect();
    Ficha {
        fuente: Fuente::Metron,
        id: numero.id.to_string(),
        titulo: format!("{} #{}", nombre_serie, numero.number)
            .trim()
            .to_string(),
        subtitulo: (!historias.is_empty()).then(|| historias.join(" · ")),
        linea: linea.join(", "),
        portada: numero.image.clone(),
        portada_grande: numero.image,
        url: numero.resource_url,
        datos,
        generos: serie
            .map(|s| s.genres.iter().map(|g| genero(&g.name)).collect())
            .unwrap_or_default(),
        sinopsis: numero.desc.as_deref().and_then(texto_con_parrafos),
        recomendaciones: Vec::new(),
    }
}

fn ficha_de_serie(serie: SerieDetalle) -> Ficha {
    let editorial = editorial(serie.publisher, serie.imprint);
    let tipo = serie.series_type.map(|t| tipo_de_serie(&t.name));
    let estado = serie.status.as_deref().map(estado_de_serie);
    let anios = match (serie.year_began, serie.year_end) {
        (Some(desde), Some(hasta)) if hasta != desde => Some(format!("{desde}-{hasta}")),
        (Some(desde), _) => Some(desde.to_string()),
        _ => None,
    };

    let mut linea = Vec::new();
    linea.extend(editorial.clone());
    linea.extend(tipo);
    linea.extend(anios.clone());
    linea.extend(estado.clone());

    let mut datos = Vec::new();
    datos.extend(editorial.map(|e| ("Editorial".to_string(), e)));
    datos.extend(anios.map(|a| ("Años".to_string(), a)));
    datos.extend(estado.map(|e| ("Estado".to_string(), e)));
    datos.extend(
        serie
            .issue_count
            .map(|n| ("Publicados".to_string(), plural(n, "número", "números"))),
    );

    Ficha {
        fuente: Fuente::Metron,
        id: serie.id.to_string(),
        titulo: serie.name,
        subtitulo: None,
        linea: linea.join(", "),
        portada: None,
        portada_grande: None,
        url: serie.resource_url,
        datos,
        generos: serie.genres.iter().map(|g| genero(&g.name)).collect(),
        sinopsis: serie.desc.as_deref().and_then(texto_con_parrafos),
        recomendaciones: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quita_el_anio_del_nombre() {
        assert_eq!(sin_anio("Saga (2012)"), "Saga");
        assert_eq!(sin_anio("Batman (2016)"), "Batman");
        assert_eq!(sin_anio("Spider-Man 2099"), "Spider-Man 2099");
        assert_eq!(sin_anio("Y (The Last Man)"), "Y (The Last Man)");
    }

    #[test]
    fn lee_un_listado_de_numeros() {
        let json = r#"{"count":2,"next":null,"previous":null,"results":[
            {"id":1,"series":{"id":7,"name":"Saga","volume":1,"year_began":2012},
             "number":"67","issue":"Saga (2012) #67","cover_date":"2026-11-01",
             "store_date":"2026-10-21","image":"https://static.metron.cloud/a.jpg",
             "cover_hash":"","modified":"2026-10-01T00:00:00Z"},
            {"id":2,"series":{"id":7,"name":"Saga","volume":1,"year_began":2012},
             "number":"68","issue":"Saga (2012) #68","cover_date":"2026-12-01",
             "store_date":null,"image":null,"cover_hash":"","modified":"2026-10-01T00:00:00Z"}]}"#;
        let pagina: Pagina<NumeroMetron> = serde_json::from_str(json).unwrap();
        let numeros: Vec<Numero> = pagina.results.into_iter().map(numero_de).collect();
        assert_eq!(numeros[0].serie, "Saga (2012)");
        assert_eq!(numeros[0].fecha, Fecha::leer("2026-10-21"));
        // Sin fecha de venta, la de portada.
        assert_eq!(numeros[1].fecha, Fecha::leer("2026-12-01"));
        assert_eq!(numeros[1].id, Some(2));
    }

    #[test]
    fn lee_la_ficha_de_un_numero() {
        let json = r#"{"id": 9, "publisher": {"id": 2, "name": "DC Comics"}, "imprint": null,
            "series": {"id": 5, "name": "All Star Batman", "volume": 1, "year_began": 2016,
              "series_type": {"id": 1, "name": "Ongoing Series"},
              "genres": [{"id": 1, "name": "Super-Hero"}]},
            "number": "1", "title": "", "name": ["My Own Worst Enemy, Part 1"],
            "cover_date": "2016-10-01", "store_date": "2016-08-10", "price": "4.99",
            "price_currency": "USD", "page": 44, "desc": "Two-Face!", "image": "https://a/1.jpg",
            "arcs": [], "credits": [
              {"id": 1, "creator": "Danny Miki", "role": [{"id": 4, "name": "Inker"}, {"id": 7, "name": "Cover"}]},
              {"id": 2, "creator": "Mark Doyle", "role": [{"id": 8, "name": "Editor"}]},
              {"id": 3, "creator": "Scott Snyder", "role": [{"id": 1, "name": "Writer"}]},
              {"id": 4, "creator": "Jock", "role": [{"id": 7, "name": "Cover"}]}],
            "resource_url": "https://metron.cloud/issue/x/"}"#;
        let ficha = ficha_de_numero(serde_json::from_str(json).unwrap());
        assert_eq!(ficha.titulo, "All Star Batman #1");
        assert_eq!(
            ficha.subtitulo.as_deref(),
            Some("My Own Worst Enemy, Part 1")
        );
        assert_eq!(ficha.linea, "DC Comics, Grapa, 2016");
        assert_eq!(ficha.generos, ["Superhéroes"]);
        let dato = |nombre: &str| {
            ficha
                .datos
                .iter()
                .find(|(n, _)| n == nombre)
                .map(|(_, v)| v.as_str())
        };
        // Los autores, en el orden de los papeles y sin los editores.
        assert_eq!(ficha.datos[0].0, "Guion");
        assert_eq!(dato("Portada"), Some("Danny Miki, Jock"));
        assert_eq!(dato("Edición"), None);
        assert_eq!(dato("Precio"), Some("4.99 USD"));
        assert_eq!(dato("Páginas"), Some("44"));
        assert_eq!(dato("A la venta"), Some("10 ago 2016"));
    }
}
