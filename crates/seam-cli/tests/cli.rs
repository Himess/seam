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

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("seam-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The txid comes from --txid or from the opening `seam pay` wrote, and never both differing; nothing is read from
/// the network before that is settled.
#[test]
fn witness_takes_the_txid_from_the_opening() {
    let dir = temp_dir();
    let salt = dir.join("salt.txt");
    std::fs::write(&salt, "5a".repeat(32)).unwrap();
    let opening = |txid: Option<&str>| {
        let path = dir.join(format!("opening-{}.json", txid.map_or(0, str::len)));
        let mut json = serde_json::json!({
            "action_index": 1,
            "receiver": "00".repeat(43),
            "value": 1,
            "rseed": "42".repeat(32),
        });
        if let Some(t) = txid {
            json["txid"] = t.into();
        }
        std::fs::write(&path, json.to_string()).unwrap();
        path.display().to_string()
    };
    let salt = salt.display().to_string();
    let run = |extra: &[&str]| {
        seam(
            &[
                &["witness", "--salt-file", &salt, "--anchor", "1", "--out", "w.json"][..],
                extra,
            ]
            .concat(),
        )
    };
    let none = run(&["--opening", &opening(None)]);
    assert!(stderr(&none).contains("give --txid"), "{}", stderr(&none));
    let other = run(&["--opening", &opening(Some(&"ab".repeat(32))), "--txid", REF]);
    assert!(stderr(&other).contains("not the transaction the opening was kept for"), "{}", stderr(&other));
    // With a txid either way, the next check refuses: block 1 is in no epoch a payment proof walks.
    for extra in [
        vec!["--opening", &opening(Some(REF))],
        vec!["--opening", &opening(None), "--txid", REF],
    ] {
        let out = run(&extra);
        assert!(stderr(&out).contains("no epoch"), "{}", stderr(&out));
    }
}

/// `pay` is Testnet's only, names its payment completely, and creates a wallet only with an address file to write.
#[test]
fn pay_refuses_what_it_cannot_send() {
    let dir = temp_dir();
    let wallet = dir.join("wallet.json").display().to_string();
    let payment = [
        "pay",
        "--wallet",
        &wallet,
        "--to",
        "utest1",
        "--zat",
        "1",
        "--ref",
        REF,
        "--opening",
        "o.json",
    ];
    let incomplete = seam(&payment[..payment.len() - 2]);
    assert!(!incomplete.status.success() && stderr(&incomplete).contains("--opening"));
    let no_address_file = seam(&["pay", "--wallet", &wallet, "--new-wallet"]);
    assert!(!no_address_file.status.success() && stderr(&no_address_file).contains("--address-out"));
    let both = seam(&[&payment[..], &["--new-wallet", "--address-out", "a.txt"]].concat());
    assert!(!both.status.success() && stderr(&both).contains("cannot be used with"));
    if cfg!(not(feature = "mainnet")) {
        let mainnet = seam(&[&payment[..], &["--network", "mainnet"]].concat());
        assert!(!mainnet.status.success() && stderr(&mainnet).contains("needs a build"));
    }
    let bad_receiver = seam(&payment);
    assert!(!bad_receiver.status.success() && stderr(&bad_receiver).contains("--to"));
    assert!(!std::path::Path::new(&wallet).exists() && !std::path::Path::new("o.json").exists());
}
