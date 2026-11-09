#!/bin/sh
# One Harbor suite from evals/suites.json, run through the sweep driver's runner mode, caps and
# watcher: `harbor.sh <suite> [--runner <overrides.json> <task>...]`. A suite with no pinned
# dataset is refused here, by name, before anything is built or paid for.
set -eu
suite="${1:?usage: harbor.sh <suite> [tbv4_sweep.sh arguments]}"
shift
dataset=$(python3 evals/suites.py field "$suite" dataset)
[ -n "$dataset" ] && [ "$dataset" != "null" ] || { echo "refused: $suite has no pinned dataset" >&2; exit 1; }
TBV4_DATASET="$dataset"
TBV4_PREFIX=$(python3 evals/suites.py field "$suite" prefix)
export TBV4_DATASET TBV4_PREFIX
exec sh evals/drivers/tbv4_sweep.sh "$@"
