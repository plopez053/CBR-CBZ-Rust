//! Los mismos archivos (`1.jpg`, `2.jpg`, `10.jpg`, `notas.txt` y `ComicInfo.xml`)
//! guardados en cada formato. Dentro de los rar, 7z y tar van desordenados a propósito.

use comic_core::{Comic, Formato};

const FIXTURES: [(&str, Formato); 7] = [
    ("tests/fixtures/prueba.cbz", Formato::Zip),
    ("tests/fixtures/prueba.cbr", Formato::Rar),
    ("tests/fixtures/prueba_solida.cbr", Formato::Rar),
    ("tests/fixtures/prueba.cb7", Formato::SieteZip),
    ("tests/fixtures/prueba.cbt", Formato::Tar),
    ("tests/fixtures/prueba_carpeta", Formato::Carpeta),
    // Un zip con extensión de rar: se reconoce por el contenido.
    ("tests/fixtures/zip_disfrazado.cbr", Formato::Zip),
];

#[test]
fn detecta_el_formato_por_el_contenido() {
    for (ruta, formato) in FIXTURES {
        let comic = Comic::abrir(ruta).unwrap_or_else(|e| panic!("{ruta}: {e}"));
        assert_eq!(comic.formato(), formato, "{ruta}");
    }
}

#[test]
fn todos_listan_las_mismas_paginas_en_orden_natural() {
    for (ruta, _) in FIXTURES {
        let comic = Comic::abrir(ruta).unwrap();
        assert_eq!(comic.paginas(), ["1.jpg", "2.jpg", "10.jpg"], "{ruta}");
    }
}

#[test]
fn lee_las_paginas_en_cualquier_orden() {
    // Hacia delante, hacia atrás y repitiendo: en el rar obliga a reabrir el cursor.
    for (ruta, _) in FIXTURES {
        let mut comic = Comic::abrir(ruta).unwrap();
        for indice in [0, 1, 2, 0, 2, 1, 1] {
            let esperado = format!("contenido falso de {}", comic.paginas()[indice]);
            let bytes = comic.leer(indice).unwrap_or_else(|e| panic!("{ruta}: {e}"));
            assert!(
                bytes.starts_with(esperado.as_bytes()),
                "{ruta}, página {indice}"
            );
        }
    }
}

#[test]
fn rechaza_lo_que_no_es_un_comic() {
    assert!(Comic::abrir("Cargo.toml").is_err());
}
