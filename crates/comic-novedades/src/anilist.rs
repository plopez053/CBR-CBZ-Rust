//! AniList (anilist.co): base de datos de manga con una API GraphQL abierta, sin cuenta.
//! Dice si una serie sigue publicándose y cuántos tomos y capítulos tiene la edición
//! original japonesa, y qué manga se está leyendo más. Límite: 30 peticiones por minuto.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::fecha::Fecha;
use crate::ficha::{genero, pais, papel_anilist, texto_con_parrafos, Ficha, Recomendada};
use crate::red::{Cliente, ErrorRed, Limitador};
use crate::{Candidato, Fuente, ListaManga, Manga, Publicacion};

const URL: &str = "https://graphql.anilist.co";
/// Algo más de 2 s entre peticiones: no se pasa nunca de las 30 por minuto.
static LIMITADOR: Limitador = Limitador::nuevo(Duration::from_millis(2100));
/// Cuántas series se piden de una vez (el máximo de AniList por página).
const POR_PAGINA: usize = 50;
/// Cuántas se piden de cada vez para las listas de la pestaña de manga.
const POR_LISTA: usize = 30;
/// Lo que se pide en las listas de manga: solo japonés, sin novelas ligeras ni nada para
/// adultos.
const FILTRO_MANGA: &str = "type: MANGA, countryOfOrigin: JP, isAdult: false, format_not: NOVEL";
/// Recomendadas que se miran de cada serie de la biblioteca para hacer "Para ti", y
/// cuántas se enseñan en una ficha.
const RECOMENDADAS_POR_SERIE: usize = 25;
const RECOMENDADAS_EN_FICHA: usize = 12;
/// "Recién empezadas" son las que empezaron en estos últimos días.
const DIAS_RECIENTES: i64 = 120;

const CAMPOS: &str = "id title { romaji english native } synonyms status volumes chapters \
                      format countryOfOrigin startDate { year month day } \
                      coverImage { large extraLarge } siteUrl description(asHtml: false) \
                      genres averageScore \
                      staff(perPage: 6, sort: RELEVANCE) { edges { role node { name { full } } } }";

/// Una serie de AniList con lo que se sabe de su publicación.
#[derive(Clone, Debug)]
pub struct Obra {
    pub candidato: Candidato,
    pub publicacion: Publicacion,
    pub tomos: Option<u32>,
    pub capitulos: Option<u32>,
    pub ficha: Ficha,
}

pub struct AniList {
    cliente: Cliente,
}

#[derive(Deserialize)]
struct Respuesta<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<ErrorGraphql>,
}

#[derive(Deserialize)]
struct ErrorGraphql {
    message: String,
}

#[derive(Deserialize)]
struct Datos {
    #[serde(rename = "Page")]
    pagina: Pagina,
}

#[derive(Deserialize)]
struct DatosManga {
    tendencia: Pagina,
    nuevas: Pagina,
}

#[derive(Deserialize)]
struct DatosLista {
    lista: Pagina,
}

#[derive(Deserialize)]
struct DatosFicha {
    #[serde(rename = "Media")]
    medio: Medio,
}

/// Solo las recomendadas de cada serie (para "Para ti").
#[derive(Deserialize)]
struct DatosRecomendaciones {
    #[serde(rename = "Page")]
    pagina: PaginaRecomendaciones,
}

#[derive(Deserialize)]
struct PaginaRecomendaciones {
    media: Vec<MedioRecomendaciones>,
}

#[derive(Deserialize)]
struct MedioRecomendaciones {
    recommendations: Option<Recomendaciones>,
}

#[derive(Deserialize)]
struct Pagina {
    #[serde(rename = "pageInfo")]
    info: Option<InfoPagina>,
    media: Vec<Medio>,
}

#[derive(Deserialize)]
struct InfoPagina {
    #[serde(rename = "hasNextPage")]
    hay_mas: Option<bool>,
}

