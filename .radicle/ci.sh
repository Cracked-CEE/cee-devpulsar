#!/usr/bin/env bash
set -euo pipefail

RID=$(rad inspect --rid | sed 's/^rad://')
COMMIT=$(git rev-parse HEAD)
URI=rad://$RID/ci/$COMMIT

if ! rad-job show $COMMIT; then
    rad-job new $COMMIT
fi
RUN_UUID=$(rad-job run $COMMIT $URI)

if make contract_test; then
    STATUS=succeeded
else
    STATUS=failed
fi

rad job $STATUS $COMMIT $RUN_UUID

echo "[rad-ci] $STATUS $URI"
