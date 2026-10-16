# DataCraft

DataGrip-style database client for macOS (Islands theme), written in Rust with egui.

![DataCraft table data editor](docs/screenshots/table-editor.png)

- **Engines:** PostgreSQL, SQL Server and SQLite.
- **Database Explorer:** every database on the server (loaded on expand), schemas, tables, views, columns and primary keys.
- **SQL consoles:** highlighting, autocompletion, ⌘↩ (statement under the cursor or selection) and ⌘⇧↩ (everything; `GO` batches on SQL Server). Each console runs against the database you pick.
- **Table data editor:** WHERE / ORDER BY filters, per-column value filters, in-place editing (update, insert, delete rows) with a preview of the generated SQL, saved in a single transaction.
- **Export:** results to CSV, TSV, JSON, SQL `INSERT`s or Markdown, to the clipboard or a file.
- **Zoom:** IDE zoom (⌘+ / ⌘− / ⌘0) and editor font size (⌥⌘+ / ⌥⌘−).
- **Native macOS menu bar** with the usual shortcuts.
- **Languages:** English and Spanish; follows the system language and can be changed in Settings.
- **Passwords:** stored in the macOS Keychain, never in the connections file.

![About DataCraft](docs/screenshots/about.png)

## Download

Get the latest `.dmg` (universal: Apple Silicon and Intel, macOS 11+) from
[Releases](https://github.com/javy-fernandez/datacraft/releases). The app is not notarized: the first
time, right-click DataCraft.app → **Open**, or run `xattr -dr com.apple.quarantine /Applications/DataCraft.app`.

## Build

```bash
cargo run                                  # development
scripts/bundle.sh --install                # optimized DataCraft.app in /Applications
scripts/bundle.sh --universal --dmg        # universal app + .dmg (needs: rustup target add x86_64-apple-darwin)
```

### Code signing (optional)

`bundle.sh` signs the app with a certificate named **DataCraft** if it exists (or the one in
`DATACRAFT_SIGN_IDENTITY`). With a stable certificate the Keychain recognizes every new build
as the same app and doesn't ask again for access to the saved passwords; without it the app is
signed ad hoc and macOS asks after every install.

To create it: open **Keychain Access → Certificate Assistant → Create a Certificate…**, name it
`DataCraft`, choose *Identity Type: Self-Signed Root* and *Certificate Type: Code Signing*, and
create it in the *login* keychain.

## Tests

```bash
cargo test
scripts/test_mssql.sh <host>   # real SQL Server (non-destructive: only #temp tables), asks for the password
```

The PostgreSQL live tests run with `cargo test -- --ignored` and the `DATACRAFT_PG_*` variables
(see `src/db/postgres.rs`).

Demo mode for screenshots: `DATACRAFT_DEMO=path.db cargo run` (see the header of `src/app.rs`).

## License

DataCraft is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Bundled fonts keep their own open license (SIL OFL 1.1); see [assets/fonts/README.md](assets/fonts/README.md).

## Español

Cliente de bases de datos para macOS al estilo de DataGrip (tema Islands), escrito en Rust con egui.

- **Motores:** PostgreSQL, SQL Server y SQLite.
- **Database Explorer:** todas las bases del servidor (se cargan al expandirlas), esquemas, tablas, vistas, columnas y claves primarias.
- **Consolas SQL:** resaltado, autocompletado, ⌘↩ (sentencia bajo el cursor o selección) y ⌘⇧↩ (todo; lotes `GO` en SQL Server). Cada consola ejecuta en la base que elijas.
- **Editor de datos de tabla:** filtros WHERE / ORDER BY, filtros de valores por columna, edición en el sitio (modificar, insertar y borrar filas) con vista previa del SQL generado, que se guarda en una sola transacción.
- **Exportación:** resultados a CSV, TSV, JSON, `INSERT`s SQL o Markdown, al portapapeles o a un fichero.
- **Zoom:** zoom del IDE (⌘+ / ⌘− / ⌘0) y tamaño de fuente del editor (⌥⌘+ / ⌥⌘−).
- **Menú nativo de macOS** con los atajos habituales.
- **Idiomas:** inglés y español; sigue el idioma del sistema y se puede cambiar en Ajustes.
- **Contraseñas:** se guardan en el Llavero de macOS, nunca en el fichero de conexiones.

### Descarga

Descarga el último `.dmg` (universal: Apple Silicon e Intel, macOS 11+) desde
[Releases](https://github.com/javy-fernandez/datacraft/releases). La app no está notarizada: la primera
vez, clic derecho en DataCraft.app → **Abrir**, o ejecuta `xattr -dr com.apple.quarantine /Applications/DataCraft.app`.

### Compilar

```bash
cargo run                                  # desarrollo
scripts/bundle.sh --install                # DataCraft.app optimizada en /Applications
scripts/bundle.sh --universal --dmg        # app universal + .dmg (requiere: rustup target add x86_64-apple-darwin)
```

**Firma (opcional):** `bundle.sh` firma la app con un certificado llamado **DataCraft** si existe
(o con el de `DATACRAFT_SIGN_IDENTITY`). Con un certificado estable el Llavero reconoce cada
compilación nueva como la misma app y no vuelve a pedir acceso a las contraseñas guardadas; sin
él la firma es ad hoc y macOS lo pide tras cada instalación.

Para crearlo: abre **Acceso a Llaveros → Asistente de certificados → Crear un certificado…**,
llámalo `DataCraft`, elige *Tipo de identidad: Raíz autofirmada* y *Tipo de certificado: Firma de
código*, y créalo en el llavero *inicio de sesión*.

### Tests

```bash
cargo test
scripts/test_mssql.sh <host>   # SQL Server real (no destructivo: solo tablas #temp), pide la contraseña
```

Los tests en vivo de PostgreSQL se ejecutan con `cargo test -- --ignored` y las variables
`DATACRAFT_PG_*` (ver `src/db/postgres.rs`).

Modo demo para capturas: `DATACRAFT_DEMO=ruta.db cargo run` (ver la cabecera de `src/app.rs`).

### Licencia

DataCraft tiene doble licencia [MIT](LICENSE-MIT) o [Apache-2.0](LICENSE-APACHE), a tu elección.
Las fuentes incluidas mantienen su propia licencia abierta (SIL OFL 1.1); ver [assets/fonts/README.md](assets/fonts/README.md).
