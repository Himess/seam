# Seam patch

Vendored from `pasta_curves` 0.6.1 (crates.io), as Zecured vendored and patched it
(`tier2/vendor/pasta_curves` in https://github.com/Himess/zecured at
73de9f909e270e8aea10c2b221905f62a6a4c4df, where the patch is called `ZECURED PATCH`). The files are byte-identical to
Zecured's copy apart from this note, which replaces its `ZECURED_PATCH.md`.

The only change to the crate: when compiled for the RISC Zero zkVM (`target_os = "zkvm"`), `Fp` and `Fq`
multiplication, squaring and inversion go through RISC Zero's `bigint2` modular arithmetic accelerator
(`src/fields/fp.rs`, `src/fields/fq.rs`, look for `ZECURED PATCH`), which takes about a third of the cycles. Every other
target compiles the original code. The crate keeps its licences (`LICENSE-MIT`, `LICENSE-APACHE`).
