#!/usr/bin/env bash
# Builds the payment guests and proves a payment on a rented x86 machine with an NVIDIA GPU and Docker. Runs ON that
# machine, in a copy of the repository (`git archive`): no .env, no keys, nothing private but the witness being proven,
# which `wipe` removes afterwards. Whoever runs the machine can read its disk and memory: prove a real payment's witness
# only on a machine you control, and a rented one only for a disposable wallet's (docs/PAYMENTS.md § 5). Adapted from
# Zecured's scripts/gpu-prove.sh (73de9f909e270e8aea10c2b221905f62a6a4c4df).
#
#   scripts/gpu-prove.sh setup                         # once: CUDA 12.8 if needed, Rust, rzup, seam with `cuda`
#   scripts/gpu-prove.sh guests <out-dir>              # both guests, built in RISC Zero's guest-builder image: their
#                                                      # ELFs, image ids and lines for zkvm/pinned-image-ids.txt, and
#                                                      # each guest's K4 parity with the native statement
#   scripts/gpu-prove.sh prove <witness.json> <out-dir>  # cycles, the succinct receipt on the GPU, the Groth16 seal
#   scripts/gpu-prove.sh wipe <witness.json> <out-dir>   # removes the witness and the receipts; keeps seal.json
#
# `prove` runs the guest SEAM_PAYMENT_ELF names (the pinned ELF `guests` exports), so the receipt is for the pinned
# image id. The STARK is proven by seam built with `cuda`. The Groth16 wrap runs in Docker, through the seam that
# `guests` built without `cuda`: risc0-groth16 3.0.5's CUDA prover failed on Zecured's receipts, and Docker wrapped
# them on the same kind of machine on 27 October 2026. GROTH16=cuda tries the CUDA prover instead (`p254`, then
# `shrink-wrap`). Testnet only: a Mainnet prover builds both with the `mainnet` feature.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CUDA_SEAM="$ROOT/target/release/seam"
# The prover writes its segments, which hold the witness, to TMPDIR: in memory (tmpfs), never on the disk, and removed
# by `wipe`.
SCRATCH=/dev/shm/seam-prove
export PATH="$HOME/.cargo/bin:$HOME/.risc0/bin:$PATH"

# The seam that embeds the `net` guest, built in Docker by `guests` (no `cuda`): parity, export-elf and the Docker wrap.
docker_seam() {
  echo "$ROOT/target/guests-$1/release/seam"
}

setup() {
  nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv
  command -v nvcc >/dev/null || { echo "nvcc not found: use a CUDA devel image" >&2; exit 1; }
  command -v docker >/dev/null || { echo "docker not found: the guests and the Groth16 wrap need it" >&2; exit 1; }
  nvcc --version | tail -1
  # risc0 3.0's kernels do not compile against CUDA 13's headers (CCCL's driver_api.h): build with a CUDA 12 compiler
  # next to the image's toolkit. The driver runs code built for 12.x.
  if nvcc --version | grep -q "release 13"; then
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq cuda-nvcc-12-8 cuda-cudart-dev-12-8 cuda-cccl-12-8 \
      cuda-driver-dev-12-8 cuda-nvrtc-dev-12-8
    # Every build script must see 12.8: some link through /usr/local/cuda (cust), so it points there too.
    update-alternatives --set cuda /usr/local/cuda-12.8
    export PATH="/usr/local/cuda-12.8/bin:$PATH" CUDA_PATH=/usr/local/cuda-12.8 CUDA_HOME=/usr/local/cuda-12.8 \
      CUDA_ROOT=/usr/local/cuda-12.8
  fi
  # circom-witnesscalc (in the Groth16 prover) needs libclang for bindgen and protoc for its build script.
  if ! command -v clang >/dev/null || ! command -v protoc >/dev/null; then
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq clang libclang-dev pkg-config libssl-dev protobuf-compiler
  fi
  if ! command -v cargo >/dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
  fi
  rustup toolchain install "$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$ROOT/rust-toolchain.toml")" --profile minimal
  if ! command -v rzup >/dev/null; then
    curl --proto '=https' --tlsv1.2 -sSfL https://risczero.com/install | bash
  fi
  # risc0-build asks rzup for the guest toolchain's version even when Docker builds the guest; r0vm runs the executor
  # and the Docker wrap of the builds without `cuda`; risc0-groth16 3.0.5 looks for version 0.1.0 of its component.
  rzup install rust 1.97.0
  rzup install r0vm 3.0.6
  rzup install risc0-groth16 0.1.0
  cd "$ROOT"
  # No guest (the pinned ELF) and no LTO: the prover is GPU-bound, the build is not.
  RISC0_SKIP_BUILD=1 CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_DEBUG=0 \
    cargo build --release --locked -p seam-cli --features cuda
}

