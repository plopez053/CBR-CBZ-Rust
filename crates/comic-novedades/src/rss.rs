//! Titulares de las webs de noticias, de su RSS (2.0 o 1.0) o Atom, con su foto.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde::{Deserialize, Serialize};

use crate::fecha::leer_instante;

/// Largo máximo del resumen de una noticia, en caracteres.
const LARGO_RESUMEN: usize = 320;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Noticia {
    pub titulo: String,
    pub enlace: String,
    /// El nombre de la web (el título del canal).
    pub fuente: String,
    /// Segundos desde 1970.
    pub fecha: Option<i64>,
    /// Texto plano, sin HTML, recortado.
    pub resumen: Option<String>,
    /// La foto de la noticia: la del RSS o, si no trae, la de la página del artículo.
    #[serde(default)]
    pub imagen: Option<String>,
    /// Si ya se buscó la foto en la página del artículo (para no volver a pedirla).
    #[serde(default)]
    pub foto_buscada: bool,
}

/// Lo que se saca de un RSS o Atom.
#[derive(Debug, Default)]
pub struct Canal {
    pub titulo: Option<String>,
    pub noticias: Vec<Noticia>,
}

#[derive(Default)]
struct Borrador {
    titulo: String,
    enlace: String,
    fecha: String,
    resumen: String,
    /// El artículo entero en HTML (`content:encoded` o el `content` de Atom).
    contenido: String,
    imagen: Option<String>,
}

/// Qué campo de la noticia se está leyendo.
#[derive(Clone, Copy, PartialEq)]
enum Campo {
    Titulo,
    Enlace,
    Fecha,
    Resumen,
    Contenido,
    TituloDelCanal,
}

