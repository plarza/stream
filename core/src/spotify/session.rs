//! librespot: the streaming session and its player, feeding our output.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use librespot_core::authentication::Credentials;
use librespot_core::cache::Cache;
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_core::{SpotifyId, SpotifyUri};
use librespot_playback::audio_backend::{Sink, SinkResult};
use librespot_playback::config::{Bitrate, NormalisationType, PlayerConfig};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::mixer::NoOpVolume;
use librespot_playback::player::{Player, PlayerEvent};
use librespot_protocol::authentication::AuthenticationType;
use parking_lot::Mutex;

use super::auth::PLAYBACK_CLIENT_ID;
use crate::audio::{Output, TrackWriter};
use crate::local::Outcome;
use crate::types::{LyricLine, LyricSync, LyricWord, Lyrics};

/// librespot always decodes to 44.1 kHz stereo.
const SPOTIFY_RATE: u32 = 44_100;

type Tap = Arc<Mutex<Option<TrackWriter>>>;

struct TapSink(Tap);

impl Sink for TapSink {
    fn write(&mut self, packet: AudioPacket, _: &mut Converter) -> SinkResult<()> {
        if let AudioPacket::Samples(samples) = packet {
            let f: Vec<f32> = samples.iter().map(|s| *s as f32).collect();
            if let Some(w) = self.0.lock().as_mut() {
                w.write(&f, 2);
            }
        }
        Ok(())
    }
}

pub struct Playback {
    pub session: Session,
    /// Spotify's own APIs turn away requests carrying our user agent.
    http: reqwest::Client,
    player: Arc<Player>,
    tap: Tap,
    events: std::sync::Mutex<mpsc::Receiver<PlayerEvent>>,
}

/// Credentials from the browser grant: the streaming access token bound to the account.
pub fn token_credentials(access_token: &str) -> Credentials {
    Credentials::with_access_token(access_token)
}

pub fn credentials_to_json(c: &Credentials) -> String {
    serde_json::to_string(c).unwrap_or_default()
}

pub fn credentials_from_json(s: &str) -> Option<Credentials> {
    serde_json::from_str(s).ok()
}

