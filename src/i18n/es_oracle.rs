//! Spanish translations for the oracle driver. Each entry maps the English source text
//! (exactly as written in the code) to its Spanish translation.

pub const ENTRIES: &[(&str, &str)] = &[
    ("The Oracle connection became unusable", "La conexión Oracle quedó inutilizable"),
    (
        "Oracle connections need the Oracle Instant Client, which could not be loaded ({code}).\n{hint}",
        "Las conexiones Oracle necesitan Oracle Instant Client, que no se ha podido cargar ({code}).\n{hint}",
    ),
    (
        "Download the Instant Client Basic package for macOS from oracle.com, unpack it and link libclntsh.dylib into ~/lib (or /usr/local/lib), or leave the instantclient folder in your home or Downloads folder.",
        "Descarga el paquete Instant Client Basic para macOS de oracle.com, descomprímelo y enlaza libclntsh.dylib en ~/lib (o /usr/local/lib), o deja la carpeta instantclient en tu carpeta de inicio o en Descargas.",
    ),
    (
        "Download the Instant Client Basic package for Windows (64-bit) from oracle.com, unpack it and add its folder to PATH (or copy its DLLs next to DataCraft.exe). It also needs the Visual C++ Redistributable.",
        "Descarga el paquete Instant Client Basic para Windows (64 bits) de oracle.com, descomprímelo y añade su carpeta al PATH (o copia sus DLL junto a DataCraft.exe). También necesita Visual C++ Redistributable.",
    ),
    (
        "Install the Oracle Instant Client Basic package and make libclntsh.so findable (ldconfig or LD_LIBRARY_PATH).",
        "Instala el paquete Oracle Instant Client Basic y haz que libclntsh.so se encuentre (ldconfig o LD_LIBRARY_PATH).",
    ),
];
