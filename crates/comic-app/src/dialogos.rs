//! Selectores de archivos y carpetas del sistema (los de Windows, o los del escritorio
//! en Linux a través de `xdg-desktop-portal`).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

/// Extensiones que se ven en el selector de cómics. En Linux el filtro distingue
/// mayúsculas, así que van también en mayúsculas.
const EXTENSIONES: [&str; 16] = [
    "cbz", "cbr", "cb7", "cbt", "zip", "rar", "7z", "tar", "CBZ", "CBR", "CB7", "CBT", "ZIP",
    "RAR", "7Z", "TAR",
];

#[derive(Clone, Copy)]
pub enum Dialogo {
    /// Una carpeta para añadir a la biblioteca.
    CarpetaDeBiblioteca,
    /// Un cómic suelto para leerlo sin añadirlo.
    Comic,
    /// Una carpeta de imágenes para leerla como cómic.
    CarpetaDeImagenes,
}

/// Abre el selector. El resultado llega por el canal: `None` si se cancela.
///
/// En Windows se abre en el hilo de la ventana y por encima de ella, como hacen todos
/// los programas. En Linux va en otro hilo: el portal tarda en contestar y, si la
/// ventana dejara de responder mientras tanto, el escritorio avisaría de que la app
/// se ha colgado.
pub fn abrir(dialogo: Dialogo, ventana: &eframe::Frame) -> Receiver<Option<PathBuf>> {
    let (enviar, recibir) = mpsc::channel();

    #[cfg(windows)]
    {
        let selector = selector(dialogo).set_parent(ventana);
        let _ = enviar.send(elegir(selector, dialogo));
    }

    #[cfg(not(windows))]
    {
        let _ = ventana;
        std::thread::spawn(move || {
            let _ = enviar.send(elegir(selector(dialogo), dialogo));
        });
    }

    recibir
}

fn selector(dialogo: Dialogo) -> rfd::FileDialog {
    let selector = rfd::FileDialog::new();
    match dialogo {
        Dialogo::CarpetaDeBiblioteca => {
            selector.set_title("Añadir una carpeta de cómics a la biblioteca")
        }
        Dialogo::Comic => selector
            .set_title("Abrir un cómic")
            .add_filter("Cómics", &EXTENSIONES),
        Dialogo::CarpetaDeImagenes => selector.set_title("Abrir una carpeta de imágenes"),
    }
}

fn elegir(selector: rfd::FileDialog, dialogo: Dialogo) -> Option<PathBuf> {
    match dialogo {
        Dialogo::Comic => selector.pick_file(),
        Dialogo::CarpetaDeBiblioteca | Dialogo::CarpetaDeImagenes => selector.pick_folder(),
    }
}
