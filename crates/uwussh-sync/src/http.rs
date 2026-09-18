//! Talking to the server over HTTPS.
//!
//! Blocking, on purpose: every call here belongs to the sync thread, where one
//! request at a time is the whole of the concurrency needed. The terminal's
//! data path has its own runtime and must not share a thread with this.
//!
//! Three things this module is careful about:
//!
//! 1. **Which server.** A pinned fingerprint means exactly one server answers
//!    to that address; anything else fails the handshake (see [`crate::pin`]).
//!    Plain HTTP is refused unless it is localhost, where there is no network
//!    to listen on.
//! 2. **Signing in again.** A device token lasts an hour, and the app runs for
//!    days. Any call that comes back unauthorised signs in again — once — and
//!    is retried, so nothing above this has to know about tokens.
//! 3. **Saying why.** A refusal carries the server's own short reason
//!    (`unauthorized`, `schema`, `rate-limited`), because the interface says
//!    different things for each.

use crate::engine::{Transport, TransportError};
use ed25519_dalek::{Signer, SigningKey};
use parking_lot::Mutex;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use std::time::Duration;
use uuid::Uuid;
use uwussh_proto::api::{
    self, Admitted, ChallengeRequest, ChallengeResponse, ChangePassword, CreateAccount,
    DeviceSummary, EnrolDevice, EnrolmentToken, Health, LoginRequest, LoginResponse, PairMessage,
    PairMessages, PairOpened, WireVault, WireVaultParams,
};
use uwussh_proto::{
    Envelope, PullResponse, PushRequest, PushResponse, SyncCursor, MAX_BATCH, SCHEMA_VERSION,
};

/// How long a request may take. Generous, because the other end may be a NAS
/// with a spinning disk on the other side of a VPN.
const TIMEOUT: Duration = Duration::from_secs(30);
/// Reading pairing messages holds the request open on purpose.
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// The identity a device signs with, once it has one.
struct Device {
    account: Uuid,
    id: Uuid,
    key: SigningKey,
}

/// One server, as this device talks to it.
pub struct Server {
    base: String,
    client: Client,
    device: Option<Device>,
    token: Mutex<Option<String>>,
}

