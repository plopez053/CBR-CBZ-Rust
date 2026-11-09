//! Página de novedades: tomos y números nuevos de las series de la biblioteca, el manga
//! que más se lee, el cómic americano que sale esta semana y noticias. Cada portada abre
//! la ficha de su obra. Cada pestaña se pone al día sola al entrar en ella, en un hilo
//! aparte y sin avisos de "cargando": lo nuevo aparece cuando llega. El último resultado
//! se guarda en disco, así que al abrir la app ya se ve lo de la última vez.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use comic_biblioteca::{
    clave_de_serie, Biblioteca, EstadoVinculo, Favorito, Serie, SerieExterna, Vinculo,
};
use comic_novedades::{
    ahora, sin_anio, Ajustes, Aviso, Candidato, Cuenta, EstadoSerie, Fecha, Ficha, Fuente, Informe,
    ListaManga, Noticia, Numero, Parte, Pedido, Publicacion, Recomendada, VERSION_INFORME,
};
use eframe::egui::{self, FontId, Rect, Sense, Stroke};

use crate::estilo::{self, Colores};
use crate::navegador::abrir_web;
use crate::portadas_remotas::PortadasRemotas;
use crate::vista_biblioteca::{
    area_de_desplazamiento, boton_subrayado, boton_texto, etiqueta_sobre_portada, panel, plural,
    texto_ajustado, MARGEN,
};

/// Una pestaña se pone al día al entrar en ella, salvo que se haya hecho hace menos de
/// esto (para no repetir las mismas peticiones al ir y venir entre pestañas).
const REFRESCO: i64 = 10 * 60;
/// La página siguiente de una lista se pide cuando su final queda a menos de esto por
/// debajo de lo que se ve, para que esté antes de llegar.
const ANTES_DEL_FINAL: f32 = 900.0;
/// Si falla una página, no se vuelve a pedir hasta pasado esto.
const PAUSA_TRAS_FALLO: Duration = Duration::from_secs(30);
const ANCHO_MAXIMO: f32 = 900.0;
const PORTADA_FILA: egui::Vec2 = egui::vec2(64.0, 96.0);
const PORTADA_SEMANA: egui::Vec2 = egui::vec2(120.0, 180.0);
const ALTO_TARJETA_SEMANA: f32 = PORTADA_SEMANA.y + 50.0;
const PORTADA_CANDIDATO: egui::Vec2 = egui::vec2(46.0, 69.0);
const PORTADA_FICHA: egui::Vec2 = egui::vec2(200.0, 300.0);
const ANCHO_FICHA: f32 = 660.0;
/// Las fotos de las noticias, en 16:9.
const FOTO_NOTICIA: egui::Vec2 = egui::vec2(176.0, 99.0);
/// Las portadas de las recomendadas, en la ficha.
const PORTADA_RECOMENDADA: egui::Vec2 = egui::vec2(84.0, 126.0);
/// Números nuevos que se enseñan de cada serie; del resto solo se dice cuántos hay.
const MAX_NUMEROS_FILA: usize = 8;

/// RSS que se ofrecen para empezar (comprobados a mano: responden y son RSS válidos).
const CANALES_SUGERIDOS: [(&str, &str); 3] = [
    (
        "Ramen Para Dos (manga y anime)",
        "https://www.ramenparados.com/feed/",
    ),
    (
        "Zona Negativa (cómic)",
        "https://www.zonanegativa.com/feed/",
    ),
    (
        "Anime News Network (en inglés)",
        "https://www.animenewsnetwork.com/news/rss.xml?ann-edition=w",
    ),
];

#[derive(Clone, Copy, PartialEq)]
enum Pestana {
    Series,
    Manga,
    Semana,
    Noticias,
    Favoritos,
    #[cfg(fuentes)]
    Fuentes,
}

impl Pestana {
    /// Lo que hay que poner al día al entrar. Los favoritos están guardados: nada.
    fn parte(self) -> Option<Parte> {
        match self {
            Pestana::Series => Some(Parte::Series),
            Pestana::Manga => Some(Parte::Manga),
            Pestana::Semana => Some(Parte::Semana),
            Pestana::Noticias => Some(Parte::Noticias),
            Pestana::Favoritos => None,
            #[cfg(fuentes)]
            Pestana::Fuentes => None,
        }
    }
}

/// Lo que se ha pulsado en la página; se atiende al terminar de dibujarla.
#[derive(Clone)]
enum Accion {
    AbrirSerie(String),
    AbrirWeb(String),
    Buscar {
        clave: String,
        nombre: String,
    },
    Ignorar(String),
    VolverABuscar(String),
    Ajustes,
    /// Abrir una ficha que ya se tiene.
    VerFicha(Box<Ficha>),
    /// Pedir la página siguiente de una lista de manga.
    MasManga(ListaManga),
    /// Guardar en favoritos, o quitar si ya estaba.
    Favorito(Favorito),
    /// Abrir una ficha que hay que pedir a Metron.
    PedirFicha {
        pedido: Pedido,
        titulo: String,
        portada: Option<String>,
    },
}

/// Una página de una lista de manga y si hay más, o lo que falló.
type PaginaDeManga = Result<(Vec<Ficha>, bool), String>;

/// La ventana de la ficha de una obra o de un número. Se abre con lo que ya se sabe y se
/// completa cuando llega lo que se ha pedido.
struct FichaAbierta {
    titulo: String,
    /// La portada que ya se conoce, para enseñarla mientras llega la ficha (y para las
    /// fichas de serie de Metron, que no traen).
    portada: Option<String>,
    /// Lo que se enseña; mientras no hay nada, la portada y el título.
    ficha: Option<Box<Ficha>>,
    /// Lo que se ha pedido: la ficha de Metron, o la de AniList con sus recomendadas.
    pedido: Option<(Pedido, Receiver<Result<Ficha, String>>)>,
    /// Qué obra o número es en el catálogo, para guardarla en favoritos. `None` en los
    /// números guardados por una versión anterior, que no lo sabían.
    clave: Option<Pedido>,
    error: Option<String>,
    /// Cuándo se pidió: la ficha no se enseña hasta tenerlo todo, pero tampoco para siempre.
    desde: Instant,
}

/// Lo máximo que se espera a que llegue todo antes de enseñar la ficha con lo que haya.
const ESPERA_FICHA: Duration = Duration::from_secs(8);

impl FichaAbierta {
    /// Si ya se puede enseñar: ha llegado la ficha pedida (o ha fallado) y están las
    /// portadas que lleva, para que la ventana salga entera de una vez, sin crecer ni
    /// rellenarse sobre la marcha.
    fn lista(&self, ctx: &egui::Context, portadas: &mut PortadasRemotas) -> bool {
        if self.error.is_some() || self.desde.elapsed() >= ESPERA_FICHA {
            return true;
        }
        if self.pedido.is_some() {
            // Aún falta la ficha: nada que enseñar, ni portadas que pedir todavía.
            return false;
        }
        let Some(ficha) = &self.ficha else {
            return true;
        };
        let mut lista = portadas.preparar(
            ctx,
            PORTADA_FICHA,
            ficha.portada_grande.as_deref().or(ficha.portada.as_deref()),
        );
        for recomendada in ficha.recomendaciones.iter().take(recomendadas_que_caben()) {
            lista &= portadas.preparar(ctx, PORTADA_RECOMENDADA, recomendada.portada.as_deref());
        }
        lista
    }
}

/// Cuántas portadas recomendadas caben en la fila de la ficha.
fn recomendadas_que_caben() -> usize {
    let separacion = 12.0;
    ((ANCHO_FICHA + separacion) / (PORTADA_RECOMENDADA.x + separacion))
        .floor()
        .max(1.0) as usize
}

/// La ventana de ajustes mientras se edita.
struct Borrador {
    usuario: String,
    contrasena: String,
    canales: Vec<String>,
    nuevo_canal: String,
}

/// La ventana para elegir a mano qué serie del catálogo es una de la biblioteca.
struct Busqueda {
    clave: String,
    nombre: String,
    texto: String,
    respuesta: Option<Receiver<(Vec<Candidato>, Vec<String>)>>,
    candidatos: Vec<Candidato>,
    avisos: Vec<String>,
    buscada: bool,
}

pub struct VistaNovedades {
    ruta_db: PathBuf,
    ruta_ajustes: PathBuf,
    ruta_informe: PathBuf,
    ajustes: Ajustes,
    informe: Informe,
    vinculos: HashMap<String, Vinculo>,
    /// Las partes que se están comprobando, cada una en su hilo.
    comprobaciones: HashMap<Parte, Receiver<Aviso>>,
    /// Las que hay que volver a comprobar en cuanto termine la que está en marcha (se ha
    /// cambiado algo mientras tanto).
    repetir: HashSet<Parte>,
    pestana: Pestana,
    lista_manga: ListaManga,
    /// La página de manga que se está pidiendo.
    mas_manga: Option<(ListaManga, Receiver<PaginaDeManga>)>,
    /// Tras un fallo, cuándo se puede volver a pedir otra página.
    pausa_manga: Option<Instant>,
    filtro_semana: String,
    portadas: PortadasRemotas,
    borrador: Option<Borrador>,
    busqueda: Option<Busqueda>,
    ficha: Option<FichaAbierta>,
    /// Se ha cambiado de pestaña: la página empieza arriba del todo.
    arriba: bool,
    /// Las fichas pedidas a Metron en esta sesión, para no volver a pedirlas.
    fichas: HashMap<Pedido, Ficha>,
    /// Del más nuevo al más antiguo.
    favoritos: Vec<Favorito>,
    #[cfg(fuentes)]
    fuentes: crate::fuentes::PestanaFuentes,
    error: Option<String>,
}

