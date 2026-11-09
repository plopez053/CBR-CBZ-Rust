//! Fechas sin zona horaria (las de salida de los cómics) y las de las noticias, que
//! llegan en RFC 2822 (RSS) o RFC 3339 (Atom). Lo justo para esto, sin dependencias.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const MESES: [&str; 12] = [
    "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
];

/// Un día del calendario.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Fecha {
    pub anio: i32,
    pub mes: u32,
    pub dia: u32,
}

impl Fecha {
    /// Lee `2026-10-19` (y lo que venga detrás, como `T12:00:00Z`, se ignora).
    pub fn leer(texto: &str) -> Option<Fecha> {
        let mut partes = texto.get(..10)?.split('-');
        let anio = partes.next()?.parse().ok()?;
        let mes = partes.next()?.parse().ok()?;
        let dia = partes.next()?.parse().ok()?;
        let fecha = Fecha { anio, mes, dia };
        fecha.valida().then_some(fecha)
    }

    fn valida(self) -> bool {
        (1..=12).contains(&self.mes) && self.dia >= 1 && self.dia <= dias_del_mes(self)
    }

    /// Hoy, en UTC.
    pub fn hoy() -> Fecha {
        Fecha::desde_dias(ahora().div_euclid(86_400))
    }

    /// Días desde el 1 de enero de 1970.
    pub fn dias(self) -> i64 {
        // Algoritmo de Howard Hinnant (days_from_civil).
        let anio = i64::from(self.anio) - i64::from(self.mes <= 2);
        let era = anio.div_euclid(400);
        let del_era = anio - era * 400;
        let mes = i64::from(self.mes);
        let del_anio =
            (153 * (mes + if mes > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(self.dia) - 1;
        let del_era_dias = del_era * 365 + del_era / 4 - del_era / 100 + del_anio;
        era * 146_097 + del_era_dias - 719_468
    }

    pub fn desde_dias(dias: i64) -> Fecha {
        // Algoritmo de Howard Hinnant (civil_from_days).
        let z = dias + 719_468;
        let era = z.div_euclid(146_097);
        let del_era = z - era * 146_097;
        let anio_era = (del_era - del_era / 1460 + del_era / 36_524 - del_era / 146_096) / 365;
        let del_anio = del_era - (365 * anio_era + anio_era / 4 - anio_era / 100);
        let mp = (5 * del_anio + 2) / 153;
        let dia = (del_anio - (153 * mp + 2) / 5 + 1) as u32;
        let mes = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let anio = (anio_era + era * 400 + i64::from(mes <= 2)) as i32;
        Fecha { anio, mes, dia }
    }

    pub fn mas_dias(self, dias: i64) -> Fecha {
        Fecha::desde_dias(self.dias() + dias)
    }

    /// El lunes de la semana de esta fecha.
    pub fn lunes(self) -> Fecha {
        // El 1 de enero de 1970 fue jueves: con +3, el lunes da 0.
        let desde_lunes = (self.dias() + 3).rem_euclid(7);
        self.mas_dias(-desde_lunes)
    }

    /// `2026-10-19`, como lo piden las API.
    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.anio, self.mes, self.dia)
    }

    /// `19 oct`
    pub fn corta(self) -> String {
        format!("{} {}", self.dia, MESES[(self.mes - 1) as usize])
    }
}

impl fmt::Display for Fecha {
    /// `19 oct 2026`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.corta(), self.anio)
    }
}

