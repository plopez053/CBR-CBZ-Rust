//! Novedades: tomos y números nuevos de las series de la biblioteca, lo que sale esta
//! semana y titulares de noticias.
//!
//! - Manga: AniList, sin cuenta. También el manga en tendencia y el recién empezado.
//! - Cómic americano y lo que sale cada semana: Metron, con una cuenta gratuita.
//! - Noticias: los RSS que elija el usuario, con la foto de cada una.
//!
//! Solo se consulta: nunca se descarga ningún cómic. Cada parte (tus series, el manga,
//! lo de esta semana y las noticias) se comprueba por separado, al entrar en su pestaña,
//! en un hilo aparte.

mod anilist;
mod emparejar;
mod fecha;
mod ficha;
mod metron;
mod red;
mod rss;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread;

use comic_biblioteca::{Biblioteca, Coleccion, EstadoVinculo, SerieExterna, Vinculo};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

pub use fecha::{ahora, Fecha};
pub use ficha::{Ficha, Recomendada};
pub use metron::sin_anio;
pub use red::{Cliente, ErrorRed};
pub use rss::Noticia;

use anilist::AniList;
use metron::Metron;

/// Sube cuando el informe guarda cosas nuevas: uno de una versión anterior se vuelve a
/// comprobar al entrar en la página, aunque sea reciente.
pub const VERSION_INFORME: u32 = 5;

/// Una serie que no encontró nada seguro se vuelve a buscar pasado este tiempo, por si
/// el catálogo la ha añadido.
const REBUSCAR_TRAS: i64 = 30 * 86_400;
/// Noticias que se leen de cada RSS y que se guardan en total.
const NOTICIAS_POR_CANAL: usize = 30;
const MAX_NOTICIAS: usize = 150;
/// Fotos de noticias que se buscan como mucho en cada comprobación, con cuántos hilos y
/// cuánto se lee de cada artículo (la cabecera va al principio).
const MAX_FOTOS: usize = 80;
const HILOS_FOTOS: usize = 6;
const BYTES_CABECERA: u64 = 256 * 1024;

/// De dónde sale la información de una serie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Fuente {
    AniList,
    Metron,
}

impl Fuente {
    pub fn nombre(self) -> &'static str {
        match self {
            Fuente::AniList => "AniList",
            Fuente::Metron => "Metron",
        }
    }

    /// Cómo se guarda en la biblioteca.
    pub fn clave(self) -> &'static str {
        match self {
            Fuente::AniList => "anilist",
            Fuente::Metron => "metron",
        }
    }

    pub fn desde_clave(clave: &str) -> Option<Fuente> {
        [Fuente::AniList, Fuente::Metron]
            .into_iter()
            .find(|f| f.clave() == clave)
    }
}

/// Una serie de un catálogo que puede ser la del usuario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidato {
    pub fuente: Fuente,
    pub id: String,
    pub titulo: String,
    /// Otros nombres con los que se la conoce, para reconocerla.
    pub otros_titulos: Vec<String>,
    /// Año en que empezó.
    pub anio: Option<u32>,
    /// `Manga, 2005, terminada, 29 tomos`
    pub detalle: String,
    /// URL de la portada.
    pub portada: Option<String>,
    /// Su página en la web del catálogo.
    pub url: Option<String>,
    /// En AniList, el formato (`MANGA`, `NOVEL`...).
    pub formato: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Publicacion {
    EnCurso,
    Terminada,
    Pausada,
    Cancelada,
    Anunciada,
    Desconocida,
}

impl Publicacion {
    pub fn nombre(self) -> &'static str {
        match self {
            Publicacion::EnCurso => "En publicación",
            Publicacion::Terminada => "Terminada",
            Publicacion::Pausada => "En pausa",
            Publicacion::Cancelada => "Cancelada",
            Publicacion::Anunciada => "Anunciada",
            Publicacion::Desconocida => "",
        }
    }
}

/// Un número de una serie de Metron.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Numero {
    /// El identificador en Metron, para pedir su ficha.
    #[serde(default)]
    pub id: Option<u64>,
    pub numero: String,
    /// `Saga (2012)`
    pub serie: String,
    /// La de salida a la venta, o la de portada si no se sabe.
    pub fecha: Option<Fecha>,
    pub portada: Option<String>,
}