impl VistaNovedades {
    pub fn nueva(
        ctx: &egui::Context,
        biblioteca: &Biblioteca,
        ruta_db: PathBuf,
        ruta_ajustes: PathBuf,
        carpeta_cache: PathBuf,
    ) -> VistaNovedades {
        let ruta_informe = carpeta_cache.join("novedades.json");
        let mut informe = Informe::cargar(&ruta_informe);
        if informe.version < VERSION_INFORME {
            // De una versión anterior que sabía menos cosas: todo se pone al día al entrar.
            informe.comprobadas.clear();
            informe.version = VERSION_INFORME;
        }
        VistaNovedades {
            ajustes: Ajustes::cargar(&ruta_ajustes),
            informe,
            vinculos: biblioteca.vinculos().unwrap_or_default(),
            portadas: PortadasRemotas::nuevo(carpeta_cache.join("portadas"), ctx.clone()),
            ruta_db,
            ruta_ajustes,
            ruta_informe,
            comprobaciones: HashMap::new(),
            repetir: HashSet::new(),
            pestana: Pestana::Series,
            lista_manga: ListaManga::ParaTi,
            mas_manga: None,
            pausa_manga: None,
            filtro_semana: String::new(),
            borrador: None,
            busqueda: None,
            ficha: None,
            arriba: false,
            fichas: HashMap::new(),
            favoritos: biblioteca.favoritos().unwrap_or_else(|e| {
                eprintln!("[novedades] No se pudieron leer los favoritos: {e}");
                Vec::new()
            }),
            #[cfg(fuentes)]
            fuentes: crate::fuentes::PestanaFuentes::nueva(),
            error: None,
        }
    }

    /// Cuántas series tienen tomos o números nuevos, para la barra lateral.
    pub fn con_novedades(&self, series: &[Serie]) -> usize {
        let claves: HashSet<&str> = series.iter().map(|s| s.clave.as_str()).collect();
        self.informe
            .series
            .iter()
            .filter(|s| s.hay_novedades() && claves.contains(s.clave.as_str()))
            .count()
    }

    /// Al entrar en la página: se pone al día la pestaña que se ve.
    pub fn al_entrar(&mut self, ctx: &egui::Context, biblioteca: &Biblioteca) {
        self.vinculos = biblioteca.vinculos().unwrap_or_default();
        self.poner_al_dia_pestana(ctx);
    }

    /// Para el modo de captura: elige una pestaña ("manga", "semana", "noticias"; si no,
    /// la de tus series) o abre la ficha del primer manga en tendencia ("ficha").
    pub(crate) fn para_captura(&mut self, ctx: &egui::Context, que: &str) {
        self.ficha = None;
        self.pestana = match que {
            "manga" | "ficha" => Pestana::Manga,
            "semana" => Pestana::Semana,
            "noticias" => Pestana::Noticias,
            "favoritos" => Pestana::Favoritos,
            _ => Pestana::Series,
        };
        self.poner_al_dia_pestana(ctx);
        if que == "ficha" {
            let primera = self
                .informe
                .manga
                .as_ref()
                .and_then(|m| m.tendencia.fichas.first())
                .cloned();
            if let Some(ficha) = primera {
                let pedido = Some(Pedido::Manga(ficha.id.clone()));
                let (titulo, portada) = (ficha.titulo.clone(), ficha.portada.clone());
                self.abrir_ficha(ctx, Some(ficha), pedido, titulo, portada);
            }
        }
    }

    /// Pone al día la pestaña que se está viendo, si le hace falta.
    fn poner_al_dia_pestana(&mut self, ctx: &egui::Context) {
        if let Some(parte) = self.pestana.parte() {
            self.poner_al_dia(ctx, parte);
        }
    }

    /// Las claves de los favoritos, para saber si algo ya lo es.
    fn claves_favoritas(&self) -> HashSet<String> {
        self.favoritos.iter().map(|f| f.clave.clone()).collect()
    }

    /// Guarda `favorito`, o lo quita si ya estaba.
    fn alternar_favorito(&mut self, biblioteca: &Biblioteca, favorito: Favorito) {
        let ya = self.favoritos.iter().any(|f| f.clave == favorito.clave);
        let resultado = if ya {
            biblioteca.quitar_favorito(&favorito.clave)
        } else {
            biblioteca.anadir_favorito(&favorito)
        };
        match resultado.and_then(|()| biblioteca.favoritos()) {
            Ok(favoritos) => self.favoritos = favoritos,
            Err(e) => self.error = Some(format!("No se pudieron guardar los favoritos: {e}")),
        }
    }

    /// Comprueba `parte` si no se ha hecho hace poco.
    fn poner_al_dia(&mut self, ctx: &egui::Context, parte: Parte) {
        let reciente = self
            .informe
            .comprobada(parte)
            .is_some_and(|cuando| ahora() - cuando < REFRESCO);
        if !reciente {
            self.comprobar(ctx, parte);
        }
    }

