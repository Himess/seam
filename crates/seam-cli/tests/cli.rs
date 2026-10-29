//! The `seam` binary: what it prints, and what it refuses before it reads anything from the network.

use std::process::{Command, Output};

/// `paymentRef` of crates/seam-payment's fixture (`k4::fixture_payment_ref`).
const REF: &str = "0ffd65bace87c3ecbd455fa36283257624b7a3dcdfaa610f6f37d30dd1cbbf19";

fn seam(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_seam")).args(args).output().expect("run seam")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn memo_prints_the_memo_text() {
    for given in [REF.to_owned(), format!("0x{}", REF.to_uppercase())] {
        let out = seam(&["memo", &given]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("seam1:{REF}\n"));
    }
    let short = seam(&["memo", &REF[..60]]);
    assert!(!short.status.success() && short.stdout.is_empty());
}

/// A URI needs both the address and the amount, and an address the build's network decodes; nothing is printed
/// before the inputs are checked.
#[test]
fn memo_checks_the_address_before_printing() {
    let alone = seam(&["memo", REF, "--address", "utest1"]);
    assert!(!alone.status.success() && stderr(&alone).contains("--amount"));
    let bad = seam(&["memo", REF, "--address", "utest1notanaddress", "--amount", "0.1"]);
    assert!(!bad.status.success() && bad.stdout.is_empty());
    assert!(stderr(&bad).contains("not a unified address"));
}

/// One build proves on one network, and a witness needs exactly one of an opening and a viewing key.
#[test]
fn witness_refuses_what_it_cannot_build() {
    let base = [
        "witness",
        "--txid",
        REF,
        "--salt-file",
        "salt",
        "--anchor",
        "4488560",
        "--out",
        "w.json",
    ];
    let both = seam(&[&base[..], &["--opening", "o.json", "--ufvk-file", "k"]].concat());
    assert!(!both.status.success() && stderr(&both).contains("cannot be used with"));
    let neither = seam(&base);
    assert!(!neither.status.success() && stderr(&neither).contains("--opening"));
    let other = if cfg!(feature = "mainnet") {
        "testnet"
    } else {
        "mainnet"
    };
    let network = seam(&[&base[..], &["--opening", "o.json", "--network", other]].concat());
    assert!(!network.status.success() && stderr(&network).contains("needs a build"));
    let missing = seam(&[&base[..], &["--opening", "no-such-opening.json"]].concat());
    assert!(!missing.status.success() && stderr(&missing).contains("read salt"));
}
