//! Cada test monta una biblioteca de verdad en una carpeta temporal, con cómics
//! creados al vuelo (las "imágenes" son texto: para escanear no hace falta decodificarlas).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use comic_biblioteca::{Biblioteca, Resumen};
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