impl Playback {
    /// Connects and returns the session plus reusable credentials to store.
    pub async fn connect(
        credentials: Credentials,
        device_id: &str,
        audio_cache: &Path,
        normalisation: bool,
        rt: tokio::runtime::Handle,
    ) -> Result<(Self, Credentials)> {
        let config = SessionConfig {
            client_id: PLAYBACK_CLIENT_ID.to_string(),
            device_id: device_id.to_string(),
            ..SessionConfig::default()
        };
        // The audio cache holds downloads for offline play; 20 GB ceiling.
        let cache = Cache::new(None::<&Path>, None, Some(audio_cache), Some(20 * 1024 * 1024 * 1024))?;
        let session = Session::new(config, Some(cache));
        // librespot's own reason ("Bad credentials", "Try another access point", ...)
        // comes through as the message; sign-in turns it into words for people.
        tokio::time::timeout(Duration::from_secs(30), session.connect(credentials, false))
            .await
            .map_err(|_| anyhow::anyhow!("timed out"))?
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let reusable = Credentials {
            username: Some(session.username()),
            auth_type: AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
            auth_data: session.auth_data(),
        };

        let tap: Tap = Arc::new(Mutex::new(None));
        let sink_tap = tap.clone();
        let player_config = PlayerConfig {
            bitrate: Bitrate::Bitrate320,
            gapless: true,
            normalisation,
            normalisation_type: NormalisationType::Auto,
            position_update_interval: None,
            ..PlayerConfig::default()
        };
        let player = Player::new(player_config, session.clone(), Box::new(NoOpVolume), move || {
            Box::new(TapSink(sink_tap)) as Box<dyn Sink>
        });

        let (tx, rx) = mpsc::channel();
        let mut channel = player.get_player_event_channel();
        rt.spawn(async move {
            while let Some(ev) = channel.recv().await {
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });

        Ok((Self { session, http: reqwest::Client::new(), player, tap, events: std::sync::Mutex::new(rx) }, reusable))
    }

    /// Plays one song into the output, returning when it has been fully
    /// written or a jump cancelled it. `next` is preloaded for gapless play.
    pub fn play(
        &self,
        uri: &str,
        start_ms: u32,
        out: Arc<Output>,
        generation: u64,
        next: Option<&str>,
    ) -> Result<Outcome> {
        let track = SpotifyUri::from_uri(uri).map_err(|e| anyhow::anyhow!("bad uri {uri}: {e}"))?;
        let events = self.events.lock().unwrap_or_else(|p| p.into_inner());
        while events.try_recv().is_ok() {}
        *self.tap.lock() = Some(TrackWriter::new(out.clone(), generation, SPOTIFY_RATE, 1.0));
        self.player.load(track.clone(), true, start_ms);
        let mut preloaded = false;
        let outcome = loop {
            if out.generation() != generation {
                self.player.stop();
                break Outcome::Cancelled;
            }
            match events.recv_timeout(Duration::from_millis(50)) {
                Ok(PlayerEvent::EndOfTrack { track_id, .. }) if track_id == track => break Outcome::Finished,
                Ok(PlayerEvent::Unavailable { track_id, .. }) if track_id == track => {
                    anyhow::bail!("this song is unavailable on Spotify")
                }
                Ok(PlayerEvent::TimeToPreloadNextTrack { .. }) => {
                    if !preloaded
                        && let Some(n) = next.and_then(|n| SpotifyUri::from_uri(n).ok())
                    {
                        self.player.preload(n);
                        preloaded = true;
                    }
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.session.is_invalid() {
                        anyhow::bail!("the Spotify session dropped");
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("player stopped"),
            }
        };
        if let Some(mut w) = self.tap.lock().take()
            && matches!(outcome, Outcome::Finished)
        {
            w.finish();
        }
        Ok(outcome)
    }

    pub fn stop(&self) {
        self.player.stop();
    }

    /// One of the Spotify apps' own GraphQL queries, signed with this session.
    /// Covers what the Web API no longer gives developer apps (artist pages).
    pub async fn pathfinder(&self, operation: &str, hash: &str, variables: serde_json::Value) -> Result<serde_json::Value> {
        let token = self.session.login5().auth_token().await?;
        let client_token = self.session.spclient().client_token().await?;
        let body = serde_json::json!({
            "variables": variables,
            "operationName": operation,
            "extensions": { "persistedQuery": { "version": 1, "sha256Hash": hash } },
        });
        let v: serde_json::Value = self
            .http
            .post("https://api-partner.spotify.com/pathfinder/v2/query")
            .bearer_auth(&token.access_token)
            .header("client-token", client_token)
            .header("app-platform", "WebPlayer")
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if v["data"].is_null() {
            anyhow::bail!("Spotify didn't answer {operation}: {}", v["errors"]);
        }
        Ok(v)
    }

    pub async fn lyrics(&self, uri: &str) -> Option<Lyrics> {
        let id = SpotifyId::from_base62(uri.rsplit(':').next()?).ok()?;
        let bytes = self.session.spclient().get_lyrics(&id).await.ok()?;
        let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        parse_spotify_lyrics(&json)
    }
}

fn ms(v: &serde_json::Value) -> Option<u32> {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_u64().map(|n| n as u32))
}

pub fn parse_spotify_lyrics(json: &serde_json::Value) -> Option<Lyrics> {
    let l = json.get("lyrics")?;
    let sync_type = l["syncType"].as_str().unwrap_or("");
    let synced = sync_type == "LINE_SYNCED" || sync_type == "SYLLABLE_SYNCED";
    let lines: Vec<LyricLine> = l["lines"]
        .as_array()?
        .iter()
        .filter_map(|line| {
            let text = line["words"].as_str()?.trim().to_string();
            if text.is_empty() || text == "\u{266a}" {
                return None;
            }
            let words = line["syllables"]
                .as_array()
                .map(|s| {
                    s.iter()
                        .filter_map(|w| Some(LyricWord { start_ms: ms(&w["startTimeMs"])?, text: w["chars"].as_str()?.to_string() }))
                        .collect()
                })
                .unwrap_or_default();
            Some(LyricLine { start_ms: if synced { ms(&line["startTimeMs"]) } else { None }, text, words })
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    let sync = if lines.iter().any(|l| !l.words.is_empty()) {
        LyricSync::Word
    } else if synced && lines.iter().all(|l| l.start_ms.is_some()) {
        LyricSync::Line
    } else {
        LyricSync::Unsynced
    };
    Some(Lyrics { sync, lines, source: "Spotify".into() })
}

