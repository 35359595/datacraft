# DataCraft

Cliente de bases de datos para macOS al estilo de DataGrip (tema Islands), escrito en Rust con egui.

- **Motores:** PostgreSQL, SQL Server y SQLite.
- **Database Explorer:** esquemas, tablas, vistas, columnas y claves primarias.
- **Consolas SQL:** resaltado, autocompletado, ⌘↩ (sentencia o selección) y ⌘⇧↩ (todo; lotes `GO` en SQL Server).
- **Editor de datos de tabla:** filtros WHERE / ORDER BY y exportación CSV.
- **Contraseñas:** se guardan en el Llavero de macOS.

## Compilar

```bash
cargo run                      # desarrollo
scripts/bundle.sh --install    # DataCraft.app optimizada en /Applications
```

## Tests

```bash
cargo test
scripts/test_mssql.sh          # SQL Server real (no destructivo), pide la contraseña
```

Modo demo para capturas: `DATACRAFT_DEMO=ruta.db cargo run` (ver cabecera de `src/app.rs`).
