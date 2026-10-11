//! Solves Equihash (n = 200, k = 9) for a synthetic Zcash block header.
//!
//! Test-fixture tool for `contracts/test/relay`: the fork-choice and negative tests need headers
//! with genuinely valid Equihash solutions (at the Testnet minimum difficulty), which cannot be taken
//! from the real chain. This program only searches; `contracts/test/relay/tools/make_synthetic.py`
//! builds the header prefixes and independently re-verifies every result.
//!
//! Usage:
//!   relay-synthetic-headers <prefix108-hex> <nBits-hex> <pass|fail> <nonce-tag-hex> [max-runs]
//!
//! For nonce = tag (first bytes) || run counter (u32 LE, bytes 28..32) it runs Tromp's solver and
//! checks each solution's block hash SHA256d(prefix || nonce || fd4005 || solution), read as a
//! little-endian integer, against the target decoded from nBits. `pass` returns the first solution
//! whose hash is <= target, `fail` the first whose hash is > target. Output: one line
//! `<nonce-hex> <solution-hex> <hash-display-hex>`.

use sha2::{Digest, Sha256};

fn decode_compact(bits: u32) -> [u8; 32] {
    // Big-endian 256-bit target (zcashd SetCompact, valid non-negative values only).
    let size = (bits >> 24) as usize;
    let word = bits & 0x007f_ffff;
    assert!(bits & 0x0080_0000 == 0 && word != 0 && (3..=32).contains(&size), "unsupported nBits");
    let mut out = [0u8; 32];
    let mantissa = word.to_be_bytes(); // [0, b1, b2, b3]
    for i in 0..3 {
        let pos = 32 - size + i;
        out[pos] = mantissa[1 + i];
    }
    out
}

fn sha256d(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    Sha256::digest(first).into()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: {} <prefix108-hex> <nBits-hex> <pass|fail> <nonce-tag-hex> [max-runs]", args[0]);
        std::process::exit(2);
    }
    let prefix = hex::decode(&args[1]).expect("prefix hex");
    assert_eq!(prefix.len(), 108, "the header prefix (version .. nBits) is 108 bytes");
    let bits = u32::from_str_radix(args[2].trim_start_matches("0x"), 16).expect("nBits hex");
    let want_pass = match args[3].as_str() {
        "pass" => true,
        "fail" => false,
        other => panic!("expected pass|fail, got {other}"),
    };
    let tag = hex::decode(&args[4]).expect("nonce tag hex");
    assert!(tag.len() <= 28, "nonce tag is at most 28 bytes");
    let max_runs: u32 = args.get(5).map(|s| s.parse().expect("max-runs")).unwrap_or(2000);

    let target_be = decode_compact(bits);

    for run in 0..max_runs {
        let mut nonce = [0u8; 32];
        nonce[..tag.len()].copy_from_slice(&tag);
        nonce[28..32].copy_from_slice(&run.to_le_bytes());

        let mut yielded = false;
        let solutions = equihash::tromp::solve_200_9(&prefix, || {
            if yielded {
                None
            } else {
                yielded = true;
                Some(nonce)
            }
        });

        for solution in solutions {
            assert_eq!(solution.len(), 1344);
            // Tromp's solver can emit candidates with duplicate indices; like a miner, keep only the
            // solutions the reference verifier accepts.
            if let Err(err) = equihash::is_valid_solution(200, 9, &prefix, &nonce, &solution) {
                eprintln!("discarding invalid solver output: {err}");
                continue;
            }
            let mut header = Vec::with_capacity(1487);
            header.extend_from_slice(&prefix);
            header.extend_from_slice(&nonce);
            header.extend_from_slice(&[0xfd, 0x40, 0x05]);
            header.extend_from_slice(&solution);
            let hash = sha256d(&header);
            let mut hash_be = hash;
            hash_be.reverse(); // display order = big-endian integer
            let passes = hash_be <= target_be;
            if passes == want_pass {
                println!("{} {} {}", hex::encode(nonce), hex::encode(&solution), hex::encode(hash_be));
                eprintln!("found after {} solver runs", run + 1);
                return;
            }
        }
    }
    eprintln!("no suitable solution in {max_runs} runs");
    std::process::exit(1);
}