impl Server {
    /// Reach a server, pinning its certificate when there is a fingerprint to
    /// pin. Without one, the certificate has to stand up to the usual checks —
    /// which is what a server behind a reverse proxy with a real certificate
    /// wants.
    pub fn connect(base_url: &str, fingerprint: Option<&str>) -> Result<Self, TransportError> {
        let base = base_url.trim().trim_end_matches('/').to_string();
        if !safe_address(&base) {
            return Err(TransportError::Refused(
                "a sync server must be reached over https (localhost may use http)".into(),
            ));
        }
        let tls = match fingerprint {
            Some(fingerprint) => crate::pin::pinned_config(fingerprint),
            None => crate::pin::webpki_config(),
        };
        let client = Client::builder()
            .use_preconfigured_tls(tls)
            .timeout(TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            // One server, one connection, kept warm between passes.
            .pool_max_idle_per_host(1)
            .user_agent(concat!("UwUSSH/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| TransportError::Unreachable(error.to_string()))?;

        Ok(Self {
            base,
            client,
            device: None,
            token: Mutex::new(None),
        })
    }

    /// The device this client signs as. Without it only the endpoints that
    /// need no token work: health, creating an account, joining one.
    pub fn as_device(mut self, account: Uuid, device: Uuid, key: &[u8; 32]) -> Self {
        self.device = Some(Device {
            account,
            id: device,
            key: SigningKey::from_bytes(key),
        });
        self
    }

    /// A token handed out by creating an account or joining one, so the first
    /// pass after pairing needs no separate sign-in.
    pub fn with_token(self, token: String) -> Self {
        *self.token.lock() = Some(token);
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    // ── The endpoints that need no token ────────────────────────────────────

    pub fn health(&self) -> Result<Health, TransportError> {
        self.send(|| self.client.get(self.url("/healthz")), false)
    }

    /// Create an account and enrol this device as its first.
    pub fn create_account(&self, request: &CreateAccount) -> Result<Admitted, TransportError> {
        let admitted: Admitted = self.send(
            || self.client.post(self.url("/v1/accounts")).json(request),
            false,
        )?;
        *self.token.lock() = Some(admitted.token.clone());
        Ok(admitted)
    }

    /// What a joining device needs before it can prove it knows the master
    /// password: the salt, the costs, and whether an account key is wanted.
    pub fn vault_params(&self, enrolment: &str) -> Result<WireVaultParams, TransportError> {
        self.send(
            || {
                self.client
                    .get(self.url("/v1/vault/params"))
                    .query(&[("enrolment", enrolment)])
            },
            false,
        )
    }

    /// Join an account this device was invited into.
    pub fn enrol(&self, request: &EnrolDevice) -> Result<Admitted, TransportError> {
        let admitted: Admitted = self.send(
            || {
                self.client
                    .post(self.url("/v1/devices/enrol"))
                    .json(request)
            },
            false,
        )?;
        *self.token.lock() = Some(admitted.token.clone());
        Ok(admitted)
    }

    // ── The endpoints a device token opens ──────────────────────────────────

    /// The vault header, wrapped key and all.
    pub fn vault(&self) -> Result<WireVault, TransportError> {
        self.send(|| self.client.get(self.url("/v1/vault")), true)
    }

    /// A new master password: the vault key wrapped again, no record touched.
    pub fn change_password(&self, request: &ChangePassword) -> Result<(), TransportError> {
        self.send_empty(
            || self.client.put(self.url("/v1/vault/key")).json(request),
            true,
        )
    }

    pub fn devices(&self) -> Result<Vec<DeviceSummary>, TransportError> {
        self.send(|| self.client.get(self.url("/v1/devices")), true)
    }

    pub fn revoke(&self, device: Uuid) -> Result<(), TransportError> {
        self.send_empty(
            || {
                self.client
                    .delete(self.url(&format!("/v1/devices/{device}")))
            },
            true,
        )
    }

    /// A one-time token to hand to a device that is joining.
    pub fn enrolment_token(&self) -> Result<EnrolmentToken, TransportError> {
        self.send(|| self.client.post(self.url("/v1/devices/invite")), true)
    }

    // ── Pairing ────────────────────────────────────────────────────────────

    pub fn open_pairing(&self) -> Result<PairOpened, TransportError> {
        self.send(|| self.client.post(self.url("/v1/pair")), true)
    }

    pub fn close_pairing(&self, id: &str) -> Result<(), TransportError> {
        self.send_empty(
            || self.client.delete(self.url(&format!("/v1/pair/{id}"))),
            true,
        )
    }

    /// Leave a handshake message for the other side.
    pub fn pair_send(&self, id: &str, side: &str, message: &[u8]) -> Result<(), TransportError> {
        let request = PairMessage {
            side: side.to_string(),
            message: encode(message),
        };
        self.send_empty(
            || {
                self.client
                    .post(self.url(&format!("/v1/pair/{id}")))
                    .json(&request)
            },
            false,
        )
    }

    /// What the other side has said. With `wait`, the server holds the request
    /// open until something arrives or it gives up — so a handshake costs a
    /// handful of requests rather than hundreds.
    pub fn pair_receive(
        &self,
        id: &str,
        side: &str,
        after: usize,
        wait: bool,
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        let answer: PairMessages = self.send(
            || {
                self.client
                    .get(self.url(&format!("/v1/pair/{id}")))
                    .query(&[
                        ("side", side.to_string()),
                        ("after", after.to_string()),
                        ("wait", wait.to_string()),
                    ])
                    .timeout(WAIT_TIMEOUT)
            },
            false,
        )?;
        answer
            .messages
            .iter()
            .map(|message| {
                decode(message).ok_or_else(|| {
                    TransportError::Refused("a pairing message that is not base64".into())
                })
            })
            .collect()
    }

    // ── Signing in ─────────────────────────────────────────────────────────

    /// Ask for a challenge, sign it, keep the token.
    pub fn sign_in(&self) -> Result<LoginResponse, TransportError> {
        let device = self
            .device
            .as_ref()
            .ok_or_else(|| TransportError::Refused("this device is not enrolled".into()))?;

        let challenge: ChallengeResponse = self.send(
            || {
                self.client
                    .post(self.url("/v1/session/challenge"))
                    .json(&ChallengeRequest {
                        device_id: device.id,
                    })
            },
            false,
        )?;
        let challenge = decode(&challenge.challenge)
            .ok_or_else(|| TransportError::Refused("a challenge that is not base64".into()))?;

        let material = api::session_material(device.account, device.id, &challenge);
        let request = LoginRequest {
            device_id: device.id,
            signature: encode(&device.key.sign(&material).to_bytes()),
        };
        let session: LoginResponse = self.send(
            || self.client.post(self.url("/v1/session")).json(&request),
            false,
        )?;
        *self.token.lock() = Some(session.token.clone());
        Ok(session)
    }

    // ── The plumbing ───────────────────────────────────────────────────────

    fn send<T: DeserializeOwned>(
        &self,
        build: impl Fn() -> RequestBuilder,
        authenticated: bool,
    ) -> Result<T, TransportError> {
        let response = self.attempt(&build, authenticated)?;
        response
            .json()
            .map_err(|error| TransportError::Refused(format!("the server's answer: {error}")))
    }

    fn send_empty(
        &self,
        build: impl Fn() -> RequestBuilder,
        authenticated: bool,
    ) -> Result<(), TransportError> {
        self.attempt(&build, authenticated).map(drop)
    }

    /// One request — and, if the token has run out, one more after signing in
    /// again. Nothing above this has to think about tokens.
    fn attempt(
        &self,
        build: &impl Fn() -> RequestBuilder,
        authenticated: bool,
    ) -> Result<Response, TransportError> {
        let first = self.once(build, authenticated)?;
        if first.status() != StatusCode::UNAUTHORIZED || !authenticated || self.device.is_none() {
            return check(first);
        }
        tracing::debug!("the session had run out; signing in again");
        self.sign_in()?;
        check(self.once(build, authenticated)?)
    }

    fn once(
        &self,
        build: &impl Fn() -> RequestBuilder,
        authenticated: bool,
    ) -> Result<Response, TransportError> {
        let mut request = build();
        if authenticated {
            let token = self.token.lock().clone();
            let token = token.ok_or_else(|| {
                TransportError::Refused("this device has not signed in yet".into())
            })?;
            request = request.bearer_auth(token);
        }
        request
            .send()
            .map_err(|error| TransportError::Unreachable(error.to_string()))
    }
}

impl Transport for Server {
    fn pull(&self, since: SyncCursor, limit: usize) -> Result<PullResponse, TransportError> {
        let limit = limit.min(MAX_BATCH);
        self.send(
            || {
                self.client
                    .get(self.url("/v1/records"))
                    .query(&[("since", since.0.to_string()), ("limit", limit.to_string())])
            },
            true,
        )
    }

    fn push(&self, envelopes: Vec<Envelope>) -> Result<PushResponse, TransportError> {
        let request = PushRequest {
            schema: SCHEMA_VERSION,
            envelopes,
        };
        self.send(
            || self.client.post(self.url("/v1/records")).json(&request),
            true,
        )
    }
}

/// Whether this is an address a secret may be sent to. Plain HTTP reaches no
/// further than this machine, where there is no network in between to listen.
fn safe_address(base: &str) -> bool {
    if let Some(rest) = base.strip_prefix("https://") {
        return !rest.is_empty();
    }
    let Some(rest) = base.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or(rest);
    let host = host.rsplit_once(':').map_or(host, |(host, _)| host);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

/// What the server said, or why it refused.
fn check(response: Response) -> Result<Response, TransportError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    // The server's own short reason, when it sent one: the interface says
    // different things for a wrong password, an old client and too many tries.
    let reason = response
        .json::<api::ApiError>()
        .map(|error| format!("{} ({})", error.message, error.error))
        .unwrap_or_else(|_| format!("HTTP {}", status.as_u16()));
    Err(TransportError::Refused(reason))
}

fn encode(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode(text: &str) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    URL_SAFE_NO_PAD.decode(text.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_only_travels_over_tls_or_to_this_machine() {
        for address in [
            "https://nas.lan:8443",
            "https://uwussh.example.com",
            "http://localhost:8443",
            "http://127.0.0.1:8443",
            "http://[::1]:8443",
        ] {
            assert!(safe_address(address), "{address}");
            assert!(Server::connect(address, None).is_ok(), "{address}");
        }
        for address in [
            "http://nas.lan:8443",
            "http://10.0.0.5:8443",
            "http://uwussh.example.com",
            "nas.lan:8443",
            "ftp://nas.lan",
            "",
        ] {
            assert!(!safe_address(address), "{address}");
            assert!(
                matches!(
                    Server::connect(address, None),
                    Err(TransportError::Refused(_))
                ),
                "{address}"
            );
        }
    }

    #[test]
    fn a_trailing_slash_is_not_part_of_the_address() {
        let server = Server::connect("https://nas.lan:8443/", None).unwrap();
        assert_eq!(server.url("/healthz"), "https://nas.lan:8443/healthz");
    }

    #[test]
    fn a_device_that_has_not_signed_in_does_not_send_a_request_without_a_token() {
        let server = Server::connect("https://nas.lan:8443", None).unwrap();
        // Nothing is sent: it fails before the socket is touched.
        assert!(matches!(
            server.devices(),
            Err(TransportError::Refused(reason)) if reason.contains("signed in")
        ));
    }

    #[test]
    fn signing_in_needs_a_device_identity() {
        let server = Server::connect("https://nas.lan:8443", None).unwrap();
        assert!(matches!(
            server.sign_in(),
            Err(TransportError::Refused(reason)) if reason.contains("not enrolled")
        ));
    }

    #[test]
    fn base64_here_is_the_base64_the_server_reads() {
        // URL-safe, no padding — the same as the server's `b64` module.
        assert_eq!(encode(&[251, 255, 190]), "-_--");
        assert_eq!(decode("-_--").unwrap(), vec![251, 255, 190]);
        assert_eq!(decode(" -_-- \n").unwrap(), vec![251, 255, 190]);
        assert!(decode("not base64!!").is_none());
    }
}