/// Lo publicado de una serie de la biblioteca, comparado con lo que tiene el usuario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EstadoSerie {
    /// La de la serie en la biblioteca.
    pub clave: String,
    /// Como se llama en la biblioteca.
    pub nombre: String,
    pub fuente: Fuente,
    pub id: String,
    /// Como se llama en el catálogo.
    pub titulo: String,
    pub url: Option<String>,
    pub portada: Option<String>,
    pub publicacion: Publicacion,
    /// Lo último que tiene el usuario: `tomo 12`, `#5`.
    pub tienes: Option<String>,
    /// Lo que hay publicado: `29 tomos, 224 capítulos`, `66 números`. En AniList es la
    /// edición original japonesa, que puede tener otro número de tomos que la española.
    pub publicado: Option<String>,
    /// Los números posteriores al último que tiene el usuario, incluidos los anunciados.
    pub nuevos: Vec<Numero>,
    /// Cuántos tomos o números ya a la venta hay después del último que tiene.
    pub mas: u32,
    /// Cuántos números anteriores al último que tiene le faltan.
    pub faltan: u32,
    /// La ficha de la obra, si el catálogo la da con lo demás (AniList sí; de Metron
    /// se pide aparte al abrirla).
    #[serde(default)]
    pub ficha: Option<Ficha>,
}

impl EstadoSerie {
    pub fn hay_novedades(&self) -> bool {
        self.mas > 0
    }
}

/// Lo que sale a la venta en una semana (de lunes a domingo).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Semana {
    pub desde: Fecha,
    pub hasta: Fecha,
    pub numeros: Vec<Numero>,
}

/// Las listas de manga de AniList.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ListaManga {
    /// Lo que recomiendan los lectores a quien lee los manga de la biblioteca.
    ParaTi,
    /// El que más se está leyendo ahora.
    Tendencia,
    /// Series que han empezado hace poco, de la más leída a la menos.
    Nuevas,
}

/// Lo leído de una lista de manga: se va pidiendo página a página según se baja.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ListaDeManga {
    pub fichas: Vec<Ficha>,
    /// Cuántas páginas de AniList se han leído.
    pub paginas: u32,
    /// Si AniList tiene más páginas.
    pub hay_mas: bool,
}

impl ListaDeManga {
    /// Añade una página. Entre una página y otra la lista puede haber cambiado (la de
    /// tendencia cambia cada poco), así que las que ya estaban no se repiten.
    pub fn anadir(&mut self, fichas: Vec<Ficha>, hay_mas: bool) {
        for ficha in fichas {
            if !self.fichas.iter().any(|f| f.id == ficha.id) {
                self.fichas.push(ficha);
            }
        }
        self.paginas += 1;
        self.hay_mas = hay_mas;
    }
}

/// El manga de AniList que se enseña aunque no esté en la biblioteca: solo japonés.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Manga {
    #[serde(default)]
    pub para_ti: ListaDeManga,
    pub tendencia: ListaDeManga,
    pub nuevas: ListaDeManga,
}

impl Manga {
    pub fn lista(&self, lista: ListaManga) -> &ListaDeManga {
        match lista {
            ListaManga::ParaTi => &self.para_ti,
            ListaManga::Tendencia => &self.tendencia,
            ListaManga::Nuevas => &self.nuevas,
        }
    }

    pub fn lista_mut(&mut self, lista: ListaManga) -> &mut ListaDeManga {
        match lista {
            ListaManga::ParaTi => &mut self.para_ti,
            ListaManga::Tendencia => &mut self.tendencia,
            ListaManga::Nuevas => &mut self.nuevas,
        }
    }
}

/// Lo que no se entiende (lo guardó una versión anterior, con otra forma) se deja vacío
/// en vez de perder todo el informe.
fn o_nada<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Option<T>, D::Error> {
    let valor = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Option<T>>(valor).ok().flatten())
}

/// El resultado de la última comprobación. Se guarda en disco para enseñarlo nada más
/// abrir la app, sin esperar a internet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Informe {
    /// `VERSION_INFORME` del programa que lo hizo.
    #[serde(default)]
    pub version: u32,
    /// Cuándo se comprobó cada parte por última vez, en segundos desde 1970.
    #[serde(default)]
    pub comprobadas: HashMap<Parte, i64>,
    /// Primero las que tienen novedades y luego por nombre.
    pub series: Vec<EstadoSerie>,
    pub semana: Option<Semana>,
    #[serde(default, deserialize_with = "o_nada")]
    pub manga: Option<Manga>,
    /// De la más reciente a la más antigua.
    pub noticias: Vec<Noticia>,
    /// Lo que falló la última vez que se comprobó cada parte.
    #[serde(default)]
    pub fallos: HashMap<Parte, Vec<String>>,
}

