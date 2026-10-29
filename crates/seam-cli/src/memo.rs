//! `seam memo`: the memo text that binds a payment to a `paymentRef` (docs/PAYMENTS.md § 4), and a ZIP 321 payment
//! URI that carries it, for any wallet that sends an Ironwood payment with a memo.

use anyhow::{Context, Result, anyhow, bail, ensure};
use clap::Args;
use seam_chain::ZcashNetwork;
use seam_payment::memo_text;
use zcash_address::ZcashAddress;
use zcash_keys::address::UnifiedAddress;
use zcash_protocol::{
    memo::MemoBytes,
    value::{COIN, Zatoshis},
};
use zip321::{Payment, TransactionRequest};

#[derive(Args)]
pub struct MemoArgs {
    /// The paymentRef: 32 bytes, as 64 hex digits (0x optional).
    payment_ref: String,
    /// The receiver's unified address: with --amount, also print a ZIP 321 payment URI.
    #[arg(long, requires = "amount")]
    address: Option<String>,
    /// The amount, in ZEC (TAZ on Testnet), with up to 8 decimals.
    #[arg(long, requires = "address")]
    amount: Option<String>,
    /// The address's Zcash network: this build's (testnet, or mainnet with the `mainnet` feature).
    #[arg(long, value_parser = ["testnet", "mainnet"])]
    network: Option<String>,
}

pub fn run(args: MemoArgs) -> Result<()> {
    let payment_ref = parse_ref(&args.payment_ref).context("the paymentRef")?;
    // Everything is checked before anything is printed.
    let uri = match (args.address, args.amount) {
        (Some(address), Some(amount)) => {
            let network = crate::network(args.network.as_deref())?;
            let amount = parse_zec(&amount).context("--amount")?;
            Some(payment_uri(network, &address, amount, &payment_ref)?)
        }
        _ => None,
    };
    println!("{}", memo_text(&payment_ref));
    if let Some((uri, other_receivers)) = uri {
        println!("{uri}");
        if other_receivers {
            eprintln!(
                "note: the address also has a transparent or Sapling receiver; the payment must be an Ironwood-only \
                 one to its Orchard-protocol receiver, or no proof can show it"
            );
        }
    }
    Ok(())
}

/// A 32-byte value written as 64 hex digits, `0x` optional, in either case.
pub fn parse_ref(s: &str) -> Result<[u8; 32]> {
    let s = s.trim();
    let digits = s.strip_prefix("0x").unwrap_or(s);
    ensure!(digits.len() == 64, "64 hex digits are expected");
    hex::decode(digits).context("not hex")?.try_into().map_err(|_| anyhow!("32 bytes are expected"))
}

/// An amount of ZEC, written in decimal with up to 8 decimals, in zatoshi (1 ZEC = 10^8 zatoshi). Zero is refused.
pub fn parse_zec(s: &str) -> Result<Zatoshis> {
    let (whole, fraction) = s.split_once('.').unwrap_or((s, ""));
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    ensure!(
        digits(whole) && (fraction.is_empty() || digits(fraction)) && !s.ends_with('.'),
        "a decimal amount of ZEC, such as 0.025, is expected"
    );
    ensure!(fraction.len() <= 8, "at most 8 decimals (1 zatoshi is 0.00000001 ZEC)");
    ensure!(whole.len() <= 8, "more ZEC than there will ever be");
    let whole: u64 = whole.parse()?;
    let fraction: u64 = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<8}").parse()?
    };
    let zatoshi = whole.checked_mul(COIN).and_then(|z| z.checked_add(fraction)).context("an amount too large")?;
    ensure!(zatoshi > 0, "a payment of nothing");
    Zatoshis::from_u64(zatoshi).map_err(|_| anyhow!("more ZEC than there will ever be"))
}

