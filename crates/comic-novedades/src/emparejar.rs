//! Qué serie de un catálogo es la del usuario, y qué ha salido que no tenga.
//!
//! Al emparejar solo se acepta lo seguro: el nombre tiene que coincidir (sin contar
//! mayúsculas, tildes ni puntuación) y no puede haber dudas entre varias. Si no, el
//! usuario elige a mano. Mejor no avisar de nada que avisar de otra serie.

use std::collections::HashSet;

use comic_biblioteca::{clave_de_serie, Coleccion};
use comic_core::metadatos::normalizar_numero;

use crate::anilist::{plural, Obra};
use crate::fecha::Fecha;
use crate::{Candidato, EstadoSerie, Fuente, Numero, Publicacion};

/// Un volumen mayor que esto es un año (en el cómic americano, el volumen suele ser el
/// año en que empezó la serie), no un tomo.
const PRIMER_ANIO: u32 = 1900;

/// La serie de `anilist` o de `metron` que es sin duda la de `coleccion`, si la hay.
pub fn elegir<'a>(
    coleccion: &Coleccion,
    anilist: &'a [Candidato],
    metron: &'a [Candidato],
) -> Option<&'a Candidato> {
    let claves = [clave_de_serie(&coleccion.nombre), coleccion.clave.clone()];
    let coincide = |c: &&Candidato| {
        std::iter::once(&c.titulo)
            .chain(&c.otros_titulos)
            .any(|t| claves.contains(&clave_de_serie(t)))
    };

    // Las novelas ligeras comparten nombre con su manga, pero no son cómics.
    let manga: Vec<&Candidato> = anilist
        .iter()
        .filter(|c| c.formato.as_deref() != Some("NOVEL"))
        .filter(coincide)
        .collect();
    let manga = match manga.as_slice() {
        [unico] => Some(*unico),
        _ => None,
    };

    let comic: Vec<&Candidato> = metron.iter().filter(coincide).collect();
    let comic = match comic.as_slice() {
        [] => None,
        [unico] => Some(*unico),
        varias => por_anio(coleccion, varias),
    };

    match (manga, comic) {
        (Some(m), None) => Some(m),
        (None, Some(c)) => Some(c),
        (Some(m), Some(c)) => {
            if parece_manga(coleccion) {
                Some(m)
            } else if !coleccion.numeros.is_empty() {
                Some(c)
            } else {
                None
            }
        }
        (None, None) => None,
    }
}

/// De varias series con el mismo nombre (`Batman (2011)`, `Batman (2016)`), la que
/// encaja con los años de la colección.
fn por_anio<'a>(coleccion: &Coleccion, varias: &[&'a Candidato]) -> Option<&'a Candidato> {
    // ComicVine y Metron ponen en el volumen el año en que empezó la serie.
    if let Some(&anio) = coleccion.volumenes.iter().find(|&&v| v >= PRIMER_ANIO) {
        let mismas: Vec<_> = varias.iter().filter(|c| c.anio == Some(anio)).collect();
        if let [unica] = mismas.as_slice() {
            return Some(unica);
        }
    }
    // Si no, la última que empezó antes del cómic más antiguo de la colección.
    let anio = coleccion.anio?;
    let antes: Vec<&&Candidato> = varias
        .iter()
        .filter(|c| c.anio.is_some_and(|a| a <= anio))
        .collect();
    let ultimo = antes.iter().filter_map(|c| c.anio).max()?;
    let ultimas: Vec<_> = antes.iter().filter(|c| c.anio == Some(ultimo)).collect();
    match ultimas.as_slice() {
        [unica] => Some(unica),
        _ => None,
    }
}

/// Se lee de derecha a izquierda, o va por tomos y no por números.
fn parece_manga(coleccion: &Coleccion) -> bool {
    coleccion.manga
        || (coleccion.numeros.is_empty()
            && !coleccion.volumenes.is_empty()
            && coleccion.volumenes.iter().all(|&v| v < PRIMER_ANIO))
}