impl Informe {
    /// Lee el informe guardado; si no hay o no se entiende, uno vacío.
    pub fn cargar(ruta: &Path) -> Informe {
        let Ok(texto) = fs::read_to_string(ruta) else {
            return Informe::default();
        };
        serde_json::from_str(&texto).unwrap_or_else(|e| {
            eprintln!("[novedades] No se entiende {}: {e}", ruta.display());
            Informe::default()
        })
    }

    /// Se escribe primero en un temporal y luego se renombra, para no dejarlo a medias.
    pub fn guardar(&self, ruta: &Path) -> Result<(), String> {
        if let Some(carpeta) = ruta.parent() {
            fs::create_dir_all(carpeta).map_err(|e| e.to_string())?;
        }
        let texto = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let temporal = ruta.with_extension("tmp");
        fs::write(&temporal, texto).map_err(|e| e.to_string())?;
        fs::rename(&temporal, ruta).map_err(|e| e.to_string())
    }

    /// Cuántas series tienen novedades.
    pub fn con_novedades(&self) -> usize {
        self.series.iter().filter(|s| s.hay_novedades()).count()
    }

    /// Cuándo se comprobó `parte` por última vez.
    pub fn comprobada(&self, parte: Parte) -> Option<i64> {
        self.comprobadas.get(&parte).copied()
    }

    /// Lo que falló la última vez que se comprobó `parte`.
    pub fn fallos_de(&self, parte: Parte) -> &[String] {
        self.fallos.get(&parte).map_or(&[], Vec::as_slice)
    }

    /// Copia de `otro` solo lo de `parte`. Así cada parte se comprueba por su lado, a la
    /// vez que las demás, sin pisarse.
    pub fn copiar(&mut self, otro: &Informe, parte: Parte) {
        match parte {
            Parte::Series => self.series = otro.series.clone(),
            Parte::Manga => self.manga = otro.manga.clone(),
            Parte::Semana => self.semana = otro.semana.clone(),
            Parte::Noticias => self.noticias = otro.noticias.clone(),
        }
        match otro.comprobadas.get(&parte) {
            Some(&cuando) => self.comprobadas.insert(parte, cuando),
            None => self.comprobadas.remove(&parte),
        };
        match otro.fallos.get(&parte) {
            Some(fallos) => self.fallos.insert(parte, fallos.clone()),
            None => self.fallos.remove(&parte),
        };
    }
}

/// La cuenta de Metron.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cuenta {
    pub usuario: String,
    pub contrasena: String,
}

/// Lo que configura el usuario para las novedades.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ajustes {
    pub metron: Option<Cuenta>,
    /// Las direcciones de los RSS de noticias.
    pub canales: Vec<String>,
}

impl Ajustes {
    /// Lee los ajustes de un archivo de texto con una línea `clave=valor` por ajuste.
    pub fn cargar(ruta: &Path) -> Ajustes {
        let mut ajustes = Ajustes::default();
        let Ok(texto) = fs::read_to_string(ruta) else {
            return ajustes;
        };
        let mut cuenta = Cuenta::default();
        for linea in texto.lines() {
            let Some((clave, valor)) = linea.split_once('=') else {
                continue;
            };
            let valor = valor.trim();
            match clave.trim() {
                "metron_usuario" => cuenta.usuario = valor.to_string(),
                "metron_contrasena" => cuenta.contrasena = valor.to_string(),
                "rss" if !valor.is_empty() => ajustes.canales.push(valor.to_string()),
                otra => eprintln!("[novedades] Ajuste desconocido: {otra}"),
            }
        }
        if !cuenta.usuario.is_empty() && !cuenta.contrasena.is_empty() {
            ajustes.metron = Some(cuenta);
        }
        ajustes
    }

    pub fn guardar(&self, ruta: &Path) -> Result<(), String> {
        let mut texto = String::new();
        if let Some(cuenta) = &self.metron {
            texto.push_str(&format!(
                "metron_usuario={}\nmetron_contrasena={}\n",
                cuenta.usuario, cuenta.contrasena
            ));
        }
        for canal in &self.canales {
            texto.push_str(&format!("rss={canal}\n"));
        }
        if let Some(carpeta) = ruta.parent() {
            fs::create_dir_all(carpeta).map_err(|e| e.to_string())?;
        }
        fs::write(ruta, texto).map_err(|e| e.to_string())
    }
}

/// Una ficha que se pide aparte, al abrirla: las de Metron no vienen en los listados.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Pedido {
    /// Un número de Metron.
    Numero(u64),
    /// Una serie de Metron.
    Serie(String),
    /// Un manga de AniList, con sus recomendadas.
    Manga(String),
}

