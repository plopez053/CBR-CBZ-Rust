//! Escanea una carpeta y muestra lo que la biblioteca ha encontrado en ella.
//!
//! Uso: `cargo run -p comic-biblioteca --example escanear -- <carpeta> [biblioteca.db]`
//!
//! Sin `biblioteca.db`, usa una temporal que se borra al terminar.

use std::time::Instant;

use comic_biblioteca::Biblioteca;

fn main() -> Result<(), String> {
    let mut argumentos = std::env::args_os().skip(1);
    let carpeta = argumentos
        .next()
        .ok_or("Uso: escanear <carpeta> [biblioteca.db]")?;

    let temporal = std::env::temp_dir().join(format!("biblioteca-{}.db", std::process::id()));
    let ruta_db = argumentos.next().map_or(temporal.clone(), Into::into);

    let mut biblioteca = Biblioteca::abrir(&ruta_db)?;
    if let Err(e) = biblioteca.agregar_carpeta(&carpeta) {
        // Con una biblioteca ya existente, la carpeta puede estar añadida de antes.
        eprintln!("[ejemplo] {e}");
    }

    let inicio = Instant::now();
    let resumen = biblioteca.escanear(|hechos, total| {
        if hechos % 100 == 0 || hechos == total {
            eprintln!("[ejemplo] {hechos}/{total}");
        }
    })?;
    println!("Escaneo en {:.2?}: {resumen:?}\n", inicio.elapsed());

    for serie in biblioteca.series()? {
        println!(
            "{} ({} cómics, {} terminados)",
            serie.nombre, serie.comics, serie.terminados
        );
        for ficha in biblioteca.comics_de_serie(&serie.nombre)? {
            let m = &ficha.metadatos;
            let numero = m
                .numero
                .as_deref()
                .map_or(String::new(), |n| format!(" #{n}"));
            let volumen = m.volumen.map_or(String::new(), |v| format!(" vol. {v}"));
            let anio = m.anio.map_or(String::new(), |a| format!(" ({a})"));
            let detalle = match &ficha.error {
                Some(error) => format!("ERROR: {error}"),
                None => format!("{}, {} páginas", ficha.formato, ficha.paginas),
            };
            println!("  {volumen}{numero}{anio} - {detalle}");
            println!("    {}", ficha.ruta.display());
        }
    }

    drop(biblioteca);
    if ruta_db == temporal {
        for sufijo in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{sufijo}", temporal.display()));
        }
    }
    Ok(())
}