/// El valor de un número para ordenarlo y compararlo: `5`, `5.1`, `½`... Los que no son
/// cifras (`Anual`) no cuentan.
fn valor(numero: &str) -> Option<f64> {
    let numero = normalizar_numero(numero);
    if numero == "½" || numero == "1/2" {
        return Some(0.5);
    }
    numero.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Compara la colección con lo que dice AniList de la serie.
pub fn con_anilist(coleccion: &Coleccion, obra: Obra) -> EstadoSerie {
    let tomo = coleccion
        .volumenes
        .iter()
        .copied()
        .filter(|&v| v < PRIMER_ANIO)
        .max();
    // Si no hay tomos, se miran los números (capítulos sueltos).
    let capitulo = coleccion
        .numeros
        .iter()
        .filter_map(|n| valor(n))
        .fold(None, |max: Option<f64>, v| {
            Some(max.map_or(v, |m| m.max(v)))
        });

    let mas = match (tomo, obra.tomos, capitulo, obra.capitulos) {
        (Some(t), Some(publicados), _, _) => publicados.saturating_sub(t),
        (None, _, Some(c), Some(publicados)) if f64::from(publicados) > c => {
            publicados - c.floor() as u32
        }
        _ => 0,
    };
    let tienes = match (tomo, capitulo) {
        (Some(t), _) => Some(format!("tomo {t}")),
        (None, Some(c)) => Some(format!("nº {c}")),
        (None, None) => None,
    };
    let publicado: Vec<String> = [
        obra.tomos.map(|t| plural(t, "tomo", "tomos")),
        obra.capitulos.map(|c| plural(c, "capítulo", "capítulos")),
    ]
    .into_iter()
    .flatten()
    .collect();

    EstadoSerie {
        clave: coleccion.clave.clone(),
        nombre: coleccion.nombre.clone(),
        fuente: Fuente::AniList,
        id: obra.candidato.id,
        titulo: obra.candidato.titulo,
        url: obra.candidato.url,
        portada: obra.candidato.portada,
        publicacion: obra.publicacion,
        tienes,
        publicado: (!publicado.is_empty()).then(|| publicado.join(", ")),
        nuevos: Vec::new(),
        mas,
        faltan: 0,
        ficha: Some(obra.ficha),
    }
}

/// Compara la colección con los números que dice Metron que tiene la serie.
pub fn con_metron(
    coleccion: &Coleccion,
    externa: &Candidato,
    mut numeros: Vec<Numero>,
    hoy: Fecha,
) -> EstadoSerie {
    numeros.sort_by(|a, b| {
        valor(&a.numero)
            .partial_cmp(&valor(&b.numero))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.fecha.cmp(&b.fecha))
    });
    let tiene: HashSet<String> = coleccion
        .numeros
        .iter()
        .map(|n| normalizar_numero(n))
        .collect();
    let ultimo = coleccion
        .numeros
        .iter()
        .filter_map(|n| valor(n).map(|v| (v, n)))
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let (nuevos, faltan) = match ultimo {
        Some((maximo, _)) => {
            let nuevos: Vec<Numero> = numeros
                .iter()
                .filter(|n| valor(&n.numero).is_some_and(|v| v > maximo))
                .filter(|n| !tiene.contains(&normalizar_numero(&n.numero)))
                .cloned()
                .collect();
            let faltan = numeros
                .iter()
                .filter(|n| valor(&n.numero).is_some_and(|v| v <= maximo))
                .filter(|n| !tiene.contains(&normalizar_numero(&n.numero)))
                .count();
            (nuevos, faltan as u32)
        }
        // Sin números no se puede saber qué falta.
        None => (Vec::new(), 0),
    };
    // Solo cuenta lo que ya está a la venta; lo anunciado se muestra, pero no avisa.
    let mas = nuevos
        .iter()
        .filter(|n| n.fecha.is_none_or(|f| f <= hoy))
        .count() as u32;
    let reciente = numeros
        .iter()
        .filter_map(|n| n.fecha)
        .any(|f| f.dias() >= hoy.dias() - 90);
    let portada = nuevos
        .iter()
        .chain(numeros.iter().rev())
        .find_map(|n| n.portada.clone());

    EstadoSerie {
        clave: coleccion.clave.clone(),
        nombre: coleccion.nombre.clone(),
        fuente: Fuente::Metron,
        id: externa.id.clone(),
        titulo: externa.titulo.clone(),
        url: externa.url.clone(),
        portada,
        publicacion: if reciente {
            Publicacion::EnCurso
        } else {
            Publicacion::Desconocida
        },
        tienes: ultimo.map(|(_, n)| format!("#{n}")),
        publicado: (!numeros.is_empty()).then(|| plural(numeros.len() as u32, "número", "números")),
        nuevos,
        mas,
        faltan,
        ficha: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metron::sin_anio;
    use crate::Ficha;

    fn coleccion(nombre: &str, numeros: &[&str], volumenes: &[u32]) -> Coleccion {
        Coleccion {
            nombre: nombre.to_string(),
            clave: clave_de_serie(nombre),
            numeros: numeros.iter().map(|n| n.to_string()).collect(),
            volumenes: volumenes.to_vec(),
            anio: None,
            manga: false,
            comics: numeros.len().max(volumenes.len()),
        }
    }

    fn candidato(fuente: Fuente, id: &str, titulo: &str, anio: Option<u32>) -> Candidato {
        Candidato {
            fuente,
            id: id.to_string(),
            titulo: titulo.to_string(),
            otros_titulos: match fuente {
                Fuente::Metron => vec![sin_anio(titulo).to_string()],
                Fuente::AniList => Vec::new(),
            },
            anio,
            detalle: String::new(),
            portada: None,
            url: None,
            formato: Some("MANGA".to_string()),
        }
    }

    fn numero(n: &str, fecha: &str) -> Numero {
        Numero {
            id: None,
            numero: n.to_string(),
            serie: "Saga (2012)".to_string(),
            fecha: Fecha::leer(fecha),
            portada: None,
        }
    }

    #[test]
    fn solo_empareja_si_el_nombre_coincide() {
        let vinland = coleccion("Vinland Saga", &[], &[1, 2, 12]);
        let anilist = [
            candidato(Fuente::AniList, "1", "Vinland Saga", Some(2005)),
            candidato(
                Fuente::AniList,
                "2",
                "Assassin's Creed x Vinland Saga",
                None,
            ),
        ];
        assert_eq!(
            elegir(&vinland, &anilist, &[]).map(|c| c.id.as_str()),
            Some("1")
        );

        let otra = coleccion("Vinland", &[], &[1]);
        assert_eq!(elegir(&otra, &anilist, &[]), None);
    }

    #[test]
    fn entre_varios_volumenes_elige_por_anio() {
        let mut batman = coleccion("Batman", &["50"], &[2016]);
        let metron = [
            candidato(Fuente::Metron, "a", "Batman (2011)", Some(2011)),
            candidato(Fuente::Metron, "b", "Batman (2016)", Some(2016)),
        ];
        assert_eq!(
            elegir(&batman, &[], &metron).map(|c| c.id.as_str()),
            Some("b")
        );

        // Sin volumen: la última que empezó antes del cómic más antiguo.
        batman.volumenes.clear();
        batman.anio = Some(2014);
        assert_eq!(
            elegir(&batman, &[], &metron).map(|c| c.id.as_str()),
            Some("a")
        );

        // Sin años no hay forma de saberlo.
        batman.anio = None;
        assert_eq!(elegir(&batman, &[], &metron), None);
    }

    #[test]
    fn manga_y_comic_con_el_mismo_nombre() {
        let anilist = [candidato(Fuente::AniList, "m", "Monster", None)];
        let metron = [candidato(Fuente::Metron, "c", "Monster (2020)", Some(2020))];
        let tomos = coleccion("Monster", &[], &[1, 2]);
        assert_eq!(
            elegir(&tomos, &anilist, &metron).map(|c| c.id.as_str()),
            Some("m")
        );
        let numeros = coleccion("Monster", &["1", "2"], &[]);
        assert_eq!(
            elegir(&numeros, &anilist, &metron).map(|c| c.id.as_str()),
            Some("c")
        );
    }

    #[test]
    fn tomos_nuevos_de_un_manga() {
        let vinland = coleccion("Vinland Saga", &[], &[1, 2, 12]);
        let obra = Obra {
            candidato: candidato(Fuente::AniList, "1", "Vinland Saga", Some(2005)),
            publicacion: Publicacion::Terminada,
            tomos: Some(14),
            capitulos: Some(224),
            ficha: Ficha {
                fuente: Fuente::AniList,
                id: "1".into(),
                titulo: "Vinland Saga".into(),
                subtitulo: None,
                linea: String::new(),
                portada: None,
                portada_grande: None,
                url: None,
                datos: Vec::new(),
                generos: Vec::new(),
                sinopsis: None,
                recomendaciones: Vec::new(),
            },
        };
        let estado = con_anilist(&vinland, obra);
        assert_eq!(estado.mas, 2);
        assert_eq!(estado.tienes.as_deref(), Some("tomo 12"));
        assert_eq!(estado.publicado.as_deref(), Some("14 tomos, 224 capítulos"));
    }

    #[test]
    fn numeros_nuevos_y_huecos() {
        let saga = coleccion("Saga", &["1", "2", "4"], &[]);
        let externa = candidato(Fuente::Metron, "7", "Saga (2012)", Some(2012));
        let numeros = vec![
            numero("6", "2026-11-18"),
            numero("1", "2012-03-14"),
            numero("2", "2012-04-11"),
            numero("3", "2012-05-09"),
            numero("4", "2012-06-13"),
            numero("5", "2026-10-14"),
        ];
        let hoy = Fecha::leer("2026-10-19").unwrap();
        let estado = con_metron(&saga, &externa, numeros, hoy);
        let nuevos: Vec<&str> = estado.nuevos.iter().map(|n| n.numero.as_str()).collect();
        assert_eq!(nuevos, ["5", "6"]);
        // El 6 aún no ha salido: se enseña, pero no cuenta como novedad.
        assert_eq!(estado.mas, 1);
        assert_eq!(estado.faltan, 1);
        assert_eq!(estado.tienes.as_deref(), Some("#4"));
        assert_eq!(estado.publicacion, Publicacion::EnCurso);
    }
}