impl Pedido {
    /// Un texto que lo identifica y no cambia, para guardarlo (en los favoritos).
    pub fn clave(&self) -> String {
        match self {
            Pedido::Numero(id) => format!("metron-numero:{id}"),
            Pedido::Serie(id) => format!("metron-serie:{id}"),
            Pedido::Manga(id) => format!("anilist:{id}"),
        }
    }

    pub fn desde_clave(clave: &str) -> Option<Pedido> {
        let (tipo, id) = clave.split_once(':')?;
        match tipo {
            "metron-numero" => id.parse().ok().map(Pedido::Numero),
            "metron-serie" => Some(Pedido::Serie(id.to_string())),
            "anilist" => Some(Pedido::Manga(id.to_string())),
            _ => None,
        }
    }
}

/// La página `pagina` (desde 1) de una lista de manga de AniList, y si hay más.
pub fn mas_manga(lista: ListaManga, pagina: u32) -> Result<(Vec<Ficha>, bool), String> {
    AniList::nuevo(Cliente::nuevo())
        .pagina(lista, pagina, Fecha::hoy())
        .map_err(|e| format!("AniList: {e}"))
}

/// Pide la ficha de un número o de una serie de Metron, o la de un manga de AniList.
pub fn ficha(pedido: &Pedido, ajustes: &Ajustes) -> Result<Ficha, String> {
    let metron = || {
        let cuenta = ajustes
            .metron
            .as_ref()
            .ok_or("Para ver la ficha hace falta la cuenta de Metron")?;
        Ok::<_, String>(Metron::nuevo(Cliente::nuevo(), cuenta))
    };
    match pedido {
        Pedido::Numero(id) => metron()?
            .ficha_de_numero(*id)
            .map_err(|e| format!("Metron: {e}")),
        Pedido::Serie(id) => metron()?
            .ficha_de_serie(id)
            .map_err(|e| format!("Metron: {e}")),
        Pedido::Manga(id) => {
            let id = id
                .parse()
                .map_err(|_| format!("AniList: identificador no válido ({id})"))?;
            AniList::nuevo(Cliente::nuevo())
                .ficha(id)
                .map_err(|e| format!("AniList: {e}"))
        }
    }
}

/// Las partes de las novedades. Cada una tiene su pestaña y se comprueba al entrar en ella.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Parte {
    /// Las series de la biblioteca.
    Series,
    /// El manga en tendencia y el recién empezado.
    Manga,
    /// El cómic americano que sale esta semana.
    Semana,
    Noticias,
}

/// Lo que va mandando una comprobación.
#[derive(Debug)]
pub enum Aviso {
    /// Lo que se lleva hecho, para ir enseñándolo (las noticias, antes de sus fotos).
    Parcial(Informe),
    /// Lo que ha salido al final.
    Fin(Informe),
}

/// Un candado envenenado (un hilo que se cayó con él puesto) no debe tumbar los demás.
fn bloquear<T>(candado: &Mutex<T>) -> MutexGuard<'_, T> {
    candado.lock().unwrap_or_else(|e| e.into_inner())
}

