"""Dibuja "comic" a mano alzada, como con el pincel del Paint: trazos torcidos, sin
suavizado y con los colores de la paleta clasica. Uso: python herramientas/icono_paint.py [carpeta_destino]
Sin argumento, escribe icono.png e icono.ico en la carpeta assets del proyecto."""
import math
import random
import sys
from pathlib import Path

from PIL import Image, ImageDraw
from numpy import mat

random.seed(7)

LADO = 128          # se dibuja pequeno y se amplia sin suavizar: pixeles gordos
ESCALA = 2          # 128 * 2 = 256


def arco(cx, cy, rx, ry, desde, hasta, n=40):
    return [(cx + rx * math.cos(math.radians(a)), cy + ry * math.sin(math.radians(a)))
            for a in (desde + (hasta - desde) * i / n for i in range(n + 1))]


def linea(x0, y0, x1, y1, n=12):
    return [(x0 + (x1 - x0) * i / n, y0 + (y1 - y0) * i / n) for i in range(n + 1)]



def temblor(puntos, fuerza=2.0):
    """Pulso de raton: un desvio que va cambiando poco a poco."""
    dx = dy = 0.0
    salida = []
    for x, y in puntos:
        dx = dx * 0.7 + random.uniform(-fuerza, fuerza)
        dy = dy * 0.7 + random.uniform(-fuerza, fuerza)
        salida.append((x + dx, y + dy))
    return salida


def letras():
    """Cada letra es una lista de trazos; cada trazo, una lista de puntos.
    Tamanos y alturas desiguales a proposito."""
    c1 = [arco(19, 64, 12, 17, 40, 320)]
    o = [arco(45, 60, 11, 15, -100, 275)]                        # no cierra bien
    m = [linea(62, 76, 63, 50),
         arco(70, 54, 7, 7, 180, 360, 12) + linea(77, 54, 77, 75),
         arco(84, 56, 7, 7, 180, 360, 12) + linea(91, 56, 92, 78)]
    i = [linea(101, 52, 100, 77), [(101, 38), (101.5, 39)]]       # el punto, un borron
    c2 = [arco(117, 62, 9, 18, 45, 315)]
    return [c1, o, m, i, c2]


def main():
    assets = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent / "assets"
    lienzo = Image.new("RGB", (LADO, LADO), (255, 255, 255))
    d = ImageDraw.Draw(lienzo)

    # Todo un poco inclinado, como escrito deprisa.
    giro = math.radians(-8)
    cx, cy = LADO / 2, LADO / 2

    def girar(p):
        x, y = p[0] - cx, p[1] - cy
        return (cx + x * math.cos(giro) - y * math.sin(giro),
                cy + x * math.sin(giro) + y * math.cos(giro))

    for letra in letras():
        pincel = random.triangular(1.0, 8.0, 4.0)  # Grosor diferente para cada letra
        for trazo in letra:
            puntos = temblor([girar(p) for p in trazo])
            # Se estampa el pincel redondo en cada punto, con relleno entre medias.
            for (x0, y0), (x1, y1) in zip(puntos, puntos[1:] + puntos[-1:]):
                pasos = max(1, int(math.hypot(x1 - x0, y1 - y0)))
                for k in range(pasos + 1):
                    x = x0 + (x1 - x0) * k / pasos
                    y = y0 + (y1 - y0) * k / pasos
                    d.ellipse([x - pincel, y - pincel, x + pincel, y + pincel], fill=(0, 0, 0))

    # Borde negro de 1 pixel, como el lienzo del Paint.
    d.rectangle([0, 0, LADO - 1, LADO - 1], outline=(0, 0, 0))

    grande = lienzo.resize((LADO * ESCALA, LADO * ESCALA), Image.NEAREST)
    grande.save(assets / "icono2.png")
    grande.save(assets / "icono2.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48),
                                              (64, 64), (128, 128), (256, 256)])
    print("[icono] Guardado en", assets)


if __name__ == "__main__":
    main()