#[derive(Deserialize)]
struct Medio {
    id: u64,
    title: Titulos,
    #[serde(default)]
    synonyms: Vec<String>,
    status: Option<String>,
    volumes: Option<u32>,
    chapters: Option<u32>,
    format: Option<String>,
    #[serde(rename = "countryOfOrigin")]
    pais: Option<String>,
    #[serde(rename = "startDate")]
    inicio: Option<FechaDifusa>,
    #[serde(rename = "coverImage")]
    portada: Option<Portada>,
    #[serde(rename = "siteUrl")]
    url: Option<String>,
    description: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(rename = "averageScore")]
    nota: Option<u32>,
    staff: Option<Equipo>,
    /// Solo cuando se piden (la ficha entera, o para "Para ti").
    recommendations: Option<Recomendaciones>,
}

#[derive(Deserialize)]
struct Recomendaciones {
    #[serde(default)]
    nodes: Vec<Recomendacion>,
}

#[derive(Deserialize)]
struct Recomendacion {
    /// Votos de los lectores (puede ser negativo).
    rating: Option<i64>,
    #[serde(rename = "mediaRecommendation")]
    obra: Option<ObraRecomendada>,
}

#[derive(Deserialize)]
struct ObraRecomendada {
    id: u64,
    #[serde(rename = "type")]
    tipo: Option<String>,
    #[serde(rename = "isAdult")]
    adultos: Option<bool>,
    format: Option<String>,
    #[serde(rename = "countryOfOrigin")]
    pais: Option<String>,
    title: Option<Titulos>,
    #[serde(rename = "coverImage")]
    portada: Option<Portada>,
}

impl ObraRecomendada {
    /// Manga para todos los públicos, y no una novela ligera.
    fn vale(&self) -> bool {
        self.tipo.as_deref() == Some("MANGA")
            && self.adultos != Some(true)
            && self.format.as_deref() != Some("NOVEL")
    }
}

#[derive(Deserialize)]
struct Titulos {
    romaji: Option<String>,
    english: Option<String>,
    native: Option<String>,
}

/// Una fecha a la que le puede faltar el día o el mes.
#[derive(Deserialize)]
struct FechaDifusa {
    year: Option<i32>,
    month: Option<u32>,
    day: Option<u32>,
}

#[derive(Deserialize)]
struct Portada {
    large: Option<String>,
    #[serde(rename = "extraLarge")]
    extra_large: Option<String>,
}

#[derive(Deserialize)]
struct Equipo {
    #[serde(default)]
    edges: Vec<Miembro>,
}

#[derive(Deserialize)]
struct Miembro {
    role: Option<String>,
    node: Option<Persona>,
}

#[derive(Deserialize)]
struct Persona {
    name: Option<NombrePersona>,
}

#[derive(Deserialize)]
struct NombrePersona {
    full: Option<String>,
}

impl AniList {
    pub fn nuevo(cliente: Cliente) -> AniList {
        AniList { cliente }
    }

    /// Las series de manga que más se parecen a `texto`, de más a menos.
    pub fn buscar(&self, texto: &str) -> Result<Vec<Obra>, ErrorRed> {
        let consulta = format!(
            "query ($buscar: String) {{ Page(perPage: 10) {{ \
             media(search: $buscar, type: MANGA, sort: SEARCH_MATCH) {{ {CAMPOS} }} }} }}"
        );
        let datos: Datos = self.pedir(&consulta, json!({ "buscar": texto }))?;
        Ok(datos.pagina.media.into_iter().map(obra_de).collect())
    }

    /// Lo que dice AniList ahora de estas series.
    pub fn obras(&self, ids: &[u64]) -> Result<Vec<Obra>, ErrorRed> {
        let consulta = format!(
            "query ($ids: [Int]) {{ Page(perPage: {POR_PAGINA}) {{ \
             media(id_in: $ids, type: MANGA) {{ {CAMPOS} }} }} }}"
        );
        let mut obras = Vec::new();
        for trozo in ids.chunks(POR_PAGINA) {
            let datos: Datos = self.pedir(&consulta, json!({ "ids": trozo }))?;
            obras.extend(datos.pagina.media.into_iter().map(obra_de));
        }
        Ok(obras)
    }

