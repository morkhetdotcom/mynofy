use super::*;
use aws_lc_rs::{aead, pbkdf2};
use std::num::NonZeroU32;
const HEADER: usize = 64;
const ITERATIONS: u32 = 600_000;
#[cfg(test)]
thread_local! { static KDF_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
/// Cached password derivation for one store. Secret material is zeroized on drop.
pub struct PreparedPassword {
    password: Zeroizing<Vec<u8>>,
    salt: [u8; 16],
    derived: Zeroizing<[u8; 32]>,
}
pub(super) fn prepare(
    secret: ContentSecret,
    envelope: Option<&[u8]>,
) -> std::result::Result<ContentSecret, BoxError> {
    let ContentSecret::Password(password) = secret else {
        return Ok(secret);
    };
    let salt = match envelope {
        Some(bytes) if bytes.len() >= HEADER && &bytes[..8] == b"MYNENC01" && bytes[9] == 1 => {
            bytes[16..32].try_into()?
        }
        _ => crypto::random::<16>()?,
    };
    let original = ContentSecret::Password(password);
    let derived = key(&original, &salt, ITERATIONS)?;
    let ContentSecret::Password(password) = original else {
        unreachable!()
    };
    Ok(ContentSecret::PreparedPassword(PreparedPassword {
        password,
        salt,
        derived,
    }))
}
fn key(
    secret: &ContentSecret,
    salt: &[u8],
    iterations: u32,
) -> std::result::Result<Zeroizing<[u8; 32]>, BoxError> {
    match secret {
        ContentSecret::Key(key) => Ok(Zeroizing::new(**key)),
        ContentSecret::PreparedPassword(prepared)
            if salt == prepared.salt && iterations == ITERATIONS =>
        {
            Ok(Zeroizing::new(*prepared.derived))
        }
        ContentSecret::PreparedPassword(PreparedPassword { password, .. })
        | ContentSecret::Password(password) => {
            let mut output = Zeroizing::new([0; 32]);
            let rounds = NonZeroU32::new(iterations).ok_or("Invalid password KDF parameters")?;
            #[cfg(test)]
            KDF_CALLS.with(|count| count.set(count.get() + 1));
            pbkdf2::derive(
                pbkdf2::PBKDF2_HMAC_SHA256,
                rounds,
                salt,
                password,
                &mut *output,
            );
            Ok(output)
        }
    }
}
pub(super) fn encode(
    mut bytes: Zeroizing<Vec<u8>>,
    mode: ContentEncryption,
    secret: Option<&ContentSecret>,
) -> std::result::Result<Zeroizing<Vec<u8>>, BoxError> {
    if mode == ContentEncryption::None {
        return Ok(bytes);
    }
    let secret = secret.ok_or("Content encryption requires a startup secret")?;
    let mut header = [0; HEADER];
    header[..8].copy_from_slice(b"MYNENC01");
    header[8] = 1;
    header[9] = if mode == ContentEncryption::PasswordStdin {
        1
    } else {
        2
    };
    let iterations = if mode == ContentEncryption::PasswordStdin {
        ITERATIONS
    } else {
        0
    };
    header[12..16].copy_from_slice(&iterations.to_le_bytes());
    let salt = match secret {
        ContentSecret::PreparedPassword(prepared) => prepared.salt,
        _ => crypto::random::<16>()?,
    };
    header[16..32].copy_from_slice(&salt);
    let nonce = crypto::random::<12>()?;
    header[32..44].copy_from_slice(&nonce);
    header[48..56].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
    let derived = key(secret, &header[16..32], iterations)?;
    let aead = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &*derived)
            .map_err(|_| "Content encryption failed")?,
    );
    let tag = aead
        .seal_in_place_separate_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(&header),
            &mut bytes,
        )
        .map_err(|_| "Content encryption failed")?;
    let length = bytes.len();
    bytes.resize(length + HEADER + 16, 0);
    bytes.copy_within(0..length, HEADER);
    bytes[..HEADER].copy_from_slice(&header);
    bytes[HEADER + length..].copy_from_slice(tag.as_ref());
    Ok(bytes)
}
pub(super) fn decode(
    mut bytes: Zeroizing<Vec<u8>>,
    mode: ContentEncryption,
    secret: Option<&ContentSecret>,
    maximum: u64,
) -> std::result::Result<Zeroizing<Vec<u8>>, BoxError> {
    if mode == ContentEncryption::None {
        if bytes.starts_with(b"MYNENC01") {
            return Err("Encrypted content requires the configured stdin secret mode".into());
        }
        return Ok(bytes);
    }
    if bytes.len() < HEADER + 16 || !bytes.starts_with(b"MYNENC01") {
        return Err("Missing or invalid encrypted content envelope".into());
    }
    let mut header = [0; HEADER];
    header.copy_from_slice(&bytes[..HEADER]);
    let expected = if mode == ContentEncryption::PasswordStdin {
        1
    } else {
        2
    };
    let iterations = u32::from_le_bytes(header[12..16].try_into()?);
    let length = u64::from_le_bytes(header[48..56].try_into()?);
    if header[8] != 1
        || header[9] != expected
        || header[10..12]
            .iter()
            .chain(&header[44..48])
            .chain(&header[56..64])
            .any(|byte| *byte != 0)
        || iterations != if expected == 1 { ITERATIONS } else { 0 }
        || length > maximum
        || length.checked_add((HEADER + 16) as u64) != Some(bytes.len() as u64)
    {
        return Err("Invalid encrypted content parameters or secret mode".into());
    }
    let derived = key(
        secret.ok_or("Content decryption requires a startup secret")?,
        &header[16..32],
        iterations,
    )?;
    let aead = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &*derived)
            .map_err(|_| "Content decryption failed")?,
    );
    let nonce = header[32..44].try_into()?;
    let plain = aead
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(&header),
            &mut bytes[HEADER..],
        )
        .map_err(
            |_| "Content authentication/decryption failed (wrong secret or corrupt snapshot)",
        )?;
    let length = plain.len();
    bytes.copy_within(HEADER..HEADER + length, 0);
    bytes[length..].fill(0);
    bytes.truncate(length);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_envelopes_roundtrip_and_reject_wrong_keys_headers_and_ciphertext() {
        let secret = ContentSecret::Key(Zeroizing::new([0x42; 32]));
        let wrong = ContentSecret::Key(Zeroizing::new([0x43; 32]));
        let plain = b"private snapshot marker";
        let first = encode(
            Zeroizing::new(plain.to_vec()),
            ContentEncryption::KeyStdin,
            Some(&secret),
        )
        .unwrap();
        let second = encode(
            Zeroizing::new(plain.to_vec()),
            ContentEncryption::KeyStdin,
            Some(&secret),
        )
        .unwrap();
        assert_ne!(*first, *second);
        assert!(!first.windows(plain.len()).any(|window| window == plain));
        assert_eq!(
            *decode(
                first.clone(),
                ContentEncryption::KeyStdin,
                Some(&secret),
                1024
            )
            .unwrap(),
            plain
        );
        assert!(
            decode(
                first.clone(),
                ContentEncryption::KeyStdin,
                Some(&wrong),
                1024
            )
            .is_err()
        );
        assert!(decode(first.clone(), ContentEncryption::None, None, 1024).is_err());
        for offset in [0, 8, 9, 12, 16, 32, 48, 63, 64, first.len() - 1] {
            let mut corrupt = first.clone();
            corrupt[offset] ^= 1;
            assert!(decode(corrupt, ContentEncryption::KeyStdin, Some(&secret), 1024).is_err());
        }
        assert!(decode(first.clone(), ContentEncryption::KeyStdin, Some(&secret), 1).is_err());
        assert!(
            decode(
                Zeroizing::new(first[..first.len() - 1].to_vec()),
                ContentEncryption::KeyStdin,
                Some(&secret),
                1024
            )
            .is_err()
        );
    }
    #[test]
    fn password_mode_uses_salt_and_rejects_a_wrong_password() {
        let password =
            ContentSecret::Password(Zeroizing::new(b"test password with spaces".to_vec()));
        let wrong = ContentSecret::Password(Zeroizing::new(b"different test password".to_vec()));
        let encrypted = encode(
            Zeroizing::new(vec![0xaa; 256]),
            ContentEncryption::PasswordStdin,
            Some(&password),
        )
        .unwrap();
        assert_eq!(
            u32::from_le_bytes(encrypted[12..16].try_into().unwrap()),
            600_000
        );
        assert_eq!(
            *decode(
                encrypted.clone(),
                ContentEncryption::PasswordStdin,
                Some(&password),
                1024
            )
            .unwrap(),
            vec![0xaa; 256]
        );
        assert!(
            decode(
                encrypted,
                ContentEncryption::PasswordStdin,
                Some(&wrong),
                1024
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod functional {
    use super::*;
    use crate::test_support::{self, Rng};
    #[test]
    fn functional_seeded_encrypted_envelopes_authenticate_every_accepted_byte() {
        let mut rng = Rng(test_support::seed());
        let cases = test_support::fuzz_cases();
        let secret = ContentSecret::Key(Zeroizing::new([0x37; 32]));
        let wrong = ContentSecret::Key(Zeroizing::new([0x38; 32]));
        let mut rejected = 0;
        for case in 0..cases {
            let length = [0, 1, 15, 16, 17, 255, 4096][rng.pick(7)];
            let plain: Vec<_> = (0..length).map(|_| rng.next() as u8).collect();
            let sealed = encode(
                Zeroizing::new(plain.clone()),
                ContentEncryption::KeyStdin,
                Some(&secret),
            )
            .unwrap();
            assert_eq!(
                &*decode(
                    sealed.clone(),
                    ContentEncryption::KeyStdin,
                    Some(&secret),
                    4096
                )
                .unwrap(),
                &plain
            );
            assert!(
                decode(
                    sealed.clone(),
                    ContentEncryption::KeyStdin,
                    Some(&wrong),
                    4096
                )
                .is_err()
            );
            assert!(decode(sealed.clone(), ContentEncryption::None, None, 4096).is_err());
            if length > 0 {
                assert!(
                    decode(
                        sealed.clone(),
                        ContentEncryption::KeyStdin,
                        Some(&secret),
                        length as u64 - 1
                    )
                    .is_err()
                );
            }
            let mutated = rng.mutate(&sealed, 8192);
            let result = decode(
                Zeroizing::new(mutated.clone()),
                ContentEncryption::KeyStdin,
                Some(&secret),
                4096,
            );
            if mutated.as_slice() != sealed.as_slice() {
                assert!(
                    result.is_err(),
                    "authenticated corruption accepted at seed=0x{:016x}, case={case}",
                    test_support::seed()
                );
                rejected += 1;
            } else {
                assert_eq!(&*result.unwrap(), &plain);
            }
        }
        assert!(rejected > 0);
        eprintln!(
            "Encrypted-envelope mutations: {cases} cases, {rejected} modified envelopes rejected"
        );
    }
}

#[cfg(test)]
mod prepared_tests {
    use super::*;
    #[test]
    fn prepared_store_derives_once_reuses_key_with_distinct_nonces_and_reads_legacy() {
        KDF_CALLS.with(|count| count.set(0));
        let make = || ContentSecret::Password(Zeroizing::new(b"test prepared password".to_vec()));
        let prepared = prepare(make(), None).unwrap();
        let mut encrypted = Vec::new();
        for _ in 0..3 {
            let bytes = encode(
                Zeroizing::new(vec![42; 128]),
                ContentEncryption::PasswordStdin,
                Some(&prepared),
            )
            .unwrap();
            assert_eq!(
                *decode(
                    bytes.clone(),
                    ContentEncryption::PasswordStdin,
                    Some(&prepared),
                    1024
                )
                .unwrap(),
                vec![42; 128]
            );
            encrypted.push(bytes);
        }
        assert_eq!(&encrypted[0][16..32], &encrypted[1][16..32]);
        assert_ne!(&encrypted[0][32..44], &encrypted[1][32..44]);
        KDF_CALLS.with(|count| assert_eq!(count.get(), 1));
        let restarted = prepare(make(), Some(&encrypted[0])).unwrap();
        for bytes in &encrypted {
            decode(
                bytes.clone(),
                ContentEncryption::PasswordStdin,
                Some(&restarted),
                1024,
            )
            .unwrap();
        }
        KDF_CALLS.with(|count| assert_eq!(count.get(), 2));
        // Existing raw-password consumers still read newly written files.
        decode(
            encrypted.pop().unwrap(),
            ContentEncryption::PasswordStdin,
            Some(&make()),
            1024,
        )
        .unwrap();
        // Older files with independent salts remain readable during gradual rewrite.
        let legacy = encode(
            Zeroizing::new(vec![7; 16]),
            ContentEncryption::PasswordStdin,
            Some(&make()),
        )
        .unwrap();
        assert_eq!(
            *decode(
                legacy,
                ContentEncryption::PasswordStdin,
                Some(&restarted),
                1024
            )
            .unwrap(),
            vec![7; 16]
        );
        assert!(
            decode(
                encrypted.pop().unwrap(),
                ContentEncryption::PasswordStdin,
                Some(&make_wrong()),
                1024
            )
            .is_err()
        );
    }
    fn make_wrong() -> ContentSecret {
        ContentSecret::Password(Zeroizing::new(b"wrong password".to_vec()))
    }
}