    /// Comprueba `parte` en un hilo aparte. Si ya se está comprobando, se repite al
    /// terminar (puede que haya cambiado algo, como una serie elegida a mano).
    fn comprobar(&mut self, ctx: &egui::Context, parte: Parte) {
        let sin_nada_que_mirar = match parte {
            Parte::Semana => self.ajustes.metron.is_none(),
            Parte::Noticias => self.ajustes.canales.is_empty(),
            Parte::Series | Parte::Manga => false,
        };
        if sin_nada_que_mirar {
            return;
        }
        if self.comprobaciones.contains_key(&parte) {
            self.repetir.insert(parte);
            return;
        }

        let (enviar, avisos) = mpsc::channel();
        let ruta_db = self.ruta_db.clone();
        let ajustes = self.ajustes.clone();
        let anterior = self.informe.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            comic_novedades::comprobar(&ruta_db, &ajustes, anterior, parte, |aviso| {
                let _ = enviar.send(aviso);
                ctx.request_repaint();
            });
        });
        self.comprobaciones.insert(parte, avisos);
    }

    /// Recoge lo que llegue de las comprobaciones en marcha. Se llama en cada fotograma,
    /// aunque no se esté viendo la página, para que el contador de la barra lateral se
    /// actualice.
    pub fn seguir(&mut self, ctx: &egui::Context, biblioteca: &Biblioteca) {
        self.seguir_mas_manga();
        let mut terminadas = Vec::new();
        for (&parte, avisos) in &self.comprobaciones {
            loop {
                match avisos.try_recv() {
                    Ok(Aviso::Parcial(informe)) => self.informe.copiar(&informe, parte),
                    Ok(Aviso::Fin(informe)) => {
                        self.informe.copiar(&informe, parte);
                        terminadas.push(parte);
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    // Si el hilo se cae sin avisar, no se queda en marcha para siempre.
                    Err(mpsc::TryRecvError::Disconnected) => {
                        terminadas.push(parte);
                        break;
                    }
                }
            }
        }

        for parte in terminadas {
            self.comprobaciones.remove(&parte);
            for fallo in self.informe.fallos_de(parte) {
                eprintln!("[novedades] {parte:?}: {fallo}");
            }
            if let Err(e) = self.informe.guardar(&self.ruta_informe) {
                eprintln!("[novedades] No se pudo guardar el informe: {e}");
            }
            if parte == Parte::Series {
                self.vinculos = biblioteca.vinculos().unwrap_or_default();
            }
            if self.repetir.remove(&parte) {
                self.comprobar(ctx, parte);
            }
        }
    }

    /// Pide la página siguiente de `lista`, si no se está pidiendo ya otra.
    fn pedir_mas_manga(&mut self, ctx: &egui::Context, lista: ListaManga) {
        let ocupado = self.mas_manga.is_some()
            || self.comprobaciones.contains_key(&Parte::Manga)
            || self.pausa_manga.is_some_and(|hasta| Instant::now() < hasta);
        let Some(manga) = &self.informe.manga else {
            return;
        };
        if ocupado || !manga.lista(lista).hay_mas {
            return;
        }
        let pagina = manga.lista(lista).paginas + 1;
        let (enviar, respuesta) = mpsc::channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = enviar.send(comic_novedades::mas_manga(lista, pagina));
            ctx.request_repaint();
        });
        self.mas_manga = Some((lista, respuesta));
    }

    /// Añade a su lista la página de manga que haya llegado.
    fn seguir_mas_manga(&mut self) {
        let Some((lista, respuesta)) = &self.mas_manga else {
            return;
        };
        let lista = *lista;
        let resultado = match respuesta.try_recv() {
            Ok(resultado) => resultado,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("se cortó la petición".to_string()),
        };
        self.mas_manga = None;
        match resultado {
            Ok((fichas, hay_mas)) => {
                if let Some(manga) = &mut self.informe.manga {
                    manga.lista_mut(lista).anadir(fichas, hay_mas);
                }
                if let Err(e) = self.informe.guardar(&self.ruta_informe) {
                    eprintln!("[novedades] No se pudo guardar el informe: {e}");
                }
            }
            Err(e) => {
                eprintln!("[novedades] Más manga: {e}");
                self.pausa_manga = Some(Instant::now() + PAUSA_TRAS_FALLO);
            }
        }
    }

    /// Dibuja la página. Devuelve la serie de la biblioteca que hay que abrir, si se ha
    /// pulsado una.
    pub fn mostrar(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        biblioteca: &Biblioteca,
        series: &[Serie],
        arriba: bool,
    ) -> Option<String> {
        let ctx = ui.ctx().clone();
        let mut acciones = Vec::new();
        let pestana_antes = self.pestana;

        // También al cambiar de pestaña: el cambio se ve al final del fotograma, así que
        // se aplica en el siguiente.
        let arriba = arriba || std::mem::take(&mut self.arriba);
        area_de_desplazamiento(arriba).show(ui, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::same(MARGEN as i8))
                .show(ui, |ui| {
                    ui.set_max_width(ANCHO_MAXIMO);
                    self.cabecera(ui, c, series, &mut acciones);
                    ui.add_space(18.0);
                    match self.pestana {
                        Pestana::Series => self.pestana_series(ui, c, series, &mut acciones),
                        Pestana::Manga => self.pestana_manga(ui, c, series, &mut acciones),
                        Pestana::Semana => self.pestana_semana(ui, c, series, &mut acciones),
                        Pestana::Noticias => self.pestana_noticias(ui, c, &mut acciones),
                        Pestana::Favoritos => self.pestana_favoritos(ui, c, &mut acciones),
                        #[cfg(fuentes)]
                        Pestana::Fuentes => {
                            let url = self.fuentes.mostrar(ui, c, &self.favoritos);
                            acciones.extend(url.map(Accion::AbrirWeb));
                        }
                    }
                });
        });

        self.ventana_ajustes(&ctx, c);
        self.ventana_busqueda(&ctx, c, biblioteca, &mut acciones);
        self.ventana_ficha(&ctx, c, &mut acciones);
        self.ventana_error(&ctx);
        self.portadas.fin_de_fotograma(&ctx);
        if self.pestana != pestana_antes {
            self.poner_al_dia_pestana(&ctx);
            self.arriba = true;
            ctx.request_repaint();
        }

        let mut abrir = None;
        for accion in acciones {
            match accion {
                Accion::AbrirSerie(nombre) => abrir = Some(nombre),
                Accion::AbrirWeb(url) => abrir_web(&ctx, &url),
                Accion::Buscar { clave, nombre } => {
                    self.busqueda = Some(Busqueda {
                        clave,
                        texto: nombre.clone(),
                        nombre,
                        respuesta: None,
                        candidatos: Vec::new(),
                        avisos: Vec::new(),
                        buscada: false,
                    });
                }
                Accion::Ignorar(clave) => {
                    self.guardar_vinculo(
                        biblioteca,
                        Vinculo {
                            clave: clave.clone(),
                            externa: None,
                            estado: EstadoVinculo::Ignorado,
                            buscada: ahora(),
                        },
                    );
                    self.informe.series.retain(|s| s.clave != clave);
                    let _ = self.informe.guardar(&self.ruta_informe);
                }
                Accion::VolverABuscar(clave) => {
                    if let Err(e) = biblioteca.quitar_vinculo(&clave) {
                        self.error = Some(format!("No se pudo guardar: {e}"));
                    }
                    self.vinculos.remove(&clave);
                    self.comprobar(&ctx, Parte::Series);
                }
                Accion::Ajustes => self.abrir_ajustes(),
                Accion::MasManga(lista) => self.pedir_mas_manga(&ctx, lista),
                Accion::Favorito(favorito) => self.alternar_favorito(biblioteca, favorito),
                Accion::VerFicha(ficha) => {
                    // Las de AniList se piden enteras para traer sus recomendadas.
                    let pedido = (ficha.fuente == Fuente::AniList && !ficha.id.is_empty())
                        .then(|| Pedido::Manga(ficha.id.clone()));
                    let (titulo, portada) = (ficha.titulo.clone(), ficha.portada.clone());
                    self.abrir_ficha(&ctx, Some(*ficha), pedido, titulo, portada);
                }
                Accion::PedirFicha {
                    pedido,
                    titulo,
                    portada,
                } => self.abrir_ficha(&ctx, None, Some(pedido), titulo, portada),
            }
        }
        abrir
    }

    /// Abre una ficha con lo que ya se sabe (`ficha`, o solo título y portada) y pide
    /// `pedido` en un hilo aparte para completarla, salvo que ya se pidiera antes.
    fn abrir_ficha(
        &mut self,
        ctx: &egui::Context,
        ficha: Option<Ficha>,
        pedido: Option<Pedido>,
        titulo: String,
        portada: Option<String>,
    ) {
        let clave = pedido.clone();
        if let Some(guardada) = pedido.as_ref().and_then(|p| self.fichas.get(p)) {
            self.ficha = Some(FichaAbierta {
                titulo,
                portada,
                ficha: Some(Box::new(guardada.clone())),
                pedido: None,
                clave,
                error: None,
                desde: Instant::now(),
            });
            return;
        }
        let pedido = pedido.map(|pedido| {
            let (enviar, respuesta) = mpsc::channel();
            let ajustes = self.ajustes.clone();
            let ctx = ctx.clone();
            let pedida = pedido.clone();
            thread::spawn(move || {
                let _ = enviar.send(comic_novedades::ficha(&pedida, &ajustes));
                ctx.request_repaint();
            });
            (pedido, respuesta)
        });
        self.ficha = Some(FichaAbierta {
            titulo,
            portada,
            ficha: ficha.map(Box::new),
            pedido,
            clave,
            error: None,
            desde: Instant::now(),
        });
    }

    fn guardar_vinculo(&mut self, biblioteca: &Biblioteca, vinculo: Vinculo) {
        match biblioteca.guardar_vinculo(&vinculo) {
            Ok(()) => {
                self.vinculos.insert(vinculo.clave.clone(), vinculo);
            }
            Err(e) => self.error = Some(format!("No se pudo guardar: {e}")),
        }
    }

    fn abrir_ajustes(&mut self) {
        let cuenta = self.ajustes.metron.clone().unwrap_or_default();
        self.borrador = Some(Borrador {
            usuario: cuenta.usuario,
            contrasena: cuenta.contrasena,
            canales: self.ajustes.canales.clone(),
            nuevo_canal: String::new(),
        });
    }

    fn cabecera(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        series: &[Serie],
        acciones: &mut Vec<Accion>,
    ) {
        ui.horizontal(|ui| {
            ui.label(estilo::titulo("Novedades", 24.0, c.texto));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(boton_texto(c, "Ajustes…")).clicked() {
                    acciones.push(Accion::Ajustes);
                }
            });
        });

        ui.add_space(16.0);
        let novedades = self.con_novedades(series);
        let semana = self.informe.semana.as_ref().map(|s| s.numeros.len());
        #[allow(unused_mut)]
        let mut pestanas = vec![
            (
                Pestana::Series,
                "Tus series",
                Some(novedades).filter(|&n| n > 0),
            ),
            (Pestana::Manga, "Manga", None),
            (
                Pestana::Semana,
                "Cómic americano",
                semana.filter(|&n| n > 0),
            ),
            (Pestana::Noticias, "Noticias", None),
            (
                Pestana::Favoritos,
                "Favoritos",
                Some(self.favoritos.len()).filter(|&n| n > 0),
            ),
        ];
        #[cfg(fuentes)]
        pestanas.push((Pestana::Fuentes, "Fuentes", None));
        let (fila, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), Sense::hover());
        let mut x = fila.min.x;
        for (pestana, nombre, contador) in pestanas {
            let rect = pestana_de(
                ui,
                c,
                egui::pos2(x, fila.min.y),
                nombre,
                contador,
                self.pestana == pestana,
                &mut self.pestana,
                pestana,
            );
            x = rect.max.x + 22.0;
        }
        let y = ui.painter().round_to_pixel_center(fila.max.y - 0.5);
        ui.painter()
            .hline(fila.x_range(), y, Stroke::new(1.0, c.borde));

        // Lo que falló la última vez que se puso al día esta pestaña.
        let Some(parte) = self.pestana.parte() else {
            return;
        };
        let fallos = self.informe.fallos_de(parte);
        if !self.comprobaciones.contains_key(&parte) && !fallos.is_empty() {
            ui.add_space(14.0);
            panel(c).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new("No se pudo poner al día")
                        .strong()
                        .color(c.texto),
                );
                for fallo in fallos {
                    ui.label(egui::RichText::new(fallo).color(c.texto_secundario));
                }
            });
        }
    }

    fn pestana_series(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        series: &[Serie],
        acciones: &mut Vec<Accion>,
    ) {
        if series.is_empty() {
            texto_suave(ui, c, "Todavía no hay cómics en la biblioteca.");
            return;
        }

        let nombres: HashMap<&str, &str> = series
            .iter()
            .map(|s| (s.clave.as_str(), s.nombre.as_str()))
            .collect();
        let Self {
            informe,
            portadas,
            vinculos,
            ..
        } = self;
        let estados: Vec<&EstadoSerie> = informe
            .series
            .iter()
            .filter(|s| nombres.contains_key(s.clave.as_str()))
            .filter(|s| vinculos.get(&s.clave).is_some_and(|v| v.externa.is_some()))
            .collect();
        let con: Vec<&&EstadoSerie> = estados.iter().filter(|s| s.hay_novedades()).collect();
        let sin: Vec<&&EstadoSerie> = estados.iter().filter(|s| !s.hay_novedades()).collect();

        if !con.is_empty() {
            seccion(ui, c, &format!("Con novedades ({})", con.len()));
            for estado in con {
                fila_de_serie(ui, c, portadas, estado, acciones);
                ui.add_space(8.0);
            }
            ui.add_space(14.0);
        }
        if !sin.is_empty() {
            seccion(ui, c, &format!("Al día ({})", sin.len()));
            for estado in sin {
                fila_de_serie(ui, c, portadas, estado, acciones);
                ui.add_space(8.0);
            }
            ui.add_space(14.0);
        }

        let mut sin_identificar = Vec::new();
        let mut ignoradas = Vec::new();
        for serie in series {
            match vinculos.get(&serie.clave) {
                Some(v) if v.estado == EstadoVinculo::Ignorado => ignoradas.push(serie),
                Some(v) if v.externa.is_none() => sin_identificar.push(serie),
                _ => {}
            }
        }

        if !sin_identificar.is_empty() {
            seccion(
                ui,
                c,
                &format!("Sin identificar ({})", sin_identificar.len()),
            );
            for serie in sin_identificar {
                fila_simple(ui, c, serie, "Buscar…", acciones, |s| Accion::Buscar {
                    clave: s.clave.clone(),
                    nombre: s.nombre.clone(),
                });
            }
            ui.add_space(14.0);
        }
        if !ignoradas.is_empty() {
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("No se buscan ({})", ignoradas.len()))
                    .color(c.texto_secundario),
            )
            .id_salt("ignoradas")
            .show(ui, |ui| {
                for serie in ignoradas {
                    fila_simple(ui, c, serie, "Volver a buscar", acciones, |s| {
                        Accion::VolverABuscar(s.clave.clone())
                    });
                }
            });
        }
    }

    fn pestana_semana(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        series: &[Serie],
        acciones: &mut Vec<Accion>,
    ) {
        if self.ajustes.metron.is_none() {
            texto_suave(
                ui,
                c,
                "Para ver lo que sale cada semana hace falta una cuenta gratuita de Metron.",
            );
            ui.add_space(10.0);
            if ui.add(boton_subrayado(c, "Añadir cuenta…")).clicked() {
                acciones.push(Accion::Ajustes);
            }
            return;
        }
        let Some(semana) = &self.informe.semana else {
            return;
        };

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Del {} al {}, {} {}",
                    semana.desde.corta(),
                    semana.hasta.corta(),
                    semana.numeros.len(),
                    plural(semana.numeros.len(), "número", "números")
                ))
                .color(c.texto_secundario),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filtro_semana)
                        .hint_text("Filtrar")
                        .desired_width(200.0)
                        .margin(egui::vec2(8.0, 4.0)),
                );
            });
        });
        ui.add_space(14.0);

        let tuyas: HashSet<String> = series.iter().map(|s| s.clave.clone()).collect();
        let filtro = self.filtro_semana.trim().to_lowercase();
        let numeros: Vec<&Numero> = semana
            .numeros
            .iter()
            .filter(|n| filtro.is_empty() || n.serie.to_lowercase().contains(&filtro))
            .collect();
        if numeros.is_empty() {
            texto_suave(ui, c, "Nada coincide con el filtro.");
            return;
        }

        let hoy = Fecha::hoy();
        let favoritas = self.claves_favoritas();
        let tarjetas: Vec<Tarjeta> = numeros
            .iter()
            .map(|numero| {
                let tuya = tuyas.contains(&clave_de_serie(sin_anio(&numero.serie)));
                let favorito = favorito_de_numero(numero).map(|f| favoritas.contains(&f.clave));
                tarjeta_de_numero(numero, tuya, favorito, hoy)
            })
            .collect();
        match cuadricula(ui, c, &mut self.portadas, "semana", &tarjetas) {
            Some((i, Gesto::Abrir)) => acciones.push(accion_de_numero(numeros[i])),
            Some((i, Gesto::Favorito)) => {
                acciones.extend(favorito_de_numero(numeros[i]).map(Accion::Favorito));
            }
            None => {}
        }
    }

    fn pestana_manga(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        series: &[Serie],
        acciones: &mut Vec<Accion>,
    ) {
        let Some(manga) = &self.informe.manga else {
            return;
        };
        // Las de la biblioteca: las emparejadas con AniList y, por si acaso, las que se
        // llaman igual.
        let ids: HashSet<&str> = self
            .vinculos
            .values()
            .filter_map(|v| v.externa.as_ref())
            .filter(|e| e.fuente == Fuente::AniList.clave())
            .map(|e| e.id.as_str())
            .collect();
        let claves: HashSet<&str> = series.iter().map(|s| s.clave.as_str()).collect();
        let tuya = |ficha: &Ficha| {
            ids.contains(ficha.id.as_str())
                || std::iter::once(&ficha.titulo)
                    .chain(&ficha.subtitulo)
                    .any(|t| claves.contains(clave_de_serie(t).as_str()))
        };

        // "Para ti" solo si hay manga de la biblioteca con recomendadas.
        let hay_para_ti = !manga.para_ti.fichas.is_empty();
        if self.lista_manga == ListaManga::ParaTi && !hay_para_ti {
            self.lista_manga = ListaManga::Tendencia;
        }
        let botones = [
            (ListaManga::ParaTi, "Para ti"),
            (ListaManga::Tendencia, "En tendencia"),
            (ListaManga::Nuevas, "Empezadas en los últimos meses"),
        ];
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (lista, nombre) in botones {
                if lista == ListaManga::ParaTi && !hay_para_ti {
                    continue;
                }
                if pildora(ui, c, nombre, self.lista_manga == lista).clicked() {
                    self.lista_manga = lista;
                }
            }
        });
        ui.add_space(16.0);

        let lista = manga.lista(self.lista_manga);
        let favoritas: HashSet<String> = self.favoritos.iter().map(|f| f.clave.clone()).collect();
        let tarjetas: Vec<Tarjeta> = lista
            .fichas
            .iter()
            .map(|ficha| {
                let favorito = clave_de_ficha(ficha).map(|p| favoritas.contains(&p.clave()));
                tarjeta_de_ficha(ficha, tuya(ficha), favorito)
            })
            .collect();
        let id = match self.lista_manga {
            ListaManga::ParaTi => "para_ti",
            ListaManga::Tendencia => "tendencia",
            ListaManga::Nuevas => "nuevas",
        };
        match cuadricula(ui, c, &mut self.portadas, id, &tarjetas) {
            Some((i, Gesto::Abrir)) => {
                acciones.push(Accion::VerFicha(Box::new(lista.fichas[i].clone())));
            }
            Some((i, Gesto::Favorito)) => {
                let ficha = &lista.fichas[i];
                if let Some(pedido) = clave_de_ficha(ficha) {
                    acciones.push(Accion::Favorito(favorito_de_ficha(ficha, &pedido)));
                }
            }
            None => {}
        }
        // Cerca del final, la página siguiente.
        let (final_, _) = ui.allocate_exact_size(egui::vec2(1.0, 1.0), Sense::hover());
        if lista.hay_mas && final_.min.y < ui.clip_rect().max.y + ANTES_DEL_FINAL {
            acciones.push(Accion::MasManga(self.lista_manga));
        }
    }

    fn pestana_noticias(&mut self, ui: &mut egui::Ui, c: Colores, acciones: &mut Vec<Accion>) {
        if self.ajustes.canales.is_empty() {
            texto_suave(
                ui,
                c,
                "Añade en Ajustes el RSS de las webs de noticias que quieras seguir. Hay \
                 algunas sugerencias para empezar.",
            );
            ui.add_space(10.0);
            if ui
                .add(boton_subrayado(c, "Elegir webs de noticias…"))
                .clicked()
            {
                acciones.push(Accion::Ajustes);
            }
            return;
        }
        for noticia in &self.informe.noticias {
            fila_de_noticia(ui, c, &mut self.portadas, noticia, acciones);
        }
    }

    fn pestana_favoritos(&mut self, ui: &mut egui::Ui, c: Colores, acciones: &mut Vec<Accion>) {
        if self.favoritos.is_empty() {
            texto_suave(ui, c, "Todavía no tienes favoritos.");
            return;
        }
        let tarjetas: Vec<Tarjeta> = self
            .favoritos
            .iter()
            .map(|favorito| Tarjeta {
                portada: favorito.portada.as_deref(),
                titulo: &favorito.titulo,
                detalle: favorito.linea.clone(),
                detalle_en_cifras: false,
                etiqueta: None,
                favorito: Some(true),
                ayuda: if favorito.linea.is_empty() {
                    favorito.titulo.clone()
                } else {
                    format!("{}\n{}", favorito.titulo, favorito.linea)
                },
            })
            .collect();
        match cuadricula(ui, c, &mut self.portadas, "favoritos", &tarjetas) {
            Some((i, Gesto::Abrir)) => {
                let favorito = &self.favoritos[i];
                if let Some(pedido) = Pedido::desde_clave(&favorito.clave) {
                    acciones.push(Accion::PedirFicha {
                        pedido,
                        titulo: favorito.titulo.clone(),
                        portada: favorito.portada.clone(),
                    });
                }
            }
            Some((i, Gesto::Favorito)) => {
                acciones.push(Accion::Favorito(self.favoritos[i].clone()));
            }
            None => {}
        }
    }

    fn ventana_ajustes(&mut self, ctx: &egui::Context, c: Colores) {
        let Some(borrador) = &mut self.borrador else {
            return;
        };
        let mut abierta = true;
        let mut guardar = false;
        let mut cancelar = false;

        egui::Window::new("Ajustes de novedades")
            .open(&mut abierta)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.strong("Cuenta de Metron");
                ui.hyperlink_to("Crear una cuenta gratuita", "https://metron.cloud/");
                egui::Grid::new("cuenta")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Usuario");
                        ui.add(
                            egui::TextEdit::singleline(&mut borrador.usuario).desired_width(260.0),
                        );
                        ui.end_row();
                        ui.label("Contraseña");
                        ui.add(
                            egui::TextEdit::singleline(&mut borrador.contrasena)
                                .password(true)
                                .desired_width(260.0),
                        );
                        ui.end_row();
                    });
                ui.label(
                    egui::RichText::new("Se guarda sin cifrar en este equipo.")
                        .small()
                        .color(c.texto_secundario),
                );

                ui.add_space(14.0);
                ui.strong("Webs de noticias");
                let mut quitar = None;
                for (i, canal) in borrador.canales.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.add(boton_texto(c, "Quitar")).clicked() {
                            quitar = Some(i);
                        }
                        ui.add(egui::Label::new(canal.as_str()).truncate());
                    });
                }
                if let Some(i) = quitar {
                    borrador.canales.remove(i);
                }
                ui.horizontal(|ui| {
                    let campo = ui.add(
                        egui::TextEdit::singleline(&mut borrador.nuevo_canal)
                            .hint_text("https://…/feed")
                            .desired_width(320.0),
                    );
                    let intro = campo.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let nuevo = borrador.nuevo_canal.trim().to_string();
                    let valido = nuevo.starts_with("https://") || nuevo.starts_with("http://");
                    let anadir = ui.add_enabled(valido, boton_texto(c, "Añadir"));
                    if (anadir.clicked() || (intro && valido)) && !borrador.canales.contains(&nuevo)
                    {
                        borrador.canales.push(nuevo);
                        borrador.nuevo_canal.clear();
                    }
                });
                let sugeridos: Vec<_> = CANALES_SUGERIDOS
                    .iter()
                    .filter(|(_, url)| !borrador.canales.iter().any(|c| c == url))
                    .collect();
                if !sugeridos.is_empty() {
                    ui.add_space(4.0);
                    ui.weak("Sugerencias:");
                    for (nombre, url) in sugeridos {
                        if ui
                            .add(boton_texto(c, format!("+ {nombre}")))
                            .on_hover_text(*url)
                            .clicked()
                        {
                            borrador.canales.push(url.to_string());
                        }
                    }
                }

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.add(boton_subrayado(c, "Guardar")).clicked() {
                        guardar = true;
                    }
                    if ui.add(boton_texto(c, "Cancelar")).clicked() {
                        cancelar = true;
                    }
                });
            });

        if guardar {
            let usuario = borrador.usuario.trim().to_string();
            let contrasena = borrador.contrasena.clone();
            let ajustes = Ajustes {
                metron: (!usuario.is_empty() && !contrasena.is_empty()).then_some(Cuenta {
                    usuario,
                    contrasena,
                }),
                canales: borrador.canales.clone(),
            };
            match ajustes.guardar(&self.ruta_ajustes) {
                Ok(()) => {
                    let cambiada = ajustes != self.ajustes;
                    self.ajustes = ajustes;
                    self.borrador = None;
                    if cambiada {
                        // Con una cuenta nueva o canales nuevos hay cosas que mirar: cada
                        // pestaña se pondrá al día al entrar, y esta ya.
                        self.informe.comprobadas.clear();
                        if let Some(parte) = self.pestana.parte() {
                            self.comprobar(ctx, parte);
                        }
                    }
                }
                Err(e) => self.error = Some(format!("No se pudieron guardar los ajustes: {e}")),
            }
        } else if cancelar || !abierta {
            self.borrador = None;
        }
    }

    fn ventana_busqueda(
        &mut self,
        ctx: &egui::Context,
        c: Colores,
        biblioteca: &Biblioteca,
        acciones: &mut Vec<Accion>,
    ) {
        let Some(busqueda) = &mut self.busqueda else {
            return;
        };
        if let Some(respuesta) = &busqueda.respuesta {
            if let Ok((candidatos, avisos)) = respuesta.try_recv() {
                busqueda.candidatos = candidatos;
                busqueda.avisos = avisos;
                busqueda.respuesta = None;
                busqueda.buscada = true;
            }
        }

        let mut abierta = true;
        let mut buscar = !busqueda.buscada && busqueda.respuesta.is_none();
        let mut elegido = None;
        let mut ignorar = false;

        egui::Window::new(format!("¿Qué serie es «{}»?", busqueda.nombre))
            .open(&mut abierta)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let campo = ui.add(
                        egui::TextEdit::singleline(&mut busqueda.texto)
                            .hint_text("Nombre de la serie")
                            .desired_width(360.0),
                    );
                    let intro = campo.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let boton =
                        ui.add_enabled(busqueda.respuesta.is_none(), boton_texto(c, "Buscar"));
                    if boton.clicked() || intro {
                        buscar = true;
                    }
                });
                ui.add_space(8.0);

                if busqueda.respuesta.is_none()
                    && busqueda.buscada
                    && busqueda.candidatos.is_empty()
                {
                    ui.weak("Nada con ese nombre.");
                }
                egui::ScrollArea::vertical()
                    .max_height(380.0)
                    .show(ui, |ui| {
                        for candidato in &busqueda.candidatos {
                            ui.horizontal(|ui| {
                                let (hueco, _) =
                                    ui.allocate_exact_size(PORTADA_CANDIDATO, Sense::hover());
                                self.portadas
                                    .dibujar(ui, c, hueco, candidato.portada.as_deref());
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&candidato.titulo)
                                            .strong()
                                            .color(c.texto),
                                    );
                                    if !candidato.detalle.is_empty() {
                                        ui.label(
                                            egui::RichText::new(&candidato.detalle)
                                                .color(c.texto_secundario),
                                        );
                                    }
                                    ui.horizontal(|ui| {
                                        if ui.add(boton_subrayado(c, "Es esta")).clicked() {
                                            elegido = Some(candidato.clone());
                                        }
                                        if let Some(url) = &candidato.url {
                                            if ui.add(boton_texto(c, "Ver en la web")).clicked() {
                                                acciones.push(Accion::AbrirWeb(url.clone()));
                                            }
                                        }
                                    });
                                });
                            });
                            ui.add_space(6.0);
                        }
                    });
                for aviso in &busqueda.avisos {
                    ui.label(egui::RichText::new(aviso).small().color(c.texto_secundario));
                }
                ui.add_space(8.0);
                if ui.add(boton_texto(c, "No buscarla")).clicked() {
                    ignorar = true;
                }
            });

        if buscar && busqueda.respuesta.is_none() && !busqueda.texto.trim().is_empty() {
            let (enviar, recibir) = mpsc::channel();
            let texto = busqueda.texto.trim().to_string();
            let ajustes = self.ajustes.clone();
            let ctx = ctx.clone();
            thread::spawn(move || {
                let _ = enviar.send(comic_novedades::buscar(&texto, &ajustes));
                ctx.request_repaint();
            });
            busqueda.respuesta = Some(recibir);
            busqueda.candidatos.clear();
            busqueda.avisos.clear();
        }

        let clave = busqueda.clave.clone();
        if let Some(candidato) = elegido {
            self.busqueda = None;
            self.guardar_vinculo(
                biblioteca,
                Vinculo {
                    clave: clave.clone(),
                    externa: Some(SerieExterna {
                        fuente: candidato.fuente.clave().to_string(),
                        id: candidato.id,
                        nombre: candidato.titulo,
                    }),
                    estado: EstadoVinculo::Elegido,
                    buscada: ahora(),
                },
            );
            // Lo que se sabía era de la serie que no era.
            self.informe.series.retain(|s| s.clave != clave);
            self.comprobar(ctx, Parte::Series);
        } else if ignorar {
            self.busqueda = None;
            acciones.push(Accion::Ignorar(clave));
        } else if !abierta {
            self.busqueda = None;
        }
    }

    fn ventana_ficha(&mut self, ctx: &egui::Context, c: Colores, acciones: &mut Vec<Accion>) {
        if let Some(abierta) = &mut self.ficha {
            let llegada = abierta.pedido.as_ref().and_then(|(pedido, respuesta)| {
                match respuesta.try_recv() {
                    Ok(resultado) => Some((pedido.clone(), resultado)),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => Some((
                        pedido.clone(),
                        Err("No se pudo pedir la ficha.".to_string()),
                    )),
                }
            });
            if let Some((pedido, resultado)) = llegada {
                abierta.pedido = None;
                match resultado {
                    Ok(mut ficha) => {
                        // Las fichas de serie de Metron no traen portada: la del último número.
                        if ficha.portada.is_none() {
                            ficha.portada = abierta.portada.clone();
                            ficha.portada_grande = abierta.portada.clone();
                        }
                        self.fichas.insert(pedido, ficha.clone());
                        abierta.ficha = Some(Box::new(ficha));
                    }
                    Err(error) => {
                        eprintln!("[novedades] Ficha de {}: {error}", abierta.titulo);
                        // Si ya se enseñaba algo, se queda (solo faltan las recomendadas).
                        if abierta.ficha.is_none() {
                            abierta.error = Some(error);
                        }
                    }
                }
            }
        }

        let favoritas = self.claves_favoritas();
        let Self {
            ficha, portadas, ..
        } = self;
        let Some(abierta) = ficha else {
            return;
        };
        // La ventana se abre al momento, pero no se enseña nada de la ficha hasta tenerlo
        // todo (ficha, recomendadas y portadas): así no hay saltos. Mientras, una ruleta.
        let lista = abierta.lista(ctx, portadas);
        if !lista {
            ctx.request_repaint_after(ESPERA_FICHA.saturating_sub(abierta.desde.elapsed()));
        }
        let mut cerrar = false;
        // Modal: oscurece lo de detrás y se cierra al pulsar fuera o con Esc.
        let modal = egui::Modal::new(egui::Id::new("ficha_de_novedades"))
            .frame(
                egui::Frame::new()
                    .fill(c.panel)
                    .stroke(Stroke::new(1.0, c.borde))
                    .corner_radius(estilo::RADIO_PANEL)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(ctx, |ui| {
                ui.set_width(ANCHO_FICHA);
                if !lista {
                    ui.set_height(PORTADA_FICHA.y);
                    ui.centered_and_justified(|ui| {
                        ui.add(egui::Spinner::new().color(c.acento));
                    });
                } else if let Some(ficha) = &abierta.ficha {
                    let clave = abierta.clave.as_ref();
                    if contenido_ficha(ui, c, portadas, ficha, clave, &favoritas, acciones) {
                        cerrar = true;
                    }
                } else if let Some(error) = &abierta.error {
                    ui.label(
                        egui::RichText::new(abierta.titulo.as_str())
                            .font(estilo::negrita(19.0))
                            .color(c.texto),
                    );
                    ui.add_space(6.0);
                    ui.label(error.as_str());
                    ui.add_space(8.0);
                    if ui.add(boton_texto(c, "Cerrar")).clicked() {
                        cerrar = true;
                    }
                } else {
                    ui.horizontal_top(|ui| {
                        let (hueco, _) = ui.allocate_exact_size(PORTADA_FICHA, Sense::hover());
                        portadas.dibujar(ui, c, hueco, abierta.portada.as_deref());
                        ui.add_space(18.0);
                        ui.label(
                            egui::RichText::new(abierta.titulo.as_str())
                                .font(estilo::negrita(19.0))
                                .color(c.texto),
                        );
                    });
                }
            });
        if cerrar || modal.should_close() {
            self.ficha = None;
        }
    }

    fn ventana_error(&mut self, ctx: &egui::Context) {
        let Some(error) = &self.error else {
            return;
        };
        let c = estilo::colores(ctx);
        let mut cerrar = false;
        egui::Window::new("Algo ha fallado")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(error);
                ui.add_space(8.0);
                if ui.add(boton_texto(c, "Aceptar")).clicked() {
                    cerrar = true;
                }
            });
        if cerrar {
            self.error = None;
        }
    }
}

