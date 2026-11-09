//! Cada test monta una biblioteca de verdad en una carpeta temporal, con cómics
//! creados al vuelo (las "imágenes" son texto: para escanear no hace falta decodificarlas).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use comic_biblioteca::{
    Biblioteca, EstadoVinculo, Favorito, Movimiento, Resumen, SerieExterna, Vinculo,
};
use tempfile::TempDir;

/// Crea un `.cbz` con `paginas` imágenes y, si se da, un `ComicInfo.xml`.
fn crear_cbz(ruta: &Path, paginas: usize, comic_info: Option<&str>) {
    fs::create_dir_all(ruta.parent().unwrap()).unwrap();
    let mut zip = zip::ZipWriter::new(fs::File::create(ruta).unwrap());
    let opciones = zip::write::SimpleFileOptions::default();
    for i in 1..=paginas {
        zip.start_file(format!("{i:03}.jpg"), opciones).unwrap();
        zip.write_all(format!("página {i}").as_bytes()).unwrap();
    }
    if let Some(xml) = comic_info {
        zip.start_file("ComicInfo.xml", opciones).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

/// Crea un archivo cualquiera (y su carpeta, si no existe).
fn escribir(ruta: &Path) {
    fs::create_dir_all(ruta.parent().unwrap()).unwrap();
    fs::write(ruta, "x").unwrap();
}

/// Biblioteca de prueba:
///
/// ```text
/// comics/
///   Bleeding Hearts 001 (2026).cbz   con ComicInfo.xml
///   Bleeding Hearts 002 (2026).cbz   sin él: la serie sale del nombre
///   roto.cbz                         no es un zip
///   Saga/Capítulo 10.cbz             "Capítulo" no es serie: sale de la carpeta
///   Saga/Capítulo 2.cbz
///   Akira/1.jpg, 2.jpg               carpeta de imágenes = un cómic
///   Fondos/fondo.jpg, Maus.cbz       las imágenes junto a cómics no cuentan
/// ```
struct Prueba {
    _temporal: TempDir,
    comics: PathBuf,
    biblioteca: Biblioteca,
}

fn preparar() -> Prueba {
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");

    let xml = "<ComicInfo><Series>Bleeding Hearts</Series><Number>1</Number>\
               <Writer>Deniz Camp</Writer></ComicInfo>";
    crear_cbz(&comics.join("Bleeding Hearts 001 (2026).cbz"), 3, Some(xml));
    crear_cbz(&comics.join("Bleeding Hearts 002 (2026).cbz"), 3, None);
    fs::write(comics.join("roto.cbz"), "esto no es un zip").unwrap();
    crear_cbz(&comics.join("Saga/Capítulo 10.cbz"), 3, None);
    crear_cbz(&comics.join("Saga/Capítulo 2.cbz"), 2, None);
    fs::create_dir_all(comics.join("Akira")).unwrap();
    fs::write(comics.join("Akira/1.jpg"), "a").unwrap();
    fs::write(comics.join("Akira/2.jpg"), "b").unwrap();
    crear_cbz(&comics.join("Fondos/Maus.cbz"), 1, None);
    fs::write(comics.join("Fondos/fondo.jpg"), "c").unwrap();

    let biblioteca = Biblioteca::abrir(temporal.path().join("datos/biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();

    Prueba {
        _temporal: temporal,
        comics,
        biblioteca,
    }
}

fn escanear(biblioteca: &mut Biblioteca) -> Resumen {
    biblioteca.escanear(|_, _| {}).unwrap()
}

fn nombres_de_series(biblioteca: &Biblioteca) -> Vec<(String, usize)> {
    let series = biblioteca.series().unwrap();
    series.into_iter().map(|s| (s.nombre, s.comics)).collect()
}

#[test]
fn escanea_y_agrupa_en_series() {
    let mut p = preparar();
    let mut avances = Vec::new();
    let resumen = p
        .biblioteca
        .escanear(|hechos, total| avances.push((hechos, total)))
        .unwrap();

    assert_eq!(resumen.nuevos, 7);
    assert_eq!(resumen.con_error, 1);
    assert_eq!(avances.last(), Some(&(7, 7)));
    assert_eq!(
        nombres_de_series(&p.biblioteca),
        [
            ("Akira".to_string(), 1),
            ("Bleeding Hearts".to_string(), 2),
            ("Maus".to_string(), 1),
            ("roto".to_string(), 1),
            ("Saga".to_string(), 2),
        ]
    );
}

#[test]
fn los_comics_de_una_serie_salen_en_orden_de_lectura() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);

    let saga = p.biblioteca.comics_de_serie("saga").unwrap();
    let numeros: Vec<_> = saga.iter().map(|f| f.metadatos.numero.as_deref()).collect();
    assert_eq!(numeros, [Some("2"), Some("10")]);

    let bleeding = p.biblioteca.comics_de_serie("Bleeding Hearts").unwrap();
    assert_eq!(bleeding[0].metadatos.guion.as_deref(), Some("Deniz Camp"));
    assert_eq!(bleeding[1].metadatos.anio, Some(2026));
    assert_eq!(bleeding[0].paginas, 3);
}

#[test]
fn guarda_el_error_de_los_que_no_se_abren() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);

    let roto = &p.biblioteca.comics_de_serie("roto").unwrap()[0];
    assert!(roto.error.is_some());
    assert_eq!(roto.paginas, 0);
}

#[test]
fn reescanear_sin_cambios_no_abre_nada() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);

    let resumen = escanear(&mut p.biblioteca);
    assert_eq!(
        resumen,
        Resumen {
            sin_cambios: 7,
            ..Resumen::default()
        }
    );
}

