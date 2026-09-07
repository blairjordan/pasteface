#!/usr/bin/env sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked
install -Dm755 target/release/pasteface "$HOME/.local/bin/pasteface"
mkdir -p "$HOME/.local/share/applications"
cat > "$HOME/.local/share/applications/pasteface.desktop" <<DESKTOP
[Desktop Entry]
Name=Pasteface
Comment=Local voice to text
Exec=$HOME/.local/bin/pasteface
Terminal=true
Type=Application
Categories=AudioVideo;Utility;
StartupWMClass=pasteface
DESKTOP
printf 'Installed. Run %s/.local/bin/pasteface\n' "$HOME"