/// Una pestaña: el nombre, su contador y, si está elegida, una franja de acento debajo.
#[allow(clippy::too_many_arguments)]
fn pestana_de(
    ui: &mut egui::Ui,
    c: Colores,
    posicion: egui::Pos2,
    nombre: &str,
    contador: Option<usize>,
    elegida: bool,
    actual: &mut Pestana,
    esta: Pestana,
) -> Rect {
    let color = if elegida { c.texto } else { c.texto_secundario };
    let fuente = if elegida {
        estilo::media(14.0)
    } else {
        FontId::proportional(14.0)
    };
    let pintor = ui.painter();
    let texto = pintor.layout_no_wrap(nombre.to_string(), fuente, color);
    let numero = contador
        .map(|n| pintor.layout_no_wrap(n.to_string(), estilo::cifras(12.0), c.texto_secundario));
    let ancho = texto.size().x + numero.as_ref().map_or(0.0, |n| n.size().x + 8.0);
    let rect = Rect::from_min_size(posicion, egui::vec2(ancho, 34.0));
    let respuesta = ui
        .interact(rect, ui.id().with(("pestana", nombre)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if respuesta.clicked() {
        *actual = esta;
    }
    let pintor = ui.painter();
    let y = rect.min.y + (rect.height() - 2.0 - texto.size().y) / 2.0;
    let ancho_texto = texto.size().x;
    let color = if respuesta.hovered() { c.texto } else { color };
    pintor.galley(egui::pos2(rect.min.x, y), texto, color);
    if let Some(numero) = numero {
        let alto = numero.size().y;
        pintor.galley(
            egui::pos2(
                rect.min.x + ancho_texto + 8.0,
                rect.center().y - 1.0 - alto / 2.0,
            ),
            numero,
            c.texto_secundario,
        );
    }
    if elegida {
        let franja = Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 2.0), rect.max);
        pintor.rect_filled(franja, 1.0, c.texto);
    }
    rect
}