    /// La ficha entera de un manga, con sus recomendadas.
    pub fn ficha(&self, id: u64) -> Result<Ficha, ErrorRed> {
        let consulta = format!(
            "query ($id: Int) {{ Media(id: $id, type: MANGA) {{ {CAMPOS} \
             recommendations(perPage: {RECOMENDADAS_EN_FICHA}, sort: RATING_DESC) {{ nodes {{ \
             rating mediaRecommendation {{ id type isAdult format countryOfOrigin \
             title {{ romaji english native }} coverImage {{ large extraLarge }} }} }} }} }} }}"
        );
        let datos: DatosFicha = self.pedir(&consulta, json!({ "id": id }))?;
        Ok(obra_de(datos.medio).ficha)
    }

    /// "Para ti": junta lo que recomiendan los lectores para cada serie de `ids` (las de
    /// la biblioteca) y devuelve las más recomendadas que no estén ya en ella. Cada serie
    /// pesa lo mismo: sus votos se dividen por los de su recomendada más votada, para que
    /// una serie muy popular no tape a las demás.
    pub fn para_ti(&self, ids: &[u64]) -> Result<Vec<Ficha>, ErrorRed> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let consulta = format!(
            "query ($ids: [Int]) {{ Page(perPage: {POR_PAGINA}) {{ media(id_in: $ids, type: MANGA) {{ \
             id recommendations(perPage: {RECOMENDADAS_POR_SERIE}, sort: RATING_DESC) {{ nodes {{ \
             rating mediaRecommendation {{ id type isAdult format countryOfOrigin }} }} }} }} }} }}"
        );
        let tuyas: HashSet<u64> = ids.iter().copied().collect();
        let mut puntos: HashMap<u64, f64> = HashMap::new();
        for trozo in ids.chunks(POR_PAGINA) {
            let datos: DatosRecomendaciones = self.pedir(&consulta, json!({ "ids": trozo }))?;
            for medio in datos.pagina.media {
                let validas: Vec<(u64, f64)> = medio
                    .recommendations
                    .into_iter()
                    .flat_map(|r| r.nodes)
                    .filter_map(|r| {
                        let obra = r.obra?;
                        let votos = r.rating.unwrap_or(0);
                        // Solo manga japonés, como en el resto de la pestaña.
                        let vale = obra.vale()
                            && obra.pais.as_deref() == Some("JP")
                            && votos > 0
                            && !tuyas.contains(&obra.id);
                        vale.then_some((obra.id, votos as f64))
                    })
                    .collect();
                let maximo = validas.iter().map(|(_, v)| *v).fold(0.0, f64::max);
                for (id, votos) in validas {
                    *puntos.entry(id).or_default() += votos / maximo;
                }
            }
        }
        let mut orden: Vec<(u64, f64)> = puntos.into_iter().collect();
        orden.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let elegidas: Vec<u64> = orden
            .into_iter()
            .take(POR_LISTA)
            .map(|(id, _)| id)
            .collect();
        if elegidas.is_empty() {
            return Ok(Vec::new());
        }
        let mut por_id: HashMap<String, Ficha> = self
            .obras(&elegidas)?
            .into_iter()
            .map(|o| (o.ficha.id.clone(), o.ficha))
            .collect();
        Ok(elegidas
            .iter()
            .filter_map(|id| por_id.remove(&id.to_string()))
            .collect())
    }

    /// La primera página de las dos listas de manga, en una sola petición.
    pub fn manga(&self, hoy: Fecha) -> Result<Manga, ErrorRed> {
        let consulta = format!(
            "query ($desde: FuzzyDateInt) {{ tendencia: {} nuevas: {} }}",
            pagina_de(ListaManga::Tendencia, "1"),
            pagina_de(ListaManga::Nuevas, "1"),
        );
        let desde = inicio_reciente(hoy);
        let datos: DatosManga = self.pedir(&consulta, json!({ "desde": entero_difuso(desde) }))?;
        let mut manga = Manga::default();
        let (fichas, hay_mas) = fichas_de(datos.tendencia, ListaManga::Tendencia, desde);
        manga.tendencia.anadir(fichas, hay_mas);
        let (fichas, hay_mas) = fichas_de(datos.nuevas, ListaManga::Nuevas, desde);
        manga.nuevas.anadir(fichas, hay_mas);
        Ok(manga)
    }

    /// La página `pagina` (desde 1) de una lista de manga, y si hay más.
    pub fn pagina(
        &self,
        lista: ListaManga,
        pagina: u32,
        hoy: Fecha,
    ) -> Result<(Vec<Ficha>, bool), ErrorRed> {
        let desde = inicio_reciente(hoy);
        let (consulta, variables) = match lista {
            // "Para ti" se hace entera de una vez: no tiene más páginas.
            ListaManga::ParaTi => return Ok((Vec::new(), false)),
            // `$desde` solo se declara donde se usa: GraphQL no admite variables de más.
            ListaManga::Tendencia => (
                format!(
                    "query ($pagina: Int) {{ lista: {} }}",
                    pagina_de(lista, "$pagina")
                ),
                json!({ "pagina": pagina }),
            ),
            ListaManga::Nuevas => (
                format!(
                    "query ($pagina: Int, $desde: FuzzyDateInt) {{ lista: {} }}",
                    pagina_de(lista, "$pagina")
                ),
                json!({ "pagina": pagina, "desde": entero_difuso(desde) }),
            ),
        };
        let datos: DatosLista = self.pedir(&consulta, variables)?;
        Ok(fichas_de(datos.lista, lista, desde))
    }

    fn pedir<T: for<'de> Deserialize<'de>>(
        &self,
        consulta: &str,
        variables: serde_json::Value,
    ) -> Result<T, ErrorRed> {
        LIMITADOR.esperar();
        let cuerpo = json!({ "query": consulta, "variables": variables }).to_string();
        let texto = self
            .cliente
            .enviar_json(URL, &cuerpo)
            .map_err(|e| match e {
                ErrorRed::Http(403) => {
                    ErrorRed::Conexion("AniList tiene su API desactivada temporalmente".into())
                }
                otro => otro,
            })?;
        let respuesta: Respuesta<T> =
            serde_json::from_str(&texto).map_err(|e| ErrorRed::Datos(e.to_string()))?;
        respuesta.data.ok_or_else(|| {
            let mensaje = respuesta
                .errors
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<_>>()
                .join("; ");
            ErrorRed::Datos(mensaje)
        })
    }
}