/// The ZIP 321 URI of a payment of `amount` to unified address `address` of Zcash `network`, with the memo of
/// `payment_ref` (the memo's bytes in base64url without padding), and whether the address has receivers besides its
/// Orchard-protocol one (which an Ironwood payment pays).
pub fn payment_uri(
    network: ZcashNetwork,
    address: &str,
    amount: Zatoshis,
    payment_ref: &[u8; 32],
) -> Result<(String, bool)> {
    let ua = UnifiedAddress::decode(&network.params(), address.trim())
        .map_err(|e| anyhow!("not a unified address of Zcash {network}: {e}"))?;
    if !ua.has_orchard() {
        bail!("the address has no Orchard-protocol receiver, which an Ironwood payment pays");
    }
    let other_receivers = ua.has_sapling() || ua.has_transparent();
    let recipient = ZcashAddress::try_from_encoded(address.trim()).map_err(|e| anyhow!("the address: {e}"))?;
    let memo = MemoBytes::from_bytes(memo_text(payment_ref).as_bytes()).map_err(|e| anyhow!("the memo: {e}"))?;
    let payment = Payment::new(recipient, Some(amount), Some(memo), None, None, vec![])
        .map_err(|e| anyhow!("the payment request: {e:?}"))?;
    let request = TransactionRequest::new(vec![payment]).map_err(|e| anyhow!("the payment request: {e:?}"))?;
    Ok((request.to_uri(), other_receivers))
}

#[cfg(test)]
mod tests {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use orchard::keys::{FullViewingKey, Scope, SpendingKey};
    use seam_payment::k4::{FIXTURE_RECEIVER_SPENDING_KEY, fixture_payment_ref};

    use super::*;

    /// The unified address, with an Orchard-protocol receiver only, of a test key that holds no funds.
    fn test_address(network: ZcashNetwork) -> String {
        let fvk = FullViewingKey::from(&SpendingKey::from_bytes(FIXTURE_RECEIVER_SPENDING_KEY).unwrap());
        let receiver = fvk.address_at(0u32, Scope::External);
        UnifiedAddress::from_receivers(Some(receiver), None, None, None, None)
            .unwrap()
            .encode(&network.params())
    }

    #[test]
    fn references_are_64_hex_digits() {
        let r = fixture_payment_ref();
        let lower = hex::encode(r);
        assert_eq!(parse_ref(&lower).unwrap(), r);
        assert_eq!(parse_ref(&format!("0x{}", lower.to_uppercase())).unwrap(), r);
        assert_eq!(parse_ref(&format!(" {lower}\n")).unwrap(), r);
        assert!(parse_ref(&lower[..62]).is_err());
        assert!(parse_ref(&format!("{lower}00")).is_err());
        assert!(parse_ref(&format!("{}g", &lower[..63])).is_err());
    }

    #[test]
    fn amounts_are_decimal_zec() {
        let zat = |s: &str| parse_zec(s).map(u64::from);
        assert_eq!(zat("0.025").unwrap(), 2_500_000);
        assert_eq!(zat("1").unwrap(), 100_000_000);
        assert_eq!(zat("1.5").unwrap(), 150_000_000);
        assert_eq!(zat("0.00000001").unwrap(), 1);
        assert_eq!(zat("21000000").unwrap(), 2_100_000_000_000_000);
        for bad in [
            "",
            ".",
            "1.",
            ".5",
            "0",
            "0.0",
            "-1",
            "+1",
            "1e5",
            "1,5",
            " 1",
            "0.000000001",
            "21000000.00000001",
        ] {
            assert!(zat(bad).is_err(), "{bad:?}");
        }
    }

    /// The URI is `zcash:<address>?amount=<ZEC>&memo=<base64url of the memo text, no padding>`, and zip321 reads back
    /// the amount and the memo text.
    #[test]
    fn a_payment_uri_carries_the_memo_text() {
        let address = test_address(ZcashNetwork::Testnet);
        let r = fixture_payment_ref();
        let (uri, others) = payment_uri(ZcashNetwork::Testnet, &address, parse_zec("0.025").unwrap(), &r).unwrap();
        assert!(!others);
        let text = memo_text(&r);
        assert_eq!(text.len(), 70);
        let encoded = URL_SAFE_NO_PAD.encode(text.as_bytes());
        assert!(!encoded.contains('='));
        assert_eq!(uri, format!("zcash:{address}?amount=0.025&memo={encoded}"));

        let request = TransactionRequest::from_uri(&uri).unwrap();
        let payment = request.payments().get(&0).unwrap();
        assert_eq!(payment.amount().map(u64::from), Some(2_500_000));
        assert_eq!(payment.memo().unwrap().as_slice(), text.as_bytes());

        // The address must be the network's, a unified one, with an Orchard-protocol receiver.
        assert!(payment_uri(ZcashNetwork::Mainnet, &address, parse_zec("1").unwrap(), &r).is_err());
        assert!(payment_uri(ZcashNetwork::Testnet, "utest1notanaddress", parse_zec("1").unwrap(), &r).is_err());
    }
}
