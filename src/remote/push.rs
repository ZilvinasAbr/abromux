//! Web Push to the phone app: what lets an iPhone show "Claude needs
//! input" with the app closed (a web app on the Home Screen, iOS 16.4+).
//!
//! abromux is its own push application server: a VAPID key pair (RFC 8292)
//! kept in `push.json`, and the subscriptions phones made with its public
//! key. A push is the payload encrypted for the subscription (RFC 8291,
//! `aes128gcm`) and POSTed to its endpoint (Apple's, Google's, Mozilla's)
//! with `/usr/bin/curl`. An endpoint that answers 404 or 410 is gone, and
//! its subscription is dropped.

use aes_gcm::{Aes128Gcm, KeyInit, aead::Aead};
use anyhow::{Context as _, anyhow, bail};
use base64::Engine as _;
use hkdf::Hkdf;
use p256::{
    PublicKey, SecretKey,
    ecdsa::{Signature, SigningKey, signature::Signer},
    elliptic_curve::sec1::ToEncodedPoint,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{
    io::Write as _,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Who sends (VAPID's `sub`): the project, not a person.
const CONTACT: &str = "https://github.com/ZilvinasAbr/abromux";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Subscription {
    pub endpoint: String,
    /// The phone's ECDH public key (base64url, uncompressed P-256).
    pub p256dh: String,
    /// The phone's auth secret (base64url, 16 bytes).
    pub auth: String,
    /// When it was added (Unix seconds), for `remote status`.
    #[serde(default)]
    pub added: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    /// The VAPID private key (base64url, 32 bytes).
    #[serde(default)]
    vapid: String,
    #[serde(default)]
    subscriptions: Vec<Subscription>,
}

static LOCK: Mutex<()> = Mutex::new(());

fn path() -> PathBuf {
    crate::channel::support_dir().join("push.json")
}

fn load() -> Stored {
    std::fs::read(path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(s: &Stored) -> anyhow::Result<()> {
    super::write_private(&path(), &serde_json::to_vec_pretty(s)?)
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("the system's random source");
    b
}

fn new_secret() -> SecretKey {
    loop {
        // Nearly every 32 random bytes are a valid scalar.
        if let Ok(k) = SecretKey::from_slice(&random::<32>()) {
            return k;
        }
    }
}

/// The VAPID key, made the first time it's needed.
fn vapid(s: &mut Stored) -> anyhow::Result<SecretKey> {
    if let Ok(k) = B64
        .decode(&s.vapid)
        .map_err(anyhow::Error::from)
        .and_then(|b| SecretKey::from_slice(&b).map_err(|_| anyhow!("bad VAPID key")))
    {
        return Ok(k);
    }
    let k = new_secret();
    s.vapid = B64.encode(k.to_bytes());
    save(s)?;
    Ok(k)
}

fn public_b64(k: &SecretKey) -> String {
    B64.encode(k.public_key().to_encoded_point(false).as_bytes())
}

/// The public key phones subscribe with (`applicationServerKey`).
pub fn public_key() -> anyhow::Result<String> {
    let _g = LOCK.lock().unwrap();
    let mut s = load();
    Ok(public_b64(&vapid(&mut s)?))
}

pub fn subscriptions() -> Vec<Subscription> {
    load().subscriptions
}

/// Add (or refresh) a subscription from the browser's `PushSubscription.toJSON()`.
pub fn subscribe(v: &Value) -> anyhow::Result<()> {
    let endpoint = v["endpoint"].as_str().context("no endpoint")?;
    if !endpoint.starts_with("https://") {
        bail!("endpoint isn't https");
    }
    let sub = Subscription {
        endpoint: endpoint.to_string(),
        p256dh: v["keys"]["p256dh"].as_str().context("no p256dh")?.into(),
        auth: v["keys"]["auth"].as_str().context("no auth")?.into(),
        added: now(),
    };
    // Check the keys now rather than at the first push.
    PublicKey::from_sec1_bytes(&B64.decode(&sub.p256dh)?).map_err(|_| anyhow!("bad p256dh"))?;
    if B64.decode(&sub.auth)?.len() != 16 {
        bail!("bad auth secret");
    }
    let _g = LOCK.lock().unwrap();
    let mut s = load();
    s.subscriptions.retain(|x| x.endpoint != sub.endpoint);
    s.subscriptions.push(sub);
    save(&s)
}

pub fn unsubscribe(endpoint: &str) -> anyhow::Result<bool> {
    let _g = LOCK.lock().unwrap();
    let mut s = load();
    let before = s.subscriptions.len();
    s.subscriptions.retain(|x| x.endpoint != endpoint);
    let removed = s.subscriptions.len() != before;
    if removed {
        save(&s)?;
    }
    Ok(removed)
}

/// Forget every subscription (and keep the key).
pub fn clear() -> anyhow::Result<usize> {
    let _g = LOCK.lock().unwrap();
    let mut s = load();
    let n = s.subscriptions.len();
    s.subscriptions.clear();
    save(&s)?;
    Ok(n)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Send `payload` (JSON the service worker shows) to every subscription.
/// Returns how many took it. Gone ones are dropped.
pub fn send_all(payload: &Value) -> usize {
    let (key, subs) = {
        let _g = LOCK.lock().unwrap();
        let mut s = load();
        let Ok(key) = vapid(&mut s) else { return 0 };
        (key, s.subscriptions)
    };
    let body = payload.to_string();
    let mut sent = 0;
    for sub in subs {
        match send(&key, &sub, body.as_bytes()) {
            Ok(()) => sent += 1,
            Err(Gone) => {
                let _ = unsubscribe(&sub.endpoint);
            }
        }
    }
    sent
}

/// A subscription its push service no longer has.
struct Gone;

fn send(key: &SecretKey, sub: &Subscription, payload: &[u8]) -> Result<(), Gone> {
    let body = match encrypt(sub, payload) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("abromux remote: push: {e:#}");
            return Ok(());
        }
    };
    let Some(aud) = origin(&sub.endpoint) else {
        return Err(Gone);
    };
    let jwt = match vapid_jwt(key, &aud) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("abromux remote: push: {e:#}");
            return Ok(());
        }
    };
    let auth = format!("Authorization: vapid t={jwt}, k={}", public_b64(key));
    let child = Command::new("/usr/bin/curl")
        .args([
            "-sS",
            "--max-time",
            "20",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
        ])
        .args(["-X", "POST", "--data-binary", "@-"])
        .args(["-H", "Content-Type: application/octet-stream"])
        .args(["-H", "Content-Encoding: aes128gcm"])
        .args(["-H", "TTL: 86400", "-H", "Urgency: high"])
        .args(["-H", &auth])
        .arg(&sub.endpoint)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return Ok(());
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&body);
    }
    let Ok(out) = child.wait_with_output() else {
        return Ok(());
    };
    match String::from_utf8_lossy(&out.stdout).trim() {
        "404" | "410" => Err(Gone),
        code if code.starts_with('2') => Ok(()),
        code => {
            eprintln!("abromux remote: push to {aud}: HTTP {code}");
            Ok(())
        }
    }
}