/// Desde cuándo cuenta una serie como "recién empezada".
fn inicio_reciente(hoy: Fecha) -> Fecha {
    hoy.mas_dias(-DIAS_RECIENTES)
}

/// `Page(...) { pageInfo { hasNextPage } media(...) { ... } }` de una lista de manga.
fn pagina_de(lista: ListaManga, pagina: &str) -> String {
    let orden = match lista {
        ListaManga::Tendencia => "sort: TRENDING_DESC",
        ListaManga::Nuevas | ListaManga::ParaTi => {
            "startDate_greater: $desde, sort: POPULARITY_DESC"
        }
    };
    format!(
        "Page(page: {pagina}, perPage: {POR_LISTA}) {{ pageInfo {{ hasNextPage }}          media({FILTRO_MANGA}, {orden}) {{ {CAMPOS} }} }}"
    )
}

/// Las fichas de una página de una lista de manga, y si hay más páginas.
fn fichas_de(pagina: Pagina, lista: ListaManga, desde: Fecha) -> (Vec<Ficha>, bool) {
    let hay_mas = pagina.info.and_then(|i| i.hay_mas).unwrap_or(false);
    let fichas = pagina
        .media
        .into_iter()
        // AniList cuela alguna recién empezada con la fecha a medias: se vuelve a mirar.
        .filter(|m| {
            lista == ListaManga::Tendencia
                || m.inicio
                    .as_ref()
                    .and_then(fecha_de)
                    .is_some_and(|f| f >= desde)
        })
        .map(|m| obra_de(m).ficha)
        .collect();
    (fichas, hay_mas)
}

/// Una fecha como la quiere AniList en los filtros: `20261019`.
fn entero_difuso(fecha: Fecha) -> i64 {
    i64::from(fecha.anio) * 10_000 + i64::from(fecha.mes) * 100 + i64::from(fecha.dia)
}

