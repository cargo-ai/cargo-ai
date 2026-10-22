//! Verify account access credentials without taking ownership of their renewal.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Read;

const ISSUER: &str = "https://auth.openai.com";
const AUDIENCE: &str = "https://api.openai.com/v1";
const NAMESPACE: &str = "https://api.openai.com/auth";
const MAX_BYTES: usize = 64 * 1024;
const MAX_TOKEN: usize = 16 * 1024;
pub(crate) const KEY_MAX_AGE: u64 = 24 * 60 * 60;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyCache {
    version: u32,
    fetched_at: u64,
    keys: Vec<RsaKey>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RsaKey {
    kid: String,
    n: String,
    e: String,
}

/// Contains credential bytes only in memory; deliberately neither Debug nor Serialize.
#[derive(PartialEq, Eq)]
pub(crate) struct AccountSnapshot {
    pub token: String,
    pub account: String,
    pub identity: String,
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 1024 && s.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or_else(|| "unsupported_claims".into())
}

/// Reject duplicate JSON object fields at every depth, including signed claims.
fn strict_json(bytes: &[u8]) -> Result<Value, String> {
    struct Strict(Value);
    impl<'de> Deserialize<'de> for Strict {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Strict;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("unambiguous JSON")
                }
                fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Strict, E> {
                    Ok(Strict(v.into()))
                }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Strict, E> {
                    Ok(Strict(v.into()))
                }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Strict, E> {
                    Ok(Strict(v.into()))
                }
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Strict, E> {
                    serde_json::Number::from_f64(v)
                        .map(|n| Strict(Value::Number(n)))
                        .ok_or_else(|| E::custom("invalid number"))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Strict, E> {
                    Ok(Strict(v.into()))
                }
                fn visit_none<E: serde::de::Error>(self) -> Result<Strict, E> {
                    Ok(Strict(Value::Null))
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Strict, E> {
                    Ok(Strict(Value::Null))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut a: A,
                ) -> Result<Strict, A::Error> {
                    let mut values = Vec::new();
                    while let Some(Strict(v)) = a.next_element()? {
                        values.push(v);
                    }
                    Ok(Strict(Value::Array(values)))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut a: A,
                ) -> Result<Strict, A::Error> {
                    let mut values = serde_json::Map::new();
                    while let Some((k, Strict(v))) = a.next_entry::<String, Strict>()? {
                        if values.insert(k, v).is_some() {
                            return Err(serde::de::Error::custom("duplicate field"));
                        }
                    }
                    Ok(Strict(Value::Object(values)))
                }
            }
            d.deserialize_any(Visitor)
        }
    }
    if bytes.len() > MAX_BYTES {
        return Err("unsupported_claims".into());
    }
    serde_json::from_slice::<Strict>(bytes)
        .map(|v| v.0)
        .map_err(|_| "unsupported_claims".into())
}

fn decode(raw: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| "unsupported_claims".into())
}

pub(crate) fn parse_keys(raw: &[u8], now: u64) -> Result<KeyCache, String> {
    let value = strict_json(raw)?;
    let keys = value
        .get("keys")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty() && v.len() <= 16)
        .ok_or("keys_unavailable")?;
    let mut seen = BTreeSet::new();
    let mut parsed = Vec::new();
    for key in keys {
        let kid = text(key, "kid")?;
        if !seen.insert(kid)
            || text(key, "kty")? != "RSA"
            || key.get("alg").is_some_and(|v| v != "RS256")
            || key.get("use").is_some_and(|v| v != "sig")
            || key.get("key_ops").is_some_and(|v| v != &json!(["verify"]))
            || ["d", "p", "q", "dp", "dq", "qi"]
                .iter()
                .any(|k| key.get(k).is_some())
        {
            return Err("keys_unavailable".into());
        }
        let n = text(key, "n")?;
        let e = text(key, "e")?;
        let modulus = decode(n)?;
        let exponent = decode(e)?;
        if !(256..=1024).contains(&modulus.len())
            || modulus.first() == Some(&0)
            || exponent.is_empty()
            || exponent.len() > 8
            || exponent.first() == Some(&0)
        {
            return Err("keys_unavailable".into());
        }
        parsed.push(RsaKey {
            kid: kid.into(),
            n: n.into(),
            e: e.into(),
        });
    }
    Ok(KeyCache {
        version: 1,
        fetched_at: now,
        keys: parsed,
    })
}

