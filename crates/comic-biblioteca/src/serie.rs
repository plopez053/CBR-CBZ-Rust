//! Cuándo dos nombres de serie son la misma serie.

use std::collections::HashMap;

use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

/// Clave con la que se agrupan los cómics en series: el nombre sin mayúsculas, sin tildes
/// y sin puntuación. `Vinland Saga`, `vinland-saga` y `VINLAND SAGA!` dan la misma clave.
pub fn clave_de_serie(nombre: &str) -> String {
    let sin_tildes: String = nombre
        .nfd()
        .filter(|&c| !is_combining_mark(c))
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let clave = sin_tildes.split_whitespace().collect::<Vec<_>>().join(" ");

    // Un nombre sin ninguna letra ni cifra ("???") no se puede dejar vacío: se
    // mezclaría con todos los demás.
    if clave.is_empty() {
        nombre.trim().to_lowercase()
    } else {
        clave
    }
}

/// El nombre con el que se muestra un grupo de series que son la misma: el más repetido
/// entre sus cómics. Si empatan, el más corto, y después el primero por orden alfabético.
pub fn nombre_de_grupo<'a>(nombres: impl Iterator<Item = &'a str>) -> String {
    let mut cuentas: HashMap<&str, usize> = HashMap::new();
    for nombre in nombres {
        *cuentas.entry(nombre).or_default() += 1;
    }
    cuentas
        .into_iter()
        .max_by(|(a, n), (b, m)| {
            n.cmp(m)
                .then_with(|| b.chars().count().cmp(&a.chars().count()))
                .then_with(|| b.cmp(a))
        })
        .map(|(nombre, _)| nombre.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mayusculas_tildes_y_puntuacion_no_cuentan() {
        let esperada = clave_de_serie("Vinland Saga");
        assert_eq!(esperada, "vinland saga");
        assert_eq!(clave_de_serie("VINLAND  SAGA"), esperada);
        assert_eq!(clave_de_serie("vinland-saga"), esperada);
        assert_eq!(clave_de_serie("Vinland Saga -"), esperada);
        assert_eq!(clave_de_serie("Vinland Saga!"), esperada);
        assert_eq!(
            clave_de_serie("Y: el último hombre"),
            clave_de_serie("Y el ultimo hombre")
        );
        assert_eq!(clave_de_serie("Ñoño"), "nono");
    }

    #[test]
    fn series_distintas_siguen_distintas() {
        assert_ne!(clave_de_serie("Vinland"), clave_de_serie("Vinland Saga"));
        assert_ne!(clave_de_serie("Batman"), clave_de_serie("Batman 66"));
        assert_ne!(clave_de_serie("Saga"), clave_de_serie("Sagas"));
    }

    #[test]
    fn un_nombre_sin_letras_no_se_mezcla_con_otros() {
        assert_eq!(clave_de_serie("???"), "???");
        assert_ne!(clave_de_serie("???"), clave_de_serie("!!!"));
    }

    #[test]
    fn el_nombre_del_grupo_es_el_mas_repetido() {
        let nombres = ["Vinland", "Vinland Saga", "Vinland Saga", "Vinland"];
        // Empatan a 2: gana el más corto.
        assert_eq!(nombre_de_grupo(nombres.into_iter()), "Vinland");
        let nombres = ["Vinland", "Vinland Saga", "Vinland Saga"];
        assert_eq!(nombre_de_grupo(nombres.into_iter()), "Vinland Saga");
    }
}