pub fn leer(xml: &str) -> Result<Canal, String> {
    let mut lector = Reader::from_str(xml.trim_start_matches('\u{feff}'));
    let mut canal = Canal::default();
    let mut titulo_canal = String::new();
    // Las etiquetas abiertas, sin espacio de nombres y en minúsculas.
    let mut pila: Vec<String> = Vec::new();
    let mut noticia: Option<Borrador> = None;
    let mut campo: Option<(Campo, usize)> = None;

    loop {
        let evento = lector.read_event().map_err(|e| e.to_string())?;
        match &evento {
            Event::Start(etiqueta) | Event::Empty(etiqueta) => {
                let nombre = etiqueta.local_name().as_ref().to_lowercase();
                let vacia = matches!(evento, Event::Empty(_));
                let padre = pila.last().map(String::as_str);

                if nombre == "item" || nombre == "entry" {
                    noticia = Some(Borrador::default());
                } else if let Some(borrador) = &mut noticia {
                    if padre == Some("item") || padre == Some("entry") {
                        let atributo = |clave: &str| atributo(etiqueta, clave);
                        // La foto: <media:content url="..." medium="image"/>,
                        // <media:thumbnail url="..."/> o <enclosure type="image/jpeg" url="..."/>.
                        if borrador.imagen.is_none()
                            && matches!(nombre.as_str(), "content" | "thumbnail" | "enclosure")
                        {
                            let tipo = atributo("type").or_else(|| atributo("medium"));
                            let es_imagen = tipo.as_deref().is_none_or(|t| t.starts_with("image"));
                            if let (Some(url), true) = (atributo("url"), es_imagen) {
                                borrador.imagen = imagen_valida(&url);
                            }
                        }
                        // Atom: <link rel="alternate" href="..."/>
                        if nombre == "link" {
                            let rel = atributo("rel");
                            if let Some(href) = atributo("href") {
                                if rel.as_deref().is_none_or(|r| r == "alternate")
                                    && borrador.enlace.is_empty()
                                {
                                    borrador.enlace = href;
                                }
                            }
                        }
                        let que = match nombre.as_str() {
                            "title" => Some(Campo::Titulo),
                            "link" => Some(Campo::Enlace),
                            "pubdate" | "published" | "date" => Some(Campo::Fecha),
                            // `updated` solo si no hay otra fecha (llega después, se mira al leer).
                            "updated" => Some(Campo::Fecha),
                            "description" | "summary" => Some(Campo::Resumen),
                            // `content:encoded` (RSS) o `content` (Atom): el artículo entero.
                            "encoded" | "content" => Some(Campo::Contenido),
                            _ => None,
                        };
                        if let (Some(que), false) = (que, vacia) {
                            if que == Campo::Fecha && !borrador.fecha.is_empty() {
                                // Ya hay una fecha mejor.
                            } else {
                                if que == Campo::Resumen {
                                    borrador.resumen.clear();
                                }
                                if que == Campo::Enlace {
                                    borrador.enlace.clear();
                                }
                                campo = Some((que, pila.len() + 1));
                            }
                        }
                    }
                } else if nombre == "title"
                    && matches!(padre, Some("channel") | Some("feed"))
                    && !vacia
                {
                    campo = Some((Campo::TituloDelCanal, pila.len() + 1));
                }

                if !vacia {
                    pila.push(nombre);
                }
            }
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_) => {
                let Some((que, profundidad)) = campo else {
                    continue;
                };
                if profundidad != pila.len() {
                    continue;
                }
                let destino = match (que, &mut noticia) {
                    (Campo::TituloDelCanal, _) => &mut titulo_canal,
                    (_, None) => continue,
                    (Campo::Titulo, Some(b)) => &mut b.titulo,
                    (Campo::Enlace, Some(b)) => &mut b.enlace,
                    (Campo::Fecha, Some(b)) => &mut b.fecha,
                    (Campo::Resumen, Some(b)) => &mut b.resumen,
                    (Campo::Contenido, Some(b)) => &mut b.contenido,
                };
                match &evento {
                    Event::Text(t) => destino.push_str(&t.xml10_content()),
                    Event::CData(t) => destino.push_str(&t.xml10_content()),
                    // Las entidades (`&amp;`, `&#233;`...) llegan aparte del texto.
                    Event::GeneralRef(r) => match r.resolve_char_ref() {
                        Ok(Some(c)) => destino.push(c),
                        _ => {
                            let nombre: &str = r.as_ref();
                            match nombre {
                                "amp" => destino.push('&'),
                                "lt" => destino.push('<'),
                                "gt" => destino.push('>'),
                                "quot" => destino.push('"'),
                                "apos" => destino.push('\''),
                                // Una entidad de HTML (`&nbsp;`): se deja para luego.
                                _ => destino.push_str(&format!("&{nombre};")),
                            }
                        }
                    },
                    _ => {}
                }
            }
            Event::End(_) => {
                let nombre = pila.pop().unwrap_or_default();
                if campo.is_some_and(|(_, profundidad)| profundidad > pila.len()) {
                    campo = None;
                }
                if nombre == "item" || nombre == "entry" {
                    if let Some(borrador) = noticia.take() {
                        canal.noticias.extend(terminar(borrador));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    let titulo = texto_plano(&titulo_canal);
    canal.titulo = (!titulo.is_empty()).then_some(titulo);
    if let Some(titulo) = &canal.titulo {
        for noticia in &mut canal.noticias {
            noticia.fuente = titulo.clone();
        }
    }
    Ok(canal)
}

fn atributo(etiqueta: &BytesStart, clave: &str) -> Option<String> {
    etiqueta
        .try_get_attribute(clave)
        .ok()
        .flatten()
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

/// Una noticia sin título o sin enlace no sirve.
fn terminar(borrador: Borrador) -> Option<Noticia> {
    let titulo = texto_plano(&borrador.titulo);
    let enlace = borrador.enlace.trim().to_string();
    if titulo.is_empty() || !(enlace.starts_with("https://") || enlace.starts_with("http://")) {
        return None;
    }
    // Sin resumen, el principio del artículo.
    let mut resumen = texto_plano(&borrador.resumen);
    if resumen.is_empty() {
        resumen = texto_plano(&borrador.contenido);
    }
    let resumen = recortar(&resumen, LARGO_RESUMEN);
    let imagen = borrador
        .imagen
        .or_else(|| primera_imagen(&borrador.resumen))
        .or_else(|| primera_imagen(&borrador.contenido));
    Some(Noticia {
        titulo,
        enlace,
        fuente: String::new(),
        fecha: leer_instante(&borrador.fecha),
        resumen: (!resumen.is_empty()).then_some(resumen),
        imagen,
        foto_buscada: false,
    })
}

/// La primera `<img>` de un trozo de HTML que parezca una foto de verdad.
fn primera_imagen(html: &str) -> Option<String> {
    let mut resto = html;
    while let Some(inicio) = resto.find("<img") {
        resto = &resto[inicio + 4..];
        let fin = resto.find('>').unwrap_or(resto.len());
        let src = atributos_html(&resto[..fin])
            .into_iter()
            .find(|(clave, _)| clave == "src")
            .and_then(|(_, valor)| imagen_valida(&valor));
        if src.is_some() {
            return src;
        }
        resto = &resto[fin..];
    }
    None
}

/// La foto que pone la web en la cabecera de un artículo para cuando se comparte en las
/// redes (`og:image`, o la de Twitter si no hay). `pagina` es la dirección del
/// artículo, para las rutas relativas.
pub fn imagen_de_pagina(html: &str, pagina: &str) -> Option<String> {
    let cabecera = match html.find("</head>") {
        Some(fin) => &html[..fin],
        None => html,
    };
    let mut mejor: Option<(u8, String)> = None;
    let mut resto = cabecera;
    while let Some(inicio) = resto.find("<meta") {
        resto = &resto[inicio + 5..];
        let fin = resto.find('>').unwrap_or(resto.len());
        let atributos = atributos_html(&resto[..fin]);
        resto = &resto[fin..];
        let valor = |nombre: &str| {
            atributos
                .iter()
                .find(|(clave, _)| clave == nombre)
                .map(|(_, valor)| valor.as_str())
        };
        let Some(propiedad) = valor("property").or_else(|| valor("name")) else {
            continue;
        };
        let prioridad = match propiedad.to_ascii_lowercase().as_str() {
            "og:image" | "og:image:url" | "og:image:secure_url" => 0,
            "twitter:image" | "twitter:image:src" => 1,
            _ => continue,
        };
        let Some(url) = valor("content").and_then(|c| absoluta(c, pagina)) else {
            continue;
        };
        if mejor.as_ref().is_none_or(|(p, _)| prioridad < *p) {
            mejor = Some((prioridad, url));
        }
    }
    mejor.map(|(_, url)| url)
}

/// Pasa una ruta relativa a dirección completa.
fn absoluta(url: &str, pagina: &str) -> Option<String> {
    let url = entidades(url.trim());
    let completa = if let Some(sin_esquema) = url.strip_prefix("//") {
        format!("https://{sin_esquema}")
    } else if url.starts_with('/') {
        let (esquema, resto) = pagina.split_once("://")?;
        let servidor = resto.split(['/', '?', '#']).next()?;
        format!("{esquema}://{servidor}{url}")
    } else {
        url
    };
    imagen_valida(&completa)
}

/// Solo fotos de internet, y por HTTPS (las portadas no se bajan de otra forma). Se
/// descartan los emoticonos de WordPress, que también van como `<img>`.
fn imagen_valida(url: &str) -> Option<String> {
    let url = entidades(url.trim());
    let url = match url.strip_prefix("http://") {
        Some(resto) => format!("https://{resto}"),
        None => url,
    };
    let valida = url.starts_with("https://") && !url.contains("/images/core/emoji/");
    valida.then_some(url)
}

/// Los atributos de una etiqueta HTML (lo que va tras su nombre), con las claves en
/// minúsculas. Admite valores con comillas dobles, simples o sin comillas.
fn atributos_html(etiqueta: &str) -> Vec<(String, String)> {
    let mut atributos = Vec::new();
    let mut resto = etiqueta.trim_start();
    while !resto.is_empty() {
        let fin_clave = resto
            .find(|c: char| c == '=' || c.is_whitespace() || c == '/')
            .unwrap_or(resto.len());
        if fin_clave == 0 && !resto.starts_with('=') {
            // Un `/` suelto.
            resto = resto[1..].trim_start();
            continue;
        }
        let clave = resto[..fin_clave].to_ascii_lowercase();
        resto = resto[fin_clave..].trim_start();
        let mut valor = String::new();
        if let Some(tras_igual) = resto.strip_prefix('=') {
            let tras_igual = tras_igual.trim_start();
            match tras_igual.chars().next() {
                Some(comilla @ ('"' | '\'')) => {
                    let dentro = &tras_igual[1..];
                    let fin = dentro.find(comilla).unwrap_or(dentro.len());
                    valor = dentro[..fin].to_string();
                    resto = dentro.get(fin + 1..).unwrap_or("");
                }
                _ => {
                    let fin = tras_igual
                        .find(char::is_whitespace)
                        .unwrap_or(tras_igual.len());
                    valor = tras_igual[..fin].trim_end_matches('/').to_string();
                    resto = &tras_igual[fin..];
                }
            }
        }
        if !clave.is_empty() {
            atributos.push((clave, valor));
        }
        resto = resto.trim_start();
    }
    atributos
}

/// Quita las etiquetas de HTML, traduce las entidades y junta los espacios.
pub fn texto_plano(html: &str) -> String {
    let mut sin_etiquetas = String::with_capacity(html.len());
    let mut dentro = false;
    for c in html.chars() {
        match c {
            '<' => dentro = true,
            '>' if dentro => {
                dentro = false;
                // Una etiqueta separa palabras (`<p>uno</p><p>dos</p>`).
                sin_etiquetas.push(' ');
            }
            _ if !dentro => sin_etiquetas.push(c),
            _ => {}
        }
    }
    let texto = entidades(&sin_etiquetas);
    texto.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn entidades(texto: &str) -> String {
    let mut resultado = String::with_capacity(texto.len());
    let mut resto = texto;
    while let Some(inicio) = resto.find('&') {
        resultado.push_str(&resto[..inicio]);
        resto = &resto[inicio..];
        let fin = resto.find(';').filter(|&f| f <= 10);
        let caracter = fin.and_then(|fin| {
            let nombre = &resto[1..fin];
            let caracter = if let Some(numero) = nombre.strip_prefix('#') {
                let codigo = match numero.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => numero.parse().ok(),
                };
                codigo.and_then(char::from_u32)
            } else {
                match nombre {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    "hellip" => Some('…'),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    "laquo" => Some('«'),
                    "raquo" => Some('»'),
                    "lsquo" => Some('‘'),
                    "rsquo" => Some('’'),
                    "ldquo" => Some('“'),
                    "rdquo" => Some('”'),
                    "iexcl" => Some('¡'),
                    "iquest" => Some('¿'),
                    _ => None,
                }
            };
            caracter.map(|c| (c, fin))
        });
        match caracter {
            Some((c, fin)) => {
                resultado.push(c);
                resto = &resto[fin + 1..];
            }
            None => {
                resultado.push('&');
                resto = &resto[1..];
            }
        }
    }
    resultado.push_str(resto);
    resultado
}

/// Corta `texto` en `largo` caracteres como mucho, por un espacio, con `…` al final.
fn recortar(texto: &str, largo: usize) -> String {
    if texto.chars().count() <= largo {
        return texto.to_string();
    }
    let corte: String = texto.chars().take(largo).collect();
    let corte = match corte.rfind(' ') {
        Some(espacio) if espacio > largo / 2 => &corte[..espacio],
        _ => &corte,
    };
    format!("{}…", corte.trim_end_matches([',', '.', ';', ':', ' ']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_rss_2() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/elements/1.1/">
<channel>
  <title>Ramen Para Dos</title>
  <atom:link href="https://www.ramenparados.com/feed/" rel="self" type="application/rss+xml" />
  <link>https://www.ramenparados.com</link>
  <item>
    <title>Planeta anuncia &#8220;Vinland Saga&#8221; &amp; más</title>
    <link>https://www.ramenparados.com/vinland/</link>
    <pubDate>Mon, 19 Oct 2026 15:08:00 +0000</pubDate>
    <description><![CDATA[<p>La edición <strong>integral</strong>&nbsp;llega en&hellip;</p><p>Otro párrafo</p>]]></description>
  </item>
  <item>
    <title>Sin enlace</title>
  </item>
</channel>
</rss>"#;
        let canal = leer(xml).unwrap();
        assert_eq!(canal.titulo.as_deref(), Some("Ramen Para Dos"));
        assert_eq!(canal.noticias.len(), 1);
        let noticia = &canal.noticias[0];
        assert_eq!(noticia.titulo, "Planeta anuncia “Vinland Saga” & más");
        assert_eq!(noticia.enlace, "https://www.ramenparados.com/vinland/");
        assert_eq!(noticia.fuente, "Ramen Para Dos");
        assert!(noticia.fecha.is_some());
        assert_eq!(
            noticia.resumen.as_deref(),
            Some("La edición integral llega en… Otro párrafo")
        );
    }

    #[test]
    fn lee_atom() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title type="text">Noticias</title>
  <link href="https://ejemplo.com/" rel="alternate"/>
  <entry>
    <title>Primera</title>
    <link rel="replies" href="https://ejemplo.com/1#comentarios"/>
    <link rel="alternate" type="text/html" href="https://ejemplo.com/1"/>
    <updated>2026-10-19T17:08:00+02:00</updated>
    <published>2026-10-18T10:00:00Z</published>
    <summary type="html">&lt;b&gt;Resumen&lt;/b&gt; corto</summary>
  </entry>
</feed>"#;
        let canal = leer(xml).unwrap();
        assert_eq!(canal.titulo.as_deref(), Some("Noticias"));
        let noticia = &canal.noticias[0];
        assert_eq!(noticia.enlace, "https://ejemplo.com/1");
        assert_eq!(noticia.resumen.as_deref(), Some("Resumen corto"));
        // `updated` llega antes que `published`, pero se queda la primera que llega.
        assert_eq!(noticia.fecha, leer_instante("2026-10-19T15:08:00Z"));
    }

    #[test]
    fn saca_la_foto_de_cada_noticia() {
        let xml = r#"<rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"
            xmlns:content="http://purl.org/rss/1.0/modules/content/"><channel><title>A</title>
  <item><title>Con media</title><link>https://a.com/1</link>
    <media:content url="https://a.com/1.jpg" medium="image"/></item>
  <item><title>Con enclosure</title><link>https://a.com/2</link>
    <enclosure url="https://a.com/2.mp3" type="audio/mpeg"/>
    <enclosure url="http://a.com/2.png" type="image/png"/></item>
  <item><title>En el artículo</title><link>https://a.com/3</link>
    <description>Corto</description>
    <content:encoded><![CDATA[<p><img src="https://s.w.org/images/core/emoji/x.svg"> Hola
      <img class="x" src='https://a.com/3.jpg?a=1&amp;b=2' alt="">]]></content:encoded></item>
  <item><title>Sin foto</title><link>https://a.com/4</link>
    <content:encoded><![CDATA[<p>Solo el artículo</p>]]></content:encoded></item>
</channel></rss>"#;
        let noticias = leer(xml).unwrap().noticias;
        let imagenes: Vec<Option<&str>> = noticias.iter().map(|n| n.imagen.as_deref()).collect();
        assert_eq!(
            imagenes,
            [
                Some("https://a.com/1.jpg"),
                Some("https://a.com/2.png"),
                Some("https://a.com/3.jpg?a=1&b=2"),
                None
            ]
        );
        assert_eq!(noticias[2].resumen.as_deref(), Some("Corto"));
        // Sin resumen, el principio del artículo.
        assert_eq!(noticias[3].resumen.as_deref(), Some("Solo el artículo"));
    }

    #[test]
    fn saca_la_foto_de_la_cabecera_de_un_articulo() {
        let html = r#"<html><head><meta name="twitter:image" content="https://a.com/tw.jpg">
            <meta property="og:image" content="/fotos/og.jpg" />
            <meta charset="utf-8"></head><body>
            <meta property="og:image" content="https://a.com/no.jpg"></body>"#;
        assert_eq!(
            imagen_de_pagina(html, "https://www.a.com/noticia/1").as_deref(),
            Some("https://www.a.com/fotos/og.jpg")
        );
        assert_eq!(imagen_de_pagina("<head></head>", "https://a.com"), None);
        assert_eq!(
            imagen_de_pagina(
                "<meta content='//cdn.a.com/x.jpg' property=og:image/>",
                "https://a.com"
            )
            .as_deref(),
            Some("https://cdn.a.com/x.jpg")
        );
    }

    #[test]
    fn un_xml_roto_da_error() {
        assert!(leer("<rss><channel><item><title>a</channel>").is_err());
    }

    #[test]
    fn recorta_por_palabras() {
        assert_eq!(recortar("uno dos tres", 20), "uno dos tres");
        assert_eq!(recortar("uno dos tres cuatro", 13), "uno dos tres…");
    }
}