#[test]
fn detecta_cambios_y_borrados() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);

    crear_cbz(&p.comics.join("Saga/Capítulo 2.cbz"), 5, None);
    fs::remove_file(p.comics.join("Fondos/Maus.cbz")).unwrap();

    let resumen = escanear(&mut p.biblioteca);
    assert_eq!(resumen.actualizados, 1);
    assert_eq!(resumen.borrados, 1);
    assert_eq!(p.biblioteca.comics_de_serie("Saga").unwrap()[0].paginas, 5);
    assert!(p.biblioteca.comics_de_serie("Maus").unwrap().is_empty());
}

#[test]
fn el_progreso_sobrevive_a_mover_el_archivo() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);
    let maus = p.biblioteca.comics_de_serie("Maus").unwrap().remove(0);
    p.biblioteca
        .guardar_progreso(&maus.huella, 4, false)
        .unwrap();

    // Al otro lado de la biblioteca, que Fondos/ no se quede sin cómics: sería un
    // cómic-carpeta de una página (su fondo.jpg) y el test no mediría lo que quiere.
    fs::create_dir_all(p.comics.join("Clasicos")).unwrap();
    fs::rename(maus.ruta, p.comics.join("Clasicos/Maus.cbz")).unwrap();
    crear_cbz(&p.comics.join("Fondos/Otro.cbz"), 1, None);
    let resumen = escanear(&mut p.biblioteca);
    assert_eq!((resumen.nuevos, resumen.borrados), (2, 1));

    let movido = p.biblioteca.comics_de_serie("Maus").unwrap().remove(0);
    assert!(movido.ruta.ends_with("Clasicos/Maus.cbz"));
    assert_eq!(movido.progreso.map(|pr| pr.pagina), Some(4));
}

#[test]
fn el_progreso_sobrevive_a_editar_el_comic_info() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);
    let ruta = p.comics.join("Bleeding Hearts 002 (2026).cbz");
    let antes = p
        .biblioteca
        .comics_de_serie("Bleeding Hearts")
        .unwrap()
        .remove(1);
    p.biblioteca
        .guardar_progreso(&antes.huella, 2, false)
        .unwrap();

    // Como haría un etiquetador: mismo archivo, ahora con ComicInfo.xml.
    crear_cbz(
        &ruta,
        3,
        Some("<ComicInfo><Title>Segunda parte</Title></ComicInfo>"),
    );
    escanear(&mut p.biblioteca);

    let despues = p
        .biblioteca
        .comics_de_serie("Bleeding Hearts")
        .unwrap()
        .remove(1);
    assert_ne!(despues.huella, antes.huella);
    assert_eq!(despues.metadatos.titulo.as_deref(), Some("Segunda parte"));
    assert_eq!(despues.progreso.map(|pr| pr.pagina), Some(2));
}

#[test]
fn seguir_leyendo_muestra_los_empezados_sin_terminar() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);
    let saga = p.biblioteca.comics_de_serie("Saga").unwrap();
    let akira = p.biblioteca.comics_de_serie("Akira").unwrap().remove(0);
    p.biblioteca
        .guardar_progreso(&saga[0].huella, 1, true)
        .unwrap();
    p.biblioteca
        .guardar_progreso(&akira.huella, 1, false)
        .unwrap();

    let empezados = p.biblioteca.seguir_leyendo(10).unwrap();
    assert_eq!(empezados.len(), 1);
    assert_eq!(empezados[0].serie(), "Akira");

    let series = p.biblioteca.series().unwrap();
    let saga = series.iter().find(|s| s.nombre == "Saga").unwrap();
    assert_eq!(saga.terminados, 1);
}

#[test]
fn una_carpeta_inaccesible_no_borra_sus_comics() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);

    // Como un disco desconectado.
    let fuera = p.comics.with_file_name("desconectado");
    fs::rename(&p.comics, &fuera).unwrap();
    let resumen = escanear(&mut p.biblioteca);

    assert_eq!(resumen.carpetas_inaccesibles, 1);
    assert_eq!(resumen.borrados, 0);
    assert_eq!(p.biblioteca.series().unwrap().len(), 5);
}

#[test]
fn quitar_y_volver_a_agregar_conserva_el_progreso() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);
    let maus = p.biblioteca.comics_de_serie("Maus").unwrap().remove(0);
    p.biblioteca
        .guardar_progreso(&maus.huella, 7, false)
        .unwrap();

    let carpeta = p.biblioteca.carpetas().unwrap().remove(0);
    p.biblioteca.quitar_carpeta(carpeta.id).unwrap();
    assert!(p.biblioteca.series().unwrap().is_empty());

    p.biblioteca.agregar_carpeta(&p.comics).unwrap();
    escanear(&mut p.biblioteca);
    let maus = p.biblioteca.comics_de_serie("Maus").unwrap().remove(0);
    assert_eq!(maus.progreso.map(|pr| pr.pagina), Some(7));
}

#[test]
fn una_caratula_no_oculta_los_comics_de_las_subcarpetas() {
    // Como lo dejan Kodi o Plex: carátula de la serie y los cómics por volúmenes.
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    escribir(&comics.join("Batman/folder.jpg"));
    escribir(&comics.join("Batman/otra imagen.jpg"));
    crear_cbz(&comics.join("Batman/Vol 1/Batman 001.cbz"), 2, None);
    // Y una carpeta que es un cómic, con capítulos en subcarpetas.
    escribir(&comics.join("Akira/01.jpg"));
    escribir(&comics.join("Akira/Cap 2/02.jpg"));
    // Solo la carátula: no es un cómic.
    escribir(&comics.join("Vacia/cover.jpg"));

    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);

    assert_eq!(
        nombres_de_series(&biblioteca),
        [("Akira".to_string(), 1), ("Batman".to_string(), 1)]
    );
    assert_eq!(biblioteca.comics_de_serie("Akira").unwrap()[0].paginas, 2);
}