fn seccion(ui: &mut egui::Ui, c: Colores, texto: &str) {
    ui.label(
        egui::RichText::new(texto)
            .font(estilo::negrita(16.0))
            .color(c.texto),
    );
    ui.add_space(6.0);
}

/// Una opción de un grupo que se elige de una en una: la elegida, en una píldora un
/// escalón más clara y con el texto claro; las demás, solo el texto en gris.
pub(crate) fn pildora(ui: &mut egui::Ui, c: Colores, texto: &str, elegida: bool) -> egui::Response {
    let (fuente, color) = if elegida {
        (estilo::media(14.0), c.texto)
    } else {
        (FontId::proportional(14.0), c.texto_secundario)
    };
    let galeria = ui
        .painter()
        .layout_no_wrap(texto.to_string(), fuente, color);
    let tamano = galeria.size() + egui::vec2(28.0, 12.0);
    let (rect, respuesta) = ui.allocate_exact_size(tamano, Sense::click());
    let respuesta = respuesta.on_hover_cursor(egui::CursorIcon::PointingHand);
    let pintor = ui.painter();
    if elegida {
        pintor.rect(
            rect,
            rect.height() / 2.0,
            c.resaltado,
            Stroke::new(1.0, c.borde),
            egui::StrokeKind::Inside,
        );
    } else if respuesta.hovered() {
        pintor.rect_filled(rect, rect.height() / 2.0, c.panel);
    }
    let color = if respuesta.hovered() { c.texto } else { color };
    pintor.galley(rect.min + egui::vec2(14.0, 6.0), galeria, color);
    respuesta
}

