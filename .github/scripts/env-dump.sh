#!/usr/bin/env bash
# Record the environment the wrapped command really gets, then exec it.
# Used as the last element of OPENPDF_MEASURE_WRAP (right before the binary).
set -eu
if [ -n "${OPENPDF_ENV_DUMP:-}" ]; then
  env | LC_ALL=C sort > "$OPENPDF_ENV_DUMP"
fi
exec "$@"