/// Los archivos de Vinland Saga tal como los tiene el usuario: tres carpetas de cuatro
/// tomos cada una, con nombres como `Vinland Saga - Tomo 01 (#001-005).cbr`.
fn vinland(comics: &Path) {
    for (carpeta, tomos) in [("Vinland-T01-04", 1..=4), ("Vinland-T05-08", 5..=8)] {
        for tomo in tomos {
            let nombre = format!("Vinland Saga - Tomo {tomo:02} (#001-005).cbz");
            crear_cbz(&comics.join(carpeta).join(nombre), 2, None);
        }
    }
}

#[test]
fn las_carpetas_de_una_misma_serie_se_juntan() {
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    vinland(&comics);

    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca
        .agregar_carpeta(comics.join("Vinland-T01-04"))
        .unwrap();
    biblioteca
        .agregar_carpeta(comics.join("Vinland-T05-08"))
        .unwrap();
    escanear(&mut biblioteca);

    assert_eq!(
        nombres_de_series(&biblioteca),
        [("Vinland Saga".to_string(), 8)]
    );
    // En orden de tomos, aunque estén en carpetas distintas.
    let volumenes: Vec<_> = biblioteca
        .comics_de_serie("Vinland Saga")
        .unwrap()
        .iter()
        .map(|f| f.metadatos.volumen)
        .collect();
    assert_eq!(volumenes, (1..=8).map(Some).collect::<Vec<_>>());
}

#[test]
fn sin_serie_en_el_nombre_se_usa_la_carpeta_sin_el_rango_de_tomos() {
    // Archivos que no dicen de qué serie son: solo la carpeta lo sabe.
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    crear_cbz(&comics.join("Vinland-T01-04/Tomo 01.cbz"), 2, None);
    crear_cbz(&comics.join("Vinland-T01-04/Tomo 02.cbz"), 2, None);
    crear_cbz(&comics.join("Vinland-T05-08/Tomo 05.cbz"), 2, None);

    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);

    assert_eq!(nombres_de_series(&biblioteca), [("Vinland".to_string(), 3)]);
}

#[test]
fn lo_que_no_dice_su_serie_entra_solo_en_la_de_al_lado() {
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    vinland(&comics);
    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);

    // Llega una carpeta nueva cuyos archivos no dicen de qué serie son.
    crear_cbz(&comics.join("Vinland-T13-16/Tomo 13.cbz"), 2, None);
    crear_cbz(&comics.join("Vinland-T13-16/Tomo 14.cbz"), 2, None);
    escanear(&mut biblioteca);

    assert_eq!(
        nombres_de_series(&biblioteca),
        [("Vinland Saga".to_string(), 10)]
    );
    let volumenes: Vec<_> = biblioteca
        .comics_de_serie("Vinland Saga")
        .unwrap()
        .iter()
        .map(|f| f.metadatos.volumen.unwrap())
        .collect();
    assert_eq!(volumenes, [1, 2, 3, 4, 5, 6, 7, 8, 13, 14]);
}

#[test]
fn y_tambien_al_reves_si_la_serie_llega_despues() {
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    crear_cbz(&comics.join("Vinland-T13-16/Tomo 13.cbz"), 2, None);
    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);
    assert_eq!(nombres_de_series(&biblioteca), [("Vinland".to_string(), 1)]);

    vinland(&comics);
    escanear(&mut biblioteca);
    assert_eq!(
        nombres_de_series(&biblioteca),
        [("Vinland Saga".to_string(), 9)]
    );

    // Y si la serie fiable desaparece, el cómic vuelve a la de su carpeta.
    fs::remove_dir_all(comics.join("Vinland-T01-04")).unwrap();
    fs::remove_dir_all(comics.join("Vinland-T05-08")).unwrap();
    escanear(&mut biblioteca);
    assert_eq!(nombres_de_series(&biblioteca), [("Vinland".to_string(), 1)]);
}

#[test]
fn si_encajan_varias_series_no_adivina() {
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    crear_cbz(&comics.join("Batman Beyond/Batman Beyond 001.cbz"), 1, None);
    crear_cbz(
        &comics.join("Batman Year One/Batman Year One 001.cbz"),
        1,
        None,
    );
    crear_cbz(&comics.join("Batman-T01-02/Tomo 1.cbz"), 1, None);
    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);

    assert_eq!(
        nombres_de_series(&biblioteca),
        [
            ("Batman".to_string(), 1),
            ("Batman Beyond".to_string(), 1),
            ("Batman Year One".to_string(), 1),
        ]
    );
}

#[test]
fn una_serie_que_solo_empieza_parecido_no_se_une_si_esta_lejos() {
    // Lo débil (la carpeta) solo se fía de lo que tiene cerca: dos sitios distintos
    // de la biblioteca pueden tener series distintas que empiezan igual.
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    crear_cbz(
        &comics.join("DC/Batman Beyond/Batman Beyond 001.cbz"),
        1,
        None,
    );
    crear_cbz(&comics.join("Manga/Batman-T01-02/Tomo 1.cbz"), 1, None);
    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);

    assert_eq!(biblioteca.series().unwrap().len(), 2);
}

#[test]
fn series_con_otra_escritura_son_la_misma() {
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    let xml = |serie: &str| format!("<ComicInfo><Series>{serie}</Series></ComicInfo>");
    crear_cbz(&comics.join("a.cbz"), 1, Some(&xml("Y: el último hombre")));
    crear_cbz(&comics.join("b.cbz"), 1, Some(&xml("Y El Ultimo Hombre")));
    crear_cbz(&comics.join("c.cbz"), 1, Some(&xml("y el ultimo hombre!")));

    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);

    let series = biblioteca.series().unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].comics, 3);
}