guests() {
  local out="$1" net features seam
  mkdir -p "$out"
  : > "$out/pinned-image-ids.txt" # a rerun writes the lines again, not twice
  cd "$ROOT"
  for net in test main; do
    features=zkvm
    [[ "$net" == main ]] && features=zkvm,mainnet
    SEAM_DOCKER_GUESTS=1 CARGO_TARGET_DIR="$ROOT/target/guests-$net" \
      cargo build --release --locked -p seam-cli --features "$features"
    seam="$(docker_seam "$net")"
    "$seam" export-elf "$out/payment-$net.bin"
    echo "payment-$net    $("$seam" image-id)  $(sha256sum "$out/payment-$net.bin" | cut -d' ' -f1)" \
      | tee -a "$out/pinned-image-ids.txt"
  done
  for net in test main; do
    "$(docker_seam "$net")" parity crates/seam-payment/tests/fixtures/TEST-ONLY_testnet_ironwood_tx.json \
      | tee "$out/parity-$net.txt"
  done
}

prove() {
  local witness="$1" out="$2" started
  : "${SEAM_PAYMENT_ELF:?SEAM_PAYMENT_ELF: the pinned payment guest}"
  [[ -x "$(docker_seam test)" ]] || { echo "run guests first: the Groth16 wrap uses its build" >&2; exit 1; }
  mkdir -p "$out"
  mkdir -p -m 700 "$SCRATCH"
  export TMPDIR="$SCRATCH"
  # 2^20-cycle segments suit a 24 GB GPU (seam's default, 2^18, suits an 8 GB machine).
  export SEAM_SEGMENT_PO2="${SEAM_SEGMENT_PO2:-20}"
  "$CUDA_SEAM" image-id | tee "$out/image-id"
  "$CUDA_SEAM" execute "$witness" | tee "$out/execute.txt"
  started=$(date +%s)
  "$CUDA_SEAM" prove "$witness" "$out/payment.receipt"
  echo "succinct: $(( $(date +%s) - started )) s" | tee -a "$out/timings.txt"
  started=$(date +%s)
  if [[ "${GROTH16:-}" == cuda ]]; then
    "$CUDA_SEAM" p254 "$out/payment.receipt" "$out/payment.p254"
    "$CUDA_SEAM" shrink-wrap "$out/payment.p254" "$out/payment.receipt" "$out/seal.json"
  else
    "$(docker_seam test)" groth16 "$out/payment.receipt" "$out/seal.json"
  fi
  echo "groth16: $(( $(date +%s) - started )) s" | tee -a "$out/timings.txt"
}

wipe() {
  local witness="$1" out="$2"
  # The receipts and the prover's segments first: a shred that fails must not leave them behind. shred is best effort
  # on SSDs and cloud volumes; only destroying the machine and its disk is sure.
  rm -f "$out/payment.receipt" "$out/payment.p254"
  rm -rf "$SCRATCH"
  shred -u "$witness"
  ls -l "$out"
}

case "${1:-}" in
  setup) setup ;;
  guests) [[ $# -eq 2 ]] || { echo "usage: $0 guests <out-dir>" >&2; exit 1; }
          guests "$2" ;;
  prove) [[ $# -eq 3 ]] || { echo "usage: $0 prove <witness.json> <out-dir>" >&2; exit 1; }
         prove "$2" "$3" ;;
  wipe) [[ $# -eq 3 ]] || { echo "usage: $0 wipe <witness.json> <out-dir>" >&2; exit 1; }
        wipe "$2" "$3" ;;
  *) echo "usage: $0 setup | guests <out-dir> | prove <witness.json> <out-dir> | wipe <witness.json> <out-dir>" >&2
     exit 1 ;;
esac
