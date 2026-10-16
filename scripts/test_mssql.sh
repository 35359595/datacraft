#!/bin/zsh
# Runs the live SQL Server tests (non-destructive: only #temp tables).
# Usage: scripts/test_mssql.sh <host> [user] [database]
set -euo pipefail
cd "${0:A:h}/.."
source "$HOME/.cargo/env" 2>/dev/null || true
export DATACRAFT_MSSQL_HOST="${1:?Specify the SQL Server host}"
export DATACRAFT_MSSQL_USER="${2:-sa}"
export DATACRAFT_MSSQL_DB="${3:-master}"
read -rs "DATACRAFT_MSSQL_PASSWORD?Password for $DATACRAFT_MSSQL_USER@$DATACRAFT_MSSQL_HOST: "
echo
export DATACRAFT_MSSQL_PASSWORD
cargo test db::mssql -- --ignored --nocapture
