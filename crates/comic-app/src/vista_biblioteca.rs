//! Pantalla de la biblioteca: una barra lateral (Biblioteca, Leyendo, Novedades, Carpetas), lo que
//! se está leyendo en grande arriba, las series en
//! una cuadrícula de portadas y una ficha para cada serie. El escaneo y las miniaturas
//! van en hilos aparte, así que la pantalla no se congela nunca.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use comic_biblioteca::{
    Biblioteca, Carpeta, Ficha, Informe, Movimiento, Portada, Progreso, Resumen, Serie,
};
use eframe::egui::{self, Color32, FontId, Rect, Stroke, StrokeKind};

use crate::dialogos::{self, Dialogo};
use crate::elegir_portada::{self as portada, ElegirPortada, ElegirPortadaDeSerie};
use crate::estilo::{self, Colores};
use crate::fondo::Fondo;
use crate::miniaturas::{self, Miniaturas};
use crate::precarga::Tamano;
use crate::preferencias::Preferencias;
use crate::vista_ajustes::VistaAjustes;
use crate::vista_novedades::VistaNovedades;

/// Tamaño de las tarjetas, en puntos. Las portadas de cómic suelen ser 2:3.
const ANCHO_TARJETA: f32 = 160.0;
const ALTO_PORTADA: f32 = 240.0;
/// Franja bajo la portada para la barra de progreso (se reserva en todas las tarjetas, para
/// que los títulos queden alineados) y lo que mide la barra.
const ESPACIO_BARRA: f32 = 12.0;
const ALTO_BARRA: f32 = 4.0;
/// Portada, barra de progreso, título en dos líneas y una línea de detalle.
const ALTO_TARJETA: f32 = ALTO_PORTADA + ESPACIO_BARRA + 58.0;
const SEPARACION: f32 = 20.0;
pub(crate) const MARGEN: f32 = 32.0;
const ANCHO_LATERAL: f32 = 220.0;
/// Alto de los títulos de sección ("Seguir leyendo", "Series").
const ALTO_TITULO: f32 = 40.0;

/// Portada de "Seguir leyendo" y de la ficha de una serie, más grandes que las tarjetas.
const PORTADA_DESTACADA: egui::Vec2 = egui::vec2(140.0, 210.0);
const PORTADA_FICHA: egui::Vec2 = egui::vec2(200.0, 300.0);

/// Miniaturas que se guardan en la GPU como mucho. Con miles de cómics, tenerlas
/// todas ocuparía gigas; las que hace tiempo que no se ven se sueltan.
const MAX_TEXTURAS: usize = 300;
/// Mientras se escanea, cada cuánto se recargan las listas para ir mostrando lo nuevo.
const RECARGA_DURANTE_ESCANEO: Duration = Duration::from_secs(1);
/// Cuántos cómics empezados se muestran como mucho en "Leyendo".
const MAX_SEGUIR_LEYENDO: usize = 100;

#[derive(Clone, Copy, PartialEq)]
enum Pagina {
    Biblioteca,
    Leyendo,
    Novedades,
    Carpetas,
    Ajustes,
}

/// Lo que se ve en una tarjeta y qué pasa al pulsarla.
struct Tarjeta {
    portada: Option<Portada>,
    titulo: String,
    detalle: String,
    /// Texto que aparece al dejar el ratón encima.
    ayuda: String,
    /// Cuánto se ha leído, de 0 a 1.
    progreso: Option<f32>,
    terminado: bool,
    /// `None` si no se puede abrir (un cómic que no se pudo leer).
    al_pulsar: Option<Pulsar>,
    /// Si se puede renombrar pulsando su título, y qué pasaría a renombrar.
    renombrar: Option<Renombrable>,
}

#[derive(Clone)]
enum Pulsar {
    Serie(String),
    Comic(PathBuf),
}

/// Algo a lo que el usuario le puede poner un nombre a mano, sin tocar los archivos.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Renombrable {
    /// Una serie, por su clave (la de [`Serie::clave`]): sigue valiendo aunque el nombre
    /// que se vea cambie.
    Serie(String),
    /// Un cómic suelto, por su huella: sigue valiendo aunque se mueva o se renombre.
    Comic(String),
}

/// Un nombre que se está escribiendo en el sitio, en vez de mostrarlo.
struct Renombrando {
    objetivo: Renombrable,
    texto: String,
    /// Para pedir el foco del campo solo la primera vez que se dibuja.
    foco_pedido: bool,
}

/// Lo que va encima de la cuadrícula de cada página.
enum Cabecera {
    /// Un título grande y una línea debajo.
    Titulo { titulo: String, detalle: String },
    /// La biblioteca: título y, si hay algo empezado, "Seguir leyendo" en grande.
    Inicio {
        detalle: String,
        destacado: Option<Box<Ficha>>,
    },
    /// La ficha de una serie.
    Serie(DatosSerie),
}

impl Cabecera {
    fn alto(&self) -> f32 {
        match self {
            Cabecera::Titulo { .. } => 84.0,
            Cabecera::Inicio {
                destacado: Some(_), ..
            } => 84.0 + PORTADA_DESTACADA.y + 40.0 + 28.0,
            Cabecera::Inicio { .. } => 84.0,
            Cabecera::Serie(_) => 36.0 + PORTADA_FICHA.y + 40.0,
        }
    }
}

/// Lo que se muestra en la ficha de una serie, sacado de sus cómics.
struct DatosSerie {
    nombre: String,
    clave: String,
    portada: Option<Portada>,
    /// "12 cómics · 3 leídos · 2010–2014"
    resumen_corto: String,
    /// "Guion: …", "Dibujo: …", "Editorial: …"
    creditos: Vec<String>,
    resumen: Option<String>,
    /// El texto del botón principal y el cómic que abre.
    siguiente: Option<(String, PathBuf)>,
}

/// La serie que se está viendo.
struct SerieAbierta {
    nombre: String,
    /// Identifica la serie aunque cambie el nombre con que se muestra (al unirla con otra).
    clave: String,
    comics: Vec<Ficha>,
    /// La portada de la serie: la del cómic elegido o, si no, la del primero.
    portada: Option<Portada>,
}

/// Una miniatura ya en la GPU.
struct Textura {
    textura: egui::TextureHandle,
    /// Último fotograma en que se dibujó, para soltar las que no se ven.
    usada: u64,
}

/// El estado de un arrastre en un fotograma: qué cómic se lleva (su posición en la serie),
/// sobre qué tarjeta caería, dónde está el puntero y si ya se ha soltado el botón.
struct Arrastre {
    actual: usize,
    destino: usize,
    puntero: egui::Pos2,
    soltado: bool,
}

/// La huella del cómic de una tarjeta, si es de un cómic.
fn huella_de_tarjeta(tarjeta: &Tarjeta) -> Option<&str> {
    match &tarjeta.renombrar {
        Some(Renombrable::Comic(huella)) => Some(huella),
        _ => None,
    }
}

/// Un escaneo en marcha en otro hilo.
struct Escaneo {
    hechos: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    fin: Receiver<Result<Resumen, String>>,
}

pub struct VistaBiblioteca {
    biblioteca: Biblioteca,
    ruta_db: PathBuf,
    // Lo que se muestra, leído de la base de datos. Se recarga cuando cambia algo, no
    // en cada fotograma.
    carpetas: Vec<Carpeta>,
    series: Vec<Serie>,
    seguir_leyendo: Vec<Ficha>,
    pagina: Pagina,
    /// La serie que se está viendo, o `None` para ver la página elegida.
    serie: Option<SerieAbierta>,
    /// La ventana para unir la serie abierta con otra: lo que se ha escrito para filtrar.
    unir: Option<String>,
    busqueda: String,
    novedades: VistaNovedades,
    ajustes: VistaAjustes,
    /// El nombre que se está escribiendo, si se ha pulsado el de una serie o un cómic.
    renombrando: Option<Renombrando>,
    /// La huella del cómic que se está arrastrando para cambiarlo de sitio en su serie.
    arrastrando: Option<String>,
    /// Distancia del puntero a la esquina de la tarjeta arrastrada al empezar a arrastrar.
    agarre: egui::Vec2,
    /// El cómic recién soltado: sigue encima de las demás mientras vuela a su sitio.
    soltada: Option<String>,
    /// Por huella: dónde se está dibujando cada tarjeta de la serie abierta (relativo al
    /// inicio de la cuadrícula) y el último fotograma en que se usó. Así, al reordenar,
    /// las tarjetas se deslizan a su sitio en vez de saltar.
    posiciones: HashMap<String, (egui::Vec2, u64)>,
    /// La ventana para elegir la portada de un cómic, si está abierta.
    eligiendo_portada: Option<ElegirPortada>,
    /// La ventana para elegir la portada de una serie, si está abierta.
    eligiendo_portada_de_serie: Option<ElegirPortadaDeSerie>,

    miniaturas: Miniaturas,
    /// Donde están las miniaturas guardadas en disco.
    carpeta_miniaturas: PathBuf,
    /// La portada de lo último que se está leyendo, de fondo, en gris y difuminada.
    fondo: Fondo,
    /// Por huella y tamaño en píxeles: la misma portada se usa a varios tamaños (tarjeta,
    /// "Seguir leyendo", ficha) y cada uno tiene la suya, para dibujarse siempre 1:1.
    texturas: HashMap<(String, Tamano), Textura>,
    /// Huellas cuya miniatura no se pudo hacer: no se vuelven a pedir.
    sin_miniatura: std::collections::HashSet<String>,
    /// Lo último que se pidió, para no repetir la misma petición en cada fotograma.
    pedido: Vec<(String, Tamano)>,
    fotograma: u64,
    /// El icono de la app para la barra lateral, encogido al tamaño exacto en píxeles.
    icono: Option<(f32, egui::TextureHandle)>,

    escaneo: Option<Escaneo>,
    /// Una búsqueda de cómics movidos en marcha en otro hilo.
    reubicando: Option<Receiver<Result<Informe, String>>>,
    /// Lo que resultó de la última búsqueda de cómics movidos.
    aviso_carpetas: Option<String>,
    /// Ha cambiado algo durante un escaneo (una carpeta nueva): hay que repetirlo al acabar.
    reescanear: bool,
    ultima_recarga: Instant,
    /// Se ha cambiado de página o abierto una serie: lo que se dibuje en este fotograma
    /// empieza arriba del todo, no por donde se había dejado el desplazamiento.
    arriba: bool,

    dialogo: Option<(Dialogo, Receiver<Vec<PathBuf>>)>,
    /// El nombre que se está escribiendo para una carpeta propia nueva.
    nueva_carpeta: Option<String>,
    /// Un diálogo que se ha pedido en este fotograma y se abre al final, cuando ya se
    /// tiene la ventana para ponerlo encima.
    dialogo_pedido: Option<Dialogo>,
    /// La clave de la serie a la que van los cómics del diálogo que se ha pedido.
    serie_de_dialogo: Option<String>,
    error: Option<String>,
}

impl VistaBiblioteca {
    /// Abre la biblioteca de `ruta_db` y empieza a escanear sus carpetas en segundo plano.
    /// Los ajustes de las novedades se guardan en `ajustes_novedades` y lo último que se
    /// comprobó, en `cache_novedades`.
    pub fn nueva(
        ctx: &egui::Context,
        ruta_db: PathBuf,
        carpeta_miniaturas: PathBuf,
        ajustes_novedades: PathBuf,
        cache_novedades: PathBuf,
    ) -> Result<VistaBiblioteca, String> {
        let biblioteca = Biblioteca::abrir(&ruta_db)?;
        let novedades = VistaNovedades::nueva(
            ctx,
            &biblioteca,
            ruta_db.clone(),
            ajustes_novedades,
            cache_novedades,
        );
        let nucleos = thread::available_parallelism().map_or(1, |n| n.get());
        let fondo = Fondo::nuevo(carpeta_miniaturas.clone());
        let miniaturas = Miniaturas::nuevo(
            carpeta_miniaturas.clone(),
            nucleos.saturating_sub(1).clamp(1, 4),
            ctx.clone(),
        );

        let mut vista = VistaBiblioteca {
            biblioteca,
            ruta_db,
            carpetas: Vec::new(),
            series: Vec::new(),
            seguir_leyendo: Vec::new(),
            pagina: Pagina::Biblioteca,
            serie: None,
            unir: None,
            busqueda: String::new(),
            novedades,
            ajustes: VistaAjustes::default(),
            renombrando: None,
            arrastrando: None,
            agarre: egui::Vec2::ZERO,
            soltada: None,
            posiciones: HashMap::new(),
            eligiendo_portada: None,
            eligiendo_portada_de_serie: None,
            miniaturas,
            carpeta_miniaturas,
            fondo,
            texturas: HashMap::new(),
            sin_miniatura: Default::default(),
            pedido: Vec::new(),
            fotograma: 0,
            icono: None,
            escaneo: None,
            reubicando: None,
            aviso_carpetas: None,
            reescanear: false,
            ultima_recarga: Instant::now(),
            arriba: false,
            dialogo: None,
            nueva_carpeta: None,
            dialogo_pedido: None,
            serie_de_dialogo: None,
            error: None,
        };
        vista.recargar();
        if !vista.carpetas.is_empty() {
            vista.escanear(ctx);
        }
        Ok(vista)
    }

    pub fn progreso(&self, huella: &str) -> Option<Progreso> {
        self.biblioteca.progreso(huella).unwrap_or_else(|e| {
            eprintln!("[biblioteca] No se pudo leer el progreso: {e}");
            None
        })
    }

    pub fn guardar_progreso(&self, huella: &str, pagina: usize, terminado: bool) {
        if let Err(e) = self.biblioteca.guardar_progreso(huella, pagina, terminado) {
            eprintln!("[biblioteca] No se pudo guardar el progreso: {e}");
        }
    }

