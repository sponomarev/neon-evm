#!/bin/bash
set -euo pipefail

echo "Neon EVM revision=ankr"

set ${SOLANA_REVISION:=v1.9.12-testnet-bn256-syscalls}

docker pull neonlabsorg/solana:${SOLANA_REVISION}
echo "SOLANA_REVISION=$SOLANA_REVISION"

docker build --build-arg REVISION=ankr --build-arg SOLANA_REVISION=$SOLANA_REVISION -t neonlabsorg/evm_loader:ankr .