#[test]
fn el_usuario_puede_unir_y_separar_series() {
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    let xml = |serie: &str| format!("<ComicInfo><Series>{serie}</Series></ComicInfo>");
    crear_cbz(&comics.join("a1.cbz"), 1, Some(&xml("Vinland")));
    crear_cbz(&comics.join("a2.cbz"), 1, Some(&xml("Vinland")));
    crear_cbz(&comics.join("b1.cbz"), 1, Some(&xml("Vinland Saga")));
    crear_cbz(&comics.join("c1.cbz"), 1, Some(&xml("Berserk")));

    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);
    assert_eq!(biblioteca.series().unwrap().len(), 3);

    let grupo = biblioteca.unir_series("Vinland Saga", "Vinland").unwrap();
    let series = biblioteca.series().unwrap();
    assert_eq!(series.len(), 2);
    let vinland = series.iter().find(|s| s.clave == grupo).unwrap();
    assert_eq!(vinland.comics, 3);
    // Gana el nombre que más se repite.
    assert_eq!(vinland.nombre, "Vinland");
    // Se pide con cualquiera de los dos nombres.
    assert_eq!(biblioteca.comics_de_serie("Vinland Saga").unwrap().len(), 3);
    assert_eq!(biblioteca.comics_de_serie("Vinland").unwrap().len(), 3);

    let unidas = biblioteca.series_unidas_a(&grupo).unwrap();
    assert_eq!(unidas.len(), 1);
    assert_eq!(unidas[0].nombre, "Vinland Saga");

    // La unión sobrevive a un nuevo escaneo y a reabrir la biblioteca.
    escanear(&mut biblioteca);
    assert_eq!(biblioteca.series().unwrap().len(), 2);

    biblioteca.separar_serie(&unidas[0]).unwrap();
    assert_eq!(biblioteca.series().unwrap().len(), 3);
    assert_eq!(biblioteca.comics_de_serie("Vinland").unwrap().len(), 2);
}

#[test]
fn al_unir_se_queda_la_serie_con_mas_comics_pidas_el_orden_que_pidas() {
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    let xml = |serie: &str| format!("<ComicInfo><Series>{serie}</Series></ComicInfo>");
    crear_cbz(&comics.join("a1.cbz"), 1, Some(&xml("Vinland")));
    crear_cbz(&comics.join("a2.cbz"), 1, Some(&xml("Vinland")));
    crear_cbz(&comics.join("b1.cbz"), 1, Some(&xml("Vinland Saga")));

    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);

    // Desde la pantalla de "Vinland" se pide unirla con "Vinland Saga" (la pequeña).
    let grupo = biblioteca.unir_series("Vinland", "Vinland Saga").unwrap();
    let series = biblioteca.series().unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].nombre, "Vinland");
    // Lo unido es la pequeña, no la propia serie.
    let unidas = biblioteca.series_unidas_a(&grupo).unwrap();
    assert_eq!(unidas.len(), 1);
    assert_eq!(unidas[0].nombre, "Vinland Saga");
}

#[test]
fn las_uniones_no_se_encadenan() {
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    let xml = |serie: &str| format!("<ComicInfo><Series>{serie}</Series></ComicInfo>");
    for serie in ["Uno", "Dos", "Tres"] {
        crear_cbz(&comics.join(format!("{serie}.cbz")), 1, Some(&xml(serie)));
    }
    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);

    // Uno -> Dos, y luego Dos -> Tres: Uno tiene que acabar también en Tres.
    biblioteca.unir_series("Uno", "Dos").unwrap();
    let grupo = biblioteca.unir_series("Dos", "Tres").unwrap();
    assert_eq!(biblioteca.series().unwrap().len(), 1);
    assert_eq!(biblioteca.comics_de_serie("Uno").unwrap().len(), 3);
    assert_eq!(biblioteca.series_unidas_a(&grupo).unwrap().len(), 2);
    // Unir lo que ya está unido es un error, no un cambio silencioso.
    assert!(biblioteca.unir_series("Uno", "Tres").is_err());
}

#[test]
fn escanea_muchos_comics_a_la_vez_sin_perder_ninguno() {
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    // Más que una tanda (50) y que los hilos, para que se mezclen los resultados.
    for serie in 0..8 {
        for numero in 1..=40 {
            crear_cbz(
                &comics.join(format!("Serie {serie}/Serie {serie} {numero:03}.cbz")),
                2,
                None,
            );
        }
    }

    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    let mut avances = Vec::new();
    let resumen = biblioteca
        .escanear(|hechos, total| avances.push((hechos, total)))
        .unwrap();

    assert_eq!(resumen.nuevos, 320);
    assert_eq!(avances.last(), Some(&(320, 320)));
    // El progreso nunca retrocede.
    assert!(avances.windows(2).all(|par| par[0].0 <= par[1].0));
    let series = biblioteca.series().unwrap();
    assert_eq!(series.len(), 8);
    assert!(series.iter().all(|s| s.comics == 40));
    // Cada cómic guardó sus propios datos, no los de otro hilo.
    let primera = biblioteca.comics_de_serie("Serie 3").unwrap();
    let numeros: Vec<_> = primera
        .iter()
        .map(|f| f.metadatos.numero.clone().unwrap())
        .collect();
    assert_eq!(numeros, (1..=40).map(|n| n.to_string()).collect::<Vec<_>>());
}

#[test]
fn una_biblioteca_antigua_se_actualiza_sola() {
    use rusqlite::Connection;

    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    crear_cbz(&comics.join("Vinland Saga - Tomo 01.cbz"), 2, None);
    crear_cbz(&comics.join("Vinland Saga - Tomo 02.cbz"), 2, None);
    let ruta_db = temporal.path().join("biblioteca.db");

    let mut biblioteca = Biblioteca::abrir(&ruta_db).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);
    drop(biblioteca);

    // La deja como la dejaba la versión anterior: serie con el guion de más, sin clave
    // y con el análisis sin hacer.
    let db = Connection::open(&ruta_db).unwrap();
    db.execute(
        "UPDATE comics SET serie = 'Vinland Saga -', serie_clave = '', analisis = 0",
        [],
    )
    .unwrap();
    drop(db);

    // Al abrirla ya se agrupa bien, aunque todavía muestre el nombre viejo...
    let mut biblioteca = Biblioteca::abrir(&ruta_db).unwrap();
    assert_eq!(biblioteca.series().unwrap().len(), 1);
    // ...y el siguiente escaneo vuelve a leer los cómics y lo corrige.
    let resumen = escanear(&mut biblioteca);
    assert_eq!(resumen.actualizados, 2);
    assert_eq!(
        nombres_de_series(&biblioteca),
        [("Vinland Saga".to_string(), 2)]
    );
    // Y la siguiente vez ya no hace falta.
    assert_eq!(escanear(&mut biblioteca).sin_cambios, 2);
}

