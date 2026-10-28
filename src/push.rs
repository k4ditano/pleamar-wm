//! Notices to the phone (Web Push) from the remote page (remote.rs): when an
//! agent needs the person —what waits at the gate, what one asks on its
//! screen— and, if asked, when one has answered a letter. The phone's page
//! subscribes (its browser's push service gives an endpoint and two keys);
//! here each notice is encrypted for that phone alone (RFC 8291, aes128gcm)
//! and signed as this server (VAPID, RFC 8292), so the push service that
//! carries it can read nothing. Sent with `curl`.
//!
//! By default only when the person is away: two minutes with no input in the
//! session (`person-seen`, which the session writes as input comes). What
//! needed them while they were here and still does when they leave is said
//! then.

use openssl::bn::BigNumContext;
use openssl::derive::Deriver;
use openssl::ec::{EcGroup, EcKey, EcPoint, PointConversionForm};
use openssl::ecdsa::EcdsaSig;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private, Public};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

fn state_dir() -> String {
    let base = std::env::var("XDG_STATE_HOME").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| format!("{}/.local/state", std::env::var("HOME").unwrap_or_default()));
    format!("{base}/pleamar")
}

/// Written whole, readable by its owner only.
fn write_private(path: &str, text: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = format!("{path}.new");
    let mut f = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
    f.write_all(text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub(crate) fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..=c.len() {
            out.push(A[((n >> (18 - 6 * k)) & 63) as usize] as char);
        }
    }
    out
}

pub(crate) fn unb64url(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'-' | b'+' => Some(62),
        b'_' | b'/' => Some(63),
        _ => None,
    };
    let s: Vec<u8> = s.bytes().filter(|&c| c != b'=' && !c.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for c in s.chunks(4) {
        if c.len() == 1 {
            return None;
        }
        let mut n = 0u32;
        for (k, &ch) in c.iter().enumerate() {
            n |= u32::from(val(ch)?) << (18 - 6 * k);
        }
        for k in 0..c.len() - 1 {
            out.push((n >> (16 - 8 * k)) as u8);
        }
    }
    Some(out)
}

fn group() -> EcGroup {
    EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).expect("P-256")
}

/// A point as Web Push writes it: uncompressed, 65 bytes.
fn raw_public(key: &EcKey<impl openssl::pkey::HasPublic>) -> Vec<u8> {
    let mut ctx = BigNumContext::new().expect("bn");
    key.public_key().to_bytes(&group(), PointConversionForm::UNCOMPRESSED, &mut ctx).unwrap_or_default()
}

fn public_from_raw(raw: &[u8]) -> Result<PKey<Public>, String> {
    let g = group();
    let mut ctx = BigNumContext::new().map_err(|e| e.to_string())?;
    let point = EcPoint::from_bytes(&g, raw, &mut ctx).map_err(|_| "not a P-256 point".to_owned())?;
    let key = EcKey::from_public_key(&g, &point).map_err(|e| e.to_string())?;
    PKey::from_ec_key(key).map_err(|e| e.to_string())
}

fn hmac(key: &[u8], data: &[&[u8]]) -> [u8; 32] {
    use hmac::Mac;
    let mut m = hmac::Hmac::<Sha256>::new_from_slice(key).expect("any key");
    for d in data {
        m.update(d);
    }
    m.finalize().into_bytes().into()
}

/// HKDF-SHA256 (RFC 5869), up to 32 bytes.
fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    let prk = hmac(salt, &[ikm]);
    hmac(&prk, &[info, &[1]])[..len].to_vec()
}

/// A notice encrypted for one subscription (RFC 8291, aes128gcm): with this
/// server's key for the message `sender` and a `salt`, both new each time.
fn encrypt_with(sender: &EcKey<Private>, salt: &[u8; 16], ua_public: &[u8], auth: &[u8], plain: &[u8]) -> Result<Vec<u8>, String> {
    let theirs = public_from_raw(ua_public)?;
    let mine = PKey::from_ec_key(sender.clone()).map_err(|e| e.to_string())?;
    let mut d = Deriver::new(&mine).map_err(|e| e.to_string())?;
    d.set_peer(&theirs).map_err(|e| e.to_string())?;
    let shared = d.derive_to_vec().map_err(|e| e.to_string())?;
    let as_public = raw_public(sender);
    let key_info = [b"WebPush: info\0".as_slice(), ua_public, &as_public].concat();
    let ikm = hkdf(auth, &shared, &key_info, 32);
    let cek = hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
    let nonce = hkdf(salt, &ikm, b"Content-Encoding: nonce\0", 12);
    let mut record = plain.to_vec();
    record.push(2); // the last record
    let mut tag = [0u8; 16];
    let sealed = openssl::symm::encrypt_aead(openssl::symm::Cipher::aes_128_gcm(), &cek, Some(&nonce), &[], &record, &mut tag).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(86 + sealed.len() + 16);
    out.extend_from_slice(salt);
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(&as_public);
    out.extend_from_slice(&sealed);
    out.extend_from_slice(&tag);
    Ok(out)
}

