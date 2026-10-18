//! Un módulo por formato de cómic. Todos acaban en un [`Contenedor`](crate::Contenedor),
//! y el resto del programa solo habla con [`Comic`](crate::Comic).

pub mod carpeta;
pub mod cb7;
pub mod cbr;
pub mod cbt;
pub mod cbz;