#[test]
fn no_deja_añadir_carpetas_solapadas() {
    let p = preparar();
    assert!(p.biblioteca.agregar_carpeta(&p.comics).is_err());
    assert!(p.biblioteca.agregar_carpeta(p.comics.join("Saga")).is_err());
    assert!(p
        .biblioteca
        .agregar_carpeta(p.comics.parent().unwrap())
        .is_err());
}

#[test]
fn se_puede_volver_a_abrir() {
    let temporal = TempDir::new().unwrap();
    let ruta = temporal.path().join("biblioteca.db");
    let comics = temporal.path().join("comics");
    crear_cbz(&comics.join("Maus.cbz"), 1, None);

    let mut biblioteca = Biblioteca::abrir(&ruta).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);
    drop(biblioteca);

    let biblioteca = Biblioteca::abrir(&ruta).unwrap();
    assert_eq!(biblioteca.series().unwrap().len(), 1);
}

#[test]
fn lo_que_hay_de_cada_serie() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);

    let colecciones = p.biblioteca.colecciones().unwrap();
    let nombres: Vec<&str> = colecciones.iter().map(|c| c.nombre.as_str()).collect();
    assert_eq!(
        nombres,
        ["Akira", "Bleeding Hearts", "Maus", "roto", "Saga"]
    );

    let bleeding = &colecciones[1];
    assert_eq!(bleeding.numeros, ["1", "2"]);
    assert_eq!(bleeding.anio, Some(2026));
    assert_eq!(bleeding.comics, 2);
    // En orden de lectura, no de texto.
    assert_eq!(colecciones[4].numeros, ["2", "10"]);
}

#[test]
fn los_vinculos_se_guardan_cambian_y_se_quitan() {
    let p = preparar();
    assert!(p.biblioteca.vinculos().unwrap().is_empty());

    let mut vinculo = Vinculo {
        clave: "saga".to_string(),
        externa: Some(SerieExterna {
            fuente: "metron".to_string(),
            id: "7".to_string(),
            nombre: "Saga (2012)".to_string(),
        }),
        estado: EstadoVinculo::Automatico,
        buscada: 100,
    };
    p.biblioteca.guardar_vinculo(&vinculo).unwrap();
    assert_eq!(p.biblioteca.vinculos().unwrap()["saga"], vinculo);

    // El usuario dice que no se busque: se sustituye.
    vinculo.externa = None;
    vinculo.estado = EstadoVinculo::Ignorado;
    p.biblioteca.guardar_vinculo(&vinculo).unwrap();
    let vinculos = p.biblioteca.vinculos().unwrap();
    assert_eq!(vinculos.len(), 1);
    assert_eq!(vinculos["saga"], vinculo);

    p.biblioteca.quitar_vinculo("saga").unwrap();
    assert!(p.biblioteca.vinculos().unwrap().is_empty());
}

#[test]
fn una_serie_se_puede_renombrar_y_volver_al_nombre_automatico() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);

    p.biblioteca
        .renombrar_serie("saga", "Vinland Saga")
        .unwrap();
    let series = p.biblioteca.series().unwrap();
    let saga = series.iter().find(|s| s.clave == "saga").unwrap();
    assert_eq!(saga.nombre, "Vinland Saga");
    // Sigue valiendo para pedir sus cómics, aunque el nombre ya no diga "Saga".
    assert_eq!(p.biblioteca.comics_de_serie("saga").unwrap().len(), 2);

    // Vacío: vuelve al nombre automático.
    p.biblioteca.renombrar_serie("saga", "").unwrap();
    let series = p.biblioteca.series().unwrap();
    let saga = series.iter().find(|s| s.clave == "saga").unwrap();
    assert_eq!(saga.nombre, "Saga");
}

#[test]
fn un_comic_se_puede_renombrar_y_volver_al_nombre_automatico() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);
    let comics = p.biblioteca.comics_de_serie("saga").unwrap();
    let huella = &comics[0].huella;
    assert_eq!(comics[0].nombre_personalizado, None);

    p.biblioteca
        .renombrar_comic(huella, "Un título mejor")
        .unwrap();
    let comics = p.biblioteca.comics_de_serie("saga").unwrap();
    assert_eq!(
        comics[0].nombre_personalizado.as_deref(),
        Some("Un título mejor")
    );

    // El nombre se guarda por huella: sobrevive a mover el archivo (sin cambiar de
    // carpeta, para no afectar a cómo se detecta la serie por el nombre).
    fs::rename(
        p.comics.join("Saga/Capítulo 2.cbz"),
        p.comics.join("Saga/Capítulo 02.cbz"),
    )
    .unwrap();
    escanear(&mut p.biblioteca);
    let comics = p.biblioteca.comics_de_serie("saga").unwrap();

    let movido = comics.iter().find(|f| &f.huella == huella).unwrap();
    assert_eq!(
        movido.nombre_personalizado.as_deref(),
        Some("Un título mejor")
    );

    p.biblioteca.renombrar_comic(huella, "").unwrap();
    let comics = p.biblioteca.comics_de_serie("saga").unwrap();
    let movido = comics.iter().find(|f| &f.huella == huella).unwrap();
    assert_eq!(movido.nombre_personalizado, None);
}

