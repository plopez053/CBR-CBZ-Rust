//! Comprueba las novedades de una biblioteca sin la interfaz y muestra lo que sale.
//!
//! Uso: `cargo run -p comic-novedades --example comprobar -- <biblioteca.db> [rss...]`
//!
//! Con las variables `METRON_USUARIO` y `METRON_CONTRASENA` consulta también Metron.
//! Ojo: guarda en la biblioteca a qué serie corresponde cada una.

use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use comic_novedades::{comprobar, Ajustes, Aviso, Cuenta, Informe, Parte};

fn main() -> Result<(), String> {
    let mut argumentos = std::env::args().skip(1);
    let ruta_db = argumentos
        .next()
        .ok_or("Uso: comprobar <biblioteca.db> [rss...]")?;
    let metron = match (
        std::env::var("METRON_USUARIO"),
        std::env::var("METRON_CONTRASENA"),
    ) {
        (Ok(usuario), Ok(contrasena)) => Some(Cuenta {
            usuario,
            contrasena,
        }),
        _ => None,
    };
    let ajustes = Ajustes {
        metron,
        canales: argumentos.collect(),
    };

    // Las cuatro partes a la vez, como si se entrara en cada pestaña.
    let inicio = Instant::now();
    let juntas = Mutex::new(Informe::default());
    thread::scope(|hilos| {
        for parte in [Parte::Series, Parte::Manga, Parte::Semana, Parte::Noticias] {
            let (ajustes, ruta_db, juntas) = (&ajustes, &ruta_db, &juntas);
            hilos.spawn(move || {
                comprobar(
                    ruta_db.as_ref(),
                    ajustes,
                    Informe::default(),
                    parte,
                    |aviso| {
                        if let Aviso::Fin(informe) = aviso {
                            eprintln!(
                                "[ejemplo] {parte:?} en {:.1} s",
                                inicio.elapsed().as_secs_f32()
                            );
                            juntas.lock().unwrap().copiar(&informe, parte);
                        }
                    },
                );
            });
        }
    });
    let final_ = juntas.into_inner().unwrap();

    for serie in &final_.series {
        println!(
            "{} -> {} ({}): tienes {:?}, publicado {:?}, {} más, {}",
            serie.nombre,
            serie.titulo,
            serie.fuente.nombre(),
            serie.tienes,
            serie.publicado,
            serie.mas,
            serie.publicacion.nombre()
        );
    }
    if let Some(semana) = &final_.semana {
        println!(
            "Esta semana ({} - {}): {} números",
            semana.desde,
            semana.hasta,
            semana.numeros.len()
        );
    }
    if let Some(manga) = &final_.manga {
        for (nombre, lista) in [
            ("Para ti", &manga.para_ti),
            ("En tendencia", &manga.tendencia),
            ("Nuevas", &manga.nuevas),
        ] {
            let titulos: Vec<&str> = lista
                .fichas
                .iter()
                .take(5)
                .map(|f| f.titulo.as_str())
                .collect();
            println!(
                "{nombre} ({}, hay más: {}): {}",
                lista.fichas.len(),
                lista.hay_mas,
                titulos.join(", ")
            );
        }
    }
    let con_foto = final_
        .noticias
        .iter()
        .filter(|n| n.imagen.is_some())
        .count();
    println!("Noticias: {}, {con_foto} con foto", final_.noticias.len());
    for noticia in final_.noticias.iter().take(8) {
        println!(
            "[{}] {} ({:?})",
            noticia.fuente, noticia.titulo, noticia.imagen
        );
    }
    for (parte, fallos) in &final_.fallos {
        for fallo in fallos {
            println!("Fallo ({parte:?}): {fallo}");
        }
    }
    Ok(())
}
