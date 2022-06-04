#!/bin/bash
set -euo pipefail

echo "Neon EVM revision=${BUILDKITE_COMMIT}"

set ${SOLANA_IMAGE:=neonlabsorg/solana:v1.11.x-dumper-plugin}

docker pull ${SOLANA_IMAGE}
echo "SOLANA_IMAGE=$SOLANA_IMAGE"

docker build --build-arg REVISION=${BUILDKITE_COMMIT} --build-arg SOLANA_IMAGE=$SOLANA_IMAGE -t neonlabsorg/evm_loader:${BUILDKITE_COMMIT} .