    /// El cómic que sigue al de huella `huella` en su serie: su ruta y cómo se llama.
    pub fn siguiente_de(&self, huella: &str) -> Option<(PathBuf, String)> {
        match self.biblioteca.siguiente_de(huella) {
            Ok(ficha) => ficha.map(|f| (f.ruta.clone(), nombre_de_comic(&f))),
            Err(e) => {
                eprintln!("[biblioteca] No se pudo buscar el siguiente cómic: {e}");
                None
            }
        }
    }

    /// Pone la página `pagina` como portada del cómic de huella `huella`: lo que se vea
    /// de él (tarjeta, serie, "Seguir leyendo", fondo) pasa a sacarse de esa página.
    pub fn elegir_portada(&mut self, huella: &str, pagina: usize) {
        if let Err(e) = self.biblioteca.elegir_portada(huella, Some(pagina)) {
            self.mostrar_error(format!("No se pudo guardar la portada: {e}"));
            return;
        }
        eprintln!("[ui] Portada de {huella}: página {}", pagina + 1);
        miniaturas::borrar_en_disco(&self.carpeta_miniaturas, huella);
        self.texturas.retain(|(h, _), _| h != huella);
        self.sin_miniatura.remove(huella);
        self.pedido.clear();
        self.fondo.olvidar();
        self.recargar();
    }

    pub fn mostrar_error(&mut self, error: String) {
        eprintln!("[ui] {error}");
        self.error = Some(error);
    }

    /// Vuelve a leer de la base de datos todo lo que se muestra.
    pub fn recargar(&mut self) {
        let resultado = (|| -> Result<(), String> {
            self.carpetas = self.biblioteca.carpetas()?;
            self.series = self.biblioteca.series()?;
            self.seguir_leyendo = self.biblioteca.seguir_leyendo(MAX_SEGUIR_LEYENDO)?;
            if let Some(abierta) = &mut self.serie {
                // Al unir series, la abierta puede pasar a llamarse de otra forma.
                match self.series.iter().find(|s| s.clave == abierta.clave) {
                    Some(serie) => {
                        abierta.nombre = serie.nombre.clone();
                        abierta.portada = Some(serie.portada.clone());
                        abierta.comics = self.biblioteca.comics_de_serie(&abierta.clave)?;
                    }
                    None => self.serie = None,
                }
            }
            Ok(())
        })();
        if let Err(e) = resultado {
            self.mostrar_error(format!("No se pudo leer la biblioteca: {e}"));
        }
        self.ultima_recarga = Instant::now();
    }

    /// Escanea las carpetas en otro hilo, con su propia conexión a la base de datos.
    fn escanear(&mut self, ctx: &egui::Context) {
        if self.escaneo.is_some() {
            self.reescanear = true;
            return;
        }

        let hechos = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let (enviar, fin) = mpsc::channel();
        let ruta_db = self.ruta_db.clone();
        let ctx = ctx.clone();
        let (h, t) = (Arc::clone(&hechos), Arc::clone(&total));

        thread::spawn(move || {
            let resultado = Biblioteca::abrir(&ruta_db).and_then(|mut biblioteca| {
                biblioteca.escanear(|hechos, total| {
                    h.store(hechos, Ordering::Relaxed);
                    t.store(total, Ordering::Relaxed);
                    if hechos % 10 == 0 || hechos == total {
                        ctx.request_repaint();
                    }
                })
            });
            let _ = enviar.send(resultado);
            ctx.request_repaint();
        });

        self.escaneo = Some(Escaneo { hechos, total, fin });
    }

    /// Mira cómo va el escaneo: recarga de vez en cuando y, al acabar, del todo.
    fn seguir_escaneo(&mut self, ctx: &egui::Context) {
        let Some(escaneo) = &self.escaneo else {
            return;
        };

        match escaneo.fin.try_recv() {
            Ok(resultado) => {
                self.escaneo = None;
                match resultado {
                    Ok(resumen) => eprintln!("[biblioteca] Escaneo terminado: {resumen:?}"),
                    Err(e) => self.mostrar_error(format!("El escaneo ha fallado: {e}")),
                }
                self.recargar();
                if std::mem::take(&mut self.reescanear) {
                    self.escanear(ctx);
                }
            }
            Err(_) => {
                if self.ultima_recarga.elapsed() >= RECARGA_DURANTE_ESCANEO {
                    self.recargar();
                }
                ctx.request_repaint_after(RECARGA_DURANTE_ESCANEO);
            }
        }
    }

    /// Busca en `raiz` los cómics que se han movido de sitio y corrige sus rutas, en otro
    /// hilo y con su propia conexión a la base de datos. Solo se mira dentro de `raiz`.
    fn reubicar_automaticamente(&mut self, ctx: &egui::Context, raiz: PathBuf) {
        if self.reubicando.is_some() || self.escaneo.is_some() {
            return;
        }
        let (enviar, recibir) = mpsc::channel();
        let ruta_db = self.ruta_db.clone();
        let ctx = ctx.clone();
        eprintln!("[biblioteca] Buscando cómics movidos en {}", raiz.display());
        thread::spawn(move || {
            let resultado = Biblioteca::abrir(&ruta_db)
                .and_then(|biblioteca| biblioteca.reubicar_automaticamente(&[raiz]));
            let _ = enviar.send(resultado);
            ctx.request_repaint();
        });
        self.reubicando = Some(recibir);
        self.aviso_carpetas = None;
    }

