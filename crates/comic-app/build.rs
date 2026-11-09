//! Mete el icono y el nombre de la app dentro del `.exe` de Windows, para que se vean en
//! el explorador, en la barra de tareas y en los accesos directos.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icono.ico");

    // `src/fuentes.rs` es opcional y no se sube a git. Si existe, se compila y aparece su
    // pestaña en Novedades; si no, la app es idéntica y no sabe que existe.
    println!("cargo:rerun-if-changed=src/fuentes.rs");
    println!("cargo:rustc-check-cfg=cfg(fuentes)");
    if std::path::Path::new("src/fuentes.rs").exists() {
        println!("cargo:rustc-cfg=fuentes");
    }

    // Se mira el sistema para el que se compila, no en el que se compila.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    #[cfg(windows)]
    {
        let mut recursos = winresource::WindowsResource::new();
        recursos
            .set_icon("../../assets/icono.ico")
            .set("ProductName", "Lector de cómics")
            .set("FileDescription", "Lector de cómics");
        if let Err(e) = recursos.compile() {
            // Sin el compilador de recursos de Windows la app funciona igual, solo que
            // sin icono en el .exe: mejor avisar que impedir compilar.
            println!("cargo:warning=No se pudo poner el icono al .exe: {e}");
        }
    }
}