/// Comprueba `parte` de las novedades de la biblioteca de `ruta_db`. Parte de `anterior`:
/// lo que no se pueda consultar ahora (sin internet, por ejemplo) se queda como estaba.
/// Lo que manda es `anterior` con `parte` al día; lo demás no se toca.
pub fn comprobar(
    ruta_db: &Path,
    ajustes: &Ajustes,
    anterior: Informe,
    parte: Parte,
    mut avisar: impl FnMut(Aviso),
) {
    let cliente = Cliente::nuevo();
    let mut informe = anterior;
    let mut fallos = Vec::new();
    match parte {
        Parte::Series => {
            let anteriores = std::mem::take(&mut informe.series);
            informe.series = match Biblioteca::abrir(ruta_db) {
                Ok(biblioteca) => comprobar_series(
                    &biblioteca,
                    &cliente,
                    ajustes.metron.as_ref(),
                    anteriores,
                    &mut fallos,
                ),
                Err(e) => {
                    fallos.push(format!("No se pudo abrir la biblioteca: {e}"));
                    anteriores
                }
            };
        }
        Parte::Manga => match AniList::nuevo(cliente.clone()).manga(Fecha::hoy()) {
            Ok(mut manga) => {
                // A veces AniList devuelve un rato una lista vacía: mejor la de la última vez
                // que dejar la pestaña en blanco.
                if let Some(anterior) = &informe.manga {
                    if manga.tendencia.fichas.is_empty() {
                        manga.tendencia = anterior.tendencia.clone();
                    }
                    if manga.nuevas.fichas.is_empty() {
                        manga.nuevas = anterior.nuevas.clone();
                    }
                    manga.para_ti = anterior.para_ti.clone();
                }
                informe.manga = Some(manga);
                // Las listas generales ya; "Para ti" tarda un poco más (dos peticiones).
                avisar(Aviso::Parcial(informe.clone()));
                match para_ti(ruta_db, &cliente) {
                    Ok(fichas) => {
                        if let Some(manga) = &mut informe.manga {
                            manga.para_ti = ListaDeManga {
                                fichas,
                                paginas: 1,
                                hay_mas: false,
                            };
                        }
                    }
                    Err(e) => fallos.push(e),
                }
            }
            Err(e) => fallos.push(format!("AniList: {e}")),
        },
        Parte::Semana => {
            if let Some(cuenta) = &ajustes.metron {
                let desde = Fecha::hoy().lunes();
                let hasta = desde.mas_dias(6);
                match Metron::nuevo(cliente, cuenta).salen_entre(desde, hasta) {
                    Ok(mut numeros) => {
                        numeros.sort_by(|a, b| {
                            natord::compare_ignore_case(&a.serie, &b.serie)
                                .then_with(|| natord::compare(&a.numero, &b.numero))
                        });
                        informe.semana = Some(Semana {
                            desde,
                            hasta,
                            numeros,
                        });
                    }
                    Err(e) => fallos.push(format!("Metron: {e}")),
                }
            }
        }
        Parte::Noticias => {
            let anteriores = std::mem::take(&mut informe.noticias);
            match leer_noticias(&cliente, &ajustes.canales, &anteriores, &mut fallos) {
                Some(noticias) => {
                    informe.noticias = noticias;
                    // Las que no traen foto en el RSS: la de la página del artículo. Antes
                    // se mandan sin ella, para enseñarlas ya.
                    if informe
                        .noticias
                        .iter()
                        .any(|n| n.imagen.is_none() && !n.foto_buscada)
                    {
                        avisar(Aviso::Parcial(informe.clone()));
                        buscar_fotos(&cliente, &mut informe.noticias);
                    }
                }
                None => informe.noticias = anteriores,
            }
        }
    }
    informe.fallos.insert(parte, fallos);
    informe.comprobadas.insert(parte, ahora());
    informe.version = VERSION_INFORME;
    avisar(Aviso::Fin(informe));
}

/// "Para ti": lo que recomiendan los lectores de AniList a quien lee los manga de la
/// biblioteca que están emparejados con AniList.
fn para_ti(ruta_db: &Path, cliente: &Cliente) -> Result<Vec<Ficha>, String> {
    let biblioteca =
        Biblioteca::abrir(ruta_db).map_err(|e| format!("No se pudo abrir la biblioteca: {e}"))?;
    let vinculos = biblioteca
        .vinculos()
        .map_err(|e| format!("No se pudo leer la biblioteca: {e}"))?;
    let ids: Vec<u64> = vinculos
        .values()
        .filter_map(|v| v.externa.as_ref())
        .filter(|e| e.fuente == Fuente::AniList.clave())
        .filter_map(|e| e.id.parse().ok())
        .collect();
    AniList::nuevo(cliente.clone())
        .para_ti(&ids)
        .map_err(|e| format!("AniList (recomendaciones): {e}"))
}

/// Busca la foto de cada noticia que no la tenga en la cabecera de su artículo, con
/// varios hilos (cada web es un servidor distinto y solo se lee el principio de la página).
fn buscar_fotos(cliente: &Cliente, noticias: &mut [Noticia]) {
    let pendientes: Vec<usize> = noticias
        .iter()
        .enumerate()
        .filter(|(_, n)| n.imagen.is_none() && !n.foto_buscada)
        .map(|(i, _)| i)
        .take(MAX_FOTOS)
        .collect();
    let siguiente = AtomicUsize::new(0);
    let encontradas: Mutex<Vec<(usize, Option<String>)>> = Mutex::new(Vec::new());
    let enlaces: Vec<&str> = noticias.iter().map(|n| n.enlace.as_str()).collect();
    thread::scope(|hilos| {
        for _ in 0..HILOS_FOTOS.min(pendientes.len()) {
            hilos.spawn(|| {
                while let Some(&i) = pendientes.get(siguiente.fetch_add(1, Ordering::Relaxed)) {
                    match cliente.principio(enlaces[i], BYTES_CABECERA) {
                        Ok(html) => {
                            let foto = rss::imagen_de_pagina(&html, enlaces[i]);
                            bloquear(&encontradas).push((i, foto));
                        }
                        // Sin conexión o un error del servidor: se vuelve a probar otro día.
                        Err(e) => eprintln!("[novedades] Foto de {}: {e}", enlaces[i]),
                    }
                }
            });
        }
    });
    for (i, foto) in encontradas.into_inner().unwrap_or_else(|e| e.into_inner()) {
        noticias[i].imagen = foto;
        noticias[i].foto_buscada = true;
    }
}