fn texto_suave(ui: &mut egui::Ui, c: Colores, texto: &str) {
    ui.label(egui::RichText::new(texto).color(c.texto_secundario));
}

/// Una serie identificada: portada, lo que tienes, lo publicado y lo nuevo.
fn fila_de_serie(
    ui: &mut egui::Ui,
    c: Colores,
    portadas: &mut PortadasRemotas,
    estado: &EstadoSerie,
    acciones: &mut Vec<Accion>,
) {
    let hoy = Fecha::hoy();
    panel(c).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let ver_ficha = accion_de_serie(estado);
        ui.horizontal_top(|ui| {
            let sentido = if ver_ficha.is_some() {
                Sense::click()
            } else {
                Sense::hover()
            };
            let (hueco, portada) = ui.allocate_exact_size(PORTADA_FILA, sentido);
            portadas.dibujar(ui, c, hueco, estado.portada.as_deref());
            if let Some(accion) = &ver_ficha {
                if portada
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    acciones.push(accion.clone());
                }
            }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let nombre = ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new(&estado.nombre)
                                    .font(estilo::negrita(16.0))
                                    .color(c.texto),
                            )
                            .sense(Sense::click()),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Abrir la serie en la biblioteca");
                    if nombre.clicked() {
                        acciones.push(Accion::AbrirSerie(estado.nombre.clone()));
                    }
                });
                if clave_de_serie(&estado.titulo) != clave_de_serie(&estado.nombre) {
                    ui.label(egui::RichText::new(&estado.titulo).color(c.texto_secundario));
                }

                // "Tienes hasta el tomo 12. 29 tomos, 224 capítulos, terminada."
                let mut publicado = Vec::new();
                publicado.extend(estado.publicado.clone());
                if estado.publicacion != Publicacion::Desconocida {
                    publicado.push(estado.publicacion.nombre().to_lowercase());
                }
                let mut frases = Vec::new();
                if let Some(tienes) = &estado.tienes {
                    frases.push(format!("Tienes hasta el {tienes}."));
                }
                if !publicado.is_empty() {
                    let texto = publicado.join(", ");
                    let mut letras = texto.chars();
                    let texto: String = letras
                        .next()
                        .map(|primera| primera.to_uppercase().chain(letras).collect())
                        .unwrap_or_default();
                    frases.push(format!("{texto}."));
                }
                if !frases.is_empty() {
                    ui.label(egui::RichText::new(frases.join(" ")).color(c.texto_secundario));
                }
                if let Some(ficha) = &estado.ficha {
                    if !ficha.generos.is_empty() {
                        ui.label(
                            egui::RichText::new(ficha.generos.join(", ")).color(c.texto_secundario),
                        );
                    }
                    if let Some(sinopsis) = &ficha.sinopsis {
                        let texto = texto_ajustado(
                            ui,
                            &sinopsis.replace("\n\n", " "),
                            FontId::proportional(13.0),
                            c.texto_secundario,
                            ui.available_width(),
                            2,
                        );
                        let (rect, _) = ui.allocate_exact_size(texto.size(), Sense::hover());
                        ui.painter().galley(rect.min, texto, c.texto_secundario);
                    }
                }

                if estado.mas > 0 {
                    let texto = match estado.fuente {
                        Fuente::AniList => {
                            format!("{} más", plural_u32(estado.mas, "tomo", "tomos"))
                        }
                        Fuente::Metron => plural_u32(estado.mas, "número nuevo", "números nuevos"),
                    };
                    ui.label(egui::RichText::new(texto).strong().color(c.acento));
                }
                if estado.faltan > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "Te faltan {} anteriores",
                            plural_u32(estado.faltan, "número", "números")
                        ))
                        .color(c.texto_secundario),
                    );
                }

                if !estado.nuevos.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for numero in estado.nuevos.iter().take(MAX_NUMEROS_FILA) {
                            let etiqueta = etiqueta_de_numero(ui, c, numero, hoy);
                            if numero.id.is_some()
                                && etiqueta
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                            {
                                acciones.push(accion_de_numero(numero));
                            }
                        }
                        let resto = estado.nuevos.len().saturating_sub(MAX_NUMEROS_FILA);
                        if resto > 0 {
                            ui.label(
                                egui::RichText::new(format!("y {resto} más"))
                                    .color(c.texto_secundario),
                            );
                        }
                    });
                }

                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if let Some(accion) = &ver_ficha {
                        if ui.add(boton_texto(c, "Ficha")).clicked() {
                            acciones.push(accion.clone());
                        }
                    }
                    let otra = ui.add(boton_texto(c, "No es esta serie…"));
                    if otra.clicked() {
                        acciones.push(Accion::Buscar {
                            clave: estado.clave.clone(),
                            nombre: estado.nombre.clone(),
                        });
                    }
                    if ui.add(boton_texto(c, "No buscar")).clicked() {
                        acciones.push(Accion::Ignorar(estado.clave.clone()));
                    }
                });
            });
        });
    });
}

/// `#5 · 14 oct`, o `#6 · sale el 18 nov` si aún no ha salido.
fn etiqueta_de_numero(
    ui: &mut egui::Ui,
    c: Colores,
    numero: &Numero,
    hoy: Fecha,
) -> egui::Response {
    let futuro = numero.fecha.is_some_and(|f| f > hoy);
    let texto = match numero.fecha {
        Some(fecha) if futuro => format!("#{} · sale el {}", numero.numero, fecha.corta()),
        Some(fecha) => format!("#{} · {}", numero.numero, fecha.corta()),
        None => format!("#{}", numero.numero),
    };
    let color = if futuro { c.texto_secundario } else { c.texto };
    egui::Frame::NONE
        .stroke(Stroke::new(
            1.0,
            if futuro { c.borde } else { c.borde_fuerte },
        ))
        .fill(if futuro { c.contenido } else { c.tarjeta })
        .corner_radius(estilo::RADIO_ETIQUETA)
        .inner_margin(egui::Margin::symmetric(7, 3))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(texto)
                    .font(estilo::cifras(11.5))
                    .color(color),
            );
        })
        .response
        .interact(Sense::click())
}