fn dias_del_mes(fecha: Fecha) -> u32 {
    match fecha.mes {
        2 if fecha.anio % 4 == 0 && (fecha.anio % 100 != 0 || fecha.anio % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Segundos desde 1970, ahora.
pub fn ahora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Lee una fecha de noticia, en RFC 2822 (`Mon, 19 Oct 2026 15:08:56 +0000`, la de RSS)
/// o RFC 3339 (`2026-10-19T15:08:56+02:00`, la de Atom). Devuelve segundos desde 1970.
pub fn leer_instante(texto: &str) -> Option<i64> {
    let texto = texto.trim();
    if texto.starts_with(|c: char| c.is_ascii_digit()) && texto.get(4..5) == Some("-") {
        rfc3339(texto)
    } else {
        rfc2822(texto)
    }
}

fn rfc3339(texto: &str) -> Option<i64> {
    let fecha = Fecha::leer(texto)?;
    let resto = texto.get(10..).unwrap_or("");
    let Some(hora) = resto.strip_prefix(['T', 't', ' ']) else {
        return Some(fecha.dias() * 86_400);
    };
    let segundos = segundos_del_dia(hora.get(..8).or_else(|| hora.get(..5))?)?;
    // Detrás de la hora pueden venir fracciones de segundo y luego la zona.
    let zona = hora
        .get(5..)
        .unwrap_or("")
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == ':' || c == '.');
    Some(fecha.dias() * 86_400 + segundos - desfase(zona)?)
}

fn rfc2822(texto: &str) -> Option<i64> {
    // El día de la semana es opcional.
    let texto = match texto.split_once(',') {
        Some((_, resto)) => resto,
        None => texto,
    };
    let mut partes = texto.split_whitespace();
    let dia = partes.next()?.parse().ok()?;
    let mes_texto = partes.next()?.to_ascii_lowercase();
    let mes = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| mes_texto.starts_with(m))? as u32
        + 1;
    let mut anio: i32 = partes.next()?.parse().ok()?;
    if anio < 100 {
        anio += if anio < 50 { 2000 } else { 1900 };
    }
    let fecha = Fecha { anio, mes, dia };
    if !fecha.valida() {
        return None;
    }
    let segundos = match partes.next() {
        Some(hora) => segundos_del_dia(hora)?,
        None => 0,
    };
    let zona = partes.next().unwrap_or("Z");
    Some(fecha.dias() * 86_400 + segundos - desfase(zona)?)
}

/// `15:08:56` o `15:08`.
fn segundos_del_dia(hora: &str) -> Option<i64> {
    let mut partes = hora.split(':');
    let h: i64 = partes.next()?.parse().ok()?;
    let m: i64 = partes.next()?.parse().ok()?;
    let s: i64 = partes.next().map_or(Some(0), |s| s.parse().ok())?;
    (h < 24 && m < 60 && s <= 60).then_some(h * 3600 + m * 60 + s)
}

/// Cuántos segundos va la zona por delante de UTC: `+0200`, `+02:00`, `Z`, `GMT`...
fn desfase(zona: &str) -> Option<i64> {
    let zona = zona.trim();
    // Las zonas por nombre que admite RFC 2822.
    let horas = match zona.to_ascii_uppercase().as_str() {
        "" | "Z" | "GMT" | "UT" | "UTC" => Some(0),
        "EDT" => Some(-4),
        "EST" | "CDT" => Some(-5),
        "CST" | "MDT" => Some(-6),
        "MST" | "PDT" => Some(-7),
        "PST" => Some(-8),
        _ => None,
    };
    if let Some(horas) = horas {
        return Some(horas * 3600);
    }
    let signo = match zona.chars().next()? {
        '+' => 1,
        '-' => -1,
        // Una zona por nombre que no se conoce: mejor la hora casi bien que nada.
        _ => return Some(0),
    };
    let cifras: String = zona[1..].chars().filter(char::is_ascii_digit).collect();
    if cifras.len() != 4 {
        return None;
    }
    let horas: i64 = cifras[..2].parse().ok()?;
    let minutos: i64 = cifras[2..].parse().ok()?;
    Some(signo * (horas * 3600 + minutos * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dias_y_vuelta() {
        let fecha = Fecha {
            anio: 2026,
            mes: 10,
            dia: 19,
        };
        assert_eq!(Fecha::desde_dias(fecha.dias()), fecha);
        assert_eq!(Fecha::desde_dias(0).iso(), "1970-01-01");
        assert_eq!(
            Fecha::leer("2024-02-29").unwrap().mas_dias(1).iso(),
            "2024-03-01"
        );
        assert_eq!(Fecha::leer("2023-02-29"), None);
        assert_eq!(fecha.corta(), "19 oct");
    }

    #[test]
    fn el_lunes_de_la_semana() {
        // El 19 de octubre de 2026 es lunes y el 25, domingo.
        let lunes = Fecha::leer("2026-10-19").unwrap();
        assert_eq!(lunes.lunes(), lunes);
        assert_eq!(Fecha::leer("2026-10-25").unwrap().lunes(), lunes);
        assert_eq!(Fecha::leer("2026-10-22").unwrap().lunes(), lunes);
        assert_eq!(
            Fecha::leer("2026-10-26").unwrap().lunes().iso(),
            "2026-10-26"
        );
    }

    #[test]
    fn fechas_de_noticias() {
        let esperado = Fecha::leer("2026-10-19").unwrap().dias() * 86_400 + 15 * 3600 + 8 * 60;
        assert_eq!(
            leer_instante("Mon, 19 Oct 2026 15:08:00 +0000"),
            Some(esperado)
        );
        assert_eq!(
            leer_instante("Mon, 19 Oct 2026 17:08:00 +0200"),
            Some(esperado)
        );
        assert_eq!(leer_instante("19 Oct 2026 15:08:00 GMT"), Some(esperado));
        assert_eq!(leer_instante("2026-10-19T15:08:00Z"), Some(esperado));
        assert_eq!(leer_instante("2026-10-19T17:08:00+02:00"), Some(esperado));
        assert_eq!(leer_instante("2026-10-19T15:08:00.123Z"), Some(esperado));
        assert_eq!(leer_instante("2026-10-19T10:08:00-05:00"), Some(esperado));
        assert_eq!(leer_instante("ayer por la tarde"), None);
    }
}