fn comprobar_series(
    biblioteca: &Biblioteca,
    cliente: &Cliente,
    cuenta: Option<&Cuenta>,
    anteriores: Vec<EstadoSerie>,
    avisos: &mut Vec<String>,
) -> Vec<EstadoSerie> {
    let (colecciones, mut vinculos) = match (biblioteca.colecciones(), biblioteca.vinculos()) {
        (Ok(c), Ok(v)) => (c, v),
        (Err(e), _) | (_, Err(e)) => {
            avisos.push(format!("No se pudo leer la biblioteca: {e}"));
            return anteriores;
        }
    };
    let anilist = AniList::nuevo(cliente.clone());
    let metron = cuenta.map(|cuenta| Metron::nuevo(cliente.clone(), cuenta));

    // 1. Buscar las series que aún no se sabe a cuál corresponden.
    let pendientes: Vec<&Coleccion> = colecciones
        .iter()
        .filter(|c| match vinculos.get(&c.clave) {
            None => true,
            Some(v) => {
                v.estado == EstadoVinculo::SinCoincidencia && v.buscada < ahora() - REBUSCAR_TRAS
            }
        })
        .collect();
    for coleccion in &pendientes {
        let manga = anilist.buscar(&coleccion.nombre);
        let comic = metron.as_ref().map(|m| m.buscar(&coleccion.nombre));
        // Sin conexión no tiene sentido seguir, y no se apunta como "no encontrada".
        let error = manga
            .as_ref()
            .err()
            .or(comic.as_ref().and_then(|c| c.as_ref().err()));
        if let Some(error) = error {
            avisos.push(format!("No se pudieron identificar las series: {error}"));
            break;
        }
        let manga: Vec<Candidato> = manga
            .unwrap_or_default()
            .into_iter()
            .map(|o| o.candidato)
            .collect();
        let comic = comic.and_then(Result::ok).unwrap_or_default();

        let elegido = emparejar::elegir(coleccion, &manga, &comic);
        let vinculo = Vinculo {
            clave: coleccion.clave.clone(),
            externa: elegido.map(|c| SerieExterna {
                fuente: c.fuente.clave().to_string(),
                id: c.id.clone(),
                nombre: c.titulo.clone(),
            }),
            estado: if elegido.is_some() {
                EstadoVinculo::Automatico
            } else {
                EstadoVinculo::SinCoincidencia
            },
            buscada: ahora(),
        };
        match &elegido {
            Some(c) => eprintln!(
                "[novedades] {} es {} en {}",
                coleccion.nombre,
                c.titulo,
                c.fuente.nombre()
            ),
            None => eprintln!("[novedades] {}: nada seguro", coleccion.nombre),
        }
        if let Err(e) = biblioteca.guardar_vinculo(&vinculo) {
            avisos.push(format!("No se pudo guardar: {e}"));
        }
        vinculos.insert(vinculo.clave.clone(), vinculo);
    }

    // 2. Mirar lo publicado de cada serie identificada.
    let anteriores: HashMap<String, EstadoSerie> = anteriores
        .into_iter()
        .map(|s| (s.clave.clone(), s))
        .collect();
    let mut estados = Vec::new();
    let mut de_anilist: Vec<(&Coleccion, u64)> = Vec::new();
    let mut de_metron: Vec<(&Coleccion, &SerieExterna)> = Vec::new();
    for coleccion in &colecciones {
        let Some(externa) = vinculos
            .get(&coleccion.clave)
            .and_then(|v| v.externa.as_ref())
        else {
            continue;
        };
        match Fuente::desde_clave(&externa.fuente) {
            Some(Fuente::AniList) => {
                if let Ok(id) = externa.id.parse() {
                    de_anilist.push((coleccion, id));
                }
            }
            Some(Fuente::Metron) => de_metron.push((coleccion, externa)),
            None => {}
        }
    }

    if !de_anilist.is_empty() {
        let ids: Vec<u64> = de_anilist.iter().map(|(_, id)| *id).collect();
        match anilist.obras(&ids) {
            Ok(obras) => {
                let mut por_id: HashMap<String, anilist::Obra> = obras
                    .into_iter()
                    .map(|o| (o.candidato.id.clone(), o))
                    .collect();
                for (coleccion, id) in &de_anilist {
                    if let Some(obra) = por_id.remove(&id.to_string()) {
                        estados.push(emparejar::con_anilist(coleccion, obra));
                    }
                }
            }
            Err(e) => {
                avisos.push(format!("AniList: {e}"));
                estados.extend(
                    de_anilist
                        .iter()
                        .filter_map(|(c, _)| anteriores.get(&c.clave).cloned()),
                );
            }
        }
    }

    if !de_metron.is_empty() {
        match &metron {
            Some(metron) => {
                let hoy = Fecha::hoy();
                for (i, (coleccion, externa)) in de_metron.iter().enumerate() {
                    match metron.numeros(&externa.id) {
                        Ok(numeros) => {
                            let candidato = Candidato {
                                fuente: Fuente::Metron,
                                id: externa.id.clone(),
                                titulo: externa.nombre.clone(),
                                otros_titulos: Vec::new(),
                                anio: None,
                                detalle: String::new(),
                                portada: None,
                                url: None,
                                formato: None,
                            };
                            estados
                                .push(emparejar::con_metron(coleccion, &candidato, numeros, hoy));
                        }
                        Err(e) => {
                            avisos.push(format!("Metron ({}): {e}", coleccion.nombre));
                            estados.extend(anteriores.get(&coleccion.clave).cloned());
                            if matches!(
                                e,
                                ErrorRed::Autenticacion | ErrorRed::Conexion(_) | ErrorRed::Limite
                            ) {
                                // Las demás fallarían igual.
                                estados.extend(
                                    de_metron[i + 1..]
                                        .iter()
                                        .filter_map(|(c, _)| anteriores.get(&c.clave).cloned()),
                                );
                                break;
                            }
                        }
                    }
                }
            }
            None => {
                avisos.push(format!(
                    "Hay {} series de Metron sin comprobar: añade tu cuenta de Metron en Ajustes",
                    de_metron.len()
                ));
                estados.extend(
                    de_metron
                        .iter()
                        .filter_map(|(c, _)| anteriores.get(&c.clave).cloned()),
                );
            }
        }
    }

    estados.sort_by(|a, b| {
        b.hay_novedades()
            .cmp(&a.hay_novedades())
            .then_with(|| natord::compare_ignore_case(&a.nombre, &b.nombre))
    });
    estados
}