    /// Recoge el resultado de la búsqueda de cómics movidos, si ya ha terminado.
    fn seguir_reubicacion(&mut self, ctx: &egui::Context) {
        let Some(recibir) = &self.reubicando else {
            return;
        };
        match recibir.try_recv() {
            Ok(resultado) => {
                self.reubicando = None;
                match resultado {
                    Ok(informe) => {
                        eprintln!("[biblioteca] Búsqueda terminada: {informe:?}");
                        self.aviso_carpetas = Some(texto_de_informe(informe));
                        self.recargar();
                        if informe.carpetas + informe.archivos > 0 {
                            self.escanear(ctx);
                        }
                    }
                    Err(e) => self.mostrar_error(format!("No se pudo buscar los cómics: {e}")),
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
            Err(mpsc::TryRecvError::Disconnected) => self.reubicando = None,
        }
    }

    /// Apunta una carpeta de la biblioteca a su nuevo sitio.
    fn reubicar_carpeta(&mut self, ctx: &egui::Context, id: i64, nueva: &Path) {
        match self.biblioteca.reubicar_carpeta(id, nueva) {
            Ok(()) => {
                eprintln!("[biblioteca] Carpeta reubicada en {}", nueva.display());
                self.aviso_carpetas = None;
                self.serie = None;
                self.recargar();
                self.escanear(ctx);
            }
            Err(e) => self.mostrar_error(e),
        }
    }

    fn agregar_carpeta(&mut self, ctx: &egui::Context, ruta: &Path) {
        match self.biblioteca.agregar_carpeta(ruta) {
            Ok(carpeta) => {
                eprintln!("[biblioteca] Carpeta añadida: {}", carpeta.ruta.display());
                self.recargar();
                self.escanear(ctx);
            }
            Err(e) => self.mostrar_error(e),
        }
    }

    fn crear_carpeta_propia(&mut self, nombre: &str) {
        match self.biblioteca.crear_carpeta_propia(nombre) {
            Ok(carpeta) => {
                eprintln!("[biblioteca] Carpeta creada: {}", carpeta.ruta.display());
                self.nueva_carpeta = None;
                self.recargar();
            }
            Err(e) => self.mostrar_error(e),
        }
    }

    fn anadir_archivos(&mut self, ctx: &egui::Context, carpeta: i64, archivos: &[PathBuf]) {
        if archivos.is_empty() {
            return;
        }
        match self.biblioteca.anadir_archivos(carpeta, archivos) {
            Ok(anadidos) => {
                eprintln!("[biblioteca] {anadidos} cómics añadidos a la carpeta");
                self.recargar();
                if anadidos > 0 {
                    self.escanear(ctx);
                }
            }
            Err(e) => self.mostrar_error(e),
        }
    }

    /// Mete en la serie `clave` los cómics y carpetas elegidos.
    fn anadir_a_serie(&mut self, ctx: &egui::Context, clave: &str, rutas: &[PathBuf]) {
        if rutas.is_empty() {
            return;
        }
        match self.biblioteca.anadir_a_serie(clave, rutas) {
            Ok(anadidos) => {
                eprintln!("[biblioteca] {anadidos} cómics añadidos a la serie {clave}");
                self.recargar();
                if anadidos > 0 {
                    self.escanear(ctx);
                }
            }
            Err(e) => self.mostrar_error(e),
        }
    }

    fn quitar_carpeta(&mut self, id: i64) {
        if let Err(e) = self.biblioteca.quitar_carpeta(id) {
            self.mostrar_error(format!("No se pudo quitar la carpeta: {e}"));
        }
        self.serie = None;
        self.recargar();
    }

    fn abrir_serie(&mut self, nombre: String) {
        let Some(clave) = self
            .series
            .iter()
            .find(|s| s.nombre == nombre)
            .map(|s| s.clave.clone())
        else {
            return;
        };
        let portada = self
            .series
            .iter()
            .find(|s| s.clave == clave)
            .map(|s| s.portada.clone());
        match self.biblioteca.comics_de_serie(&clave) {
            Ok(comics) => {
                self.serie = Some(SerieAbierta {
                    nombre,
                    clave,
                    comics,
                    portada,
                });
                self.arriba = true;
            }
            Err(e) => self.mostrar_error(format!("No se pudo leer la serie: {e}")),
        }
    }

    /// Une la serie abierta con `otra` y pasa a mostrar la serie resultante.
    fn unir_con(&mut self, otra_clave: &str) {
        let Some(abierta) = &self.serie else {
            return;
        };
        match self.biblioteca.unir_series(&abierta.clave, otra_clave) {
            Ok(clave) => {
                eprintln!(
                    "[biblioteca] Series unidas: {} y {otra_clave}",
                    abierta.nombre
                );
                if let Some(abierta) = &mut self.serie {
                    abierta.clave = clave;
                }
                self.recargar();
            }
            Err(e) => self.mostrar_error(e),
        }
    }

    /// Empieza a renombrar `objetivo`, con `actual` ya escrito en el campo.
    fn empezar_a_renombrar(&mut self, objetivo: Renombrable, actual: &str) {
        self.renombrando = Some(Renombrando {
            objetivo,
            texto: actual.to_string(),
            foco_pedido: false,
        });
    }

    /// Guarda lo que se ha escrito y deja de editar. Vacío vuelve al nombre automático.
    fn confirmar_renombrar(&mut self) {
        let Some(renombrando) = self.renombrando.take() else {
            return;
        };
        let resultado = match &renombrando.objetivo {
            Renombrable::Serie(clave) => self.biblioteca.renombrar_serie(clave, &renombrando.texto),
            Renombrable::Comic(huella) => {
                self.biblioteca.renombrar_comic(huella, &renombrando.texto)
            }
        };
        match resultado {
            Ok(()) => self.recargar(),
            Err(e) => self.mostrar_error(format!("No se pudo renombrar: {e}")),
        }
    }

    /// Quita `huella` de la colección (el archivo no se toca; no vuelve sola en el
    /// siguiente escaneo).
    fn eliminar_comic(&mut self, huella: &str) {
        match self.biblioteca.quitar_comic(huella) {
            Ok(()) => self.recargar(),
            Err(e) => self.mostrar_error(format!("No se pudo eliminar: {e}")),
        }
    }

    /// Mueve `huella` dentro de la serie abierta; la serie queda con el orden puesto a mano.
    fn mover_comic(&mut self, huella: &str, movimiento: Movimiento) {
        let Some(clave) = self.serie.as_ref().map(|s| s.clave.clone()) else {
            return;
        };
        match self.biblioteca.mover_comic(&clave, huella, movimiento) {
            Ok(()) => self.recargar(),
            Err(e) => self.mostrar_error(format!("No se pudo mover el cómic: {e}")),
        }
    }

    /// Devuelve la serie `clave` al orden de lectura de siempre.
    fn restablecer_orden(&mut self, clave: &str) {
        match self.biblioteca.restablecer_orden(clave) {
            Ok(()) => self.recargar(),
            Err(e) => self.mostrar_error(format!("No se pudo restablecer el orden: {e}")),
        }
    }

    /// El campo de edición de un nombre que se está renombrando, en el sitio donde iría
    /// el texto. Con Enter se guarda; pulsando fuera o con Esc, se deja como estaba.
    fn campo_de_renombrar(&mut self, ui: &mut egui::Ui, c: Colores, rect: Rect) {
        let renombrando = self
            .renombrando
            .as_mut()
            .expect("solo se llama mientras se está renombrando algo");
        let respuesta = ui.put(
            rect,
            egui::TextEdit::singleline(&mut renombrando.texto)
                .font(FontId::proportional(13.5))
                .text_color(c.texto)
                .margin(egui::vec2(4.0, 2.0)),
        );
        if !renombrando.foco_pedido {
            respuesta.request_focus();
            renombrando.foco_pedido = true;
        }
        if respuesta.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.renombrando = None;
        } else if respuesta.lost_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.confirmar_renombrar();
            } else {
                // Se ha pulsado fuera, o se ha cambiado de ventana con el tabulador:
                // se deja como estaba, no se guarda.
                self.renombrando = None;
            }
        }
    }

    /// El menú del botón derecho para renombrar `objetivo` sin tocar los archivos, y
    /// para quitar `huella` de la colección si es un cómic. `actual` es el texto con el
    /// que se abre el campo al elegir "Renombrar".
    fn menu_de_renombrar(
        &mut self,
        respuesta: &egui::Response,
        objetivo: Renombrable,
        actual: &str,
    ) {
        let actual = actual.to_string();
        // Para elegir la portada de un cómic hace falta su ficha (dónde está y cuántas
        // páginas tiene): sale de la serie abierta, que es donde se ven los cómics sueltos.
        let ficha = match &objetivo {
            Renombrable::Comic(huella) => self
                .serie
                .as_ref()
                .and_then(|s| s.comics.iter().find(|f| &f.huella == huella))
                .filter(|f| f.error.is_none() && f.paginas > 0)
                .cloned(),
            Renombrable::Serie(_) => None,
        };
        // Dónde está el cómic dentro de la serie abierta, para saber qué movimientos caben.
        let sitio = match (&objetivo, &self.serie) {
            (Renombrable::Comic(huella), Some(abierta)) => abierta
                .comics
                .iter()
                .position(|f| &f.huella == huella)
                .map(|i| (i, abierta.comics.len())),
            _ => None,
        };
        respuesta.context_menu(|ui| {
            ui.set_min_width(180.0);
            // Dónde está el submenú, si está abierto: alejarse del menú cuenta ambos.
            let mut zona_submenu: Option<Rect> = None;
            if let (Renombrable::Comic(huella), Some((i, total))) = (&objetivo, sitio) {
                let movimientos = [
                    ("Mover al principio", Movimiento::Principio, i > 0),
                    ("Mover antes", Movimiento::Antes, i > 0),
                    ("Mover después", Movimiento::Despues, i + 1 < total),
                    ("Mover al final", Movimiento::Final, i + 1 < total),
                ];
                let mover = ui.menu_button("Mover", |ui| {
                    ui.set_min_width(160.0);
                    for (texto, movimiento, cabe) in movimientos {
                        if ui.add_enabled(cabe, egui::Button::new(texto)).clicked() {
                            self.mover_comic(huella, movimiento);
                            ui.close();
                        }
                    }
                    zona_submenu = Some(ui.min_rect());
                });
                if let Some(zona) = zona_submenu {
                    estilo::cerrar_submenu_al_alejarse(ui, zona, mover.response.rect);
                }
                ui.separator();
            }
            if ui.button("Renombrar").clicked() {
                self.empezar_a_renombrar(objetivo.clone(), &actual);
                ui.close();
            }
            if let Some(ficha) = &ficha {
                if ui.button("Cambiar portada…").clicked() {
                    self.eligiendo_portada =
                        Some(ElegirPortada::nuevo(ficha, nombre_de_comic(ficha)));
                    ui.close();
                }
            }
            if let Renombrable::Serie(clave) = &objetivo {
                if ui.button("Cambiar portada…").clicked() {
                    self.abrir_elegir_portada_de_serie(clave);
                    ui.close();
                }
                if ui.button("Añadir cómics…").clicked() {
                    self.pedir_dialogo_de_serie(Dialogo::ComicsParaSerie, clave);
                    ui.close();
                }
                if ui.button("Añadir carpeta…").clicked() {
                    self.pedir_dialogo_de_serie(Dialogo::CarpetasParaSerie, clave);
                    ui.close();
                }
                let leida = self
                    .series
                    .iter()
                    .find(|s| &s.clave == clave)
                    .is_some_and(|s| s.comics > 0 && s.terminados == s.comics);
                let texto = if leida {
                    "Marcar como no leída"
                } else {
                    "Marcar como leída"
                };
                if ui.button(texto).clicked() {
                    self.marcar_serie_leida(clave, !leida);
                    ui.close();
                }
                if self.biblioteca.tiene_orden_manual(clave).unwrap_or(false)
                    && ui.button("Restablecer orden").clicked()
                {
                    self.restablecer_orden(clave);
                    ui.close();
                }
            }
            if let Renombrable::Comic(huella) = &objetivo {
                let leido = self
                    .biblioteca
                    .progreso(huella)
                    .ok()
                    .flatten()
                    .is_some_and(|p| p.terminado);
                let texto = if leido {
                    "Marcar como no leído"
                } else {
                    "Marcar como leído"
                };
                if ui.button(texto).clicked() {
                    self.marcar_leido(huella, !leido);
                    ui.close();
                }
                // Apartado del resto, para no darle sin querer.
                ui.separator();
                if ui.button("Eliminar").clicked() {
                    self.eliminar_comic(huella);
                    ui.close();
                }
            }

            estilo::cerrar_menu_al_alejarse(ui, zona_submenu);
        });
    }

    fn marcar_leido(&mut self, huella: &str, leido: bool) {
        match self.biblioteca.marcar_leido(huella, leido) {
            Ok(()) => self.recargar(),
            Err(e) => self.mostrar_error(format!("No se pudo cambiar el estado: {e}")),
        }
    }

    fn marcar_serie_leida(&mut self, clave: &str, leido: bool) {
        match self.biblioteca.marcar_serie_leida(clave, leido) {
            Ok(_) => self.recargar(),
            Err(e) => self.mostrar_error(format!("No se pudo cambiar el estado: {e}")),
        }
    }

    /// Pide un diálogo para elegir cómics o carpetas que van a la serie `clave`.
    fn pedir_dialogo_de_serie(&mut self, dialogo: Dialogo, clave: &str) {
        self.dialogo_pedido = Some(dialogo);
        self.serie_de_dialogo = Some(clave.to_string());
    }

    /// Un título con menú del botón derecho para renombrarlo: mientras no se edite, el
    /// texto tal cual; al elegir "Renombrar", el campo para escribir el nuevo nombre en
    /// su sitio. No hace falta botón: el propio texto lleva el menú.
    fn titulo_renombrable(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        posicion: egui::Pos2,
        ancho: f32,
        texto: &str,
        objetivo: Renombrable,
    ) -> egui::Vec2 {
        let editando = self
            .renombrando
            .as_ref()
            .is_some_and(|r| r.objetivo == objetivo);
        if editando {
            let alto = 34.0;
            self.campo_de_renombrar(
                ui,
                c,
                Rect::from_min_size(posicion, egui::vec2(ancho, alto)),
            );
            return egui::vec2(ancho, alto);
        }

        let galeria = texto_ajustado(ui, texto, estilo::negrita(24.0), c.texto, ancho, 2);
        let tamano = galeria.size();
        let rect = Rect::from_min_size(posicion, tamano);
        let respuesta = ui
            .interact(
                rect,
                ui.id().with(("renombrar", &objetivo)),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::ContextMenu);
        ui.painter().galley(posicion, galeria, c.texto);
        self.menu_de_renombrar(&respuesta, objetivo, texto);
        tamano
    }

    /// Dibuja la pantalla. Devuelve el cómic que hay que abrir, si se ha elegido uno.
    pub fn mostrar(
        &mut self,
        ui: &mut egui::Ui,
        ventana: &eframe::Frame,
        preferencias: &mut Preferencias,
    ) -> Option<PathBuf> {
        let ctx = ui.ctx().clone();
        self.fotograma += 1;
        self.seguir_escaneo(&ctx);
        self.seguir_reubicacion(&ctx);
        self.novedades.seguir(&ctx, &self.biblioteca);
        self.recibir_miniaturas(&ctx);
        let leyendo = self.seguir_leyendo.first().map(Ficha::portada);
        self.fondo.pedir(&ctx, leyendo.as_ref());

        // Esc mientras se arrastra un cómic cancela el arrastre, no cierra la serie.
        let cancelar_arrastre =
            self.arrastrando.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if cancelar_arrastre {
            // La tarjeta vuelve volando a su sitio.
            self.soltada = self.arrastrando.take();
        }
        if !cancelar_arrastre
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
            && self.serie.is_some()
            && self.unir.is_none()
            && self.eligiendo_portada.is_none()
            && self.eligiendo_portada_de_serie.is_none()
        {
            self.serie = None;
        }

        let c = estilo::colores(&ctx);
        egui::Panel::left("lateral")
            .resizable(false)
            .exact_size(ANCHO_LATERAL)
            .frame(
                egui::Frame::NONE
                    .fill(c.lateral)
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show(ui, |ui| self.barra_lateral(ui, c));

        let pulsada = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(c.lateral))
            .show(ui, |ui| {
                // La zona de contenido, algo más clara, separada de la barra lateral por
                // una línea.
                let zona = ui.max_rect();
                let pintor = ui.painter();
                pintor.rect_filled(zona, 0, c.contenido);
                self.fondo.pintar(ui, zona);
                let pintor = ui.painter();
                let x = pintor.round_to_pixel_center(zona.min.x);
                pintor.vline(x, zona.y_range(), Stroke::new(1.0, c.borde));
                let pulsada = self.contenido(ui, c, preferencias);
                self.arriba = false;
                pulsada
            })
            .inner;

        self.ventana_unir(&ctx);
        self.ventana_portada(&ctx);
        self.ventana_error(&ctx);
        self.soltar_texturas_viejas();

        let mut abrir = match pulsada {
            Some(Pulsar::Serie(nombre)) => {
                self.abrir_serie(nombre);
                None
            }
            Some(Pulsar::Comic(ruta)) => Some(ruta),
            None => None,
        };
        abrir = abrir.or(self.atender_dialogos(&ctx, ventana));
        abrir
    }

    /// La ventana para elegir la portada de un cómic, si está abierta.
    fn ventana_portada(&mut self, ctx: &egui::Context) {
        if let Some(ventana) = &mut self.eligiendo_portada {
            match ventana.mostrar(ctx) {
                portada::Resultado::Abierta => {}
                portada::Resultado::Cerrada => self.eligiendo_portada = None,
                portada::Resultado::Elegida(pagina) => {
                    let huella = ventana.huella.clone();
                    self.eligiendo_portada = None;
                    self.elegir_portada(&huella, pagina);
                }
            }
        }
        if let Some(ventana) = &mut self.eligiendo_portada_de_serie {
            match ventana.mostrar(ctx) {
                portada::Resultado::Abierta => {}
                portada::Resultado::Cerrada => self.eligiendo_portada_de_serie = None,
                portada::Resultado::Elegida(i) => {
                    let (clave, huella) = (ventana.clave.clone(), ventana.huella_de(i).to_string());
                    self.eligiendo_portada_de_serie = None;
                    self.elegir_portada_de_serie(&clave, &huella);
                }
            }
        }
    }

    /// Abre la ventana para elegir la portada de la serie `clave` entre las de sus cómics.
    fn abrir_elegir_portada_de_serie(&mut self, clave: &str) {
        let Some((nombre, actual)) = self
            .series
            .iter()
            .find(|s| s.clave == clave)
            .map(|s| (s.nombre.clone(), s.portada.huella.clone()))
        else {
            return;
        };
        match self.biblioteca.comics_de_serie(clave) {
            Ok(comics) => {
                let comics = comics
                    .iter()
                    .filter(|f| f.error.is_none())
                    .map(|f| (f.portada(), nombre_de_comic(f)))
                    .collect();
                self.eligiendo_portada_de_serie = Some(ElegirPortadaDeSerie::nuevo(
                    clave.to_string(),
                    nombre,
                    comics,
                    actual,
                    self.carpeta_miniaturas.clone(),
                ));
            }
            Err(e) => self.mostrar_error(format!("No se pudo leer la serie: {e}")),
        }
    }

    /// Hace que la portada de la serie `clave` sea la del cómic de huella `huella`.
    fn elegir_portada_de_serie(&mut self, clave: &str, huella: &str) {
        match self.biblioteca.elegir_portada_de_serie(clave, Some(huella)) {
            Ok(()) => {
                eprintln!("[ui] Portada de la serie {clave}: la de {huella}");
                self.recargar();
            }
            Err(e) => self.mostrar_error(format!("No se pudo guardar la portada: {e}")),
        }
    }

    /// Abre el diálogo que se haya pedido y recoge lo que se elija en él.
    fn atender_dialogos(
        &mut self,
        ctx: &egui::Context,
        ventana: &eframe::Frame,
    ) -> Option<PathBuf> {
        if let Some(dialogo) = self.dialogo_pedido.take() {
            // Uno cada vez: en Linux el anterior puede seguir abierto.
            if self.dialogo.is_none() {
                self.dialogo = Some((dialogo, dialogos::abrir(dialogo, ventana)));
            }
        }

        let (dialogo, respuesta) = self.dialogo.as_ref()?;
        let elegidos = match respuesta.try_recv() {
            Ok(elegidos) => elegidos,
            Err(mpsc::TryRecvError::Empty) => {
                // En Linux llega desde otro hilo: hay que seguir mirando.
                ctx.request_repaint_after(Duration::from_millis(100));
                return None;
            }
            Err(mpsc::TryRecvError::Disconnected) => Vec::new(),
        };
        let dialogo = *dialogo;
        self.dialogo = None;

        match dialogo {
            Dialogo::CarpetaDeBiblioteca => {
                if let Some(ruta) = elegidos.first() {
                    self.agregar_carpeta(ctx, ruta);
                }
                None
            }
            Dialogo::Comic | Dialogo::CarpetaDeImagenes => elegidos.into_iter().next(),
            Dialogo::ComicsParaCarpeta(carpeta) => {
                self.anadir_archivos(ctx, carpeta, &elegidos);
                None
            }
            Dialogo::ComicsParaSerie | Dialogo::CarpetasParaSerie => {
                if let Some(clave) = self.serie_de_dialogo.take() {
                    self.anadir_a_serie(ctx, &clave, &elegidos);
                }
                None
            }
            Dialogo::DondeBuscar => {
                if let Some(raiz) = elegidos.into_iter().next() {
                    self.reubicar_automaticamente(ctx, raiz);
                }
                None
            }
            Dialogo::NuevaUbicacion(id) => {
                if let Some(nueva) = elegidos.first() {
                    self.reubicar_carpeta(ctx, id, nueva);
                }
                None
            }
        }
    }

    /// El icono de la app, encogido con Lanczos3 a su tamaño exacto en píxeles.
    fn icono(&mut self, ctx: &egui::Context, lado: f32) -> Option<egui::TextureHandle> {
        let ppp = ctx.pixels_per_point();
        if let Some((hecho_para, textura)) = &self.icono {
            if *hecho_para == ppp {
                return Some(textura.clone());
            }
        }
        let original =
            comic_imagen::decodificar(include_bytes!("../../../assets/icono.png")).ok()?;
        let px = (lado * ppp).round() as u32;
        let imagen = comic_imagen::Encogedor::default()
            .encoger(original, px, px)
            .ok()?;
        let textura = ctx.load_texture(
            "icono",
            egui::ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &imagen.rgba),
            egui::TextureOptions::LINEAR,
        );
        self.icono = Some((ppp, textura.clone()));
        Some(textura)
    }

    fn barra_lateral(&mut self, ui: &mut egui::Ui, c: Colores) {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if let Some(icono) = self.icono(ui.ctx(), 20.0) {
                ui.add(egui::Image::new(&icono).fit_to_exact_size(egui::vec2(20.0, 20.0)));
            }
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Lector de cómics")
                    .font(estilo::negrita(14.0))
                    .color(c.texto),
            );
        });
        ui.add_space(16.0);

        let buscador = ui.add(
            egui::TextEdit::singleline(&mut self.busqueda)
                .hint_text("Buscar serie")
                .desired_width(f32::INFINITY)
                .margin(egui::vec2(10.0, 7.0)),
        );
        if buscador.changed() {
            self.pagina = Pagina::Biblioteca;
            self.serie = None;
        }
        ui.add_space(14.0);

        let leyendo = self.seguir_leyendo.len();
        let novedades = self.novedades.con_novedades(&self.series);
        let paginas = [
            (Pagina::Biblioteca, "Biblioteca", None),
            (Pagina::Leyendo, "Leyendo", Some(leyendo).filter(|&n| n > 0)),
            (
                Pagina::Novedades,
                "Novedades",
                Some(novedades).filter(|&n| n > 0),
            ),
        ];
        for (pagina, nombre, contador) in paginas {
            if elemento_lateral(ui, c, nombre, contador, self.pagina == pagina).clicked() {
                self.ir_a(ui.ctx(), pagina);
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(14.0);
            // Estado del escaneo o, si no se está escaneando, lo que hay.
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                let texto = match &self.escaneo {
                    Some(escaneo) => {
                        ui.spinner();
                        let hechos = escaneo.hechos.load(Ordering::Relaxed);
                        let total = escaneo.total.load(Ordering::Relaxed);
                        if total > 0 {
                            format!("Escaneando {hechos} de {total}")
                        } else {
                            "Buscando cómics".to_string()
                        }
                    }
                    None => {
                        let comics: usize = self.series.iter().map(|s| s.comics).sum();
                        format!(
                            "{} {}, {} {}",
                            self.series.len(),
                            plural(self.series.len(), "serie", "series"),
                            comics,
                            plural(comics, "cómic", "cómics"),
                        )
                    }
                };
                ui.label(
                    egui::RichText::new(texto)
                        .size(12.0)
                        .color(c.texto_secundario),
                );
            });
            ui.add_space(8.0);

            let oscuro = ui.ctx().theme() == egui::Theme::Dark;
            let tema = if oscuro { "Tema claro" } else { "Tema oscuro" };
            if elemento_lateral(ui, c, tema, None, false).clicked() {
                ui.ctx().set_theme(if oscuro {
                    egui::Theme::Light
                } else {
                    egui::Theme::Dark
                });
            }
            let abrir = elemento_lateral(ui, c, "Abrir un cómic…", None, false)
                .on_hover_text("Leer un cómic sin añadirlo a la biblioteca");
            if abrir.clicked() {
                self.dialogo_pedido = Some(Dialogo::Comic);
            }
            let carpetas = self.pagina == Pagina::Carpetas;
            if elemento_lateral(ui, c, "Carpetas", None, carpetas).clicked() {
                self.ir_a(ui.ctx(), Pagina::Carpetas);
            }
            let ajustes = self.pagina == Pagina::Ajustes;
            if elemento_lateral(ui, c, "Ajustes", None, ajustes).clicked() {
                self.ir_a(ui.ctx(), Pagina::Ajustes);
            }
        });
    }

    /// Para el modo de captura: va a `paso` ("biblioteca", "leyendo", "carpetas" o
    /// "novedades", con la pestaña detrás: "novedades:manga"). Con "lector" devuelve el
    /// cómic que se está leyendo, para abrirlo.
    pub(crate) fn ir_a_para_captura(&mut self, ctx: &egui::Context, paso: &str) -> Option<PathBuf> {
        if paso == "lector" {
            return self.seguir_leyendo.first().map(|f| f.ruta.clone());
        }
        if paso == "serie" {
            if let Some(nombre) = self.series.first().map(|s| s.nombre.clone()) {
                self.pagina = Pagina::Biblioteca;
                self.abrir_serie(nombre);
            }
            return None;
        }
        let (pagina, pestana) = match paso.split_once(':') {
            Some((pagina, pestana)) => (pagina, pestana),
            None => (paso, ""),
        };
        let pagina = match pagina {
            "leyendo" => Pagina::Leyendo,
            "novedades" => Pagina::Novedades,
            "carpetas" => Pagina::Carpetas,
            "ajustes" => Pagina::Ajustes,
            _ => Pagina::Biblioteca,
        };
        self.ir_a(ctx, pagina);
        if pagina == Pagina::Novedades {
            self.novedades.para_captura(ctx, pestana);
        }
        None
    }

    /// Cambia de página desde la barra lateral. La ficha de una serie cuelga de la página
    /// desde la que se abrió, así que se cierra.
    fn ir_a(&mut self, ctx: &egui::Context, pagina: Pagina) {
        if pagina == Pagina::Novedades && self.pagina != pagina {
            self.novedades.al_entrar(ctx, &self.biblioteca);
        }
        self.pagina = pagina;
        self.serie = None;
        self.arriba = true;
    }

    /// Lo que va en la zona de contenido según la página.
    fn contenido(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        preferencias: &mut Preferencias,
    ) -> Option<Pulsar> {
        if self.pagina == Pagina::Ajustes {
            self.ajustes.mostrar(ui, c, preferencias, self.arriba);
            return None;
        }
        if self.pagina == Pagina::Carpetas && self.serie.is_none() {
            self.pagina_carpetas(ui, c);
            return None;
        }
        if self.pagina == Pagina::Novedades && self.serie.is_none() {
            return self
                .novedades
                .mostrar(ui, c, &self.biblioteca, &self.series, self.arriba)
                .map(Pulsar::Serie);
        }
        if self.carpetas.is_empty() {
            self.biblioteca_vacia(ui, c);
            return None;
        }

        if let Some(abierta) = &self.serie {
            let cabecera = Cabecera::Serie(datos_de_serie(abierta));
            let tarjetas = abierta.comics.iter().map(tarjeta_de_comic).collect();
            return self.cuadricula(ui, cabecera, None, Some("Cómics".into()), tarjetas, None);
        }

        if self.pagina == Pagina::Leyendo {
            let n = self.seguir_leyendo.len();
            let cabecera = Cabecera::Titulo {
                titulo: "Leyendo".into(),
                detalle: format!("{n} {} sin terminar", plural(n, "cómic", "cómics")),
            };
            let tarjetas = self.seguir_leyendo.iter().map(tarjeta_de_seguir).collect();
            let vacio = "Todavía no has empezado ningún cómic.".to_string();
            return self.cuadricula(ui, cabecera, None, None, tarjetas, Some(vacio));
        }

        let busqueda = self.busqueda.trim().to_lowercase();
        let series: Vec<Tarjeta> = self
            .series
            .iter()
            .filter(|s| busqueda.is_empty() || s.nombre.to_lowercase().contains(&busqueda))
            .map(tarjeta_de_serie)
            .collect();

        // Al buscar, lo que importa es lo que se busca: sin "Seguir leyendo".
        let (destacado, seccion) = if busqueda.is_empty() {
            let destacado = self.seguir_leyendo.first().cloned().map(Box::new);
            let resto: Vec<Tarjeta> = self
                .seguir_leyendo
                .iter()
                .skip(1)
                .map(tarjeta_de_seguir)
                .collect();
            let seccion = (!resto.is_empty()).then(|| ("También estás leyendo".to_string(), resto));
            (destacado, seccion)
        } else {
            (None, None)
        };

        let comics: usize = self.series.iter().map(|s| s.comics).sum();
        let cabecera = Cabecera::Inicio {
            detalle: format!(
                "{} {}, {} {}",
                self.series.len(),
                plural(self.series.len(), "serie", "series"),
                comics,
                plural(comics, "cómic", "cómics")
            ),
            destacado,
        };
        let vacio = if busqueda.is_empty() {
            "Todavía no hay cómics. Si se está escaneando, irán apareciendo aquí.".to_string()
        } else {
            format!("Ninguna serie contiene «{}».", self.busqueda.trim())
        };
        self.cuadricula(
            ui,
            cabecera,
            seccion,
            Some("Series".into()),
            series,
            Some(vacio),
        )
    }

    fn biblioteca_vacia(&mut self, ui: &mut egui::Ui, c: Colores) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.label(estilo::titulo("Tu biblioteca está vacía", 24.0, c.texto));
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Añade la carpeta donde guardas tus cómics.").color(c.texto),
            );
            ui.label(
                egui::RichText::new(
                    "La biblioteca solo los lee: nunca mueve ni cambia tus archivos.",
                )
                .color(c.texto_secundario),
            );
            ui.add_space(20.0);
            if ui.add(boton_subrayado(c, "Añadir carpeta")).clicked() {
                self.dialogo_pedido = Some(Dialogo::CarpetaDeBiblioteca);
            }
        });
    }

    fn pagina_carpetas(&mut self, ui: &mut egui::Ui, c: Colores) {
        let escaneando = self.escaneo.is_some();
        let reubicando = self.reubicando.is_some();
        let hay_perdidas = self
            .carpetas
            .iter()
            .any(|c| c.perdida || c.archivos_perdidos > 0);
        let mut buscar_movidos = false;
        let mut reubicar = None;
        let mut quitar = None;
        let mut reescanear = false;
        let mut anadir_a = None;
        let mut crear = None;
        let mut cancelar_nueva = false;

        area_de_desplazamiento(self.arriba).show(ui, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::same(MARGEN as i8))
                .show(ui, |ui| {
                    ui.set_max_width(760.0);
                    ui.label(estilo::titulo("Carpetas", 24.0, c.texto));
                    ui.label(
                        egui::RichText::new(
                            "La biblioteca solo lee estas carpetas: nunca mueve ni cambia \
                                 tus archivos.",
                        )
                        .color(c.texto_secundario),
                    );
                    ui.add_space(18.0);

                    if hay_perdidas || reubicando || self.aviso_carpetas.is_some() {
                        panel(c).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if hay_perdidas {
                                ui.label(
                                    egui::RichText::new(
                                        "Algunos cómics ya no están donde la biblioteca los \
                                         guardó, quizá porque has movido sus carpetas.",
                                    )
                                    .color(c.texto),
                                );
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    let boton = ui
                                        .add_enabled(
                                            !escaneando && !reubicando,
                                            boton_subrayado(c, "Buscar automáticamente…"),
                                        )
                                        .on_hover_text(
                                            "Eliges una carpeta y se busca solo dentro de \
                                             ella, y de sus subcarpetas. Cada cómic se \
                                             reconoce por su contenido, aunque haya cambiado \
                                             de nombre; tus archivos no se tocan.",
                                        )
                                        .on_disabled_hover_text("Espera a que termine");
                                    if boton.clicked() {
                                        buscar_movidos = true;
                                    }
                                    if reubicando {
                                        ui.spinner();
                                        ui.label(
                                            egui::RichText::new("Buscando…")
                                                .color(c.texto_secundario),
                                        );
                                    }
                                });
                            } else if reubicando {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(
                                        egui::RichText::new("Buscando…").color(c.texto_secundario),
                                    );
                                });
                            }
                            if let Some(aviso) = &self.aviso_carpetas {
                                if hay_perdidas {
                                    ui.add_space(8.0);
                                }
                                ui.label(egui::RichText::new(aviso).color(c.texto_secundario));
                            }
                        });
                        ui.add_space(12.0);
                    }

                    if self.carpetas.is_empty() {
                        ui.label(
                            egui::RichText::new("Todavía no hay ninguna.")
                                .color(c.texto_secundario),
                        );
                    }
                    for carpeta in &self.carpetas {
                        panel(c).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                // La ruta se recorta con "…" para no pisar los botones;
                                // entera, al pasar el ratón.
                                let ruta = carpeta.ruta.display().to_string();
                                let botones = match (carpeta.propia, carpeta.perdida) {
                                    (true, _) => 330.0,
                                    (false, true) => 200.0,
                                    (false, false) => 90.0,
                                };
                                let ancho = (ui.available_width() - botones).max(60.0);
                                ui.scope(|ui| {
                                    ui.set_max_width(ancho);
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(&ruta).color(c.texto))
                                            .truncate(),
                                    )
                                    .on_hover_text(&ruta);
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let boton = ui
                                            .add_enabled(!escaneando, boton_texto(c, "Quitar"))
                                            .on_hover_text(
                                                "Quita sus cómics de la biblioteca. Los \
                                                     archivos no se tocan, y si la vuelves a \
                                                     añadir, cada cómic sigue por donde ibas.",
                                            )
                                            .on_disabled_hover_text(
                                                "Espera a que termine el escaneo",
                                            );
                                        if boton.clicked() {
                                            quitar = Some(carpeta.id);
                                        }
                                        if carpeta.perdida {
                                            let boton = ui
                                                .add_enabled(
                                                    !escaneando && !reubicando,
                                                    boton_texto(c, "Reubicar…"),
                                                )
                                                .on_hover_text(
                                                    "Elige dónde está ahora esta carpeta. Los \
                                                     cómics conservan su progreso.",
                                                )
                                                .on_disabled_hover_text("Espera a que termine");
                                            if boton.clicked() {
                                                reubicar = Some(carpeta.id);
                                            }
                                        }
                                        if carpeta.propia {
                                            if ui.add(boton_texto(c, "Añadir cómics…")).clicked()
                                            {
                                                anadir_a = Some(carpeta.id);
                                            }
                                            let cuantos = match carpeta.comics {
                                                0 => "Vacía".to_string(),
                                                n => {
                                                    format!("{n} {}", plural(n, "cómic", "cómics"))
                                                }
                                            };
                                            ui.label(
                                                egui::RichText::new(cuantos)
                                                    .color(c.texto_secundario),
                                            );
                                        }
                                    },
                                );
                            });
                            let aviso = if carpeta.perdida {
                                Some("No se encuentra en el disco.".to_string())
                            } else if carpeta.archivos_perdidos > 0 {
                                let n = carpeta.archivos_perdidos;
                                Some(format!(
                                    "{n} {} no se {} en el disco.",
                                    plural(n, "cómic", "cómics"),
                                    plural(n, "encuentra", "encuentran"),
                                ))
                            } else {
                                None
                            };
                            if let Some(aviso) = aviso {
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new(aviso).color(c.acento));
                            }
                        });
                        ui.add_space(6.0);
                    }

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.add(boton_subrayado(c, "Añadir carpeta")).clicked() {
                            self.dialogo_pedido = Some(Dialogo::CarpetaDeBiblioteca);
                        }
                        if ui.add(boton_texto(c, "Nueva carpeta")).clicked() {
                            self.nueva_carpeta.get_or_insert_with(String::new);
                        }
                        let boton = ui
                            .add_enabled(
                                !escaneando && !reubicando,
                                boton_texto(c, "Volver a escanear"),
                            )
                            .on_hover_text("Buscar cómics nuevos, cambiados o borrados");
                        if boton.clicked() {
                            reescanear = true;
                        }
                    });

                    if let Some(nombre) = &mut self.nueva_carpeta {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let campo = ui.add(
                                egui::TextEdit::singleline(nombre)
                                    .hint_text("Nombre de la carpeta")
                                    .desired_width(260.0)
                                    .margin(egui::vec2(10.0, 7.0)),
                            );
                            if !campo.has_focus() && !campo.lost_focus() {
                                campo.request_focus();
                            }
                            let intro =
                                campo.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let valido = !nombre.trim().is_empty();
                            let boton = ui.add_enabled(valido, boton_texto(c, "Crear"));
                            if boton.clicked() || (intro && valido) {
                                crear = Some(nombre.clone());
                            }
                            if ui.add(boton_texto(c, "Cancelar")).clicked()
                                || ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                cancelar_nueva = true;
                            }
                        });
                    }

                    if let Some(escaneo) = &self.escaneo {
                        ui.add_space(12.0);
                        let hechos = escaneo.hechos.load(Ordering::Relaxed);
                        let total = escaneo.total.load(Ordering::Relaxed).max(1);
                        ui.add(
                            egui::ProgressBar::new(hechos as f32 / total as f32)
                                .text(format!("Escaneando {hechos} de {total}"))
                                .fill(c.acento)
                                .corner_radius(estilo::RADIO_ETIQUETA),
                        );
                    }

                    ui.add_space(32.0);
                    ui.label(
                        egui::RichText::new("Leer sin añadir a la biblioteca")
                            .font(estilo::negrita(16.0))
                            .color(c.texto),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add(boton_texto(c, "Abrir un cómic…")).clicked() {
                            self.dialogo_pedido = Some(Dialogo::Comic);
                        }
                        if ui
                            .add(boton_texto(c, "Abrir una carpeta de imágenes…"))
                            .clicked()
                        {
                            self.dialogo_pedido = Some(Dialogo::CarpetaDeImagenes);
                        }
                    });
                });
        });

        if let Some(id) = quitar {
            self.quitar_carpeta(id);
        }
        if buscar_movidos {
            self.dialogo_pedido = Some(Dialogo::DondeBuscar);
        }
        if let Some(id) = reubicar {
            self.dialogo_pedido = Some(Dialogo::NuevaUbicacion(id));
        }
        if let Some(id) = anadir_a {
            self.dialogo_pedido = Some(Dialogo::ComicsParaCarpeta(id));
        }
        if let Some(nombre) = crear {
            self.crear_carpeta_propia(&nombre);
        }
        if cancelar_nueva {
            self.nueva_carpeta = None;
        }
        if reescanear {
            let ctx = ui.ctx().clone();
            self.escanear(&ctx);
        }
    }

    /// Una cuadrícula de tarjetas que se desplaza en vertical, con una cabecera y una fila
    /// opcional (`seccion`) encima. Solo se dibujan las filas que se ven, así va igual de
    /// fluida con diez series que con diez mil.
    fn cuadricula(
        &mut self,
        ui: &mut egui::Ui,
        cabecera: Cabecera,
        seccion: Option<(String, Vec<Tarjeta>)>,
        titulo: Option<String>,
        tarjetas: Vec<Tarjeta>,
        vacio: Option<String>,
    ) -> Option<Pulsar> {
        let c = estilo::colores(ui.ctx());
        let paso = egui::vec2(ANCHO_TARJETA + SEPARACION, ALTO_TARJETA + SEPARACION);
        let tamano = egui::vec2(ANCHO_TARJETA, ALTO_TARJETA);

        let mut pulsada = None;
        // Cómic soltado sobre otro: el que se movía y el que ocupaba el sitio.
        let mut soltado: Option<(String, String)> = None;
        // Primero las que se ven y luego las de la fila de más que se prepara por
        // arriba y por abajo, para que al desplazarse ya estén.
        let mut peticiones = (Vec::new(), Vec::new());

        area_de_desplazamiento(self.arriba).show_viewport(ui, |ui, vista| {
            let ancho = ui.available_width() - 2.0 * MARGEN;
            let columnas = (((ancho + SEPARACION) / paso.x).floor() as usize).max(1);
            let origen = ui.max_rect().min + egui::vec2(MARGEN, MARGEN - 8.0);

            let zona = Rect::from_min_size(origen, egui::vec2(ancho, cabecera.alto()));
            if let Some(p) = self.cabecera(ui, c, &cabecera, zona, &mut peticiones) {
                pulsada = Some(p);
            }
            let mut y = cabecera.alto();

            if let Some((nombre, fila)) = &seccion {
                titulo_de_seccion(ui, c, origen + egui::vec2(0.0, y), ancho, nombre);
                y += ALTO_TITULO;
                for (columna, tarjeta) in fila.iter().take(columnas).enumerate() {
                    let rect = Rect::from_min_size(
                        origen + egui::vec2(columna as f32 * paso.x, y),
                        tamano,
                    );
                    let id = ui.id().with(("seccion", columna));
                    if self.tarjeta(ui, c, rect, id, tarjeta, &mut peticiones) {
                        pulsada = tarjeta.al_pulsar.clone();
                    }
                }
                y += paso.y + SEPARACION;
            }
            if let Some(titulo) = &titulo {
                titulo_de_seccion(ui, c, origen + egui::vec2(0.0, y), ancho, titulo);
                y += ALTO_TITULO;
            }
            if tarjetas.is_empty() {
                if let Some(vacio) = &vacio {
                    ui.painter().text(
                        origen + egui::vec2(0.0, y),
                        egui::Align2::LEFT_TOP,
                        vacio,
                        FontId::proportional(14.0),
                        c.texto_secundario,
                    );
                }
            }

            let filas = tarjetas.len().div_ceil(columnas);
            ui.set_height(y + filas as f32 * paso.y + 2.0 * MARGEN);

            if self.serie.is_some() {
                // Los cómics de una serie abierta se reordenan arrastrándolos y se animan.
                soltado = self.cuadricula_de_serie(
                    ui,
                    c,
                    origen + egui::vec2(0.0, y),
                    columnas,
                    &tarjetas,
                    &mut peticiones,
                    &mut pulsada,
                );
                return;
            }
            if !self.posiciones.is_empty() {
                self.posiciones.clear();
            }

            // Filas visibles, más una por arriba y otra por abajo.
            let desde = (vista.min.y - MARGEN - y) / paso.y;
            let hasta = (vista.max.y - MARGEN - y) / paso.y;
            let primera = (desde.floor().max(0.0) as usize).saturating_sub(1);
            let ultima = ((hasta.ceil().max(0.0) as usize) + 1).min(filas);

            for fila in primera..ultima {
                for columna in 0..columnas {
                    let Some(tarjeta) = tarjetas.get(fila * columnas + columna) else {
                        break;
                    };
                    let rect = Rect::from_min_size(
                        origen + egui::vec2(columna as f32 * paso.x, y + fila as f32 * paso.y),
                        tamano,
                    );
                    let id = ui.id().with(("tarjeta", fila * columnas + columna));
                    if self.tarjeta(ui, c, rect, id, tarjeta, &mut peticiones) {
                        pulsada = tarjeta.al_pulsar.clone();
                    }
                }
            }
        });

        if let Some((huella, ocupante)) = soltado {
            self.soltar_comic(&huella, &ocupante);
        }

        let (mut visibles, cercanas) = peticiones;
        visibles.extend(cercanas);
        self.pedir_miniaturas(visibles);
        pulsada
    }

    /// Lo que pasa en cada fotograma mientras se arrastra un cómic de la serie abierta:
    /// el cursor y, cerca del borde, el desplazamiento de la cuadrícula. `inicio` es dónde
    /// empieza la primera tarjeta. Devuelve dónde está el cómic, adónde caería y si ya se
    /// ha soltado el botón.
    fn seguir_arrastre(
        &mut self,
        ui: &mut egui::Ui,
        inicio: egui::Pos2,
        columnas: usize,
        tarjetas: &[Tarjeta],
    ) -> Option<Arrastre> {
        let huella = self.arrastrando.clone()?;
        let Some(actual) = tarjetas
            .iter()
            .position(|t| huella_de_tarjeta(t) == Some(huella.as_str()))
        else {
            // La serie ha cambiado y el cómic ya no está.
            self.arrastrando = None;
            return None;
        };

        // Con el botón soltado se acaba el arrastre, pase lo que pase (también si se
        // suelta fuera de la ventana).
        let soltado = !ui.input(|i| i.pointer.any_down());
        let Some(puntero) = ui.ctx().pointer_latest_pos() else {
            if soltado {
                self.arrastrando = None;
            }
            return None;
        };
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);

        // Cerca del borde de arriba o de abajo, la cuadrícula se desplaza sola.
        let visible = ui.clip_rect();
        let franja = 70.0;
        let velocidad = if puntero.y < visible.min.y + franja {
            18.0
        } else if puntero.y > visible.max.y - franja {
            -18.0
        } else {
            0.0
        };
        if velocidad != 0.0 {
            ui.scroll_with_delta(egui::vec2(0.0, velocidad));
            ui.ctx().request_repaint();
        }

        // La tarjeta sobre la que está el puntero (la más cercana, si está en un hueco).
        let paso = egui::vec2(ANCHO_TARJETA + SEPARACION, ALTO_TARJETA + SEPARACION);
        let relativo = puntero - inicio;
        let columna = ((relativo.x / paso.x).floor().max(0.0) as usize).min(columnas - 1);
        let fila = (relativo.y / paso.y).floor().max(0.0) as usize;
        let destino = (fila * columnas + columna).min(tarjetas.len() - 1);

        Some(Arrastre {
            actual,
            destino,
            puntero,
            soltado,
        })
    }

    /// Dibuja las tarjetas de la serie abierta, que se deslizan a su sitio cuando cambia
    /// el orden. Mientras se arrastra un cómic, las demás se colocan como quedarían al
    /// soltarlo y la que se lleva va encima, levantada y pegada al puntero. `inicio` es
    /// dónde empieza la primera tarjeta. Al soltar el botón devuelve el cómic movido y
    /// el que ocupaba el sitio de destino.
    #[allow(clippy::too_many_arguments)]
    fn cuadricula_de_serie(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        inicio: egui::Pos2,
        columnas: usize,
        tarjetas: &[Tarjeta],
        peticiones: &mut Peticiones,
        pulsada: &mut Option<Pulsar>,
    ) -> Option<(String, String)> {
        let paso = egui::vec2(ANCHO_TARJETA + SEPARACION, ALTO_TARJETA + SEPARACION);
        let tamano = egui::vec2(ANCHO_TARJETA, ALTO_TARJETA);
        let ctx = ui.ctx().clone();
        let fotograma = self.fotograma;

        let arrastre = self.seguir_arrastre(ui, inicio, columnas, tarjetas);
        // El orden en que se colocan: con un arrastre en marcha, el previsto.
        let mut orden: Vec<usize> = (0..tarjetas.len()).collect();
        if let Some(a) = &arrastre {
            let movida = orden.remove(a.actual);
            orden.insert(a.destino, movida);
        }

        // Suavizado exponencial: cada fotograma se recorre la misma fracción de lo que falta.
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        let acercamiento = 1.0 - (-dt * 18.0).exp();
        // Con una fila de margen por arriba y por abajo, para que entren ya colocadas.
        let visible = ui.clip_rect().expand2(egui::vec2(0.0, paso.y));
        let mut en_movimiento = false;
        let mut limpiar_soltada = false;
        // Las levantadas se dibujan al final, encima de las demás.
        let mut levantadas = Vec::new();

        for (sitio, &indice) in orden.iter().enumerate() {
            let tarjeta = &tarjetas[indice];
            let clave: Cow<str> = huella_de_tarjeta(tarjeta)
                .map_or_else(|| Cow::Owned(format!("#{indice}")), Cow::Borrowed);
            let es_arrastrada = arrastre.as_ref().is_some_and(|a| a.actual == indice);
            let es_soltada = self.soltada.as_deref() == Some(clave.as_ref());

            let del_sitio = egui::vec2(
                (sitio % columnas) as f32 * paso.x,
                (sitio / columnas) as f32 * paso.y,
            );
            let meta = match &arrastre {
                Some(a) if es_arrastrada => a.puntero - self.agarre - inicio,
                _ => del_sitio,
            };
            if es_arrastrada {
                // El hueco donde caerá.
                let marca = Rect::from_min_size(
                    inicio + del_sitio,
                    egui::vec2(ANCHO_TARJETA, ALTO_PORTADA),
                );
                ui.painter().rect_stroke(
                    marca,
                    estilo::RADIO_PORTADA,
                    Stroke::new(1.0, c.borde),
                    StrokeKind::Inside,
                );
            }

            // Si no se usó en el fotograma anterior (acaba de aparecer), se coloca ya.
            let (posicion, se_mueve) = match self.posiciones.get_mut(clave.as_ref()) {
                Some(entrada) => {
                    if entrada.1 + 1 < fotograma {
                        entrada.0 = meta;
                    }
                    entrada.1 = fotograma;
                    let falta = meta - entrada.0;
                    if falta.length() < 0.5 {
                        entrada.0 = meta;
                        (meta, false)
                    } else {
                        entrada.0 += falta * acercamiento;
                        (entrada.0, true)
                    }
                }
                None => {
                    self.posiciones.insert(clave.to_string(), (meta, fotograma));
                    (meta, false)
                }
            };
            en_movimiento |= se_mueve;

            let rect = Rect::from_min_size(inicio + posicion, tamano);
            if !es_arrastrada && !es_soltada && !visible.intersects(rect) {
                continue;
            }

            let id = ui.id().with(("comic", clave.as_ref()));
            let elevacion = ctx.animate_bool_with_time(
                egui::Id::new(("elevar", clave.as_ref())),
                es_arrastrada,
                0.12,
            );
            if es_soltada && !se_mueve && elevacion <= 0.001 {
                limpiar_soltada = true;
            }
            if es_arrastrada || es_soltada || elevacion > 0.001 {
                levantadas.push((indice, rect, id, elevacion, clave.to_string()));
            } else if self.tarjeta(ui, c, rect, id, tarjeta, peticiones) {
                *pulsada = tarjeta.al_pulsar.clone();
            }
        }

        for (indice, rect, id, elevacion, clave) in levantadas {
            let tarjeta = &tarjetas[indice];
            let capa =
                egui::LayerId::new(egui::Order::Tooltip, egui::Id::new(("levantada", &clave)));
            let hueco = Rect::from_min_size(rect.min, egui::vec2(ANCHO_TARJETA, ALTO_PORTADA));
            // Sin ocupar sitio en la cuadrícula ni recortar con el área de desplazamiento.
            let mut hijo = ui.new_child(egui::UiBuilder::new().layer_id(capa).max_rect(rect));
            hijo.set_clip_rect(ctx.content_rect());
            if elevacion > 0.001 {
                // Sombra suave: capas semitransparentes cada vez más grandes bajo la portada.
                let alfa = (10.0 * elevacion).round() as u8;
                for i in (1..=6).rev() {
                    let margen = i as f32 * 2.5;
                    hijo.painter().rect_filled(
                        hueco.expand(margen).translate(egui::vec2(0.0, 5.0)),
                        estilo::RADIO_PORTADA + margen as u8,
                        Color32::from_black_alpha(alfa),
                    );
                }
            }
            if self.tarjeta(&mut hijo, c, rect, id, tarjeta, peticiones) {
                *pulsada = tarjeta.al_pulsar.clone();
            }
            // Se levanta: crece un poco desde el centro de la portada.
            let escala = 1.0 + 0.05 * elevacion;
            ctx.transform_layer_shapes(
                capa,
                egui::emath::TSTransform::new(hueco.center().to_vec2() * (1.0 - escala), escala),
            );
        }

        if en_movimiento || arrastre.is_some() {
            ctx.request_repaint();
        }
        // Fuera las posiciones de lo que ya no se muestra.
        self.posiciones.retain(|_, (_, usada)| *usada == fotograma);
        if limpiar_soltada {
            self.soltada = None;
        }

        let arrastre = arrastre.filter(|a| a.soltado)?;
        // La tarjeta soltada vuela desde el puntero hasta su sitio definitivo.
        let huella = self.arrastrando.take()?;
        self.soltada = Some(huella.clone());
        if arrastre.destino == arrastre.actual {
            return None;
        }
        Some((
            huella,
            huella_de_tarjeta(&tarjetas[arrastre.destino])?.to_string(),
        ))
    }

    /// Termina un arrastre: `huella` pasa al sitio que ocupa `ocupante` en la serie abierta.
    fn soltar_comic(&mut self, huella: &str, ocupante: &str) {
        let Some(clave) = self.serie.as_ref().map(|s| s.clave.clone()) else {
            return;
        };
        match self
            .biblioteca
            .colocar_en_lugar_de(&clave, huella, ocupante)
        {
            Ok(()) => self.recargar(),
            Err(e) => self.mostrar_error(format!("No se pudo mover el cómic: {e}")),
        }
    }

    /// Dibuja la cabecera de una página en `zona`. Devuelve lo que se haya pulsado.
    fn cabecera(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        cabecera: &Cabecera,
        zona: Rect,
        peticiones: &mut Peticiones,
    ) -> Option<Pulsar> {
        match cabecera {
            Cabecera::Titulo { titulo, detalle } => {
                titulo_de_pagina(ui, c, zona.min, titulo, detalle);
                None
            }
            Cabecera::Inicio { detalle, destacado } => {
                titulo_de_pagina(ui, c, zona.min, "Biblioteca", detalle);
                let ficha = destacado.as_ref()?;
                let panel = Rect::from_min_size(
                    zona.min + egui::vec2(0.0, 84.0),
                    egui::vec2(zona.width().min(760.0), PORTADA_DESTACADA.y + 40.0),
                );
                self.seguir_leyendo_destacado(ui, c, panel, ficha, peticiones)
            }
            Cabecera::Serie(datos) => self.ficha_de_serie(ui, c, zona, datos, peticiones),
        }
    }

    /// "Seguir leyendo" en grande: la portada, por dónde vas y un botón para seguir.
    fn seguir_leyendo_destacado(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        panel: Rect,
        ficha: &Ficha,
        peticiones: &mut Peticiones,
    ) -> Option<Pulsar> {
        ui.painter().rect(
            panel,
            estilo::RADIO_PANEL,
            c.panel,
            Stroke::new(1.0, c.borde),
            StrokeKind::Inside,
        );

        let hueco = Rect::from_min_size(panel.min + egui::vec2(20.0, 20.0), PORTADA_DESTACADA);
        let portada_pulsada = ui
            .interact(hueco, ui.id().with("destacado"), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let encima = portada_pulsada.hovered();
        self.portada(ui, c, hueco, Some(&ficha.portada()), encima, peticiones);

        let x = hueco.max.x + 28.0;
        let ancho = panel.max.x - x - 20.0;
        let mut y = hueco.min.y + 2.0;
        let pintor = ui.painter();
        let serie = texto_ajustado(ui, ficha.serie(), estilo::negrita(24.0), c.texto, ancho, 2);
        let alto = serie.size().y;
        pintor.galley(egui::pos2(x, y), serie, c.texto);
        y += alto + 2.0;
        let numero = texto_ajustado(
            ui,
            &nombre_de_comic(ficha),
            FontId::proportional(15.0),
            c.texto_secundario,
            ancho,
            1,
        );
        let alto = numero.size().y;
        pintor.galley(egui::pos2(x, y), numero, c.texto_secundario);
        y += alto + 18.0;

        let (progreso, _) = progreso_de(ficha);
        let pagina = ficha.progreso.map_or(0, |p| p.pagina) + 1;
        let porcentaje = progreso.map_or(0.0, |p| p * 100.0);
        pintor.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_TOP,
            format!("Página {pagina} de {} · {porcentaje:.0}%", ficha.paginas),
            estilo::cifras(12.0),
            c.texto_secundario,
        );
        y += 24.0;
        let barra = Rect::from_min_size(egui::pos2(x, y), egui::vec2(ancho.min(320.0), 4.0));
        barra_de_progreso(pintor, c, barra, progreso.unwrap_or(0.0), c.borde);

        let boton = Rect::from_min_size(egui::pos2(x, hueco.max.y - 32.0), egui::vec2(150.0, 32.0));
        let seguir = ui.put(boton, boton_subrayado(c, "Seguir leyendo"));
        (seguir.clicked() || portada_pulsada.clicked()).then(|| Pulsar::Comic(ficha.ruta.clone()))
    }

    /// La ficha de una serie: portada grande, autores, resumen y el botón para seguir.
    fn ficha_de_serie(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        zona: Rect,
        datos: &DatosSerie,
        peticiones: &mut Peticiones,
    ) -> Option<Pulsar> {
        let volver_a = match self.pagina {
            Pagina::Biblioteca => "‹  Biblioteca",
            Pagina::Leyendo => "‹  Leyendo",
            Pagina::Novedades => "‹  Novedades",
            Pagina::Carpetas => "‹  Carpetas",
            Pagina::Ajustes => "‹  Ajustes",
        };
        let atras = ui.put(
            Rect::from_min_size(zona.min, egui::vec2(120.0, 26.0)),
            egui::Button::new(egui::RichText::new(volver_a).color(c.texto_secundario)).frame(false),
        );
        if atras.on_hover_text("Volver (Esc)").clicked() {
            self.serie = None;
        }

        let hueco = Rect::from_min_size(zona.min + egui::vec2(0.0, 36.0), PORTADA_FICHA);
        self.portada(ui, c, hueco, datos.portada.as_ref(), false, peticiones);

        let x = hueco.max.x + 32.0;
        let ancho = (zona.max.x - x).min(680.0);
        let mut y = hueco.min.y;
        let tamano_titulo = self.titulo_renombrable(
            ui,
            c,
            egui::pos2(x, y),
            ancho,
            &datos.nombre,
            Renombrable::Serie(datos.clave.clone()),
        );
        y += tamano_titulo.y + 4.0;
        let pintor = ui.painter();
        pintor.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_TOP,
            &datos.resumen_corto,
            FontId::proportional(14.0),
            c.texto_secundario,
        );
        y += 30.0;
        for credito in &datos.creditos {
            let linea = texto_ajustado(ui, credito, FontId::proportional(14.0), c.texto, ancho, 1);
            let alto = linea.size().y;
            pintor.galley(egui::pos2(x, y), linea, c.texto);
            y += alto + 2.0;
        }
        if let Some(resumen) = &datos.resumen {
            y += 12.0;
            // Lo que quepa encima de los botones.
            let filas = (((hueco.max.y - 48.0 - y) / 20.0).floor() as usize).max(1);
            let texto = texto_ajustado(
                ui,
                resumen,
                FontId::proportional(14.0),
                c.texto_secundario,
                ancho,
                filas,
            );
            pintor.galley(egui::pos2(x, y), texto, c.texto_secundario);
        }

        let mut pulsada = None;
        let mut boton_x = x;
        if let Some((texto, ruta)) = &datos.siguiente {
            let ancho_boton = 40.0 + texto.chars().count() as f32 * 7.5;
            let rect = Rect::from_min_size(
                egui::pos2(boton_x, hueco.max.y - 32.0),
                egui::vec2(ancho_boton, 32.0),
            );
            if ui.put(rect, boton_subrayado(c, texto.clone())).clicked() {
                pulsada = Some(Pulsar::Comic(ruta.clone()));
            }
            boton_x = rect.max.x + 8.0;
        }
        let mut anadir = None;
        for (texto, dialogo, ancho_boton) in [
            ("Añadir cómics…", Dialogo::ComicsParaSerie, 130.0),
            ("Añadir carpeta…", Dialogo::CarpetasParaSerie, 138.0),
        ] {
            let rect = Rect::from_min_size(
                egui::pos2(boton_x, hueco.max.y - 32.0),
                egui::vec2(ancho_boton, 32.0),
            );
            let boton = ui.put(rect, boton_texto(c, texto)).on_hover_text(
                "Mete en esta serie cómics que no encajan en ella por su nombre o su carpeta. Los archivos no se tocan.",
            );
            if boton.clicked() {
                anadir = Some(dialogo);
            }
            boton_x = rect.max.x + 8.0;
        }
        if let Some(dialogo) = anadir {
            self.pedir_dialogo_de_serie(dialogo, &datos.clave);
        }
        let unir = ui.put(
            Rect::from_min_size(
                egui::pos2(boton_x, hueco.max.y - 32.0),
                egui::vec2(170.0, 32.0),
            ),
            boton_texto(c, "Unir con otra serie…"),
        );
        let unir = unir.on_hover_text(
            "Si esta serie y otra son en realidad la misma (por ejemplo, «Vinland» y \
             «Vinland Saga»), mostrarlas juntas",
        );
        if unir.clicked() {
            self.unir = Some(String::new());
        }
        pulsada
    }

    /// Dibuja una portada en `hueco`: la miniatura con un borde fino, o un hueco vacío
    /// mientras llega. Si falta (o es de otro tamaño), la pide. Con `encima` (el ratón
    /// sobre ella) el borde se marca más.
    /// Devuelve si se dibujó la miniatura y dónde quedó.
    fn portada(
        &mut self,
        ui: &egui::Ui,
        c: Colores,
        hueco: Rect,
        portada: Option<&Portada>,
        encima: bool,
        peticiones: &mut Peticiones,
    ) -> (bool, Rect) {
        let ppp = ui.ctx().pixels_per_point();
        // Tamaño exacto en píxeles, para que la miniatura se dibuje 1:1.
        let caja = [
            (hueco.width() * ppp).round() as u32,
            (hueco.height() * ppp).round() as u32,
        ];
        let pintor = ui.painter();
        let borde = if encima {
            Stroke::new(1.5, c.texto_secundario)
        } else {
            Stroke::new(1.0, c.borde)
        };
        let vacia = |pintor: &egui::Painter| {
            pintor.rect(
                hueco,
                estilo::RADIO_PORTADA,
                c.tarjeta,
                borde,
                StrokeKind::Inside,
            );
            (false, hueco)
        };

        let Some(portada) = portada else {
            return vacia(pintor);
        };

        let clave = (portada.huella.clone(), caja);
        let exacta = self.texturas.contains_key(&clave);
        if !exacta && !self.sin_miniatura.contains(&portada.huella) {
            let peticion = miniaturas::Peticion {
                portada: portada.clone(),
                caja,
            };
            if ui.clip_rect().intersects(hueco) {
                peticiones.0.push(peticion);
            } else {
                peticiones.1.push(peticion);
            }
        }

        // La del tamaño justo o, mientras llega, la de otro tamaño (se ve escalada un momento).
        let fotograma = self.fotograma;
        let textura = match self.texturas.get_mut(&clave) {
            Some(t) => Some(t),
            None => self
                .texturas
                .iter_mut()
                .find(|((huella, _), _)| *huella == portada.huella)
                .map(|(_, t)| t),
        };
        let Some(textura) = textura else {
            return vacia(pintor);
        };
        textura.usada = fotograma;

        // Centrada en el hueco y alineada con los píxeles de la pantalla.
        let tamano = if exacta {
            textura.textura.size_vec2() / ppp
        } else {
            let [ancho, alto] = textura.textura.size();
            let escala = (hueco.width() / ancho as f32).min(hueco.height() / alto as f32);
            egui::vec2(ancho as f32, alto as f32) * escala
        };
        let colocada = Rect::from_center_size(hueco.center(), tamano);
        let esquina = (colocada.min.to_vec2() * ppp).round() / ppp;
        let rect = Rect::from_min_size(esquina.to_pos2(), tamano);

        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        estilo::imagen(
            pintor,
            textura.textura.id(),
            rect,
            uv,
            estilo::RADIO_PORTADA,
        );
        // Por fuera, para no tapar ni un píxel de la portada. Así una portada negra no
        // se pierde en el fondo oscuro.
        pintor.rect_stroke(rect, estilo::RADIO_PORTADA, borde, StrokeKind::Outside);
        (true, rect)
    }

    /// Dibuja una tarjeta. Devuelve si se ha pulsado.
    fn tarjeta(
        &mut self,
        ui: &mut egui::Ui,
        c: Colores,
        rect: Rect,
        id: egui::Id,
        tarjeta: &Tarjeta,
        peticiones: &mut Peticiones,
    ) -> bool {
        // Mientras se renombra esta tarjeta, pulsar en ella no debe abrirla: el campo de
        // edición ocupa el sitio del título y necesita sus propios clics y teclas.
        let editando_este = tarjeta
            .renombrar
            .as_ref()
            .zip(self.renombrando.as_ref())
            .is_some_and(|(obj, r)| &r.objetivo == obj);
        // Los cómics de una serie abierta se arrastran para cambiarlos de sitio.
        let huella_arrastrable = match &tarjeta.renombrar {
            Some(Renombrable::Comic(huella)) if self.serie.is_some() && !editando_este => {
                Some(huella)
            }
            _ => None,
        };
        let sentido = if editando_este {
            egui::Sense::hover()
        } else if huella_arrastrable.is_some() {
            egui::Sense::click_and_drag()
        } else if tarjeta.al_pulsar.is_some() || tarjeta.renombrar.is_some() {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let respuesta = ui.interact(rect, id, sentido);
        // Mientras se arrastra, el texto de ayuda de las tarjetas solo estorba.
        let respuesta = if self.arrastrando.is_some() {
            respuesta
        } else {
            respuesta.on_hover_text(&tarjeta.ayuda)
        };
        if let Some(huella) = huella_arrastrable {
            if respuesta.drag_started() {
                self.arrastrando = Some(huella.clone());
                self.agarre = respuesta
                    .interact_pointer_pos()
                    .map_or(egui::Vec2::ZERO, |p| p - rect.min);
            }
        }
        let encima = respuesta.hovered() && tarjeta.al_pulsar.is_some();
        if encima {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let hueco = Rect::from_min_size(rect.min, egui::vec2(ANCHO_TARJETA, ALTO_PORTADA));
        let (dibujada, portada) =
            self.portada(ui, c, hueco, tarjeta.portada.as_ref(), encima, peticiones);
        let pintor = ui.painter();

        // Sin miniatura: o no se puede abrir, o no se pudo sacar la portada. Mientras
        // se está haciendo, el hueco se queda vacío (suele tardar un instante).
        let sin_portada = tarjeta
            .portada
            .as_ref()
            .is_none_or(|p| self.sin_miniatura.contains(&p.huella));
        if !dibujada && (tarjeta.al_pulsar.is_none() || sin_portada) {
            let aviso = if tarjeta.al_pulsar.is_none() {
                "No se puede abrir"
            } else {
                "Sin portada"
            };
            pintor.text(
                hueco.center(),
                egui::Align2::CENTER_CENTER,
                aviso,
                FontId::proportional(13.0),
                c.texto_secundario,
            );
        }

        if let Some(progreso) = tarjeta.progreso.filter(|_| !tarjeta.terminado) {
            // Debajo de la portada, no encima.
            let arriba = hueco.max.y + (ESPACIO_BARRA - ALTO_BARRA) / 2.0 + 1.0;
            let barra = Rect::from_min_max(
                egui::pos2(hueco.min.x, arriba),
                egui::pos2(hueco.max.x, arriba + ALTO_BARRA),
            );
            barra_de_progreso(pintor, c, barra, progreso, c.borde);
        }
        if tarjeta.terminado {
            etiqueta_sobre_portada(pintor, c, portada, "Leído");
        }

        let posicion_titulo = hueco.left_bottom() + egui::vec2(0.0, ESPACIO_BARRA + 8.0);
        let titulo_tam = if editando_este {
            let rect = Rect::from_min_size(posicion_titulo, egui::vec2(ANCHO_TARJETA, 20.0));
            self.campo_de_renombrar(ui, c, rect);
            rect.size()
        } else {
            let titulo = texto_ajustado(
                ui,
                &tarjeta.titulo,
                FontId::proportional(13.5),
                c.texto,
                ANCHO_TARJETA,
                2,
            );
            let tam = titulo.size();
            ui.painter().galley(posicion_titulo, titulo, c.texto);
            tam
        };

        let detalle = texto_ajustado(
            ui,
            &tarjeta.detalle,
            FontId::proportional(12.0),
            c.texto_secundario,
            ANCHO_TARJETA,
            1,
        );
        ui.painter().galley(
            hueco.left_bottom() + egui::vec2(0.0, ESPACIO_BARRA + 10.0 + titulo_tam.y),
            detalle,
            c.texto_secundario,
        );

        // Botón derecho: renombrar (y, en un cómic, eliminarlo de la colección).
        if let Some(objetivo) = tarjeta.renombrar.clone() {
            self.menu_de_renombrar(&respuesta, objetivo, &tarjeta.titulo);
        }

        respuesta.clicked() && tarjeta.al_pulsar.is_some()
    }

    fn pedir_miniaturas(&mut self, peticiones: Vec<miniaturas::Peticion>) {
        let pedido: Vec<(String, Tamano)> = peticiones
            .iter()
            .map(|p| (p.portada.huella.clone(), p.caja))
            .collect();
        if pedido != self.pedido {
            self.miniaturas.pedir(peticiones);
            self.pedido = pedido;
        }
    }

    /// Sube a la GPU las miniaturas que hayan terminado los hilos.
    fn recibir_miniaturas(&mut self, ctx: &egui::Context) {
        let listas: Vec<_> = self.miniaturas.listas().collect();
        for respuesta in listas {
            match respuesta {
                miniaturas::Respuesta::Lista {
                    huella,
                    caja,
                    imagen,
                } => {
                    // Como las páginas: se dibujan 1:1, y el filtro solo actúa si la
                    // ventana cambia de escala hasta que llega la nueva.
                    let textura = ctx.load_texture(
                        format!("miniatura-{huella}-{}x{}", caja[0], caja[1]),
                        imagen,
                        egui::TextureOptions::LINEAR,
                    );
                    self.texturas.insert(
                        (huella, caja),
                        Textura {
                            textura,
                            usada: self.fotograma,
                        },
                    );
                }
                miniaturas::Respuesta::Error { huella } => {
                    self.sin_miniatura.insert(huella);
                }
            }
        }
    }

    fn soltar_texturas_viejas(&mut self) {
        if self.texturas.len() <= MAX_TEXTURAS {
            return;
        }
        let mut usadas: Vec<u64> = self.texturas.values().map(|t| t.usada).collect();
        usadas.sort_unstable();
        // Se quedan las dos terceras partes más recientes.
        let limite = usadas[usadas.len() - MAX_TEXTURAS * 2 / 3];
        self.texturas.retain(|_, t| t.usada >= limite);
    }

    /// La ventana para unir la serie abierta con otra, y para deshacer uniones.
    fn ventana_unir(&mut self, ctx: &egui::Context) {
        let (Some(filtro), Some(abierta)) = (&mut self.unir, &self.serie) else {
            return;
        };

        let unidas = self
            .biblioteca
            .series_unidas_a(&abierta.clave)
            .unwrap_or_default();
        let c = estilo::colores(ctx);
        let mut abierta_ventana = true;
        let mut elegida = None;
        let mut separar = None;

        egui::Window::new(format!("Unir «{}» con otra serie", abierta.nombre))
            .open(&mut abierta_ventana)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.weak(
                    "Los cómics se mostrarán juntos. Los archivos no se tocan y se puede deshacer.",
                );

                if !unidas.is_empty() {
                    ui.add_space(8.0);
                    ui.strong("Ya unidas a esta serie");
                    for unida in &unidas {
                        ui.horizontal(|ui| {
                            if ui.add(boton_texto(c, "Separar")).clicked() {
                                separar = Some(unida.clone());
                            }
                            ui.label(&unida.nombre);
                        });
                    }
                }

                ui.add_space(8.0);
                ui.strong("Unir con");
                ui.add(
                    egui::TextEdit::singleline(filtro)
                        .hint_text("Buscar serie")
                        .desired_width(f32::INFINITY),
                );
                let buscado = filtro.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        let mut hay = false;
                        for serie in &self.series {
                            if serie.clave == abierta.clave
                                || !(buscado.is_empty()
                                    || serie.nombre.to_lowercase().contains(&buscado))
                            {
                                continue;
                            }
                            hay = true;
                            let texto = format!(
                                "{}, {} {}",
                                serie.nombre,
                                serie.comics,
                                plural(serie.comics, "cómic", "cómics")
                            );
                            if ui.add(boton_texto(c, texto)).clicked() {
                                elegida = Some(serie.clave.clone());
                            }
                        }
                        if !hay {
                            ui.weak("No hay ninguna otra serie con ese nombre.");
                        }
                    });
            });

        if !abierta_ventana {
            self.unir = None;
        }
        if let Some(unida) = separar {
            match self.biblioteca.separar_serie(&unida) {
                Ok(()) => self.recargar(),
                Err(e) => self.mostrar_error(e),
            }
        }
        if let Some(otra) = elegida {
            self.unir = None;
            self.unir_con(&otra);
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

/// Lo que se cuenta al usuario cuando acaba la búsqueda de cómics movidos.
fn texto_de_informe(informe: Informe) -> String {
    let arreglados = informe.carpetas + informe.archivos;
    if arreglados == 0 {
        return "No se ha encontrado ninguno en esa carpeta. Prueba con otra, o con una más \
                grande que la contenga."
            .to_string();
    }
    let mut partes = Vec::new();
    if informe.carpetas > 0 {
        let n = informe.carpetas;
        partes.push(format!("{n} {}", plural(n, "carpeta", "carpetas")));
    }
    if informe.archivos > 0 {
        let n = informe.archivos;
        partes.push(format!(
            "{n} {}",
            plural(n, "cómic suelto", "cómics sueltos")
        ));
    }
    let mut texto = format!("Reubicado: {}.", partes.join(" y "));
    if informe.sin_encontrar > 0 {
        let n = informe.sin_encontrar;
        texto.push_str(&format!(
            " {n} {} sin encontrar.",
            plural(n, "cómic", "cómics")
        ));
    }
    texto
}

/// Miniaturas que faltan: primero las que se ven y luego las que están a punto de verse.
type Peticiones = (Vec<miniaturas::Peticion>, Vec<miniaturas::Peticion>);

pub(crate) fn plural(n: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if n == 1 {
        singular
    } else {
        plural
    }
}

/// Un elemento de la barra lateral: lo elegido va en una píldora gris y con el texto
/// claro; lo demás, en gris, con un fondo apenas marcado al pasar el ratón.
fn elemento_lateral(
    ui: &mut egui::Ui,
    c: Colores,
    texto: &str,
    contador: Option<usize>,
    elegido: bool,
) -> egui::Response {
    let (rect, respuesta) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
    let respuesta = respuesta.on_hover_cursor(egui::CursorIcon::PointingHand);
    let pintor = ui.painter();

    if elegido {
        pintor.rect_filled(rect, estilo::RADIO_CONTROL, c.resaltado);
    } else if respuesta.hovered() {
        pintor.rect_filled(rect, estilo::RADIO_CONTROL, c.panel);
    }
    let (fuente, color) = if elegido {
        (estilo::media(14.0), c.texto)
    } else {
        (FontId::proportional(14.0), c.texto_secundario)
    };
    pintor.text(
        egui::pos2(rect.min.x + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        texto,
        fuente,
        color,
    );
    if let Some(n) = contador {
        pintor.text(
            egui::pos2(rect.max.x - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            n.to_string(),
            estilo::cifras(12.0),
            c.texto_secundario,
        );
    }
    respuesta
}

/// Una etiqueta de estado en la esquina de una portada ("Leído", "Tu serie"): oscura y
/// en píldora, para leerse sobre cualquier portada.
pub(crate) fn etiqueta_sobre_portada(
    pintor: &egui::Painter,
    c: Colores,
    portada: Rect,
    texto: &str,
) {
    let galeria = pintor.layout_no_wrap(texto.to_string(), estilo::media(11.0), c.texto);
    let fondo = Rect::from_min_size(
        egui::pos2(portada.max.x - galeria.size().x - 22.0, portada.min.y + 8.0),
        galeria.size() + egui::vec2(14.0, 6.0),
    );
    pintor.rect(
        fondo,
        fondo.height() / 2.0,
        c.panel.gamma_multiply(0.92),
        Stroke::new(1.0, c.borde_fuerte),
        StrokeKind::Inside,
    );
    pintor.galley(fondo.min + egui::vec2(7.0, 3.0), galeria, c.texto);
}

/// El botón de la acción principal de cada sitio: gris, como los demás botones, con una
/// línea de acento debajo en vez de ir relleno. Vale tanto con `ui.add` (se ajusta al
/// texto) como con `ui.put(rect, …)` (se ajusta al hueco que se le dé).
pub(crate) fn boton_subrayado(c: Colores, texto: impl Into<String>) -> impl egui::Widget {
    let texto = texto.into();
    move |ui: &mut egui::Ui| -> egui::Response {
        // Sin fondo ni borde: solo el texto, en gris, con una línea de acento debajo.
        let galeria = ui
            .painter()
            .layout_no_wrap(texto, estilo::media(14.0), c.texto_secundario);
        let tamano = galeria.size() + egui::vec2(28.0, 12.0);
        let (rect, respuesta) = ui.allocate_exact_size(tamano, egui::Sense::click());
        let respuesta = respuesta.on_hover_cursor(egui::CursorIcon::PointingHand);
        let pintor = ui.painter();
        // Al pasar el ratón, la línea se aclara; el texto no cambia.
        let color_linea = if respuesta.hovered() {
            c.acento.lerp_to_gamma(Color32::WHITE, 0.35)
        } else {
            c.acento
        };
        let subrayado = Rect::from_min_max(
            egui::pos2(rect.min.x + 10.0, rect.max.y - 4.0),
            egui::pos2(rect.max.x - 10.0, rect.max.y - 2.0),
        );
        pintor.rect_filled(subrayado, 1, color_linea);
        let posicion = rect.center() - galeria.size() / 2.0 - egui::vec2(0.0, 1.0);
        pintor.galley(posicion, galeria, c.texto_secundario);
        respuesta
    }
}

/// El desplazamiento vertical de una página entera. Con `arriba`, empieza arriba del todo
/// en vez de por donde se hubiera dejado.
pub(crate) fn area_de_desplazamiento(arriba: bool) -> egui::ScrollArea {
    let area = egui::ScrollArea::vertical().auto_shrink(false);
    if arriba {
        area.vertical_scroll_offset(0.0)
    } else {
        area
    }
}

/// Un botón corriente: solo el texto, en gris, sin marco; al pasar el ratón se enciende
/// el texto y aparece una píldora gris, como en la barra lateral. Con `add_enabled(false, …)`
/// se apaga y no responde.
pub(crate) fn boton_texto(c: Colores, texto: impl Into<String>) -> impl egui::Widget {
    let texto = texto.into();
    move |ui: &mut egui::Ui| -> egui::Response {
        let activo = ui.is_enabled();
        let galeria =
            ui.painter()
                .layout_no_wrap(texto.clone(), estilo::media(14.0), c.texto_secundario);
        let tamano = galeria.size() + egui::vec2(28.0, 12.0);
        let (rect, respuesta) = ui.allocate_exact_size(tamano, egui::Sense::click());
        let respuesta = if activo {
            respuesta.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            respuesta
        };
        let encendido = activo && respuesta.hovered();
        let pintor = ui.painter();
        if encendido {
            pintor.rect_filled(rect, estilo::RADIO_CONTROL, c.resaltado);
        }
        let color = if !activo {
            c.texto_secundario.gamma_multiply(0.45)
        } else if encendido {
            c.texto
        } else {
            c.texto_secundario
        };
        pintor.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            texto,
            estilo::media(14.0),
            color,
        );
        respuesta
    }
}

/// Un recuadro: el primer escalón sobre el fondo, con borde fino y esquinas de panel.
pub(crate) fn panel(c: Colores) -> egui::Frame {
    egui::Frame::new()
        .fill(c.panel)
        .stroke(Stroke::new(1.0, c.borde))
        .corner_radius(estilo::RADIO_PANEL)
        .inner_margin(egui::Margin::symmetric(16, 14))
}

/// Una barra de progreso en píldora. `fondo` es lo que queda por leer: oscuro sobre una
/// portada, el color de las líneas sobre un panel.
fn barra_de_progreso(
    pintor: &egui::Painter,
    c: Colores,
    barra: Rect,
    progreso: f32,
    fondo: Color32,
) {
    let radio = barra.height() / 2.0;
    pintor.rect_filled(barra, radio, fondo);
    let hecho = Rect::from_min_size(
        barra.min,
        egui::vec2(barra.width() * progreso.clamp(0.0, 1.0), barra.height()),
    );
    pintor.rect_filled(hecho, radio, c.acento);
}

fn titulo_de_pagina(ui: &egui::Ui, c: Colores, posicion: egui::Pos2, titulo: &str, detalle: &str) {
    let pintor = ui.painter();
    let galeria = pintor.layout_job(egui::text::LayoutJob::single_section(
        titulo.to_string(),
        egui::TextFormat {
            font_id: estilo::negrita(24.0),
            color: c.texto,
            extra_letter_spacing: -0.5,
            ..Default::default()
        },
    ));
    pintor.galley(posicion, galeria, c.texto);
    pintor.text(
        posicion + egui::vec2(0.0, 36.0),
        egui::Align2::LEFT_TOP,
        detalle,
        FontId::proportional(14.0),
        c.texto_secundario,
    );
}

/// El título de una sección, seguido de una línea fina hasta el final de `ancho`.
fn titulo_de_seccion(ui: &egui::Ui, c: Colores, posicion: egui::Pos2, ancho: f32, texto: &str) {
    let pintor = ui.painter();
    let titulo = pintor.text(
        posicion,
        egui::Align2::LEFT_TOP,
        texto,
        estilo::negrita(16.0),
        c.texto,
    );
    // En el centro de un píxel, para que la línea salga nítida y no repartida en dos.
    let y = pintor.round_to_pixel_center(titulo.center().y);
    let desde = titulo.max.x + 12.0;
    let hasta = posicion.x + ancho;
    if desde < hasta {
        pintor.hline(desde..=hasta, y, Stroke::new(1.0, c.borde));
    }
}

fn tarjeta_de_serie(serie: &Serie) -> Tarjeta {
    let mut detalle = format!(
        "{} {}",
        serie.comics,
        plural(serie.comics, "cómic", "cómics")
    );
    if serie.terminados > 0 {
        detalle.push_str(&format!(
            ", {} {}",
            serie.terminados,
            plural(serie.terminados, "leído", "leídos")
        ));
    }
    Tarjeta {
        portada: Some(serie.portada.clone()),
        titulo: serie.nombre.clone(),
        ayuda: format!("{}\n{detalle}", serie.nombre),
        detalle,
        progreso: (serie.terminados > 0).then(|| serie.terminados as f32 / serie.comics as f32),
        terminado: serie.terminados == serie.comics,
        al_pulsar: Some(Pulsar::Serie(serie.nombre.clone())),
        renombrar: Some(Renombrable::Serie(serie.clave.clone())),
    }
}

/// Cómo se llama un cómic dentro de su serie: `#1 · Título`, `Vol. 2`, o el nombre
/// del archivo si no hay nada mejor.
fn nombre_de_comic(ficha: &Ficha) -> String {
    if let Some(nombre) = &ficha.nombre_personalizado {
        return nombre.clone();
    }
    let m = &ficha.metadatos;
    let numero = match (&m.numero, m.volumen) {
        (Some(numero), _) => Some(format!("#{numero}")),
        (None, Some(volumen)) => Some(format!("Vol. {volumen}")),
        (None, None) => None,
    };
    match (numero, &m.titulo) {
        (Some(numero), Some(titulo)) => format!("{numero} · {titulo}"),
        (Some(numero), None) => numero,
        (None, Some(titulo)) => titulo.clone(),
        (None, None) => ficha
            .ruta
            .file_stem()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned()),
    }
}

fn progreso_de(ficha: &Ficha) -> (Option<f32>, bool) {
    match ficha.progreso {
        Some(p) if ficha.paginas > 0 => (
            Some((p.pagina + 1) as f32 / ficha.paginas as f32),
            p.terminado,
        ),
        _ => (None, false),
    }
}

/// Los nombres distintos que aparecen en un campo de los cómics de una serie ("Ana, Luis"
/// en uno y "Ana" en otro dan "Ana, Luis"), como mucho tres.
fn nombres_de(comics: &[Ficha], campo: impl Fn(&Ficha) -> Option<&str>) -> Option<String> {
    let mut nombres: Vec<&str> = Vec::new();
    for nombre in comics.iter().filter_map(&campo).flat_map(|n| n.split(',')) {
        let nombre = nombre.trim();
        if !nombre.is_empty() && !nombres.contains(&nombre) {
            nombres.push(nombre);
        }
    }
    let mas = nombres.len() > 3;
    nombres.truncate(3);
    let mut texto = nombres.join(", ");
    if mas {
        texto.push('…');
    }
    (!texto.is_empty()).then_some(texto)
}

fn datos_de_serie(abierta: &SerieAbierta) -> DatosSerie {
    let comics = &abierta.comics;
    let leidos = comics
        .iter()
        .filter(|f| f.progreso.is_some_and(|p| p.terminado))
        .count();

    let mut resumen_corto = format!(
        "{} {}",
        comics.len(),
        plural(comics.len(), "cómic", "cómics")
    );
    if leidos > 0 {
        resumen_corto.push_str(&format!(", {leidos} {}", plural(leidos, "leído", "leídos")));
    }
    let anios: Vec<u32> = comics.iter().filter_map(|f| f.metadatos.anio).collect();
    match (anios.iter().min(), anios.iter().max()) {
        (Some(desde), Some(hasta)) if desde == hasta => {
            resumen_corto.push_str(&format!(", {desde}"))
        }
        (Some(desde), Some(hasta)) => resumen_corto.push_str(&format!(", {desde}–{hasta}")),
        _ => {}
    }

    let mut creditos = Vec::new();
    if let Some(guion) = nombres_de(comics, |f| f.metadatos.guion.as_deref()) {
        creditos.push(format!("Guion: {guion}"));
    }
    if let Some(dibujo) = nombres_de(comics, |f| f.metadatos.dibujo.as_deref()) {
        creditos.push(format!("Dibujo: {dibujo}"));
    }
    if let Some(editorial) = nombres_de(comics, |f| f.metadatos.editorial.as_deref()) {
        creditos.push(format!("Editorial: {editorial}"));
    }

    // Lo que toca leer: lo empezado más reciente; si no hay nada a medias, el primero
    // sin terminar; y si está todo leído, volver a empezar.
    let se_abre = |f: &&Ficha| f.error.is_none();
    let a_medias = comics
        .iter()
        .filter(se_abre)
        .filter(|f| f.progreso.is_some_and(|p| !p.terminado))
        .max_by_key(|f| f.progreso.map_or(0, |p| p.leido_en));
    let siguiente = match a_medias {
        Some(f) => Some((
            format!("Seguir leyendo {}", nombre_de_comic(f)),
            f.ruta.clone(),
        )),
        None => match comics
            .iter()
            .filter(se_abre)
            .find(|f| !f.progreso.is_some_and(|p| p.terminado))
        {
            Some(f) if leidos == 0 => Some(("Empezar a leer".to_string(), f.ruta.clone())),
            Some(f) => Some((format!("Seguir con {}", nombre_de_comic(f)), f.ruta.clone())),
            None => comics
                .iter()
                .find(se_abre)
                .map(|f| ("Volver a leer".to_string(), f.ruta.clone())),
        },
    };

    DatosSerie {
        nombre: abierta.nombre.clone(),
        clave: abierta.clave.clone(),
        portada: abierta
            .portada
            .clone()
            .or_else(|| comics.iter().find(se_abre).map(Ficha::portada)),
        resumen_corto,
        creditos,
        resumen: comics.iter().find_map(|f| f.metadatos.resumen.clone()),
        siguiente,
    }
}

fn tarjeta_de_comic(ficha: &Ficha) -> Tarjeta {
    let (progreso, terminado) = progreso_de(ficha);
    let m = &ficha.metadatos;
    let detalle = match &ficha.error {
        Some(error) => error.clone(),
        None => {
            let paginas = format!(
                "{} {}",
                ficha.paginas,
                plural(ficha.paginas, "página", "páginas")
            );
            match m.anio {
                Some(anio) => format!("{anio}, {paginas}"),
                None => paginas,
            }
        }
    };
    let titulo = nombre_de_comic(ficha);
    let mut ayuda = format!("{}\n{titulo}\n{}", ficha.serie(), ficha.ruta.display());
    if let Some(autor) = &m.guion {
        ayuda.push_str(&format!("\nGuion: {autor}"));
    }
    if let Some(autor) = &m.dibujo {
        ayuda.push_str(&format!("\nDibujo: {autor}"));
    }
    Tarjeta {
        portada: ficha.error.is_none().then(|| ficha.portada()),
        titulo,
        detalle,
        ayuda,
        progreso,
        terminado,
        al_pulsar: ficha
            .error
            .is_none()
            .then(|| Pulsar::Comic(ficha.ruta.clone())),
        renombrar: Some(Renombrable::Comic(ficha.huella.clone())),
    }
}

fn tarjeta_de_seguir(ficha: &Ficha) -> Tarjeta {
    let (progreso, terminado) = progreso_de(ficha);
    let pagina = ficha.progreso.map_or(0, |p| p.pagina) + 1;
    Tarjeta {
        portada: Some(ficha.portada()),
        titulo: format!("{} {}", ficha.serie(), nombre_de_comic(ficha)),
        detalle: format!("Página {pagina} de {}", ficha.paginas),
        ayuda: format!("Seguir por la página {pagina}\n{}", ficha.ruta.display()),
        progreso,
        terminado,
        al_pulsar: Some(Pulsar::Comic(ficha.ruta.clone())),
        renombrar: None,
    }
}

/// Texto partido en líneas a `ancho`, con `…` si no cabe en `filas`.
pub(crate) fn texto_ajustado(
    ui: &egui::Ui,
    texto: &str,
    fuente: FontId,
    color: Color32,
    ancho: f32,
    filas: usize,
) -> Arc<egui::Galley> {
    let mut trabajo = egui::text::LayoutJob::single_section(
        texto.to_string(),
        egui::TextFormat::simple(fuente, color),
    );
    trabajo.wrap = egui::text::TextWrapping {
        max_width: ancho,
        max_rows: filas,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(trabajo)
}
