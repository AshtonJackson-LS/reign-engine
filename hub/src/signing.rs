//! Release signatures: an Ed25519 signature over a release's zip, so the
//! hub only installs updates made by whoever holds the release key.
//!
//! What's signed is `reign-release:<tag>:` followed by the zip's bytes, so a
//! signature for one version can't be passed off as another (an old release
//! re-tagged as new would fail). Keys and signatures are hex text.
//!
//! Shared by the hub (which verifies) and `reign-release` (which makes keys
//! and signs).

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// The public half of the release key: updates must be signed by its secret.
/// A fork that ships its own releases makes its own key (`reign-release
/// keygen`) and builds with `REIGN_UPDATE_PUBKEY=<hex>`, alongside
/// `REIGN_UPDATE_REPO`.
pub const PUBLIC_KEY: &str = match option_env!("REIGN_UPDATE_PUBKEY") {
    Some(key) => key,
    None => "e3ad1c021f47a19a134145b3c3548eb9958aafbcc2fe450f361a938a8e6cd1c2",
};

/// The bytes a release's signature covers.
fn message(tag: &str, zip: &[u8]) -> Vec<u8> {
    let mut message = format!("reign-release:{tag}:").into_bytes();
    message.extend_from_slice(zip);
    message
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex<const N: usize>(text: &str) -> Result<[u8; N], String> {
    let text = text.trim();
    if text.len() != N * 2 || !text.is_ascii() {
        return Err(format!("expected {} hex characters, found {}", N * 2, text.len()));
    }
    let mut out = [0; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
            .map_err(|_| "not hexadecimal".to_owned())?;
    }
    Ok(out)
}

/// A new key pair: (secret, public), both hex.
#[allow(dead_code)] // only reign-release makes keys
pub fn generate() -> Result<(String, String), String> {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
    let key = SigningKey::from_bytes(&seed);
    Ok((to_hex(&seed), to_hex(key.verifying_key().as_bytes())))
}

/// Signs `zip` as release `tag` with the hex secret key. Returns hex.
#[allow(dead_code)] // only reign-release signs
pub fn sign(secret: &str, tag: &str, zip: &[u8]) -> Result<String, String> {
    let key = SigningKey::from_bytes(&from_hex::<32>(secret).map_err(|e| format!("key: {e}"))?);
    Ok(to_hex(&key.sign(&message(tag, zip)).to_bytes()))
}

/// Whether `signature` (hex) is `public` (hex)'s signature of `zip` as
/// release `tag`.
pub fn verify(public: &str, tag: &str, zip: &[u8], signature: &str) -> Result<(), String> {
    let public = from_hex::<32>(public).map_err(|e| format!("public key: {e}"))?;
    let public = VerifyingKey::from_bytes(&public).map_err(|e| format!("public key: {e}"))?;
    let signature = from_hex::<64>(signature).map_err(|e| format!("signature: {e}"))?;
    public
        .verify(&message(tag, zip), &Signature::from_bytes(&signature))
        .map_err(|_| "the signature doesn't match".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_verifies_only_for_its_own_file_tag_and_key() {
        let (secret, public) = generate().unwrap();
        let zip = b"pretend zip";
        let sig = sign(&secret, "v0.2.0", zip).unwrap();
        assert_eq!(verify(&public, "v0.2.0", zip, &sig), Ok(()));
        assert!(verify(&public, "v0.3.0", zip, &sig).is_err(), "another tag");
        assert!(verify(&public, "v0.2.0", b"other zip", &sig).is_err(), "another file");
        let (_, other) = generate().unwrap();
        assert!(verify(&other, "v0.2.0", zip, &sig).is_err(), "another key");
        assert!(verify(&public, "v0.2.0", zip, "zz").is_err(), "garbage");
    }

    #[test]
    fn hex_round_trips() {
        let bytes = [0u8, 1, 0xab, 0xff];
        assert_eq!(from_hex::<4>(&to_hex(&bytes)), Ok(bytes));
        assert!(from_hex::<4>("abc").is_err());
    }
}