/// Una serie con un solo botón (sin identificar, o que no se busca).
fn fila_simple(
    ui: &mut egui::Ui,
    c: Colores,
    serie: &Serie,
    boton: &str,
    acciones: &mut Vec<Accion>,
    accion: impl Fn(&Serie) -> Accion,
) {
    ui.horizontal(|ui| {
        if ui.add(boton_texto(c, boton)).clicked() {
            acciones.push(accion(serie));
        }
        let nombre = ui
            .add(
                egui::Label::new(egui::RichText::new(&serie.nombre).color(c.texto))
                    .sense(Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Abrir la serie en la biblioteca");
        if nombre.clicked() {
            acciones.push(Accion::AbrirSerie(serie.nombre.clone()));
        }
        ui.label(
            egui::RichText::new(format!(
                "{} {}",
                serie.comics,
                plural(serie.comics, "cómic", "cómics")
            ))
            .font(estilo::cifras(11.5))
            .color(c.texto_secundario),
        );
    });
}

/// Lo que se enseña de cada obra o número en una cuadrícula de portadas.
struct Tarjeta<'a> {
    portada: Option<&'a str>,
    titulo: &'a str,
    detalle: String,
    /// Si el detalle son cifras (`#6 · 21 oct`), que van con su fuente.
    detalle_en_cifras: bool,
    /// Lo que se pone sobre la portada: "Tu serie" o "Favorito".
    etiqueta: Option<&'static str>,
    /// Si es favorito; `None` si no se puede guardar (no se sabe qué es en el catálogo).
    favorito: Option<bool>,
    ayuda: String,
}

/// Lo que se ha hecho con una tarjeta.
#[derive(Clone, Copy, PartialEq)]
enum Gesto {
    Abrir,
    /// Guardarla en favoritos o quitarla, desde el menú del botón derecho.
    Favorito,
}

/// La etiqueta sobre la portada: si está en la biblioteca, eso; si no, si es favorito.
fn etiqueta(tuya: bool, favorito: Option<bool>) -> Option<&'static str> {
    if tuya {
        Some("Tu serie")
    } else if favorito == Some(true) {
        Some("Favorito")
    } else {
        None
    }
}

/// Qué es una ficha de un listado en su catálogo. Solo los manga de AniList: los números
/// de Metron se guardan desde su tarjeta.
fn clave_de_ficha(ficha: &Ficha) -> Option<Pedido> {
    (ficha.fuente == Fuente::AniList && !ficha.id.is_empty())
        .then(|| Pedido::Manga(ficha.id.clone()))
}

fn favorito_de_ficha(ficha: &Ficha, pedido: &Pedido) -> Favorito {
    Favorito {
        clave: pedido.clave(),
        titulo: ficha.titulo.clone(),
        portada: ficha.portada.clone(),
        linea: ficha.linea.clone(),
        anadido: ahora(),
    }
}

fn favorito_de_numero(numero: &Numero) -> Option<Favorito> {
    let id = numero.id?;
    Some(Favorito {
        clave: Pedido::Numero(id).clave(),
        titulo: format!("{} #{}", sin_anio(&numero.serie), numero.numero),
        portada: numero.portada.clone(),
        linea: numero
            .fecha
            .map(|f| format!("Cómic, {f}"))
            .unwrap_or_else(|| "Cómic".to_string()),
        anadido: ahora(),
    })
}

fn favorito_de_recomendada(recomendada: &Recomendada) -> Favorito {
    Favorito {
        clave: Pedido::Manga(recomendada.id.clone()).clave(),
        titulo: recomendada.titulo.clone(),
        portada: recomendada.portada.clone(),
        linea: "Manga".to_string(),
        anadido: ahora(),
    }
}

/// El menú del botón derecho de una portada para guardarla en favoritos o quitarla.
/// Devuelve si se ha elegido.
fn menu_de_favorito(respuesta: &egui::Response, favorito: bool) -> bool {
    let mut elegido = false;
    respuesta.context_menu(|ui| {
        let texto = if favorito {
            "Quitar de favoritos"
        } else {
            "Añadir a favoritos"
        };
        if ui.button(texto).clicked() {
            elegido = true;
            ui.close();
        }
        estilo::cerrar_menu_al_alejarse(ui, None);
    });
    elegido
}

fn tarjeta_de_numero(
    numero: &Numero,
    tuya: bool,
    favorito: Option<bool>,
    hoy: Fecha,
) -> Tarjeta<'_> {
    let mut ayuda = format!("{} #{}", numero.serie, numero.numero);
    if let Some(fecha) = numero.fecha {
        let cuando = if fecha > hoy { "Sale el" } else { "Salió el" };
        ayuda.push_str(&format!("\n{cuando} {fecha}"));
    }
    if tuya {
        ayuda.push_str("\nTienes esta serie en la biblioteca");
    }
    Tarjeta {
        portada: numero.portada.as_deref(),
        titulo: sin_anio(&numero.serie),
        detalle: match numero.fecha {
            Some(fecha) => format!("#{} · {}", numero.numero, fecha.corta()),
            None => format!("#{}", numero.numero),
        },
        detalle_en_cifras: true,
        etiqueta: etiqueta(tuya, favorito),
        favorito,
        ayuda,
    }
}

fn tarjeta_de_ficha(ficha: &Ficha, tuya: bool, favorito: Option<bool>) -> Tarjeta<'_> {
    let mut ayuda = ficha.titulo.clone();
    if let Some(subtitulo) = &ficha.subtitulo {
        ayuda.push_str(&format!("\n{subtitulo}"));
    }
    if !ficha.linea.is_empty() {
        ayuda.push_str(&format!("\n{}", ficha.linea));
    }
    if tuya {
        ayuda.push_str("\nTienes esta serie en la biblioteca");
    }
    let detalle = if ficha.generos.is_empty() {
        ficha.linea.clone()
    } else {
        ficha
            .generos
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    Tarjeta {
        portada: ficha.portada.as_deref(),
        titulo: &ficha.titulo,
        detalle,
        detalle_en_cifras: false,
        etiqueta: etiqueta(tuya, favorito),
        favorito,
        ayuda,
    }
}

/// Lo que se hace al pulsar un número de Metron: pedir su ficha. Los guardados por una
/// versión anterior no tienen identificador, así que se enseña lo que se sabe.
fn accion_de_numero(numero: &Numero) -> Accion {
    let titulo = format!("{} #{}", sin_anio(&numero.serie), numero.numero);
    match numero.id {
        Some(id) => Accion::PedirFicha {
            pedido: Pedido::Numero(id),
            titulo,
            portada: numero.portada.clone(),
        },
        None => Accion::VerFicha(Box::new(Ficha {
            fuente: Fuente::Metron,
            id: String::new(),
            titulo,
            subtitulo: None,
            linea: String::new(),
            portada: numero.portada.clone(),
            portada_grande: numero.portada.clone(),
            url: None,
            datos: numero
                .fecha
                .map(|f| ("A la venta".to_string(), f.to_string()))
                .into_iter()
                .collect(),
            generos: Vec::new(),
            sinopsis: None,
            recomendaciones: Vec::new(),
        })),
    }
}

/// Lo que se hace al pulsar la portada de una serie de la biblioteca: su ficha, que en
/// AniList ya se tiene y en Metron hay que pedir.
fn accion_de_serie(estado: &EstadoSerie) -> Option<Accion> {
    match (&estado.ficha, estado.fuente) {
        (Some(ficha), _) => Some(Accion::VerFicha(Box::new(ficha.clone()))),
        (None, Fuente::Metron) => Some(Accion::PedirFicha {
            pedido: Pedido::Serie(estado.id.clone()),
            titulo: estado.titulo.clone(),
            portada: estado.portada.clone(),
        }),
        (None, Fuente::AniList) => None,
    }
}

/// Una cuadrícula de portadas que ocupa todo el ancho. Solo se dibujan las filas que se
/// ven. Devuelve la que se ha pulsado o guardado en favoritos.
fn cuadricula(
    ui: &mut egui::Ui,
    c: Colores,
    portadas: &mut PortadasRemotas,
    id: &str,
    tarjetas: &[Tarjeta],
) -> Option<(usize, Gesto)> {
    let separacion = 18.0;
    let paso = egui::vec2(
        PORTADA_SEMANA.x + separacion,
        ALTO_TARJETA_SEMANA + separacion,
    );
    let ancho = ui.available_width();
    let columnas = (((ancho + separacion) / paso.x).floor() as usize).max(1);
    let mut pulsada = None;
    for (f, fila) in tarjetas.chunks(columnas).enumerate() {
        let (zona, _) = ui.allocate_exact_size(egui::vec2(ancho, paso.y), Sense::hover());
        if !ui.is_rect_visible(zona) {
            continue;
        }
        for (columna, tarjeta) in fila.iter().enumerate() {
            let origen = zona.min + egui::vec2(columna as f32 * paso.x, 0.0);
            let indice = f * columnas + columna;
            if let Some(gesto) = dibujar_tarjeta(ui, c, portadas, origen, (id, indice), tarjeta) {
                pulsada = Some((indice, gesto));
            }
        }
    }
    pulsada
}