#[test]
fn un_comic_eliminado_no_vuelve_a_aparecer_solo() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);
    assert_eq!(p.biblioteca.comics_de_serie("saga").unwrap().len(), 2);
    let huella = p.biblioteca.comics_de_serie("saga").unwrap()[0]
        .huella
        .clone();

    p.biblioteca.quitar_comic(&huella).unwrap();
    let saga = p.biblioteca.comics_de_serie("saga").unwrap();
    assert_eq!(saga.len(), 1);
    assert!(saga.iter().all(|f| f.huella != huella));

    // Un escaneo normal no lo vuelve a traer, aunque el archivo siga ahí.
    escanear(&mut p.biblioteca);
    let saga = p.biblioteca.comics_de_serie("saga").unwrap();
    assert_eq!(saga.len(), 1);
    assert!(saga.iter().all(|f| f.huella != huella));
}

#[test]
fn la_portada_elegida_gana_y_se_puede_quitar() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);
    let saga = p.biblioteca.comics_de_serie("saga").unwrap();
    let primero = saga[0].huella.clone();
    assert_eq!(saga[0].portada().pagina, 0);

    p.biblioteca.elegir_portada(&primero, Some(1)).unwrap();
    let saga = p.biblioteca.comics_de_serie("saga").unwrap();
    assert_eq!(saga[0].portada().pagina, 1);
    assert_eq!(saga[1].portada().pagina, 0);
    // La serie se enseña con la portada de su primer cómic.
    let serie = p
        .biblioteca
        .series()
        .unwrap()
        .into_iter()
        .find(|s| s.clave == "saga")
        .unwrap();
    assert_eq!(serie.portada.pagina, 1);

    // Sobrevive a un nuevo escaneo y se puede cambiar y quitar.
    escanear(&mut p.biblioteca);
    p.biblioteca.elegir_portada(&primero, Some(0)).unwrap();
    assert_eq!(
        p.biblioteca.comics_de_serie("saga").unwrap()[0]
            .portada()
            .pagina,
        0
    );
    p.biblioteca.elegir_portada(&primero, Some(1)).unwrap();
    p.biblioteca.elegir_portada(&primero, None).unwrap();
    assert_eq!(
        p.biblioteca.comics_de_serie("saga").unwrap()[0]
            .portada()
            .pagina,
        0
    );
}

#[test]
fn la_portada_de_una_serie_puede_ser_la_de_cualquiera_de_sus_comics() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);
    let saga = p.biblioteca.comics_de_serie("saga").unwrap();
    let portada_de_saga = |p: &Prueba| {
        let series = p.biblioteca.series().unwrap();
        series
            .into_iter()
            .find(|s| s.clave == "saga")
            .unwrap()
            .portada
    };
    // Por defecto, la del primero.
    assert_eq!(portada_de_saga(&p).huella, saga[0].huella);

    p.biblioteca
        .elegir_portada_de_serie("saga", Some(&saga[1].huella))
        .unwrap();
    assert_eq!(portada_de_saga(&p).huella, saga[1].huella);

    // Y con la página que se haya elegido para ese cómic.
    p.biblioteca
        .elegir_portada(&saga[1].huella, Some(2))
        .unwrap();
    assert_eq!(portada_de_saga(&p).pagina, 2);

    // Un cómic que ya no está en la serie no deja la serie sin portada.
    p.biblioteca
        .elegir_portada_de_serie("saga", Some("otra-huella"))
        .unwrap();
    assert_eq!(portada_de_saga(&p).huella, saga[0].huella);

    p.biblioteca
        .elegir_portada_de_serie("saga", Some(&saga[1].huella))
        .unwrap();
    p.biblioteca.elegir_portada_de_serie("saga", None).unwrap();
    assert_eq!(portada_de_saga(&p).huella, saga[0].huella);
}

#[test]
fn el_siguiente_comic_es_el_que_toca_leer_despues_en_su_serie() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);
    let saga = p.biblioteca.comics_de_serie("saga").unwrap();

    let despues = p.biblioteca.siguiente_de(&saga[0].huella).unwrap();
    assert_eq!(despues.map(|f| f.huella), Some(saga[1].huella.clone()));
    // El último de la serie no tiene siguiente, ni un cómic que no está en la biblioteca.
    assert!(p
        .biblioteca
        .siguiente_de(&saga[1].huella)
        .unwrap()
        .is_none());
    assert!(p.biblioteca.siguiente_de("no-existe").unwrap().is_none());
}

#[test]
fn los_favoritos_se_guardan_ponen_al_dia_y_quitan() {
    let p = preparar();
    let favorito = |clave: &str, titulo: &str, anadido| Favorito {
        clave: clave.to_string(),
        titulo: titulo.to_string(),
        portada: Some(format!("https://ejemplo/{clave}.jpg")),
        linea: "Manga, Japón".to_string(),
        anadido,
    };
    assert!(p.biblioteca.favoritos().unwrap().is_empty());

    p.biblioteca
        .anadir_favorito(&favorito("anilist:1", "Uno", 100))
        .unwrap();
    p.biblioteca
        .anadir_favorito(&favorito("anilist:2", "Dos", 200))
        .unwrap();
    let claves: Vec<String> = p
        .biblioteca
        .favoritos()
        .unwrap()
        .into_iter()
        .map(|f| f.clave)
        .collect();
    assert_eq!(claves, ["anilist:2", "anilist:1"], "los más nuevos primero");

    // Guardarlo otra vez pone al día lo que se enseña, pero no cuándo se añadió.
    p.biblioteca
        .anadir_favorito(&favorito("anilist:1", "Uno bis", 999))
        .unwrap();
    let uno = p
        .biblioteca
        .favoritos()
        .unwrap()
        .into_iter()
        .find(|f| f.clave == "anilist:1")
        .unwrap();
    assert_eq!((uno.titulo.as_str(), uno.anadido), ("Uno bis", 100));

    p.biblioteca.quitar_favorito("anilist:2").unwrap();
    let claves: Vec<String> = p
        .biblioteca
        .favoritos()
        .unwrap()
        .into_iter()
        .map(|f| f.clave)
        .collect();
    assert_eq!(claves, ["anilist:1"]);
}

