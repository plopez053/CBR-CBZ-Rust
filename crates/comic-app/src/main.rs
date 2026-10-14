use comic_core::listar_paginas;

fn main() {
    // args() devuelve los argumentos de la línea de comandos; el 0 es el propio programa.
    let Some(ruta) = std::env::args().nth(1) else {
        println!("Uso: comic-app <archivo.cbz>");
        return;
    };

    match listar_paginas(&ruta) {
        Ok(paginas) => {
            println!("{} páginas en {ruta}:", paginas.len());
            for pagina in paginas {
                println!("  {pagina}");
            }
        }
        Err(e) => eprintln!("No se pudo abrir {ruta}: {e}"),
    }
}
