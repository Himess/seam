# Seam patch

Vendored from `pasta_curves` 0.6.1 (crates.io), as Zecured vendored and patched it
(`tier2/vendor/pasta_curves` in https://github.com/Himess/zecured at
73de9f909e270e8aea10c2b221905f62a6a4c4df, where the patch is called `ZECURED PATCH`). The files are byte-identical to
Zecured's copy apart from this note, which replaces its `ZECURED_PATCH.md`.

The only change to the crate: when compiled for the RISC Zero zkVM (`target_os = "zkvm"`), `Fp` and `Fq`
multiplication, squaring and inversion go through RISC Zero's `bigint2` modular arithmetic accelerator
(`src/fields/fp.rs`, `src/fields/fq.rs`, look for `ZECURED PATCH`), which takes about a third of the cycles. Every other
target compiles the original code. The crate keeps its licences (`LICENSE-MIT`, `LICENSE-APACHE`).

Exactly what changes, in each field:
- **Accelerated:** `mul_runtime`, `square_runtime` and `sqr_n_mul_runtime`, behind `*`, `*=`, `Field::square` and
  the exponentiations (square roots included), and `invert` and `invert_vartime`.
- **Unchanged:** the `const fn`s (`mul`, `square`, `from_raw`), `from_u512` and `to_repr`. They return the same
  canonical Montgomery value as the accelerated paths.
- **The invariant it relies on:** every `Fp` and `Fq` value is canonical, below the modulus. The zero check before an
  inversion and the square-root tables depend on it. The checked `bigint2` calls only ever return canonical values,
  and `from_repr` refuses an encoding at or above the modulus before using it.
