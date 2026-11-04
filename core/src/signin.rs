//! Spotify sign-in, the same on iPhone and Mac.
//!
//! One call does whatever is still missing, in order:
//! 1. **Account**: approval in the browser sheet for your own Spotify app (the Web API:
//!    library, likes, search).
//! 2. **Playback**: approval for Spotify's desktop client, whose token is the only kind
//!    its streaming servers let in. librespot trades it for reusable credentials, kept in
//!    the keychain, so later launches reconnect without the browser.
//!
//! Each step shows in `AccountState::sign_in`, and every failure ends as a sentence and
//! a retry (calling sign-in again picks up where things stand), never a stuck spinner.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::*;

/// Why the streaming servers didn't take a login.
enum Refusal {
    /// The credentials are no good (expired, revoked, or for another client): approve again.
    Credentials,
    Premium,
    /// Couldn't get through (network, or every access point turning us away).
    Unreachable(String),
}

impl Refusal {
    fn message(&self) -> String {
        match self {
            Refusal::Credentials => "Spotify didn't accept the playback sign-in. Sign in again to approve playback.".into(),
            Refusal::Premium => "Playback needs Spotify Premium.".into(),
            Refusal::Unreachable(why) if why.contains("Try another access point") => {
                "Spotify's streaming servers turned the app away. Try again in a minute.".into()
            }
            Refusal::Unreachable(why) => format!("Couldn't reach Spotify's streaming servers ({why})."),
        }
    }
}

/// One playback reconnect at a time (launch, coming back to the app).
static RECONNECTING: AtomicBool = AtomicBool::new(false);

#[uniffi::export]
impl StreamCore {
    /// Signs in to Spotify, doing only the steps still missing. Safe to call again after
    /// a failure or a cancel.
    pub async fn spotify_sign_in(self: Arc<Self>) -> Result<(), CoreError> {
        let me = self.clone();
        let task = self.rt.spawn(async move { me.sign_in_steps().await });
        if let Some(old) = self.sign_in_task.lock().replace(task.abort_handle()) {
            old.abort();
        }
        match task.await {
            Ok(Ok(())) => {
                self.set_step(SignInStep::Idle);
                Ok(())
            }
            Ok(Err(message)) => {
                self.set_step(SignInStep::Failed { message: message.clone() });
                Err(CoreError::Failed { message })
            }
            // Cancelled, or replaced by a newer sign-in that now owns the state.
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => {
                let message = e.to_string();
                self.set_step(SignInStep::Failed { message: message.clone() });
                Err(CoreError::Failed { message })
            }
        }
    }

    /// The browser sheet was closed: stop waiting on it and free its port.
    pub fn cancel_sign_in(&self) {
        if let Some(h) = self.sign_in_task.lock().take() {
            h.abort();
        }
        self.set_step(SignInStep::Idle);
    }
}

impl StreamCore {
    async fn sign_in_steps(self: Arc<Self>) -> Result<(), String> {
        let client_id = self
            .account
            .lock()
            .spotify_client_id
            .clone()
            .ok_or_else(|| "Enter your Spotify app's Client ID first.".to_string())?;

        if self.web.read().is_none() {
            self.set_step(SignInStep::Approving { step: 1 });
            let tokens = self.approve(&client_id, auth::WEB_PORT, auth::WEB_SCOPES).await?;
            self.delegate.secret_set(SECRET_WEB_TOKENS.into(), serde_json::to_string(&tokens).ok());
            self.install_web(client_id, tokens);
            self.verify_web().await.map_err(|e| format!("Spotify signed you in but didn't answer for the account ({e})."))?;
            self.emit_account();
            // The library needs only the account: it syncs while playback is approved, and
            // whether or not playback connects.
            let me = self.clone();
            self.rt.spawn(async move {
                let _ = me.refresh_spotify_inner().await;
            });
        }

        if self.playback.read().is_none() {
            // Saved credentials first; a fresh approval only when Spotify refuses them.
            let saved = self.delegate.secret_get(SECRET_PLAYBACK.into()).and_then(|s| sp_session::credentials_from_json(&s));
            let mut connected = false;
            if let Some(creds) = saved {
                self.set_step(SignInStep::Connecting);
                match self.connect_playback(creds).await {
                    Ok(()) => connected = true,
                    Err(Refusal::Credentials) => self.delegate.secret_set(SECRET_PLAYBACK.into(), None),
                    Err(other) => return Err(other.message()),
                }
            }
            if !connected {
                self.set_step(SignInStep::Approving { step: 2 });
                let grant = self.approve(auth::PLAYBACK_CLIENT_ID, auth::PLAYBACK_PORT, auth::PLAYBACK_SCOPES).await?;
                self.set_step(SignInStep::Connecting);
                self.connect_playback(sp_session::token_credentials(&grant.access_token)).await.map_err(|r| r.message())?;
            }
            self.emit_account();
        }
        Ok(())
    }