/// La fecha entera, solo si se sabe el día.
fn fecha_de(difusa: &FechaDifusa) -> Option<Fecha> {
    Fecha::leer(&format!(
        "{:04}-{:02}-{:02}",
        difusa.year?, difusa.month?, difusa.day?
    ))
}

fn obra_de(medio: Medio) -> Obra {
    let titulo = medio
        .title
        .romaji
        .clone()
        .or_else(|| medio.title.english.clone())
        .or_else(|| medio.title.native.clone())
        .unwrap_or_else(|| format!("AniList {}", medio.id));
    let mut otros_titulos: Vec<String> = [medio.title.english.clone(), medio.title.native.clone()]
        .into_iter()
        .flatten()
        .chain(medio.synonyms.iter().cloned())
        .filter(|t| *t != titulo)
        .collect();
    otros_titulos.dedup();

    let publicacion = match medio.status.as_deref() {
        Some("RELEASING") => Publicacion::EnCurso,
        Some("FINISHED") => Publicacion::Terminada,
        Some("HIATUS") => Publicacion::Pausada,
        Some("CANCELLED") => Publicacion::Cancelada,
        Some("NOT_YET_RELEASED") => Publicacion::Anunciada,
        _ => Publicacion::Desconocida,
    };
    let anio = medio.inicio.as_ref().and_then(|f| f.year);
    let tipo = formato(medio.format.as_deref(), medio.pais.as_deref());

    let mut detalle = vec![tipo.to_string()];
    detalle.extend(anio.map(|a| a.to_string()));
    if publicacion != Publicacion::Desconocida {
        detalle.push(publicacion.nombre().to_lowercase());
    }
    detalle.extend(medio.volumes.map(|t| plural(t, "tomo", "tomos")));

    let portada = medio.portada.as_ref().and_then(|p| p.large.clone());
    let ficha = Ficha {
        fuente: Fuente::AniList,
        id: medio.id.to_string(),
        titulo: titulo.clone(),
        subtitulo: medio
            .title
            .english
            .clone()
            .filter(|ingles| ingles.to_lowercase() != titulo.to_lowercase()),
        linea: {
            let mut linea = vec![tipo.to_string()];
            linea.extend(medio.pais.as_deref().and_then(pais).map(str::to_string));
            linea.extend(anio.map(|a| a.to_string()));
            if publicacion != Publicacion::Desconocida {
                linea.push(publicacion.nombre().to_lowercase());
            }
            linea.join(", ")
        },
        portada: portada.clone(),
        portada_grande: medio
            .portada
            .as_ref()
            .and_then(|p| p.extra_large.clone())
            .or_else(|| portada.clone()),
        url: medio.url.clone(),
        datos: datos_de(&medio, publicacion),
        generos: medio.genres.iter().map(|g| genero(g)).collect(),
        sinopsis: medio.description.as_deref().and_then(texto_con_parrafos),
        recomendaciones: recomendadas_de(&medio),
    };

    Obra {
        candidato: Candidato {
            fuente: Fuente::AniList,
            id: medio.id.to_string(),
            titulo,
            otros_titulos,
            anio: anio.and_then(|a| u32::try_from(a).ok()),
            detalle: detalle.join(", "),
            portada,
            url: medio.url,
            formato: medio.format,
        },
        publicacion,
        tomos: medio.volumes,
        capitulos: medio.chapters,
        ficha,
    }
}

/// Las recomendadas de una obra, si se pidieron, de más a menos votada.
fn recomendadas_de(medio: &Medio) -> Vec<Recomendada> {
    medio
        .recommendations
        .iter()
        .flat_map(|r| &r.nodes)
        .filter(|r| r.rating.unwrap_or(0) > 0)
        .filter_map(|r| r.obra.as_ref())
        .filter(|obra| obra.vale())
        .filter_map(|obra| {
            let titulos = obra.title.as_ref()?;
            let titulo = titulos.romaji.clone().or_else(|| titulos.english.clone())?;
            Some(Recomendada {
                id: obra.id.to_string(),
                titulo,
                portada: obra.portada.as_ref().and_then(|p| p.large.clone()),
            })
        })
        .collect()
}