#[cfg(cargo_ai_cli)]
pub(crate) fn fetch_keys(now: u64) -> Result<KeyCache, String> {
    #[cfg(test)]
    if let Some(value) = TEST_KEYS.with(|keys| keys.borrow().clone()) {
        return value;
    }
    // A short owned thread keeps the blocking client's runtime separate from a CLI async caller.
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|_| "keys_unavailable")?;
        let response = client
            .get("https://auth.openai.com/.well-known/jwks.json")
            .send()
            .map_err(|_| "keys_unavailable")?;
        if response.status() != reqwest::StatusCode::OK {
            return Err("keys_unavailable".into());
        }
        let mut bytes = Vec::new();
        response
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "keys_unavailable")?;
        parse_keys(&bytes, now)
    })
    .join()
    .map_err(|_| "keys_unavailable".to_owned())?
}

fn verify(token: &str, account: &str, cache: &KeyCache, now: u64) -> Result<String, String> {
    if cache.version != 1 || cache.fetched_at > now || now - cache.fetched_at > KEY_MAX_AGE {
        return Err("keys_unavailable".into());
    }
    let cache_shape = json!({"keys":cache.keys.iter().map(|k|json!({"kid":k.kid,"kty":"RSA","alg":"RS256","n":k.n,"e":k.e})).collect::<Vec<_>>()});
    parse_keys(
        &serde_json::to_vec(&cache_shape).map_err(|_| "keys_unavailable")?,
        cache.fetched_at,
    )?;
    if token.len() > MAX_TOKEN {
        return Err("unsupported_claims".into());
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return Err("unsupported_token".into());
    }
    let header = strict_json(&decode(parts[0])?)?;
    if text(&header, "alg")? != "RS256"
        || header.get("crit").is_some()
        || header.get("jku").is_some()
        || header.get("jwk").is_some()
        || header.get("x5u").is_some()
        || header.get("b64").is_some()
    {
        return Err("unsupported_token".into());
    }
    let kid = text(&header, "kid")?;
    let mut matches = cache.keys.iter().filter(|k| k.kid == kid);
    let key = matches.next().ok_or("keys_unavailable")?;
    if matches.next().is_some() {
        return Err("keys_unavailable".into());
    }
    let n = decode(&key.n)?;
    let e = decode(&key.e)?;
    let signature = decode(parts[2])?;
    ring::signature::RsaPublicKeyComponents { n: &n, e: &e }
        .verify(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &signature,
        )
        .map_err(|_| "invalid_signature")?;
    let claims = strict_json(&decode(parts[1])?)?;
    if text(&claims, "iss")? != ISSUER {
        return Err("unsupported_issuer".into());
    }
    let valid_audience = match claims.get("aud") {
        Some(Value::String(v)) => v == AUDIENCE,
        Some(Value::Array(v)) => {
            !v.is_empty() && v.len() <= 8 && v.iter().all(|a| a.as_str() == Some(AUDIENCE))
        }
        _ => false,
    };
    if !valid_audience {
        return Err("unsupported_audience".into());
    }
    let exp = claims
        .get("exp")
        .and_then(Value::as_u64)
        .ok_or("unsupported_claims")?;
    let iat = claims
        .get("iat")
        .and_then(Value::as_u64)
        .ok_or("unsupported_claims")?;
    let nbf = claims
        .get("nbf")
        .and_then(Value::as_u64)
        .ok_or("unsupported_claims")?;
    if exp <= now.saturating_add(30) || iat > now || nbf > now || exp <= iat || exp <= nbf {
        return Err("credential_expired_or_not_current".into());
    }
    let sub = text(&claims, "sub")?;
    let client = text(&claims, "client_id")?;
    let auth = claims
        .get(NAMESPACE)
        .and_then(Value::as_object)
        .ok_or("unsupported_claims")?;
    let auth_value = Value::Object(auth.clone());
    if text(&auth_value, "chatgpt_account_id")? != account {
        return Err("selected_account_mismatch".into());
    }
    text(&auth_value, "chatgpt_user_id")?;
    for key in ["chatgpt_account_user_id", "user_id", "poid"] {
        if auth.get(key).is_some() {
            text(&auth_value, key)?;
        }
    }
    // Preserve every recognized security/routing field, including optional presence.
    const AUTH_FIELDS: &[&str] = &[
        "amr",
        "chatgpt_account_id",
        "chatgpt_account_user_id",
        "chatgpt_compute_residency",
        "chatgpt_plan_type",
        "chatgpt_user_id",
        "localhost",
        "poid",
        "user_id",
    ];
    if auth.keys().any(|k| !AUTH_FIELDS.contains(&k.as_str())) {
        return Err("unsupported_claims".into());
    }
    let scopes = claims
        .get("scp")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .ok_or("unsupported_claims")?;
    let mut scope_set = BTreeSet::new();
    for scope in scopes {
        let value = scope
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_graphic()))
            .ok_or("unsupported_claims")?;
        scope_set.insert(value);
    }
    // Unrecognized claims cannot silently introduce an unbound routing or authority dimension.
    const CLAIMS: &[&str] = &[
        "aud",
        "client_id",
        "exp",
        NAMESPACE,
        "https://api.openai.com/profile",
        "iat",
        "iss",
        "jti",
        "nbf",
        "pwd_auth_time",
        "scp",
        "session_id",
        "sl",
        "sub",
    ];
    if claims
        .as_object()
        .ok_or("unsupported_claims")?
        .keys()
        .any(|k| !CLAIMS.contains(&k.as_str()))
    {
        return Err("unsupported_claims".into());
    }
    let mut routing = auth.clone();
    routing.remove("chatgpt_plan_type");
    let anchor = json!({"version":1,"issuer":ISSUER,"audience":AUDIENCE,"sub":sub,"client_id":client,"auth":routing,"scp":scope_set,"sl":claims.get("sl")});
    let bytes = serde_json::to_vec(&anchor).map_err(|_| "unsupported_claims")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn account_snapshot(
    cache: Option<&KeyCache>,
    now: u64,
) -> Result<AccountSnapshot, String> {
    #[cfg(cargo_ai_cli)]
    let path = super::openai_oauth::codex_auth_path().map_err(|_| "credential_unavailable")?;
    #[cfg(not(cargo_ai_cli))]
    let path = crate::codex_auth_path().map_err(|_| "credential_unavailable")?;
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| "credential_unavailable")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("credential_unavailable".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("credential_unavailable".into());
        }
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "credential_unavailable")?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "credential_unavailable")?;
    let value = strict_json(&bytes)?;
    if value.get("auth_mode").is_some_and(|v| v != "chatgpt")
        || value.get("OPENAI_API_KEY").is_some_and(|v| !v.is_null())
    {
        return Err("unsupported_token".into());
    }
    let tokens = value.get("tokens").ok_or("unsupported_token")?;
    let token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= MAX_TOKEN)
        .ok_or("unsupported_token")?;
    let account = text(tokens, "account_id")?;
    let identity = verify(token, account, cache.ok_or("keys_unavailable")?, now)?;
    Ok(AccountSnapshot {
        token: token.into(),
        account: account.into(),
        identity,
    })
}

