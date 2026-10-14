use comic_core::listar_paginas;

#[test]
fn lista_solo_las_imagenes() {
    let paginas = listar_paginas("tests/fixtures/prueba.cbz").unwrap();

    assert_eq!(paginas.len(), 3);
    assert!(paginas.contains(&"1.jpg".to_string()));
    assert!(!paginas.contains(&"notas.txt".to_string()));
    assert!(!paginas.contains(&"ComicInfo.xml".to_string()));
}

#[test]
fn las_paginas_salen_en_orden_natural() {
    let paginas = listar_paginas("tests/fixtures/prueba.cbz").unwrap();

    assert_eq!(paginas, vec!["1.jpg", "2.jpg", "10.jpg"]);
}

#[test]
fn archivo_inexistente_devuelve_error() {
    assert!(listar_paginas("tests/fixtures/no_existe.cbz").is_err());
}
