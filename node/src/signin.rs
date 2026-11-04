//! Signing in to the Stream node with aza (protocol section 7).
//!
//! A Stream client proves who it is with its certificate and a signed challenge, and gets a bearer
//! token. A Mac has no aza app, so it takes the other-machine path: it posts its certificate request
//! here, shows the QR code, the aza app on the phone fetches the request from
//! `/.well-known/aza/request/<id>` and posts the certificate to the return URL (`/aza`), and the
//! Mac collects it from `/aza/certificate/<its fingerprint>`.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aza::certificate::{self, Certificate, Request};

/// How long a challenge, a hosted request or a received certificate is kept.
const TTL: Duration = Duration::from_secs(10 * 60);
/// The most of each kept at once, so nobody can fill the node's memory.
const MAX: usize = 10_000;

struct Hosted {
    bytes: Vec<u8>,
    client: Vec<u8>,
    at: Instant,
}

#[derive(Default)]
struct State {
    challenges: HashMap<[u8; 32], Instant>,
    /// Request ID → the request.
    requests: HashMap<String, Hosted>,
    /// Client fingerprint (hex) → the return URL with its certificate.
    received: HashMap<String, (String, Instant)>,
}

impl State {
    fn prune(&mut self) {
        let fresh = |at: &Instant| at.elapsed() < TTL;
        self.challenges.retain(|_, at| fresh(at));
        self.requests.retain(|_, h| fresh(&h.at));
        self.received.retain(|_, (_, at)| fresh(at));
    }
}

pub struct SignIns {
    pub app: String,
    state: Mutex<State>,
}

#[derive(Debug)]
pub enum Refused {
    Busy,
    Bad(String),
}

impl SignIns {
    pub fn new(app: &str) -> SignIns {
        SignIns { app: app.into(), state: Default::default() }
    }

    pub fn challenge(&self) -> Result<[u8; 32], Refused> {
        let mut s = self.state.lock().unwrap();
        s.prune();
        if s.challenges.len() >= MAX {
            return Err(Refused::Busy);
        }
        let c: [u8; 32] = rand::random();
        s.challenges.insert(c, Instant::now());
        Ok(c)
    }

    /// Checks a sign-in (checks 1 to 5) and returns the identity ID. Each challenge works once.
    pub async fn check(&self, cert: &Certificate, challenge: &[u8; 32], proof: &[u8]) -> Result<String, Refused> {
        if self.state.lock().unwrap().challenges.remove(challenge).is_none() {
            return Err(Refused::Bad("unknown or used challenge".into()));
        }
        let (log, _) = aza::client::fetch_identity(&cert.node, &cert.identity)
            .await
            .map_err(|e| Refused::Bad(e.to_string()))?;
        let id = aza::app::check_sign_in_with(&log, &self.app, cert, challenge, proof).map_err(|e| Refused::Bad(e.to_string()))?;
        Ok(hex::encode(id))
    }

    /// Hosts a request (`aza:approve#req=…`) for the other-machine path. Returns its QR code payload.
    pub fn host(&self, approve_url: &str) -> Result<String, Refused> {
        let request = Request::from_approve_url(approve_url).map_err(|e| Refused::Bad(e.to_string()))?;
        if request.app != self.app {
            return Err(Refused::Bad("request is for another app".into()));
        }
        request.validate().map_err(|e| Refused::Bad(e.to_string()))?;
        let mut s = self.state.lock().unwrap();
        s.prune();
        if s.requests.len() >= MAX {
            return Err(Refused::Busy);
        }
        let id = hex::encode(rand::random::<[u8; 16]>());
        let bytes = request.to_value().to_bytes();
        let qr = certificate::qr_payload(&Request::well_known_url(&self.app, &id), &aza::crypto::sha256(&bytes));
        s.requests.insert(id, Hosted { bytes, client: request.client, at: Instant::now() });
        Ok(qr)
    }

    /// `GET /.well-known/aza/request/<id>`.
    pub fn request(&self, id: &str) -> Option<Vec<u8>> {
        let mut s = self.state.lock().unwrap();
        s.prune();
        s.requests.get(id).map(|h| h.bytes.clone())
    }

    /// The aza app's `POST` to the return URL. Only for a client whose request is hosted here.
    pub fn receive(&self, body: &[u8]) -> Result<(), Refused> {
        let cert = Certificate::from_bytes(body).map_err(|e| Refused::Bad(e.to_string()))?;
        let mut s = self.state.lock().unwrap();
        s.prune();
        let ours = cert.app == self.app && s.requests.values().any(|h| h.client == cert.client);
        if !ours {
            return Err(Refused::Bad("certificate for a client that didn't ask here".into()));
        }
        s.requests.retain(|_, h| h.client != cert.client);
        let fingerprint = hex::encode(aza::crypto::fingerprint(&cert.client));
        let url = certificate::return_with_certificate(&format!("https://{}/aza", self.app), &cert);
        s.received.insert(fingerprint, (url, Instant::now()));
        Ok(())
    }

    /// Hands a received certificate, as its return URL, to the client that asked.
    pub fn take(&self, fingerprint: &str) -> Option<String> {
        let mut s = self.state.lock().unwrap();
        s.prune();
        s.received.remove(fingerprint).map(|(url, _)| url)
    }
}
