#!/bin/zsh
# Lanza los tests en vivo de SQL Server (no destructivos: solo tablas #temp).
# Uso: scripts/test_mssql.sh <host> [usuario] [base]
set -euo pipefail
cd "${0:A:h}/.."
source "$HOME/.cargo/env" 2>/dev/null || true
export DATACRAFT_MSSQL_HOST="${1:?Indica el host del SQL Server}"
export DATACRAFT_MSSQL_USER="${2:-sa}"
export DATACRAFT_MSSQL_DB="${3:-master}"
read -rs "DATACRAFT_MSSQL_PASSWORD?Contraseña de $DATACRAFT_MSSQL_USER@$DATACRAFT_MSSQL_HOST: "
echo
export DATACRAFT_MSSQL_PASSWORD
cargo test db::mssql -- --ignored --nocapture
