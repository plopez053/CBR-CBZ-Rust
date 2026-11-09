//! Abrir direcciones web: con Brave si está instalado (con sus Shields, sin anuncios) y,
//! si no, con el navegador predeterminado del sistema.

use std::path::{Path, PathBuf};
use std::process::Command;

use eframe::egui;

/// Abre una dirección web. Intenta abrirla primero con el navegador Brave si está
/// instalado; si no, recurre al navegador predeterminado del sistema.
pub fn abrir_web(ctx: &egui::Context, url: &str) {
    if lanzar_brave(url) {
        return;
    }
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

/// Lanza Brave con esta dirección. Devuelve si lo ha conseguido.
fn lanzar_brave(url: &str) -> bool {
    for ruta in rutas_de_brave() {
        if ruta.exists() && Command::new(ruta).arg(url).spawn().is_ok() {
            return true;
        }
    }

    // Si está registrado en el PATH del sistema.
    #[cfg(windows)]
    let nombres = ["brave.exe", "brave"];
    #[cfg(not(windows))]
    let nombres = ["brave-browser", "brave"];

    nombres
        .into_iter()
        .any(|nombre| Command::new(nombre).arg(url).spawn().is_ok())
}

/// Dónde suele instalarse Brave.
fn rutas_de_brave() -> Vec<PathBuf> {
    let mut rutas = Vec::new();

    #[cfg(windows)]
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(variable) {
            rutas.push(
                Path::new(&base)
                    .join("BraveSoftware")
                    .join("Brave-Browser")
                    .join("Application")
                    .join("brave.exe"),
            );
        }
    }

    #[cfg(target_os = "macos")]
    rutas.push(PathBuf::from(
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    ));

    rutas
}
