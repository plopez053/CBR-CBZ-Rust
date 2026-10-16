use comic_core::leer_pagina;

#[test]
fn lee_el_contenido_de_una_pagina() {
    let bytes = leer_pagina("tests/fixtures/prueba.cbz", "2.jpg").unwrap();

    // Las "imágenes" del cbz de prueba son texto: "contenido falso de 2.jpg".
    assert!(bytes.starts_with(b"contenido falso de 2.jpg"));
}

#[test]
fn pagina_inexistente_devuelve_error() {
    assert!(leer_pagina("tests/fixtures/prueba.cbz", "99.jpg").is_err());
}