/// `https://host` of an endpoint URL (VAPID's audience).
fn origin(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split('/').next()?;
    (!host.is_empty()).then(|| format!("https://{host}"))
}

/// The VAPID JWT (ES256) for `aud`, good for 12 hours.
fn vapid_jwt(key: &SecretKey, aud: &str) -> anyhow::Result<String> {
    let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = B64.encode(
        json!({ "aud": aud, "exp": now() + 12 * 3600, "sub": CONTACT })
            .to_string()
            .as_bytes(),
    );
    let input = format!("{header}.{claims}");
    let sig: Signature = SigningKey::from(key).sign(input.as_bytes());
    Ok(format!("{input}.{}", B64.encode(sig.to_bytes())))
}

/// RFC 8291: `payload` encrypted for `sub` as one aes128gcm record, with a
/// new ephemeral key and salt.
fn encrypt(sub: &Subscription, payload: &[u8]) -> anyhow::Result<Vec<u8>> {
    encrypt_with(sub, payload, &new_secret(), random::<16>())
}

fn encrypt_with(
    sub: &Subscription,
    payload: &[u8],
    eph: &SecretKey,
    salt: [u8; 16],
) -> anyhow::Result<Vec<u8>> {
    let ua_bytes = B64.decode(&sub.p256dh).context("p256dh")?;
    let ua = PublicKey::from_sec1_bytes(&ua_bytes).map_err(|_| anyhow!("bad p256dh"))?;
    let auth = B64.decode(&sub.auth).context("auth")?;
    let shared = p256::ecdh::diffie_hellman(eph.to_nonzero_scalar(), ua.as_affine());
    let as_pub = eph.public_key().to_encoded_point(false);
    let as_pub = as_pub.as_bytes();

    // IKM = HKDF(auth, ecdh_secret, "WebPush: info" || 0 || ua_public || as_public)
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(&ua_bytes);
    info.extend_from_slice(as_pub);
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&auth), shared.raw_secret_bytes())
        .expand(&info, &mut ikm)
        .map_err(|_| anyhow!("hkdf"))?;
    let prk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| anyhow!("hkdf"))?;
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| anyhow!("hkdf"))?;

    // One record: the payload, then the last-record delimiter.
    let mut plain = payload.to_vec();
    plain.push(2);
    let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|_| anyhow!("aes key"))?;
    let sealed = cipher
        .encrypt(&nonce.into(), plain.as_slice())
        .map_err(|_| anyhow!("aes-gcm"))?;

    let mut out = Vec::with_capacity(86 + sealed.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(as_pub.len() as u8);
    out.extend_from_slice(as_pub);
    out.extend_from_slice(&sealed);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8291 appendix A: the example's keys, salt and message give its
    /// exact body.
    #[test]
    fn rfc8291_example() {
        let as_private = B64
            .decode("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")
            .unwrap();
        let sub = Subscription {
            endpoint: "https://push.example.net/push/JzLQ3raZJfFBR0aqvOMsLrt54w4rJUsV".into(),
            p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4".into(),
            auth: "BTBZMqHH6r4Tts7J_aSIgg".into(),
            added: 0,
        };
        let eph = SecretKey::from_slice(&as_private).unwrap();
        let salt: [u8; 16] = B64
            .decode("DGv6ra1nlYgDCS1FRnbzlw")
            .unwrap()
            .try_into()
            .unwrap();
        let body = encrypt_with(
            &sub,
            b"When I grow up, I want to be a watermelon",
            &eph,
            salt,
        )
        .unwrap();
        assert_eq!(
            B64.encode(body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn vapid_jwt_verifies() {
        use p256::ecdsa::{VerifyingKey, signature::Verifier};
        let key = new_secret();
        let jwt = vapid_jwt(&key, "https://web.push.apple.com").unwrap();
        let (input, sig) = jwt.rsplit_once('.').unwrap();
        let sig = Signature::from_slice(&B64.decode(sig).unwrap()).unwrap();
        VerifyingKey::from(key.public_key())
            .verify(input.as_bytes(), &sig)
            .unwrap();
        let claims = input.split('.').nth(1).unwrap();
        let claims: Value = serde_json::from_slice(&B64.decode(claims).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://web.push.apple.com");
    }

    #[test]
    fn endpoint_origin() {
        assert_eq!(
            origin("https://web.push.apple.com/QH3x?x=1").as_deref(),
            Some("https://web.push.apple.com")
        );
        assert_eq!(origin("http://x/y"), None);
    }
}