/// Portada, título en dos líneas y un detalle debajo. Devuelve lo que se ha hecho con
/// ella: pulsarla o, con el botón derecho, guardarla en favoritos o quitarla.
fn dibujar_tarjeta(
    ui: &mut egui::Ui,
    c: Colores,
    portadas: &mut PortadasRemotas,
    origen: egui::Pos2,
    id: (&str, usize),
    tarjeta: &Tarjeta,
) -> Option<Gesto> {
    let hueco = Rect::from_min_size(origen, PORTADA_SEMANA);
    let zona = Rect::from_min_size(origen, egui::vec2(PORTADA_SEMANA.x, ALTO_TARJETA_SEMANA));
    let respuesta = ui
        .interact(zona, ui.id().with(("tarjeta", id)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(&tarjeta.ayuda);

    portadas.dibujar(ui, c, hueco, tarjeta.portada);
    let pintor = ui.painter();
    if tarjeta.portada.is_none() {
        pintor.text(
            hueco.center(),
            egui::Align2::CENTER_CENTER,
            "Sin portada",
            FontId::proportional(12.0),
            c.texto_secundario,
        );
    }
    if respuesta.hovered() {
        pintor.rect_stroke(
            hueco,
            estilo::RADIO_PORTADA,
            Stroke::new(1.5, c.texto_secundario),
            egui::StrokeKind::Outside,
        );
    }
    if let Some(etiqueta) = tarjeta.etiqueta {
        etiqueta_sobre_portada(pintor, c, hueco, etiqueta);
    }

    let titulo = texto_ajustado(
        ui,
        tarjeta.titulo,
        FontId::proportional(13.0),
        c.texto,
        PORTADA_SEMANA.x,
        2,
    );
    let alto = titulo.size().y;
    let pintor = ui.painter();
    pintor.galley(hueco.left_bottom() + egui::vec2(0.0, 6.0), titulo, c.texto);
    let fuente_detalle = if tarjeta.detalle_en_cifras {
        estilo::cifras(11.5)
    } else {
        FontId::proportional(12.0)
    };
    let detalle = texto_ajustado(
        ui,
        &tarjeta.detalle,
        fuente_detalle,
        c.texto_secundario,
        PORTADA_SEMANA.x,
        1,
    );
    ui.painter().galley(
        hueco.left_bottom() + egui::vec2(0.0, 8.0 + alto),
        detalle,
        c.texto_secundario,
    );
    let favorito = tarjeta
        .favorito
        .is_some_and(|es| menu_de_favorito(&respuesta, es));
    if favorito {
        Some(Gesto::Favorito)
    } else if respuesta.clicked() {
        Some(Gesto::Abrir)
    } else {
        None
    }
}

/// Lo de dentro de la ventana de una ficha. `clave` es qué es en el catálogo (para
/// guardarla en favoritos) y `favoritas`, las claves de los favoritos. Devuelve si se ha
/// pulsado "Cerrar".
fn contenido_ficha(
    ui: &mut egui::Ui,
    c: Colores,
    portadas: &mut PortadasRemotas,
    ficha: &Ficha,
    clave: Option<&Pedido>,
    favoritas: &HashSet<String>,
    acciones: &mut Vec<Accion>,
) -> bool {
    let ancho_texto = ANCHO_FICHA - PORTADA_FICHA.x - 18.0;
    ui.horizontal_top(|ui| {
        let (hueco, _) = ui.allocate_exact_size(PORTADA_FICHA, Sense::hover());
        let portada = ficha.portada_grande.as_deref().or(ficha.portada.as_deref());
        portadas.dibujar(ui, c, hueco, portada);
        ui.add_space(18.0);
        ui.vertical(|ui| {
            ui.set_width(ancho_texto);
            ui.label(
                egui::RichText::new(&ficha.titulo)
                    .font(estilo::negrita(19.0))
                    .color(c.texto),
            );
            if let Some(subtitulo) = &ficha.subtitulo {
                ui.label(egui::RichText::new(subtitulo).color(c.texto_secundario));
            }
            ui.add_space(10.0);
            if !ficha.datos.is_empty() {
                egui::Grid::new("datos_de_la_ficha")
                    .num_columns(2)
                    .spacing([14.0, 5.0])
                    .max_col_width(ancho_texto - 120.0)
                    .show(ui, |ui| {
                        for (nombre, valor) in &ficha.datos {
                            ui.label(egui::RichText::new(nombre).color(c.texto_secundario));
                            ui.add(
                                egui::Label::new(egui::RichText::new(valor).color(c.texto)).wrap(),
                            );
                            ui.end_row();
                        }
                    });
            }
            if !ficha.generos.is_empty() {
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    for genero in &ficha.generos {
                        egui::Frame::NONE
                            .fill(c.tarjeta)
                            .corner_radius(255)
                            .inner_margin(egui::Margin::symmetric(9, 3))
                            .show(ui, |ui| {
                                // Sin partir: cada género entero en su píldora.
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(genero).size(12.0).color(c.texto),
                                    )
                                    .extend(),
                                );
                            });
                    }
                });
            }
        });
    });

    ui.add_space(16.0);
    match &ficha.sinopsis {
        Some(sinopsis) => {
            ui.label(
                egui::RichText::new("Sinopsis")
                    .font(estilo::negrita(16.0))
                    .color(c.texto),
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(170.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(sinopsis).size(14.0).color(c.texto));
                });
        }
        None => texto_suave(ui, c, "Sin sinopsis."),
    }

    if !ficha.recomendaciones.is_empty() {
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new("Recomendados")
                .font(estilo::negrita(16.0))
                .color(c.texto),
        );
        ui.add_space(8.0);
        let separacion = 12.0;
        let caben = ((ui.available_width() + separacion) / (PORTADA_RECOMENDADA.x + separacion))
            .floor()
            .max(1.0) as usize;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = separacion;
            for recomendada in ficha.recomendaciones.iter().take(caben) {
                let tamano = PORTADA_RECOMENDADA + egui::vec2(0.0, 38.0);
                let (zona, respuesta) = ui.allocate_exact_size(tamano, Sense::click());
                let hueco = Rect::from_min_size(zona.min, PORTADA_RECOMENDADA);
                portadas.dibujar(ui, c, hueco, recomendada.portada.as_deref());
                let titulo = texto_ajustado(
                    ui,
                    &recomendada.titulo,
                    FontId::proportional(12.0),
                    c.texto,
                    PORTADA_RECOMENDADA.x,
                    2,
                );
                ui.painter()
                    .galley(hueco.left_bottom() + egui::vec2(0.0, 4.0), titulo, c.texto);
                let respuesta = respuesta
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(&recomendada.titulo);
                let favorito = favorito_de_recomendada(recomendada);
                let es_favorito = favoritas.contains(&favorito.clave);
                if es_favorito {
                    etiqueta_sobre_portada(ui.painter(), c, hueco, "Favorito");
                }
                if menu_de_favorito(&respuesta, es_favorito) {
                    acciones.push(Accion::Favorito(favorito));
                }
                if respuesta.hovered() {
                    ui.painter().rect_stroke(
                        hueco,
                        estilo::RADIO_PORTADA,
                        Stroke::new(1.5, c.texto_secundario),
                        egui::StrokeKind::Outside,
                    );
                }
                if respuesta.clicked() {
                    acciones.push(Accion::PedirFicha {
                        pedido: Pedido::Manga(recomendada.id.clone()),
                        titulo: recomendada.titulo.clone(),
                        portada: recomendada.portada.clone(),
                    });
                }
            }
        });
    }

    ui.add_space(14.0);
    let mut cerrar = false;
    ui.horizontal(|ui| {
        if let Some(pedido) = clave {
            let favorito = favorito_de_ficha(ficha, pedido);
            let texto = if favoritas.contains(&favorito.clave) {
                "Quitar de favoritos"
            } else {
                "Añadir a favoritos"
            };
            if ui.add(boton_subrayado(c, texto)).clicked() {
                acciones.push(Accion::Favorito(favorito));
            }
        }
        if let Some(url) = &ficha.url {
            if ui.add(boton_subrayado(c, "Ver en la web")).clicked() {
                acciones.push(Accion::AbrirWeb(url.clone()));
            }
        }
        if ui.add(boton_texto(c, "Cerrar")).clicked() {
            cerrar = true;
        }
    });
    cerrar
}

/// Una noticia: su foto a la izquierda y el titular, la web, la fecha y el resumen a la
/// derecha. La foto y el titular abren el artículo.
fn fila_de_noticia(
    ui: &mut egui::Ui,
    c: Colores,
    portadas: &mut PortadasRemotas,
    noticia: &Noticia,
    acciones: &mut Vec<Accion>,
) {
    ui.horizontal_top(|ui| {
        let (hueco, foto) = ui.allocate_exact_size(FOTO_NOTICIA, Sense::click());
        match &noticia.imagen {
            Some(url) => portadas.dibujar_recortada(ui, c, hueco, Some(url)),
            None => {
                // Sin foto, el nombre de la web en su lugar.
                let pintor = ui.painter();
                pintor.rect(
                    hueco,
                    0,
                    c.tarjeta,
                    Stroke::new(1.0, c.borde),
                    egui::StrokeKind::Inside,
                );
                let nombre = texto_ajustado(
                    ui,
                    &noticia.fuente,
                    estilo::negrita(12.0),
                    c.texto_secundario,
                    hueco.width() - 16.0,
                    2,
                );
                let esquina = hueco.center() - nombre.size() / 2.0;
                ui.painter().galley(esquina, nombre, c.texto_secundario);
            }
        }
        if foto
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(&noticia.enlace)
            .clicked()
        {
            acciones.push(Accion::AbrirWeb(noticia.enlace.clone()));
        }
        ui.add_space(14.0);

        ui.vertical(|ui| {
            let titulo = ui
                .add(
                    egui::Label::new(
                        egui::RichText::new(&noticia.titulo)
                            .font(estilo::negrita(15.0))
                            .color(c.texto),
                    )
                    .sense(Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(&noticia.enlace);
            if titulo.clicked() {
                acciones.push(Accion::AbrirWeb(noticia.enlace.clone()));
            }
            let mut meta = noticia.fuente.clone();
            if let Some(fecha) = noticia.fecha {
                meta.push_str(&format!(", {}", hace(fecha)));
            }
            ui.label(egui::RichText::new(meta).color(c.texto_secundario));
            if let Some(resumen) = &noticia.resumen {
                let texto = texto_ajustado(
                    ui,
                    resumen,
                    FontId::proportional(13.5),
                    c.texto_secundario,
                    ui.available_width(),
                    3,
                );
                let (rect, _) = ui.allocate_exact_size(texto.size(), Sense::hover());
                ui.painter().galley(rect.min, texto, c.texto_secundario);
            }
        });
    });
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(10.0);
}

fn plural_u32(n: u32, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

/// `hace 5 min`, `hace 3 h`, `hace 2 días`, o la fecha si es de hace más de una semana.
fn hace(instante: i64) -> String {
    let segundos = (ahora() - instante).max(0);
    match segundos {
        0..60 => "ahora mismo".to_string(),
        60..3600 => format!("hace {} min", segundos / 60),
        3600..86_400 => format!("hace {} h", segundos / 3600),
        86_400..604_800 => {
            let dias = segundos / 86_400;
            format!("hace {dias} {}", if dias == 1 { "día" } else { "días" })
        }
        _ => format!("el {}", Fecha::desde_dias(instante.div_euclid(86_400))),
    }
}
