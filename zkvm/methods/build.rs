use std::collections::HashMap;

use risc0_build::{DockerOptionsBuilder, GuestOptionsBuilder, embed_methods_with_options};

/// Builds the payment guest for the network this crate is built for: Testnet by default, Mainnet with the `mainnet`
/// feature, which reaches the guest as its own `mainnet` feature and the statement's. Each network's guest has its
/// own image id (`payment-test`, `payment-main`).
///
/// `SEAM_DOCKER_GUESTS=1` builds the guest in RISC Zero's guest-builder image, with the repository's root as the build
/// context (it holds the guest, crates/seam-payment and the vendored pasta_curves): the reproducible build whose image
/// ids are pinned in zkvm/pinned-image-ids.txt. Without it, the guest is built with the local risc0 toolchain, whose
/// image id depends on the machine. `RISC0_SKIP_BUILD=1` builds no guest at all (risc0-build's own switch), so the
/// host crates build where no risc0 toolchain is installed.
fn main() {
    println!("cargo:rerun-if-env-changed=SEAM_DOCKER_GUESTS");
    let features = if std::env::var_os("CARGO_FEATURE_MAINNET").is_some() {
        vec!["mainnet".to_owned()]
    } else {
        vec![]
    };
    let mut options = GuestOptionsBuilder::default();
    options.features(features);
    if std::env::var_os("SEAM_DOCKER_GUESTS").is_some() {
        // Build scripts run in the package directory (zkvm/methods); the context is the repository's root. The tag is
        // pinned here rather than left to risc0-build's default, so the build is fully specified by this repository.
        let docker = DockerOptionsBuilder::default()
            .root_dir("../..")
            .docker_container_tag("r0.1.97.0")
            .build()
            .expect("docker options");
        options.use_docker(docker);
    }
    let options = options.build().expect("guest options");
    embed_methods_with_options(HashMap::from([("payment", options)]));
}
