#!/bin/sh
# Compila la versión final del lector y la instala para el usuario actual, con su entrada
# en el menú de aplicaciones y un acceso directo en el escritorio (si existe).
# Se puede volver a ejecutar para actualizar la app tras cambiar el código.
#
# Uso: sh instalar.sh

set -e
cd "$(dirname "$0")"

echo "Compilando la versión final (la primera vez tarda unos minutos)..."
cargo build --release -p comic-app

install -Dm755 target/release/comic-app "$HOME/.local/bin/lector-comics"
install -Dm644 assets/icono.png "$HOME/.local/share/icons/hicolor/256x256/apps/lector-comics.png"

# El nombre del archivo coincide con el `app_id` de la ventana: así el escritorio
# asocia la ventana abierta con su icono.
entrada="$HOME/.local/share/applications/lector-comics.desktop"
mkdir -p "$(dirname "$entrada")"
cat > "$entrada" <<FIN
[Desktop Entry]
Type=Application
Name=Lector de cómics
Comment=Lee y organiza tus cómics
Exec=$HOME/.local/bin/lector-comics %f
Icon=lector-comics
Terminal=false
Categories=Graphics;Viewer;
MimeType=application/vnd.comicbook+zip;application/vnd.comicbook-rar;application/x-cbz;application/x-cbr;application/x-cb7;application/x-cbt;
StartupWMClass=lector-comics
FIN

escritorio="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"
if [ -d "$escritorio" ]; then
    cp "$entrada" "$escritorio/"
    chmod +x "$escritorio/lector-comics.desktop"
fi

echo "Instalado en $HOME/.local/bin/lector-comics"