#[cfg(all(test, cargo_ai_cli))]
thread_local! { static TEST_KEYS: std::cell::RefCell<Option<Result<KeyCache,String>>> = const { std::cell::RefCell::new(None) }; }

#[cfg(all(test, cargo_ai_cli))]
pub(crate) mod test_support {
    use super::*;
    pub(crate) fn keys(now: u64) -> KeyCache {
        parse_keys(include_bytes!("testdata/access-continuity-jwks.json"), now).unwrap()
    }
    pub(crate) fn keys_without_signer(now: u64) -> KeyCache {
        let mut cache = keys(now);
        cache.keys[0].kid = "different-synthetic-signer".into();
        cache
    }
    pub(crate) fn set_keys(value: Option<Result<KeyCache, String>>) {
        TEST_KEYS.with(|keys| *keys.borrow_mut() = value);
    }
    pub(crate) fn claims(now: u64) -> Value {
        json!({"iss":ISSUER,"aud":AUDIENCE,"sub":"synthetic-user","client_id":"synthetic-client","iat":now-60,"nbf":now-60,"exp":now+3600,"scp":["model.request","offline_access"],NAMESPACE:{"chatgpt_account_id":"synthetic-account","chatgpt_user_id":"synthetic-user","chatgpt_account_user_id":"synthetic-membership"}})
    }
    pub(crate) fn signed_token(claims: &Value) -> String {
        sign_raw(
            &serde_json::to_vec(claims).unwrap(),
            json!({"alg":"RS256","kid":"synthetic-continuity-test"}),
        )
    }
    fn sign_raw(claims: &[u8], header: Value) -> String {
        let key = ring::signature::RsaKeyPair::from_pkcs8(include_bytes!(
            "testdata/access-continuity-key.pk8"
        ))
        .unwrap();
        let input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
            URL_SAFE_NO_PAD.encode(claims)
        );
        let mut signature = vec![0; key.public().modulus_len()];
        key.sign(
            &ring::signature::RSA_PKCS1_SHA256,
            &ring::rand::SystemRandom::new(),
            input.as_bytes(),
            &mut signature,
        )
        .unwrap();
        format!("{}.{}", input, URL_SAFE_NO_PAD.encode(signature))
    }
    pub(crate) fn write_session(codex_home: &std::path::Path, claims: &Value) -> String {
        std::fs::create_dir_all(codex_home).unwrap();
        let token = signed_token(claims);
        std::fs::write(codex_home.join("auth.json"),serde_json::to_vec(&json!({"auth_mode":"chatgpt","tokens":{"access_token":token,"account_id":"synthetic-account","id_token":"expired-id-token-is-ignored"}})).unwrap()).unwrap();
        token
    }
    #[test]
    fn signed_renewal_preserves_identity_and_rejects_untrusted_access() {
        let now = 1_800_000_000;
        let original = claims(now);
        let token = signed_token(&original);
        let cache = keys(now);
        let identity = verify(&token, "synthetic-account", &cache, now).unwrap();
        let mut renewed = original.clone();
        renewed["iat"] = json!(now);
        renewed["nbf"] = json!(now);
        renewed["exp"] = json!(now + 7200);
        renewed["jti"] = json!("new-token");
        renewed["session_id"] = json!("new-session");
        let next = signed_token(&renewed);
        assert_ne!(token, next);
        assert_eq!(
            identity,
            verify(&next, "synthetic-account", &cache, now).unwrap()
        );
        for (field, value) in [
            ("sub", json!("another-user")),
            ("client_id", json!("another-client")),
            ("scp", json!(["model.request", "more.authority"])),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            assert_ne!(
                identity,
                verify(&signed_token(&changed), "synthetic-account", &cache, now).unwrap()
            );
        }
        for (field, value) in [
            ("chatgpt_account_id", json!("another-account")),
            ("chatgpt_user_id", json!("another-user")),
            ("chatgpt_account_user_id", json!("another-membership")),
            ("chatgpt_compute_residency", json!("another-region")),
            ("localhost", json!(true)),
        ] {
            let mut changed = original.clone();
            changed[NAMESPACE][field] = value;
            let selected = changed[NAMESPACE]["chatgpt_account_id"].as_str().unwrap();
            assert_ne!(
                identity,
                verify(&signed_token(&changed), selected, &cache, now).unwrap()
            );
        }
        let mut plan_label = original.clone();
        plan_label[NAMESPACE]["chatgpt_plan_type"] = json!("different-display-plan");
        assert_eq!(
            identity,
            verify(&signed_token(&plan_label), "synthetic-account", &cache, now).unwrap()
        );
        assert!(verify("opaque-replacement", "synthetic-account", &cache, now).is_err());
        for (field, value) in [
            ("iss", json!("https://untrusted.invalid")),
            ("aud", json!([AUDIENCE, "unknown-audience"])),
            ("exp", json!(now)),
            ("iat", json!(now + 1)),
            ("scp", json!("model.request")),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            assert!(verify(&signed_token(&changed), "synthetic-account", &cache, now).is_err());
        }
        assert!(verify(&token, "another-account", &cache, now).is_err());
        assert!(verify(&token, "synthetic-account", &cache, now + KEY_MAX_AGE + 1).is_err());
        let mut invalid = token.into_bytes();
        let end = invalid.len() - 8;
        invalid[end] = if invalid[end] == b'A' { b'B' } else { b'A' };
        assert!(verify(
            std::str::from_utf8(&invalid).unwrap(),
            "synthetic-account",
            &cache,
            now
        )
        .is_err());
        let raw = serde_json::to_string(&original).unwrap();
        let duplicate = format!("{{\"sub\":\"duplicate\",{}", &raw[1..]);
        let token = sign_raw(
            duplicate.as_bytes(),
            json!({"alg":"RS256","kid":"synthetic-continuity-test"}),
        );
        assert!(verify(&token, "synthetic-account", &cache, now).is_err());
        let mut duplicated = cache.clone();
        duplicated.keys.push(duplicated.keys[0].clone());
        assert!(verify(&next, "synthetic-account", &duplicated, now).is_err());
    }
}