fn encrypt(ua_public: &[u8], auth: &[u8], plain: &[u8]) -> Result<Vec<u8>, String> {
    let sender = EcKey::generate(&group()).map_err(|e| e.to_string())?;
    let mut salt = [0u8; 16];
    openssl::rand::rand_bytes(&mut salt).map_err(|e| e.to_string())?;
    encrypt_with(&sender, &salt, ua_public, auth, plain)
}

/// This server's own key (VAPID): made once and kept.
fn vapid() -> Result<EcKey<Private>, String> {
    let path = format!("{}/remote-push-key.pem", state_dir());
    if let Ok(pem) = std::fs::read(&path) {
        if let Ok(k) = EcKey::private_key_from_pem(&pem) {
            return Ok(k);
        }
    }
    let k = EcKey::generate(&group()).map_err(|e| e.to_string())?;
    write_private(&path, &k.private_key_to_pem().map_err(|e| e.to_string())?)?;
    Ok(k)
}

/// The key a page subscribes with (`applicationServerKey`).
pub(crate) fn public_key() -> Result<String, String> {
    Ok(b64url(&raw_public(&vapid()?)))
}

/// `Authorization: vapid t=…, k=…` for an endpoint: a JWT (ES256) for its origin, good for twelve hours.
fn authorization(key: &EcKey<Private>, endpoint: &str) -> Result<String, String> {
    let origin = endpoint.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
    let exp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) + 12 * 3600;
    let head = b64url(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = b64url(json!({ "aud": origin, "exp": exp, "sub": "https://github.com/k4ditano/pleamar-wm" }).to_string().as_bytes());
    let signed = format!("{head}.{claims}");
    let sig = EcdsaSig::sign(&Sha256::digest(signed.as_bytes()), key).map_err(|e| e.to_string())?;
    let mut raw = sig.r().to_vec_padded(32).map_err(|e| e.to_string())?;
    raw.extend(sig.s().to_vec_padded(32).map_err(|e| e.to_string())?);
    Ok(format!("vapid t={signed}.{}, k={}", b64url(&raw), b64url(&raw_public(key))))
}