/// Autores, publicación, fecha de inicio y nota.
fn datos_de(medio: &Medio, publicacion: Publicacion) -> Vec<(String, String)> {
    let mut datos: Vec<(String, String)> = Vec::new();
    // Los autores, juntos por papel y en el orden en que llegan.
    for miembro in medio.staff.iter().flat_map(|e| &e.edges) {
        let Some(papel) = miembro.role.as_deref().and_then(papel_anilist) else {
            continue;
        };
        let Some(nombre) = miembro
            .node
            .as_ref()
            .and_then(|n| n.name.as_ref())
            .and_then(|n| n.full.clone())
        else {
            continue;
        };
        match datos.iter_mut().find(|(p, _)| p == papel) {
            Some((_, nombres)) if !nombres.contains(&nombre) => {
                nombres.push_str(", ");
                nombres.push_str(&nombre);
            }
            Some(_) => {}
            None => datos.push((papel.to_string(), nombre)),
        }
    }

    let mut publicado = Vec::new();
    if publicacion != Publicacion::Desconocida {
        publicado.push(publicacion.nombre().to_string());
    }
    publicado.extend(medio.volumes.map(|t| plural(t, "tomo", "tomos")));
    publicado.extend(medio.chapters.map(|c| plural(c, "capítulo", "capítulos")));
    if !publicado.is_empty() {
        datos.push(("Publicación".to_string(), publicado.join(", ")));
    }
    if let Some(inicio) = &medio.inicio {
        let texto = match fecha_de(inicio) {
            Some(fecha) => Some(fecha.to_string()),
            None => inicio.year.map(|a| a.to_string()),
        };
        if let Some(texto) = texto {
            datos.push(("Empezó".to_string(), texto));
        }
    }
    if let Some(nota) = medio.nota {
        datos.push(("Nota".to_string(), format!("{nota} / 100")));
    }
    datos
}

fn formato(formato: Option<&str>, pais: Option<&str>) -> &'static str {
    match (formato, pais) {
        (Some("NOVEL"), _) => "Novela ligera",
        (Some("ONE_SHOT"), _) => "Historia corta",
        (_, Some("KR")) => "Manhwa",
        (_, Some("CN" | "TW")) => "Manhua",
        _ => "Manga",
    }
}

pub(crate) fn plural(n: u32, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_una_obra_con_su_ficha() {
        let json = r#"{"id": 30642, "title": {"romaji": "Vinland Saga", "english": "Vinland Saga",
            "native": "ヴィンランド・サガ"}, "synonyms": [], "status": "FINISHED", "volumes": 29,
            "chapters": 224, "format": "MANGA", "countryOfOrigin": "JP",
            "startDate": {"year": 2005, "month": 4, "day": 13},
            "coverImage": {"large": "https://a/l.jpg", "extraLarge": "https://a/xl.jpg"},
            "siteUrl": "https://anilist.co/manga/30642",
            "description": "Thorfinn...<br><br>(Source: X)", "genres": ["Action", "Drama"],
            "averageScore": 90, "staff": {"edges": [
              {"role": "Story & Art", "node": {"name": {"full": "Makoto Yukimura"}}},
              {"role": "Assistant", "node": {"name": {"full": "Alguien"}}}]}}"#;
        let obra = obra_de(serde_json::from_str(json).unwrap());
        let ficha = &obra.ficha;
        assert_eq!(ficha.subtitulo, None);
        assert_eq!(ficha.linea, "Manga, Japón, 2005, terminada");
        assert_eq!(ficha.portada_grande.as_deref(), Some("https://a/xl.jpg"));
        assert_eq!(ficha.generos, ["Acción", "Drama"]);
        assert_eq!(
            ficha.sinopsis.as_deref(),
            Some("Thorfinn...\n\n(Source: X)")
        );
        assert_eq!(
            ficha.datos[0],
            ("Guion y dibujo".to_string(), "Makoto Yukimura".to_string())
        );
        assert!(ficha.datos.contains(&(
            "Publicación".into(),
            "Terminada, 29 tomos, 224 capítulos".into()
        )));
        assert!(ficha
            .datos
            .contains(&("Empezó".into(), "13 abr 2005".into())));
        assert_eq!(obra.tomos, Some(29));
    }
}