/// Lee los RSS, todos a la vez. Devuelve `None` si no se pudo leer ninguno (para no
/// borrar las noticias que ya había). De las que ya estaban se conserva la foto.
fn leer_noticias(
    cliente: &Cliente,
    canales: &[String],
    anteriores: &[Noticia],
    avisos: &mut Vec<String>,
) -> Option<Vec<Noticia>> {
    let leidos: Vec<(&String, Result<rss::Canal, String>)> = thread::scope(|hilos| {
        let hilos: Vec<_> = canales
            .iter()
            .map(|url| {
                hilos.spawn(move || {
                    let canal = cliente
                        .texto(url, &[], None)
                        .map_err(|e| e.to_string())
                        .and_then(|xml| {
                            rss::leer(&xml).map_err(|e| format!("no es un RSS válido ({e})"))
                        });
                    (url, canal)
                })
            })
            .collect();
        hilos
            .into_iter()
            .filter_map(|hilo| hilo.join().ok())
            .collect()
    });

    let mut noticias = Vec::new();
    let mut alguno = false;
    for (url, canal) in leidos {
        match canal {
            Ok(canal) => {
                alguno = true;
                let fuente = canal.titulo.unwrap_or_else(|| servidor(url).to_string());
                noticias.extend(canal.noticias.into_iter().take(NOTICIAS_POR_CANAL).map(
                    |mut n| {
                        n.fuente = fuente.clone();
                        n
                    },
                ));
            }
            Err(e) => avisos.push(format!("{url}: {e}")),
        }
    }
    if !alguno {
        return None;
    }
    let ya_vistas: HashMap<&str, &Noticia> =
        anteriores.iter().map(|n| (n.enlace.as_str(), n)).collect();
    for noticia in &mut noticias {
        if let Some(anterior) = ya_vistas.get(noticia.enlace.as_str()) {
            if noticia.imagen.is_none() {
                noticia.imagen = anterior.imagen.clone();
                noticia.foto_buscada = anterior.foto_buscada;
            }
        }
    }
    // Las que no tienen fecha, al final.
    noticias.sort_by(|a, b| {
        b.fecha
            .unwrap_or(i64::MIN)
            .cmp(&a.fecha.unwrap_or(i64::MIN))
    });
    noticias.truncate(MAX_NOTICIAS);
    Some(noticias)
}