    /// One approval in the browser sheet, answered on the loopback port.
    async fn approve(&self, client_id: &str, port: u16, scopes: &[&str]) -> Result<Tokens, String> {
        let delegate = self.delegate.clone();
        auth::authorize(&self.http, &self.endpoints, client_id, port, scopes, |u| delegate.open_url(u))
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Logs in to the streaming servers, a few tries for network trouble (librespot
    /// already walks the access points on each).
    async fn connect_playback(&self, creds: librespot_core::authentication::Credentials) -> Result<(), Refusal> {
        // A stand-in Spotify (the UI tests') hands out tokens the real streaming servers
        // always refuse; refuse them here, as they would, without going online for it.
        if self.endpoints.accounts != auth::Endpoints::default().accounts {
            return Err(Refusal::Credentials);
        }
        let normalization = self.settings.lock().normalization;
        let mut last = String::new();
        for attempt in 0..3u64 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(attempt * 2)).await;
            }
            match Playback::connect(creds.clone(), &self.device_id, &self.audio_dir, normalization, tokio::runtime::Handle::current()).await {
                Ok((pb, reusable)) => {
                    self.delegate.secret_set(SECRET_PLAYBACK.into(), Some(sp_session::credentials_to_json(&reusable)));
                    *self.playback.write() = Some(Arc::new(pb));
                    self.account.lock().spotify_playback = true;
                    return Ok(());
                }
                Err(e) => {
                    last = format!("{e:#}");
                    log::warn!("playback login, try {}: {last}", attempt + 1);
                    if last.contains("Bad credentials") || last.contains("Could not validate credentials") {
                        return Err(Refusal::Credentials);
                    }
                    if last.contains("Premium") {
                        return Err(Refusal::Premium);
                    }
                }
            }
        }
        Err(Refusal::Unreachable(last))
    }

    /// Reconnects playback from saved credentials when it isn't up (launch, coming back
    /// to the app). Refused credentials are dropped, so the app asks to sign in again.
    pub(crate) fn reconnect_playback(self: Arc<Self>) {
        let busy = self.sign_in_task.lock().as_ref().is_some_and(|h| !h.is_finished());
        if busy || self.playback.read().is_some() || RECONNECTING.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(creds) = self.delegate.secret_get(SECRET_PLAYBACK.into()).and_then(|s| sp_session::credentials_from_json(&s)) else {
            RECONNECTING.store(false, Ordering::SeqCst);
            return;
        };
        let me = self.clone();
        self.rt.spawn(async move {
            match me.connect_playback(creds).await {
                Ok(()) => {}
                Err(Refusal::Credentials) => me.delegate.secret_set(SECRET_PLAYBACK.into(), None),
                Err(other) => log::warn!("playback reconnect: {}", other.message()),
            }
            RECONNECTING.store(false, Ordering::SeqCst);
            me.emit_account();
        });
    }

    /// At launch: Last.fm and Spotify as they were left.
    pub(crate) fn restore_accounts(self: &Arc<Self>) {
        let client_id = self.db.get_kv("spotify_client_id").filter(|s| !s.is_empty());
        self.account.lock().spotify_client_id = client_id.clone();
        if let Some(api) = self.delegate.secret_get(SECRET_LASTFM_API.into())
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&api)
            && let (Some(k), Some(s)) = (v["key"].as_str(), v["secret"].as_str())
        {
            *self.lastfm.write() = Some(Arc::new(lastfm::LastFm::new(self.http.clone(), k.into(), s.into())));
        }
        if let Some(s) = self.delegate.secret_get(SECRET_LASTFM.into())
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&s)
        {
            self.account.lock().lastfm_user = v["name"].as_str().map(str::to_string);
        }
        if let (Some(cid), Some(tokens)) = (
            client_id,
            self.delegate.secret_get(SECRET_WEB_TOKENS.into()).and_then(|s| serde_json::from_str::<Tokens>(&s).ok()),
        ) {
            self.install_web(cid, tokens);
        }
        self.clone().reconnect_playback();
        let me = self.clone();
        self.rt.spawn(async move {
            if me.web.read().is_some() {
                let _ = me.verify_web().await;
                me.emit_account();
                let _ = me.clone().refresh_spotify_inner().await;
            }
        });
        self.clone().rescan();
    }

    fn install_web(&self, client_id: String, tokens: Tokens) {
        let delegate = self.delegate.clone();
        let on_refresh = Arc::new(move |t: &Tokens| {
            delegate.secret_set(SECRET_WEB_TOKENS.into(), serde_json::to_string(t).ok());
        });
        *self.web.write() = Some(Arc::new(WebApi::new(self.http.clone(), self.endpoints.clone(), client_id, tokens, on_refresh)));
        self.account.lock().spotify_web = true;
    }

    async fn verify_web(&self) -> anyhow::Result<String> {
        let web = self.web.read().clone().ok_or_else(|| anyhow::anyhow!("not signed in"))?;
        let me = web.me().await?;
        let id = me["id"].as_str().unwrap_or("").to_string();
        let mut a = self.account.lock();
        a.spotify_user = me["display_name"].as_str().or(me["id"].as_str()).map(str::to_string);
        a.spotify_premium = me["product"].as_str() == Some("premium");
        drop(a);
        self.db.set_kv("spotify_user_id", &id);
        Ok(id)
    }

    fn set_step(&self, step: SignInStep) {
        self.account.lock().sign_in = step;
        self.emit_account();
    }
}
