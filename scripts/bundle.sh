#!/bin/zsh
# Compila en release y genera target/release/DataCraft.app (opcional: --install copia a /Applications).
set -euo pipefail
cd "${0:A:h}/.."
source "$HOME/.cargo/env" 2>/dev/null || true

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
cargo build --release

APP=target/release/DataCraft.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/datacraft "$APP/Contents/MacOS/DataCraft"
cp assets/DataCraft.icns "$APP/Contents/Resources/DataCraft.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>DataCraft</string>
  <key>CFBundleDisplayName</key><string>DataCraft</string>
  <key>CFBundleExecutable</key><string>DataCraft</string>
  <key>CFBundleIdentifier</key><string>com.javy.datacraft</string>
  <key>CFBundleIconFile</key><string>DataCraft</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleDevelopmentRegion</key><string>es</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSRequiresAquaSystemAppearance</key><false/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>SQL Script</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>CFBundleTypeExtensions</key><array><string>sql</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST

# Firma: con un certificado estable el llavero reconoce cada compilación nueva como la misma
# app y no vuelve a pedir permiso para las contraseñas. Sin él, firma ad hoc (cambia en cada build).
IDENTITY="${DATACRAFT_SIGN_IDENTITY:-DataCraft Dev}"
if security find-certificate -c "$IDENTITY" >/dev/null 2>&1 \
   && codesign --force --deep --identifier com.javy.datacraft --sign "$IDENTITY" "$APP" 2>/dev/null; then
  echo "Firmado con el certificado «$IDENTITY»"
else
  codesign --force --deep --identifier com.javy.datacraft --sign - "$APP"
  echo "Aviso: firmado ad hoc; el llavero pedirá permiso tras cada instalación (crea el certificado «$IDENTITY»)"
fi
echo "Generado $APP"

if [[ "${1:-}" == "--install" ]]; then
  rm -rf /Applications/DataCraft.app
  cp -R "$APP" /Applications/
  echo "Instalado en /Applications/DataCraft.app"
fi