#[test]
fn una_carpeta_propia_guarda_comics_sueltos() {
    let temporal = TempDir::new().unwrap();
    let sueltos = temporal.path().join("descargas");
    crear_cbz(&sueltos.join("Maus.cbz"), 2, None);
    crear_cbz(&sueltos.join("Akira.cbz"), 3, None);
    escribir(&sueltos.join("notas.txt"));

    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    let carpeta = biblioteca.crear_carpeta_propia("  Sueltos ").unwrap();
    assert!(carpeta.propia);
    assert_eq!(carpeta.ruta, PathBuf::from("Sueltos"));
    // Vacía, sin nada que escanear.
    assert_eq!(escanear(&mut biblioteca), Resumen::default());
    assert!(biblioteca.crear_carpeta_propia("sueltos").is_err());
    assert!(biblioteca.crear_carpeta_propia("   ").is_err());

    // Los que no son cómics se saltan, y uno no entra dos veces.
    let archivos = [
        sueltos.join("Maus.cbz"),
        sueltos.join("Akira.cbz"),
        sueltos.join("notas.txt"),
        sueltos.join("Maus.cbz"),
    ];
    assert_eq!(
        biblioteca.anadir_archivos(carpeta.id, &archivos).unwrap(),
        2
    );
    assert_eq!(escanear(&mut biblioteca).nuevos, 2);
    assert_eq!(
        nombres_de_series(&biblioteca),
        [("Akira".to_string(), 1), ("Maus".to_string(), 1)]
    );
    let carpetas = biblioteca.carpetas().unwrap();
    assert_eq!(carpetas.len(), 1);
    assert_eq!(carpetas[0].comics, 2);

    // Escanear otra vez no cambia nada ni borra lo que no esté en una carpeta del disco.
    let resumen = escanear(&mut biblioteca);
    assert_eq!(
        (resumen.nuevos, resumen.sin_cambios, resumen.borrados),
        (0, 2, 0)
    );

    // Quitar uno lo saca de la carpeta y se puede volver a añadir.
    let huella = biblioteca.comics_de_serie("Maus").unwrap().remove(0).huella;
    biblioteca.quitar_comic(&huella).unwrap();
    assert_eq!(
        escanear(&mut biblioteca),
        Resumen {
            sin_cambios: 1,
            ..Resumen::default()
        }
    );
    assert_eq!(
        biblioteca
            .anadir_archivos(carpeta.id, &[sueltos.join("Maus.cbz")])
            .unwrap(),
        1
    );
    assert_eq!(escanear(&mut biblioteca).nuevos, 1);

    // Una carpeta del disco no admite cómics sueltos, y quitar la propia se lleva los suyos.
    let del_disco = biblioteca
        .agregar_carpeta(temporal.path().join("descargas"))
        .unwrap();
    assert!(biblioteca
        .anadir_archivos(del_disco.id, &[sueltos.join("Maus.cbz")])
        .is_err());
    biblioteca.quitar_carpeta(del_disco.id).unwrap();
    biblioteca.quitar_carpeta(carpeta.id).unwrap();
    assert!(biblioteca.series().unwrap().is_empty());
}

#[test]
fn se_pueden_meter_comics_y_carpetas_en_una_serie() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);
    let fuera = p._temporal.path().join("fuera");
    crear_cbz(&fuera.join("Cualquier cosa.cbz"), 5, None);
    crear_cbz(&fuera.join("lote/Tomo A.cbz"), 6, None);
    crear_cbz(&fuera.join("lote/dentro/Tomo B.cbz"), 7, None);
    escribir(&fuera.join("lote/notas.txt"));
    fs::create_dir_all(fuera.join("imagenes")).unwrap();
    fs::write(fuera.join("imagenes/1.jpg"), "a").unwrap();
    fs::write(fuera.join("imagenes/2.jpg"), "b").unwrap();

    let rutas = [
        fuera.join("Cualquier cosa.cbz"),
        fuera.join("lote"),
        fuera.join("imagenes"),
        // Uno que ya estaba en la biblioteca, en otra serie.
        p.comics.join("Fondos/Maus.cbz"),
        fuera.join("Cualquier cosa.cbz"),
    ];
    assert_eq!(p.biblioteca.anadir_a_serie("saga", &rutas).unwrap(), 5);
    assert!(p.biblioteca.anadir_a_serie("no existe", &rutas).is_err());
    // La carpeta oculta de los cómics sueltos no se ve.
    assert_eq!(p.biblioteca.carpetas().unwrap().len(), 1);

    let resumen = escanear(&mut p.biblioteca);
    assert_eq!(resumen.nuevos, 4);
    assert_eq!(
        nombres_de_series(&p.biblioteca),
        [
            ("Akira".to_string(), 1),
            ("Bleeding Hearts".to_string(), 2),
            ("roto".to_string(), 1),
            ("Saga".to_string(), 7),
        ]
    );
    assert_eq!(p.biblioteca.carpetas().unwrap().len(), 1);

    // Escanear otra vez lo deja igual, y volver a añadir lo mismo no cambia nada.
    let resumen = escanear(&mut p.biblioteca);
    assert_eq!((resumen.nuevos, resumen.borrados), (0, 0));
    assert_eq!(p.biblioteca.anadir_a_serie("saga", &rutas).unwrap(), 0);
    assert_eq!(p.biblioteca.comics_de_serie("saga").unwrap().len(), 7);

    // Un cómic metido a mano se quita como cualquier otro, y no vuelve solo.
    let huella = p
        .biblioteca
        .comics_de_serie("saga")
        .unwrap()
        .into_iter()
        .find(|f| f.ruta.ends_with("Cualquier cosa.cbz"))
        .unwrap()
        .huella;
    p.biblioteca.quitar_comic(&huella).unwrap();
    escanear(&mut p.biblioteca);
    assert_eq!(p.biblioteca.comics_de_serie("saga").unwrap().len(), 6);
}