/// `https://www.ejemplo.com/feed/` da `www.ejemplo.com`.
pub fn servidor(url: &str) -> &str {
    let sin_esquema = url.split_once("://").map_or(url, |(_, resto)| resto);
    sin_esquema
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(sin_esquema)
}

/// Busca `texto` en AniList y, si hay cuenta, en Metron: para que el usuario elija a
/// mano qué serie es la suya. Lo que falle se cuenta en los avisos.
pub fn buscar(texto: &str, ajustes: &Ajustes) -> (Vec<Candidato>, Vec<String>) {
    let cliente = Cliente::nuevo();
    let mut candidatos = Vec::new();
    let mut avisos = Vec::new();
    match AniList::nuevo(cliente.clone()).buscar(texto) {
        Ok(obras) => candidatos.extend(obras.into_iter().map(|o| o.candidato)),
        Err(e) => avisos.push(format!("AniList: {e}")),
    }
    match &ajustes.metron {
        Some(cuenta) => match Metron::nuevo(cliente, cuenta).buscar(texto) {
            Ok(series) => candidatos.extend(series),
            Err(e) => avisos.push(format!("Metron: {e}")),
        },
        None => avisos.push(
            "Para buscar también cómic americano, añade tu cuenta de Metron en Ajustes".into(),
        ),
    }
    (candidatos, avisos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_servidor_de_una_url() {
        assert_eq!(
            servidor("https://www.ramenparados.com/feed/"),
            "www.ramenparados.com"
        );
        assert_eq!(servidor("http://ejemplo.com?x=1"), "ejemplo.com");
        assert_eq!(servidor("ejemplo.com/rss"), "ejemplo.com");
    }

    #[test]
    fn los_ajustes_se_guardan_y_se_leen() {
        let carpeta = tempfile::tempdir().unwrap();
        let ruta = carpeta.path().join("novedades.txt");
        assert_eq!(Ajustes::cargar(&ruta), Ajustes::default());
        let ajustes = Ajustes {
            metron: Some(Cuenta {
                usuario: "ana".into(),
                contrasena: "a=b c".into(),
            }),
            canales: vec!["https://a.com/feed".into(), "https://b.com/rss".into()],
        };
        ajustes.guardar(&ruta).unwrap();
        assert_eq!(Ajustes::cargar(&ruta), ajustes);
    }

    #[test]
    fn las_paginas_de_manga_no_repiten() {
        let ficha = |id: &str| Ficha {
            fuente: Fuente::AniList,
            id: id.into(),
            titulo: id.into(),
            subtitulo: None,
            linea: String::new(),
            portada: None,
            portada_grande: None,
            url: None,
            datos: Vec::new(),
            generos: Vec::new(),
            sinopsis: None,
            recomendaciones: Vec::new(),
        };
        let mut lista = ListaDeManga::default();
        lista.anadir(vec![ficha("1"), ficha("2")], true);
        // La tendencia ha cambiado entre una página y otra: "2" vuelve a salir.
        lista.anadir(vec![ficha("2"), ficha("3")], false);
        let ids: Vec<&str> = lista.fichas.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3"]);
        assert_eq!(lista.paginas, 2);
        assert!(!lista.hay_mas);
    }

    #[test]
    fn un_informe_con_el_manga_de_otra_forma_no_se_pierde() {
        // El de una versión anterior, con las listas de manga como listas sin más.
        let json = r#"{"comprobado": 5, "series": [], "semana": null,
            "manga": {"tendencia": [], "nuevas": []}, "noticias": [], "avisos": []}"#;
        let informe: Informe = serde_json::from_str(json).unwrap();
        assert_eq!(informe.manga, None);
    }

    #[test]
    fn el_informe_se_guarda_y_se_lee() {
        let carpeta = tempfile::tempdir().unwrap();
        let ruta = carpeta.path().join("novedades.json");
        assert_eq!(Informe::cargar(&ruta), Informe::default());
        let informe = Informe {
            comprobadas: HashMap::from([(Parte::Manga, 12)]),
            fallos: HashMap::from([(Parte::Semana, vec!["Metron: sin conexión".to_string()])]),
            noticias: vec![Noticia {
                titulo: "Hola".into(),
                enlace: "https://a.com/1".into(),
                fuente: "A".into(),
                fecha: Some(5),
                resumen: None,
                imagen: Some("https://a.com/1.jpg".into()),
                foto_buscada: false,
            }],
            ..Informe::default()
        };
        informe.guardar(&ruta).unwrap();
        assert_eq!(Informe::cargar(&ruta), informe);
    }
}