/// Sent: `Ok(true)` taken, `Ok(false)` the subscription is gone (404, 410).
fn send(sub: &Value, notice: &Value) -> Result<bool, String> {
    let endpoint = sub["endpoint"].as_str().ok_or("no endpoint")?;
    if !endpoint.starts_with("https://") {
        return Err("an endpoint is https".into());
    }
    let ua = unb64url(sub["p256dh"].as_str().unwrap_or("")).ok_or("bad p256dh")?;
    let auth = unb64url(sub["auth"].as_str().unwrap_or("")).ok_or("bad auth")?;
    let body = encrypt(&ua, &auth, notice.to_string().as_bytes())?;
    let authz = authorization(&vapid()?, endpoint)?;
    let mut child = std::process::Command::new("curl")
        .args(["-sS", "-m", "20", "-o", "/dev/null", "-w", "%{http_code}", "-X", "POST", "-H", "TTL: 86400", "-H", "Urgency: high", "-H", "Content-Encoding: aes128gcm", "-H", "Content-Type: application/octet-stream"])
        .arg("-H")
        .arg(format!("Authorization: {authz}"))
        .args(["--data-binary", "@-", "--", endpoint])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    {
        use std::io::Write;
        let _ = child.stdin.take().map(|mut s| s.write_all(&body));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let code = String::from_utf8_lossy(&out.stdout).trim().parse::<u16>().unwrap_or(0);
    match code {
        200..=299 => Ok(true),
        404 | 410 => Ok(false),
        _ => Err(format!("{code} {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}

// ── The subscriptions: one per phone, with what it wants ──

fn subs_path() -> String {
    format!("{}/remote-push.json", state_dir())
}

fn subs() -> Vec<Value> {
    std::fs::read_to_string(subs_path()).ok().and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok()).unwrap_or_default()
}

fn save(list: &[Value]) {
    if let Err(e) = write_private(&subs_path(), Value::Array(list.to_vec()).to_string().as_bytes()) {
        eprintln!("remote · push: {e}");
    }
}

/// What the page asks (`POST /caravan/push`): subscribe, change what it
/// wants, a test notice, unsubscribe. `session` is its sign-in's hash: when
/// that ends, its notices end.
pub(crate) fn act(session: &str, body: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "not understood".to_owned())?;
    let endpoint = v["sub"]["endpoint"].as_str().or(v["endpoint"].as_str()).unwrap_or("").to_owned();
    if !endpoint.starts_with("https://") || endpoint.len() > 2000 {
        return Err("no push endpoint".into());
    }
    let mut list = subs();
    let at = list.iter().position(|s| s["endpoint"].as_str() == Some(endpoint.as_str()));
    let flag = |k: &str, default: bool| v[k].as_bool().unwrap_or(default);
    match v["do"].as_str().unwrap_or("") {
        "subscribe" => {
            let (p256dh, auth) = (v["sub"]["keys"]["p256dh"].as_str().unwrap_or(""), v["sub"]["keys"]["auth"].as_str().unwrap_or(""));
            if unb64url(p256dh).map_or(0, |k| k.len()) != 65 || unb64url(auth).map_or(0, |k| k.len()) != 16 {
                return Err("its keys are not a push subscription's".into());
            }
            let s = json!({
                "endpoint": endpoint, "p256dh": p256dh, "auth": auth, "session": session,
                "away_only": flag("away_only", true), "finished": flag("finished", false),
                "lang": if v["lang"].as_str().is_some_and(|l| l.starts_with("es")) { "es" } else { "en" },
            });
            match at {
                Some(k) => list[k] = s,
                None => list.push(s),
            }
            save(&list);
            Ok("on".into())
        }
        "settings" => {
            let k = at.ok_or("not subscribed")?;
            for key in ["away_only", "finished"] {
                if let Some(b) = v[key].as_bool() {
                    list[k][key] = json!(b);
                }
            }
            save(&list);
            Ok(json!({ "away_only": list[k]["away_only"], "finished": list[k]["finished"] }).to_string())
        }
        "state" => Ok(match at {
            Some(k) => json!({ "on": true, "away_only": list[k]["away_only"], "finished": list[k]["finished"] }),
            None => json!({ "on": false }),
        }
        .to_string()),
        "test" => {
            let k = at.ok_or("not subscribed")?;
            let es = list[k]["lang"] == "es";
            let notice = json!({ "title": if es { "Así te llegarán los avisos" } else { "This is how notices will come" }, "body": if es { "Cuando un compañero te necesite." } else { "When a companion needs you." }, "tag": "test", "url": "/caravan" });
            match send(&list[k], &notice) {
                Ok(true) => Ok("sent".into()),
                Ok(false) => {
                    list.remove(k);
                    save(&list);
                    Err("its browser dropped the subscription: turn notices on again".into())
                }
                Err(e) => Err(e),
            }
        }
        "unsubscribe" => {
            if let Some(k) = at {
                list.remove(k);
                save(&list);
            }
            Ok("off".into())
        }
        other => Err(format!("«{other}»: nothing to do")),
    }
}

/// When a sign-in ends (signed out, expired): its phones get no more notices.
pub(crate) fn forget_session(session: &str) {
    let list = subs();
    let kept: Vec<Value> = list.iter().filter(|s| s["session"].as_str() != Some(session)).cloned().collect();
    if kept.len() != list.len() {
        save(&kept);
    }
}

// ── What needs the person ──

/// Something to say: its key (said once), whether it needs the person, who, and the words in both languages.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Need {
    pub key: String,
    pub needs: bool,
    pub name: String,
    pub title: (String, String),
    pub body: String,
}

/// From the caravan's state (`caravan::companions()`): what waits at the
/// gate, what asks on its screen, and the last letter each answered.
pub(crate) fn needs(state: &Value) -> Vec<Need> {
    let comps = state["companions"].as_array().cloned().unwrap_or_default();
    let title_of = |name: &str| -> String {
        comps.iter().find(|c| c["name"].as_str() == Some(name)).and_then(|c| c["card"]["title"].as_str().filter(|t| !t.is_empty()).map(str::to_owned)).unwrap_or_else(|| name.to_owned())
    };
    let clip = |t: &str, n: usize| -> String {
        let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.chars().count() > n { format!("{}…", t.chars().take(n).collect::<String>()) } else { t }
    };
    let mut out = Vec::new();
    for p in state["pending"].as_array().into_iter().flatten() {
        let name = p["name"].as_str().filter(|n| !n.is_empty()).or(p["agent"].as_str()).unwrap_or("?").to_owned();
        let who = title_of(&name);
        out.push(Need {
            key: format!("gate:{}", p["id"].as_str().unwrap_or("")),
            needs: true,
            title: (format!("{who} asks you for permission"), format!("{who} te pide permiso")),
            body: clip(&format!("{} · {}", p["tool"].as_str().unwrap_or(""), p["what"].as_str().unwrap_or("")), 160),
            name,
        });
    }
    for c in &comps {
        let name = c["name"].as_str().unwrap_or("").to_owned();
        let who = title_of(&name);
        if let Some(q) = c["question"].as_object() {
            let opts: Vec<String> = q.get("options").and_then(Value::as_array).into_iter().flatten().filter_map(|o| o["text"].as_str().map(str::to_owned)).collect();
            out.push(Need {
                key: format!("ask:{name}:{}", q.get("key").and_then(Value::as_str).unwrap_or("")),
                needs: true,
                title: (format!("{who} asks you"), format!("{who} te pregunta")),
                body: clip(&format!("{}{}", q.get("text").and_then(Value::as_str).unwrap_or(""), if opts.is_empty() { String::new() } else { format!(" · {}", opts.join(" / ")) }), 160),
                name: name.clone(),
            });
        }
        if let (Some(at), Some(last)) = (c["last_at"].as_u64(), c["last"].as_str()) {
            out.push(Need {
                key: format!("done:{name}:{at}"),
                needs: false,
                title: (format!("{who} has finished"), format!("{who} ha terminado")),
                body: clip(last, 160),
                name,
            });
        }
    }
    out
}

/// Away: no input in the session for two minutes. The session writes
/// `person-seen` (seconds since 1970) as input comes; with no such file (a
/// session from before it wrote one) nobody can say, and notices go.
fn away() -> bool {
    let run = std::env::var("XDG_RUNTIME_DIR").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "/tmp".into());
    let Ok(t) = std::fs::read_to_string(format!("{run}/pleamar/person-seen")) else { return true };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    t.trim().parse::<u64>().map_or(true, |seen| now.saturating_sub(seen) >= 120)
}

/// The watch, every three seconds while the server runs. What was there
/// when it began is not news; each need is said once to each phone; what
/// needed the person while they were here and still does once they leave,
/// then. `live(session)`: whether that sign-in still holds.
pub(crate) fn watch(live: impl Fn(&str) -> bool) {
    let mut said: HashMap<String, HashSet<String>> = HashMap::new();
    let mut first = true;
    let mut last_clean = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(3));
        let list = subs();
        if list.is_empty() {
            first = false;
            continue;
        }
        let Ok(state) = serde_json::from_str::<Value>(&crate::caravan::companions()) else { continue };
        let now_needs = needs(&state);
        if first {
            // (Already there: not news.)
            for s in &list {
                let e = s["endpoint"].as_str().unwrap_or("").to_owned();
                said.entry(e).or_default().extend(now_needs.iter().map(|n| n.key.clone()));
            }
            first = false;
            continue;
        }
        let is_away = away();
        let mut dropped = Vec::new();
        for s in &list {
            let endpoint = s["endpoint"].as_str().unwrap_or("").to_owned();
            if !live(s["session"].as_str().unwrap_or("")) {
                dropped.push(endpoint);
                continue;
            }
            let seen = said.entry(endpoint.clone()).or_insert_with(|| now_needs.iter().map(|n| n.key.clone()).collect());
            if s["away_only"] != false && !is_away {
                continue;
            }
            let es = s["lang"] == "es";
            for n in &now_needs {
                if seen.contains(&n.key) || (!n.needs && s["finished"] != true) {
                    continue;
                }
                seen.insert(n.key.clone());
                let notice = json!({
                    "title": if es { &n.title.1 } else { &n.title.0 }, "body": n.body,
                    "tag": n.key, "url": format!("/caravan#{}", n.name),
                });
                match send(s, &notice) {
                    Ok(true) => println!("remote · push: «{}» to a phone", n.key),
                    Ok(false) => {
                        dropped.push(endpoint.clone());
                        break;
                    }
                    Err(e) => eprintln!("remote · push: {e}"),
                }
            }
        }
        if !dropped.is_empty() {
            let kept: Vec<Value> = subs().into_iter().filter(|s| !dropped.iter().any(|d| s["endpoint"].as_str() == Some(d.as_str()))).collect();
            save(&kept);
            println!("remote · push: {} subscription(s) gone", dropped.len());
        }
        // (What was said and is no longer there, forgotten now and then.)
        if last_clean.elapsed() > Duration::from_secs(600) {
            let keys: HashSet<String> = now_needs.iter().map(|n| n.key.clone()).collect();
            for seen in said.values_mut() {
                seen.retain(|k| keys.contains(k));
            }
            last_clean = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::bn::BigNum;

    fn private_from(d: &str, x_y: &str) -> EcKey<Private> {
        let g = group();
        let mut ctx = BigNumContext::new().unwrap();
        let point = EcPoint::from_bytes(&g, &unb64url(x_y).unwrap(), &mut ctx).unwrap();
        EcKey::from_private_components(&g, &BigNum::from_slice(&unb64url(d).unwrap()).unwrap(), &point).unwrap()
    }

    /// RFC 8291, section 5: the example, byte for byte.
    #[test]
    fn a_notice_is_encrypted_as_the_rfc_says() {
        let sender = private_from("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw", "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8");
        let salt: [u8; 16] = unb64url("DGv6ra1nlYgDCS1FRnbzlw").unwrap().try_into().unwrap();
        let ua = unb64url("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4").unwrap();
        let auth = unb64url("BTBZMqHH6r4Tts7J_aSIgg").unwrap();
        let body = encrypt_with(&sender, &salt, &ua, &auth, b"When I grow up, I want to be a watermelon").unwrap();
        assert_eq!(b64url(&body), "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN");
    }

    /// The phone's side: what it decrypts is what was sent.
    #[test]
    fn the_phone_reads_what_was_sent() {
        let phone = EcKey::generate(&group()).unwrap();
        let auth = [7u8; 16];
        let body = encrypt(&raw_public(&phone), &auth, br#"{"title":"Prohard te pide permiso"}"#).unwrap();
        let (salt, idlen) = (&body[..16], body[20] as usize);
        let sender = public_from_raw(&body[21..21 + idlen]).unwrap();
        let mut d = Deriver::new(&*Box::leak(Box::new(PKey::from_ec_key(phone.clone()).unwrap()))).unwrap();
        d.set_peer(&sender).unwrap();
        let shared = d.derive_to_vec().unwrap();
        let info = [b"WebPush: info\0".as_slice(), &raw_public(&phone), &body[21..21 + idlen]].concat();
        let ikm = hkdf(&auth, &shared, &info, 32);
        let cek = hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
        let nonce = hkdf(salt, &ikm, b"Content-Encoding: nonce\0", 12);
        let rest = &body[21 + idlen..];
        let (ct, tag) = rest.split_at(rest.len() - 16);
        let plain = openssl::symm::decrypt_aead(openssl::symm::Cipher::aes_128_gcm(), &cek, Some(&nonce), &[], ct, tag).unwrap();
        assert_eq!(&plain[..plain.len() - 1], br#"{"title":"Prohard te pide permiso"}"#);
        assert_eq!(*plain.last().unwrap(), 2);
    }

    /// VAPID: a JWT for the endpoint's origin, signed with the key it gives.
    #[test]
    fn the_signature_is_this_servers_and_for_that_origin() {
        let key = EcKey::generate(&group()).unwrap();
        let a = authorization(&key, "https://fcm.googleapis.com/fcm/send/abc:def").unwrap();
        let (t, k) = a.strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
        assert_eq!(unb64url(k).unwrap(), raw_public(&key));
        let parts: Vec<&str> = t.split('.').collect();
        let claims: Value = serde_json::from_slice(&unb64url(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        let sig = unb64url(parts[2]).unwrap();
        let es = EcdsaSig::from_private_components(BigNum::from_slice(&sig[..32]).unwrap(), BigNum::from_slice(&sig[32..]).unwrap()).unwrap();
        let public = EcKey::from_public_key(&group(), key.public_key()).unwrap();
        assert!(es.verify(&Sha256::digest(format!("{}.{}", parts[0], parts[1]).as_bytes()), &public).unwrap());
    }

    #[test]
    fn what_needs_the_person_and_what_only_finished() {
        let state = json!({
            "pending": [{ "id": "a1", "name": "prohard", "agent": "codex", "tool": "Bash", "what": "git push origin main" }],
            "companions": [
                { "name": "prohard", "card": { "title": "Prohard" }, "question": null, "last": null, "last_at": null },
                { "name": "prueba", "card": { "title": "Prueba" }, "question": { "key": "k9", "text": "¿Borro tmp?", "options": [{ "n": 1, "text": "Sí" }, { "n": 2, "text": "No" }] }, "last": "Hecho: dos", "last_at": 1700000000000u64 },
            ],
        });
        let n = needs(&state);
        assert_eq!(n[0].key, "gate:a1");
        assert_eq!(n[0].title.1, "Prohard te pide permiso");
        assert_eq!(n[0].body, "Bash · git push origin main");
        assert_eq!(n[1].key, "ask:prueba:k9");
        assert_eq!(n[1].body, "¿Borro tmp? · Sí / No");
        assert!(n[0].needs && n[1].needs && !n[2].needs);
        assert_eq!(n[2].title.1, "Prueba ha terminado");
    }

    /// Through `curl` to a push service of mentira (`PUSH_TEST_ENDPOINT`, https, its certificate in
    /// `CURL_CA_BUNDLE`, which writes what it gets to `PUSH_TEST_GOT`): what arrives, the phone reads.
    #[test]
    #[ignore]
    fn sent_through_curl_and_read_by_the_phone() {
        let endpoint = std::env::var("PUSH_TEST_ENDPOINT").expect("PUSH_TEST_ENDPOINT");
        let got = std::env::var("PUSH_TEST_GOT").expect("PUSH_TEST_GOT");
        let phone = EcKey::generate(&group()).unwrap();
        let auth = [9u8; 16];
        let sub = json!({ "endpoint": endpoint, "p256dh": b64url(&raw_public(&phone)), "auth": b64url(&auth) });
        assert_eq!(send(&sub, &json!({ "title": "Prohard te pide permiso", "body": "Bash · git push" })), Ok(true));
        let got: Value = serde_json::from_str(&std::fs::read_to_string(got).unwrap()).unwrap();
        assert_eq!(got["headers"]["content-encoding"], "aes128gcm");
        assert!(got["headers"]["authorization"].as_str().unwrap().starts_with("vapid t="));
        let body = unb64url(got["body"].as_str().unwrap()).unwrap();
        let idlen = body[20] as usize;
        let sender = public_from_raw(&body[21..21 + idlen]).unwrap();
        let mine = PKey::from_ec_key(phone.clone()).unwrap();
        let mut d = Deriver::new(&mine).unwrap();
        d.set_peer(&sender).unwrap();
        let shared = d.derive_to_vec().unwrap();
        let info = [b"WebPush: info\0".as_slice(), &raw_public(&phone), &body[21..21 + idlen]].concat();
        let ikm = hkdf(&auth, &shared, &info, 32);
        let cek = hkdf(&body[..16], &ikm, b"Content-Encoding: aes128gcm\0", 16);
        let nonce = hkdf(&body[..16], &ikm, b"Content-Encoding: nonce\0", 12);
        let rest = &body[21 + idlen..];
        let (ct, tag) = rest.split_at(rest.len() - 16);
        let plain = openssl::symm::decrypt_aead(openssl::symm::Cipher::aes_128_gcm(), &cek, Some(&nonce), &[], ct, tag).unwrap();
        let notice: Value = serde_json::from_slice(&plain[..plain.len() - 1]).unwrap();
        assert_eq!(notice["title"], "Prohard te pide permiso");
    }

    #[test]
    fn base64url_both_ways() {
        for t in [b"".as_slice(), b"a", b"ab", b"abc", b"abcd", &[0xff, 0xfe, 0x00]] {
            assert_eq!(unb64url(&b64url(t)).unwrap(), t);
        }
        assert_eq!(b64url(b"\xfb\xff"), "-_8");
    }
}