#[test]
fn un_subtitulo_tras_el_nombre_no_parte_la_serie() {
    // Las Crónicas de Wormwood: sin número ni tomo, el nombre entero salía como serie.
    let temporal = TempDir::new().unwrap();
    let comics = temporal.path().join("comics");
    crear_cbz(&comics.join("Las Crónicas de Wormwood.cbz"), 2, None);
    crear_cbz(
        &comics.join("Las Crónicas de Wormwood - El Último Enemigo.cbz"),
        2,
        None,
    );
    crear_cbz(
        &comics.join("Las Crónicas de Wormwood - La Última Batalla.cbz"),
        2,
        None,
    );
    // Otra serie de la misma carpeta, que no tiene que mezclarse.
    crear_cbz(&comics.join("Batman Beyond 001.cbz"), 2, None);
    // Y otra carpeta con un nombre parecido, que tampoco.
    crear_cbz(&comics.join("Otra/Las Crónicas de Wormwood - Aparte.cbz"), 2, None);

    let mut biblioteca = Biblioteca::abrir(temporal.path().join("biblioteca.db")).unwrap();
    biblioteca.agregar_carpeta(&comics).unwrap();
    escanear(&mut biblioteca);
    // Una segunda pasada no cambia nada.
    escanear(&mut biblioteca);

    assert_eq!(
        nombres_de_series(&biblioteca),
        [
            ("Batman Beyond".to_string(), 1),
            ("Las Crónicas de Wormwood".to_string(), 3),
            ("Las Crónicas de Wormwood - Aparte".to_string(), 1),
        ]
    );
}

#[test]
fn los_comics_se_pueden_ordenar_a_mano_dentro_de_la_serie() {
    let mut p = preparar();
    escanear(&mut p.biblioteca);

    let huellas = |p: &Prueba| -> Vec<String> {
        p.biblioteca
            .comics_de_serie("saga")
            .unwrap()
            .into_iter()
            .map(|f| f.huella)
            .collect()
    };
    let original = huellas(&p);
    assert_eq!(original.len(), 2);
    assert!(!p.biblioteca.tiene_orden_manual("saga").unwrap());

    // El segundo pasa al principio.
    p.biblioteca
        .mover_comic("saga", &original[1], Movimiento::Principio)
        .unwrap();
    assert_eq!(huellas(&p), [original[1].clone(), original[0].clone()]);
    assert!(p.biblioteca.tiene_orden_manual("saga").unwrap());

    // El orden aguanta un reescaneo y manda sobre quién es el primero de la serie.
    escanear(&mut p.biblioteca);
    assert_eq!(huellas(&p), [original[1].clone(), original[0].clone()]);
    let serie = p
        .biblioteca
        .series()
        .unwrap()
        .into_iter()
        .find(|s| s.nombre == "Saga")
        .unwrap();
    assert_eq!(serie.portada.huella, original[1]);

    // Un cómic que llega después va al final.
    crear_cbz(&p.comics.join("Saga/Capítulo 1.cbz"), 5, None);
    escanear(&mut p.biblioteca);
    let ahora = huellas(&p);
    assert_eq!(ahora.len(), 3);
    assert_eq!(ahora[..2], [original[1].clone(), original[0].clone()]);

    // Mover el último "después" no hace nada; "antes" lo sube un sitio.
    p.biblioteca
        .mover_comic("saga", &ahora[2], Movimiento::Despues)
        .unwrap();
    assert_eq!(huellas(&p), ahora);
    p.biblioteca
        .mover_comic("saga", &ahora[2], Movimiento::Antes)
        .unwrap();
    assert_eq!(
        huellas(&p),
        [ahora[0].clone(), ahora[2].clone(), ahora[1].clone()]
    );

    // Restablecer vuelve al orden de lectura (1, 2, 10).
    p.biblioteca.restablecer_orden("saga").unwrap();
    assert!(!p.biblioteca.tiene_orden_manual("saga").unwrap());
    let numeros: Vec<_> = p
        .biblioteca
        .comics_de_serie("saga")
        .unwrap()
        .iter()
        .map(|f| f.metadatos.numero.clone())
        .collect();
    assert_eq!(
        numeros,
        [Some("1".to_string()), Some("2".to_string()), Some("10".to_string())]
    );
}

#[test]
fn soltar_un_comic_sobre_otro_ocupa_su_sitio() {
    let mut p = preparar();
    crear_cbz(&p.comics.join("Saga/Capítulo 1.cbz"), 5, None);
    escanear(&mut p.biblioteca);

    let huellas = |p: &Prueba| -> Vec<String> {
        p.biblioteca
            .comics_de_serie("saga")
            .unwrap()
            .into_iter()
            .map(|f| f.huella)
            .collect()
    };
    let [a, b, c] = <[String; 3]>::try_from(huellas(&p)).unwrap();

    // Hacia abajo: el primero ocupa el sitio del último.
    p.biblioteca.colocar_en_lugar_de("saga", &a, &c).unwrap();
    assert_eq!(huellas(&p), [b.clone(), c.clone(), a.clone()]);

    // Hacia arriba: el último ocupa el sitio del primero.
    p.biblioteca.colocar_en_lugar_de("saga", &a, &b).unwrap();
    assert_eq!(huellas(&p), [a.clone(), b.clone(), c.clone()]);

    // Sobre sí mismo o sobre algo que no está: no cambia nada.
    p.biblioteca.restablecer_orden("saga").unwrap();
    p.biblioteca.colocar_en_lugar_de("saga", &a, &a).unwrap();
    p.biblioteca.colocar_en_lugar_de("saga", &a, "no existe").unwrap();
    assert!(!p.biblioteca.tiene_orden_manual("saga").unwrap());
}
