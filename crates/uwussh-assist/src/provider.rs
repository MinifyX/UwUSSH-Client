//! The model providers: Ollama, any OpenAI-compatible server (llama.cpp,
//! LM Studio, vLLM), OpenAI, Anthropic and Mistral.
//!
//! Three request shapes cover them: Ollama's own `/api/chat` (it answers in
//! strict JSON when asked, and lists its models under `/api/tags`), OpenAI's
//! chat completions for OpenAI, Mistral and the compatible servers, and
//! Anthropic's Messages API. Everything is blocking and bounded by timeouts;
//! the app calls it from a thread of its own.
//!
//! An API key goes out in a header marked sensitive and never comes back in an
//! error: whatever a provider says is cut short and has the key blanked out
//! before anyone sees it.

use crate::platform::Platform;
use crate::prompt::{self, Answer, AnswerError, Language};
use reqwest::blocking::{Client as Http, RequestBuilder, Response};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_DETAIL_CHARS: usize = 200;
const MAX_MODELS: usize = 500;
const MAX_URL_CHARS: usize = 2048;
/// Where Ollama listens unless told otherwise.
pub const OLLAMA_DEFAULT_URL: &str = "http://localhost:11434";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    Ollama,
    OpenaiCompatible,
    Openai,
    Anthropic,
    Mistral,
}

impl ProviderKind {
    pub const ALL: [Self; 5] = [
        Self::Ollama,
        Self::OpenaiCompatible,
        Self::Openai,
        Self::Anthropic,
        Self::Mistral,
    ];

    /// The name settings and records use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::OpenaiCompatible => "openai-compatible",
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Mistral => "mistral",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Ollama => Some(OLLAMA_DEFAULT_URL),
            Self::OpenaiCompatible => None,
            Self::Openai => Some("https://api.openai.com/v1"),
            Self::Anthropic => Some("https://api.anthropic.com/v1"),
            Self::Mistral => Some("https://api.mistral.ai/v1"),
        }
    }

    /// Whether the person picks the address: the servers they run themselves.
    pub fn base_url_editable(self) -> bool {
        matches!(self, Self::Ollama | Self::OpenaiCompatible)
    }

    pub fn key_required(self) -> bool {
        matches!(self, Self::Openai | Self::Anthropic | Self::Mistral)
    }

    /// Whether a key can be set at all. Ollama has none.
    pub fn key_allowed(self) -> bool {
        self != Self::Ollama
    }

    /// Models to start from, before the provider's own list is asked for.
    pub fn suggested_models(self) -> &'static [&'static str] {
        match self {
            Self::Openai => &["gpt-5-mini", "gpt-5-nano", "gpt-5"],
            Self::Anthropic => &["claude-sonnet-5-5", "claude-haiku-4-5"],
            Self::Mistral => &[
                "mistral-small-latest",
                "mistral-medium-latest",
                "codestral-latest",
            ],
            Self::Ollama | Self::OpenaiCompatible => &[],
        }
    }
}

/// Why the assistant has no command.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssistError {
    #[error("no provider is set up")]
    NotConfigured,
    #[error("no model is chosen")]
    NoModel,
    #[error("the provider needs an API key")]
    NoKey,
    #[error("the address cannot be used: {0}")]
    Address(&'static str),
    #[error("the request is empty or too long")]
    Request,
    #[error("the provider cannot be reached")]
    Unreachable,
    #[error("the provider took too long to answer")]
    Timeout,
    #[error("the provider refused the key")]
    Unauthorized(Option<String>),
    #[error("the provider is busy")]
    RateLimited,
    #[error("model or address not found")]
    NotFound(Option<String>),
    #[error("the provider reported an error ({status})")]
    Provider { status: u16, detail: Option<String> },
    #[error("the answer could not be read")]
    Unreadable,
    #[error("the command spans several lines")]
    Multiline,
    #[error("the model refused to answer")]
    Refused,
}

/// An error as the page gets it: a code it has words for, and what the
/// provider said, if anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub code: &'static str,
    pub detail: Option<String>,
}

impl AssistError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not-configured",
            Self::NoModel => "no-model",
            Self::NoKey => "no-key",
            Self::Address(_) => "address",
            Self::Request => "request",
            Self::Unreachable => "unreachable",
            Self::Timeout => "timeout",
            Self::Unauthorized(_) => "unauthorized",
            Self::RateLimited => "rate-limited",
            Self::NotFound(_) => "not-found",
            Self::Provider { .. } => "provider",
            Self::Unreadable => "unreadable",
            Self::Multiline => "multiline",
            Self::Refused => "refused",
        }
    }

    pub fn failure(&self) -> Failure {
        let detail = match self {
            Self::Address(reason) => Some((*reason).to_string()),
            Self::Unauthorized(detail) | Self::NotFound(detail) => detail.clone(),
            Self::Provider { status, detail } => Some(match detail {
                Some(detail) => format!("HTTP {status}: {detail}"),
                None => format!("HTTP {status}"),
            }),
            _ => None,
        };
        Failure {
            code: self.code(),
            detail,
        }
    }
}

impl From<AnswerError> for AssistError {
    fn from(error: AnswerError) -> Self {
        match error {
            AnswerError::NoJson => Self::Unreadable,
            AnswerError::Multiline => Self::Multiline,
        }
    }
}

pub type Result<T> = std::result::Result<T, AssistError>;

/// Where and how to reach one provider. The key never shows in `Debug`.
#[derive(Clone)]
pub struct Endpoint {
    pub kind: ProviderKind,
    /// Checked and without a trailing `/`.
    pub base_url: String,
    pub api_key: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .finish()
    }
}

impl Endpoint {
    /// An endpoint from settings: the kind's own address unless the person
    /// may and did pick one, and the key when the kind takes one.
    pub fn new(
        kind: ProviderKind,
        base_url: Option<&str>,
        api_key: Option<Zeroizing<String>>,
    ) -> Result<Self> {
        let typed = base_url.map(str::trim).filter(|url| !url.is_empty());
        let base_url = match (kind.base_url_editable(), typed) {
            (true, Some(url)) => check_base_url(kind, url)?,
            (_, _) => match kind.default_base_url() {
                Some(url) => url.to_string(),
                None => return Err(AssistError::Address("missing")),
            },
        };
        let api_key = api_key.filter(|key| !key.trim().is_empty() && kind.key_allowed());
        if kind.key_required() && api_key.is_none() {
            return Err(AssistError::NoKey);
        }
        Ok(Self {
            kind,
            base_url,
            api_key,
        })
    }

    fn key(&self) -> Option<&str> {
        self.api_key.as_deref().map(|key| key.trim())
    }
}

/// Checks an address the person typed for a server of their own and returns
/// it without a trailing `/` (and, for Ollama, without the `/v1` its
/// OpenAI-compatible side lives under). Plain `http://` only to this machine
/// or the local network; never a login, query or fragment in it.
pub fn check_base_url(kind: ProviderKind, url: &str) -> Result<String> {
    let url = url.trim();
    if url.is_empty() {
        return Err(AssistError::Address("missing"));
    }
    if url.chars().count() > MAX_URL_CHARS {
        return Err(AssistError::Address("too-long"));
    }
    let parsed = url::Url::parse(url).map_err(|_| AssistError::Address("not-a-url"))?;
    let https = match parsed.scheme() {
        "https" => true,
        "http" => false,
        _ => return Err(AssistError::Address("scheme")),
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AssistError::Address("login"));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(AssistError::Address("query"));
    }
    let local = match parsed.host() {
        None => return Err(AssistError::Address("no-host")),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            if name.is_empty() {
                return Err(AssistError::Address("no-host"));
            }
            // A name without a dot, or one of the suffixes only a local
            // network answers to.
            !name.contains('.')
                || [".localhost", ".local", ".lan", ".home.arpa", ".internal"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
        }
        Some(url::Host::Ipv4(ip)) => ip_is_local(IpAddr::V4(ip))?,
        Some(url::Host::Ipv6(ip)) => ip_is_local(IpAddr::V6(ip))?,
    };
    if !https && (!kind.base_url_editable() || !local) {
        return Err(AssistError::Address("insecure"));
    }
    let mut normalized = parsed.to_string();
    while normalized.ends_with('/') {
        normalized.pop();
    }
    if kind == ProviderKind::Ollama {
        if let Some(stripped) = normalized.strip_suffix("/v1") {
            normalized = stripped.to_string();
        }
    }
    Ok(normalized)
}

/// Whether an IP is on this machine or in a private network. Link-local
/// (where cloud metadata lives), multicast and the like are refused.
fn ip_is_local(ip: IpAddr) -> Result<bool> {
    let refused = AssistError::Address("refused-ip");
    match ip {
        IpAddr::V4(ip) => ipv4_is_local(ip).ok_or(refused),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return ipv4_is_local(v4).ok_or(refused);
            }
            let first = ip.segments()[0];
            if ip.is_unspecified() || ip.is_multicast() || (first & 0xffc0) == 0xfe80 {
                return Err(refused);
            }
            Ok(ip == Ipv6Addr::LOCALHOST || (first & 0xfe00) == 0xfc00)
        }
    }
}

fn ipv4_is_local(ip: Ipv4Addr) -> Option<bool> {
    let [a, b, ..] = ip.octets();
    if a == 0 || ip.is_link_local() || ip.is_broadcast() || ip.is_multicast() {
        return None;
    }
    Some(ip.is_loopback() || ip.is_private() || (a == 100 && (b & 0xc0) == 64))
}

/// How long the steps may take.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    /// A cloud model answering.
    pub answer: Duration,
    /// A model on this machine or in the house, which may first have to load.
    pub local_answer: Duration,
    pub models: Duration,
    /// Looking whether Ollama runs here at all.
    pub detect: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            answer: Duration::from_secs(60),
            local_answer: Duration::from_secs(180),
            models: Duration::from_secs(15),
            detect: Duration::from_millis(1500),
        }
    }
}

/// Talks to the providers.
pub struct Client {
    http: Http,
    timeouts: Timeouts,
}

impl Client {
    pub fn new() -> Result<Self> {
        Self::with_timeouts(Timeouts::default())
    }

    pub fn with_timeouts(timeouts: Timeouts) -> Result<Self> {
        let http = Http::builder()
            .use_preconfigured_tls(tls_config())
            // A redirect would carry the key somewhere nobody checked.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeouts.connect)
            .user_agent(concat!("UwUSSH/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                tracing::warn!(%error, "the assistant's HTTP client could not be built");
                AssistError::Unreachable
            })?;
        Ok(Self { http, timeouts })
    }

    fn answer_timeout(&self, kind: ProviderKind) -> Duration {
        if kind.base_url_editable() {
            self.timeouts.local_answer
        } else {
            self.timeouts.answer
        }
    }

    /// One command for `request` on `platform`.
    pub fn generate(
        &self,
        endpoint: &Endpoint,
        model: &str,
        platform: &Platform,
        language: Language,
        request: &str,
    ) -> Result<Answer> {
        let request = request.trim();
        if request.is_empty() || request.chars().count() > prompt::MAX_REQUEST_CHARS {
            return Err(AssistError::Request);
        }
        let model = model.trim();
        if model.is_empty() {
            return Err(AssistError::NoModel);
        }
        let system = prompt::system_prompt(platform, language);
        let user = prompt::user_message(request);
        let text = match endpoint.kind {
            ProviderKind::Ollama => self.ollama_chat(endpoint, model, &system, &user)?,
            ProviderKind::Anthropic => self.messages(endpoint, model, &system, &user)?,
            ProviderKind::Openai | ProviderKind::Mistral | ProviderKind::OpenaiCompatible => {
                self.chat_completions(endpoint, model, &system, &user)?
            }
        };
        Ok(prompt::parse_answer(&text)?)
    }

    /// The models a provider offers, sorted. Doubles as the connection test:
    /// it needs the address and, where there is one, the key to be right.
    pub fn models(&self, endpoint: &Endpoint) -> Result<Vec<String>> {
        let (url, pick): (String, fn(&Value) -> Vec<String>) = match endpoint.kind {
            ProviderKind::Ollama => (format!("{}/api/tags", endpoint.base_url), ollama_models),
            _ => (format!("{}/models", endpoint.base_url), listed_models),
        };
        let request = self
            .http
            .get(url)
            .headers(auth_headers(endpoint)?)
            .timeout(self.timeouts.models);
        let value = self.send_json(request, endpoint.key())?;
        let mut models = pick(&value);
        if endpoint.kind == ProviderKind::Openai {
            models.retain(|model| chat_model(model));
        }
        models.sort();
        models.dedup();
        models.truncate(MAX_MODELS);
        Ok(models)
    }

    /// Whether Ollama answers at `base_url`, and its models if so. Quick and
    /// quiet: no answer is simply `None`.
    pub fn detect_ollama(&self, base_url: &str) -> Option<Vec<String>> {
        let base = check_base_url(ProviderKind::Ollama, base_url).ok()?;
        let request = self
            .http
            .get(format!("{base}/api/tags"))
            .timeout(self.timeouts.detect);
        let value = self.send_json(request, None).ok()?;
        value.get("models")?;
        let mut models = ollama_models(&value);
        models.sort();
        Some(models)
    }

    fn ollama_chat(
        &self,
        endpoint: &Endpoint,
        model: &str,
        system: &str,
        user: &str,
    ) -> Result<String> {
        let body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "stream": false,
            "format": "json",
            "options": {"temperature": 0.1},
        });
        let request = self
            .http
            .post(format!("{}/api/chat", endpoint.base_url))
            .headers(auth_headers(endpoint)?)
            .json(&body)
            .timeout(self.answer_timeout(endpoint.kind));
        let value = self.send_json(request, endpoint.key())?;
        value
            .pointer("/message/content")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(AssistError::Unreadable)
    }

    fn chat_completions(
        &self,
        endpoint: &Endpoint,
        model: &str,
        system: &str,
        user: &str,
    ) -> Result<String> {
        let send = |json_mode: bool| -> Result<Value> {
            let mut body = json!({
                "model": model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
            });
            if json_mode {
                body["response_format"] = json!({"type": "json_object"});
            }
            let request = self
                .http
                .post(format!("{}/chat/completions", endpoint.base_url))
                .headers(auth_headers(endpoint)?)
                .json(&body)
                .timeout(self.answer_timeout(endpoint.kind));
            self.send_json(request, endpoint.key())
        };
        let value = match send(true) {
            // Servers of every kind of build: one that does not know JSON mode
            // refuses the field, and is asked again without it.
            Err(AssistError::Provider { status: 400, .. })
                if endpoint.kind == ProviderKind::OpenaiCompatible =>
            {
                send(false)?
            }
            other => other?,
        };
        let message = value
            .pointer("/choices/0/message")
            .ok_or(AssistError::Unreadable)?;
        if message
            .get("refusal")
            .and_then(Value::as_str)
            .is_some_and(|refusal| !refusal.is_empty())
        {
            return Err(AssistError::Refused);
        }
        match message.get("content") {
            Some(Value::String(text)) => Ok(text.clone()),
            Some(Value::Array(parts)) => Ok(parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")),
            _ => Err(AssistError::Unreadable),
        }
    }

    fn messages(
        &self,
        endpoint: &Endpoint,
        model: &str,
        system: &str,
        user: &str,
    ) -> Result<String> {
        let send = |effort: bool| -> Result<Value> {
            let mut body = json!({
                "model": model,
                "max_tokens": 4096,
                "system": system,
                "messages": [{"role": "user", "content": user}],
            });
            // A short answer needs little thought. Models that do not take
            // an effort refuse the field and are asked again without it.
            if effort {
                body["output_config"] = json!({"effort": "low"});
            }
            let request = self
                .http
                .post(format!("{}/messages", endpoint.base_url))
                .headers(auth_headers(endpoint)?)
                .json(&body)
                .timeout(self.answer_timeout(endpoint.kind));
            self.send_json(request, endpoint.key())
        };
        let value = match send(!model.contains("haiku")) {
            Err(AssistError::Provider { status: 400, .. }) => send(false)?,
            other => other?,
        };
        if value.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
            return Err(AssistError::Refused);
        }
        let text: String = value
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        if text.trim().is_empty() {
            return Err(AssistError::Unreadable);
        }
        Ok(text)
    }

    fn send_json(&self, request: RequestBuilder, key: Option<&str>) -> Result<Value> {
        let response = request.send().map_err(|error| transport_error(&error))?;
        let status = response.status().as_u16();
        let body = read_limited(response)?;
        let value = serde_json::from_slice::<Value>(&body).ok();
        if (200..300).contains(&status) {
            return value.ok_or(AssistError::Unreadable);
        }
        let detail = value
            .as_ref()
            .and_then(|value| detail(value, key))
            .or_else(|| {
                let text = String::from_utf8_lossy(&body);
                Some(clean(&text, key)).filter(|text| !text.is_empty())
            });
        Err(match status {
            401 | 403 => AssistError::Unauthorized(detail),
            404 => AssistError::NotFound(detail),
            408 | 504 => AssistError::Timeout,
            429 | 529 => AssistError::RateLimited,
            _ => AssistError::Provider { status, detail },
        })
    }
}

fn read_limited(response: Response) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut body = Vec::new();
    response
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|_| AssistError::Unreachable)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(AssistError::Unreadable);
    }
    Ok(body)
}

/// The key (marked sensitive, so it never shows in reqwest's own output) and
/// the fixed headers.
fn auth_headers(endpoint: &Endpoint) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    let secret = |value: &str| -> Result<HeaderValue> {
        let mut value = HeaderValue::from_str(value).map_err(|_| AssistError::NoKey)?;
        value.set_sensitive(true);
        Ok(value)
    };
    match (endpoint.kind, endpoint.key()) {
        (ProviderKind::Anthropic, key) => {
            if let Some(key) = key {
                headers.insert(HeaderName::from_static("x-api-key"), secret(key)?);
            }
            headers.insert(
                HeaderName::from_static("anthropic-version"),
                HeaderValue::from_static(ANTHROPIC_VERSION),
            );
        }
        (_, Some(key)) => {
            headers.insert(AUTHORIZATION, secret(&format!("Bearer {key}"))?);
        }
        (_, None) => {}
    }
    Ok(headers)
}

fn transport_error(error: &reqwest::Error) -> AssistError {
    // A connection that never came about is unreachable, also when it ran
    // out of time: Windows retries a refused connect for about two seconds
    // instead of failing at once.
    if error.is_connect() {
        AssistError::Unreachable
    } else if error.is_timeout() {
        AssistError::Timeout
    } else if error.is_builder() {
        AssistError::Address("not-a-url")
    } else {
        AssistError::Unreachable
    }
}

/// At most [`MAX_DETAIL_CHARS`] of `text`, on one line, with the key blanked.
fn clean(text: &str, key: Option<&str>) -> String {
    let text = match key.filter(|key| key.len() >= 4) {
        Some(key) => text.replace(key, "***"),
        None => text.to_string(),
    };
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    match flat.char_indices().nth(MAX_DETAIL_CHARS) {
        Some((cut, _)) => format!("{}…", &flat[..cut]),
        None => flat.to_string(),
    }
}

/// The provider's own words about an error, in the shapes they use.
fn detail(value: &Value, key: Option<&str>) -> Option<String> {
    let text = value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .or_else(|| value.get("error").filter(|error| error.is_string()))
        .or_else(|| value.get("detail"))?
        .as_str()?;
    Some(clean(text, key)).filter(|text| !text.is_empty())
}

fn listed_models(value: &Value) -> Vec<String> {
    value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("id").or_else(|| entry.get("name"))?.as_str())
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty() && id.len() <= 200 && !id.chars().any(char::is_control))
        .collect()
}

fn ollama_models(value: &Value) -> Vec<String> {
    value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("name").or_else(|| entry.get("model"))?.as_str())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty() && name.len() <= 200 && !name.chars().any(char::is_control))
        .collect()
}

/// OpenAI's list has embeddings, speech and images in it too.
fn chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    ![
        "embedding",
        "tts",
        "whisper",
        "dall-e",
        "moderation",
        "audio",
        "realtime",
        "transcribe",
        "image",
        "search",
        "davinci",
        "babbage",
    ]
    .iter()
    .any(|part| id.contains(part))
}

/// rustls with ring, trusting the public roots and the system's: a model
/// server behind a home CA works once that CA is installed.
fn tls_config() -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let system = rustls_native_certs::load_native_certs();
    for certificate in system.certs {
        let _ = roots.add(certificate);
    }
    rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the versions rustls asks for")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::Mutex;

    const KEY: &str = "sk-test-geheim-1234"; // gitleaks:allow

    /// A request as the fake provider saw it.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: Value,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }
    }

    /// A provider on 127.0.0.1 that answers with the replies it was given,
    /// in order, and remembers what it was asked.
    struct Fake {
        base: String,
        seen: Arc<Mutex<Vec<Seen>>>,
        _stop: mpsc::Sender<()>,
    }

    impl Fake {
        fn start(replies: Vec<(u16, String)>) -> Self {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let base = format!("http://{}", server.server_addr().to_ip().unwrap());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let (stop, stopped) = mpsc::channel::<()>();
            let log = seen.clone();
            std::thread::spawn(move || {
                let mut replies = replies.into_iter();
                for mut request in server.incoming_requests() {
                    let mut body = String::new();
                    let _ = request.as_reader().read_to_string(&mut body);
                    log.lock().unwrap().push(Seen {
                        method: request.method().to_string(),
                        url: request.url().to_string(),
                        headers: request
                            .headers()
                            .iter()
                            .map(|h| (h.field.to_string(), h.value.to_string()))
                            .collect(),
                        body: serde_json::from_str(&body).unwrap_or(Value::Null),
                    });
                    let Some((status, reply)) = replies.next() else {
                        // Nothing more to say: hold the request open until
                        // the test is over, which is what a hung server does.
                        let _ = stopped.recv();
                        return;
                    };
                    let response = tiny_http::Response::from_string(reply)
                        .with_status_code(status)
                        .with_header(
                            "Content-Type: application/json"
                                .parse::<tiny_http::Header>()
                                .unwrap(),
                        );
                    let _ = request.respond(response);
                }
            });
            Self {
                base,
                seen,
                _stop: stop,
            }
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    fn client() -> Client {
        Client::with_timeouts(Timeouts {
            connect: Duration::from_secs(2),
            answer: Duration::from_secs(5),
            local_answer: Duration::from_secs(5),
            models: Duration::from_secs(5),
            detect: Duration::from_millis(500),
        })
        .unwrap()
    }

    fn endpoint(kind: ProviderKind, base: &str, key: Option<&str>) -> Endpoint {
        Endpoint {
            kind,
            base_url: base.to_string(),
            api_key: key.map(|key| Zeroizing::new(key.to_string())),
        }
    }

    fn answer_json() -> String {
        json!({"command": "getent passwd", "explanation": "Listet alle Benutzer.", "dangerous": false})
            .to_string()
    }

    fn ubuntu() -> Platform {
        Platform::for_os(Some("ubuntu"))
    }

    fn ask(client: &Client, endpoint: &Endpoint) -> Result<Answer> {
        client.generate(
            endpoint,
            "test-model",
            &ubuntu(),
            Language::De,
            "liste alle benutzer auf",
        )
    }

    #[test]
    fn an_openai_compatible_server_is_asked_for_json_with_the_key_as_bearer() {
        let reply =
            json!({"choices": [{"message": {"role": "assistant", "content": answer_json()}}]});
        let fake = Fake::start(vec![(200, reply.to_string())]);
        let answer = ask(
            &client(),
            &endpoint(
                ProviderKind::OpenaiCompatible,
                &format!("{}/v1", fake.base),
                Some(KEY),
            ),
        )
        .unwrap();
        assert_eq!(answer.command, "getent passwd");

        let seen = &fake.seen()[0];
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.url, "/v1/chat/completions");
        assert_eq!(
            seen.header("authorization"),
            Some(format!("Bearer {KEY}").as_str())
        );
        assert_eq!(seen.body["model"], "test-model");
        assert_eq!(seen.body["response_format"]["type"], "json_object");
        let system = seen.body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("apt") && system.contains("German"));
        assert!(seen.body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("liste alle benutzer auf"));
    }

    #[test]
    fn a_server_that_does_not_know_json_mode_is_asked_again_without_it() {
        let reply = json!({"choices": [{"message": {"content": format!("```json\n{}\n```", answer_json())}}]});
        let fake = Fake::start(vec![
            (
                400,
                json!({"error": {"message": "response_format not supported"}}).to_string(),
            ),
            (200, reply.to_string()),
        ]);
        let answer = ask(
            &client(),
            &endpoint(ProviderKind::OpenaiCompatible, &fake.base, None),
        )
        .unwrap();
        assert_eq!(answer.command, "getent passwd");
        let seen = fake.seen();
        assert_eq!(seen.len(), 2);
        assert!(seen[1].body.get("response_format").is_none());
        assert!(
            seen[0].header("authorization").is_none(),
            "no key, no header"
        );
    }

    #[test]
    fn anthropic_gets_its_headers_and_its_text_blocks_are_read() {
        let reply = json!({
            "content": [
                {"type": "thinking", "thinking": ""},
                {"type": "text", "text": answer_json()},
            ],
            "stop_reason": "end_turn",
        });
        let fake = Fake::start(vec![(200, reply.to_string())]);
        let answer = client()
            .generate(
                &endpoint(ProviderKind::Anthropic, &fake.base, Some(KEY)),
                "claude-sonnet-5-5",
                &ubuntu(),
                Language::En,
                "list users",
            )
            .unwrap();
        assert_eq!(answer.command, "getent passwd");
        let seen = &fake.seen()[0];
        assert_eq!(seen.url, "/messages");
        assert_eq!(seen.header("x-api-key"), Some(KEY));
        assert_eq!(seen.header("anthropic-version"), Some(ANTHROPIC_VERSION));
        assert!(seen.header("authorization").is_none());
        assert_eq!(seen.body["output_config"]["effort"], "low");
        assert!(seen.body["system"].as_str().unwrap().contains("English"));
    }

    #[test]
    fn a_model_without_effort_is_asked_again_and_a_refusal_is_reported() {
        let fake = Fake::start(vec![
            (
                400,
                json!({"error": {"message": "output_config.effort: not supported"}}).to_string(),
            ),
            (
                200,
                json!({"content": [], "stop_reason": "refusal"}).to_string(),
            ),
        ]);
        let result = client().generate(
            &endpoint(ProviderKind::Anthropic, &fake.base, Some(KEY)),
            "claude-sonnet-5-5",
            &ubuntu(),
            Language::De,
            "x",
        );
        assert_eq!(result, Err(AssistError::Refused));
        assert!(fake.seen()[1].body.get("output_config").is_none());

        let haiku = Fake::start(vec![(
            200,
            json!({"content": [{"type": "text", "text": answer_json()}]}).to_string(),
        )]);
        client()
            .generate(
                &endpoint(ProviderKind::Anthropic, &haiku.base, Some(KEY)),
                "claude-haiku-4-5",
                &ubuntu(),
                Language::De,
                "x",
            )
            .unwrap();
        assert!(haiku.seen()[0].body.get("output_config").is_none());
    }

    #[test]
    fn ollama_is_asked_natively_and_found_with_its_models() {
        let tags = json!({"models": [{"name": "qwen3:8b"}, {"name": "llama3.2:3b"}]});
        let chat =
            json!({"message": {"role": "assistant", "content": answer_json()}, "done": true});
        let fake = Fake::start(vec![(200, tags.to_string()), (200, chat.to_string())]);
        let client = client();
        assert_eq!(
            client.detect_ollama(&format!("{}/v1/", fake.base)),
            Some(vec!["llama3.2:3b".to_string(), "qwen3:8b".to_string()])
        );
        let answer = ask(&client, &endpoint(ProviderKind::Ollama, &fake.base, None)).unwrap();
        assert_eq!(answer.command, "getent passwd");
        let seen = fake.seen();
        assert_eq!(seen[0].url, "/api/tags");
        assert_eq!(seen[1].url, "/api/chat");
        assert_eq!(seen[1].body["format"], "json");
        assert_eq!(seen[1].body["stream"], false);
    }

    #[test]
    fn nobody_listening_means_no_ollama_and_unreachable() {
        // A port that was free a moment ago.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let base = format!("http://127.0.0.1:{port}");
        let client = client();
        assert_eq!(client.detect_ollama(&base), None);
        assert_eq!(
            ask(&client, &endpoint(ProviderKind::Ollama, &base, None)),
            Err(AssistError::Unreachable)
        );
    }

    #[test]
    fn model_lists_are_read_sorted_and_without_what_is_no_chat_model() {
        let list = json!({"data": [
            {"id": "gpt-5-mini"}, {"id": "text-embedding-3-small"}, {"id": "gpt-5"},
            {"id": "gpt-5"}, {"id": "whisper-1"},
        ]});
        let fake = Fake::start(vec![(200, list.to_string())]);
        let models = client()
            .models(&endpoint(ProviderKind::Openai, &fake.base, Some(KEY)))
            .unwrap();
        assert_eq!(models, vec!["gpt-5", "gpt-5-mini"]);
        assert_eq!(fake.seen()[0].url, "/models");
        assert_eq!(fake.seen()[0].method, "GET");
    }

    #[test]
    fn statuses_become_errors_and_never_carry_the_key() {
        let echo = json!({"error": {"message": format!("Incorrect API key provided: {KEY}")}});
        let fake = Fake::start(vec![
            (401, echo.to_string()),
            (429, "{}".into()),
            (404, json!({"error": "model 'x' not found"}).to_string()),
            (500, "upstream exploded".into()),
            (200, "not json".into()),
            (
                200,
                json!({"choices": [{"message": {"content": "Sorry, kann ich nicht."}}]})
                    .to_string(),
            ),
        ]);
        let client = client();
        let openai = endpoint(ProviderKind::OpenaiCompatible, &fake.base, Some(KEY));
        let unauthorized = ask(&client, &openai).unwrap_err();
        let failure = unauthorized.failure();
        assert_eq!(failure.code, "unauthorized");
        assert!(
            !failure.detail.clone().unwrap().contains(KEY),
            "{failure:?}"
        );
        assert!(failure.detail.unwrap().contains("***"));
        assert_eq!(ask(&client, &openai), Err(AssistError::RateLimited));
        assert_eq!(
            ask(&client, &openai),
            Err(AssistError::NotFound(Some("model 'x' not found".into())))
        );
        assert_eq!(
            ask(&client, &openai)
                .unwrap_err()
                .failure()
                .detail
                .as_deref(),
            Some("HTTP 500: upstream exploded")
        );
        assert_eq!(ask(&client, &openai), Err(AssistError::Unreadable));
        assert_eq!(ask(&client, &openai), Err(AssistError::Unreadable));
    }

    #[test]
    fn a_provider_that_never_answers_runs_into_the_timeout() {
        let fake = Fake::start(Vec::new());
        let client = Client::with_timeouts(Timeouts {
            answer: Duration::from_millis(300),
            local_answer: Duration::from_millis(300),
            ..Timeouts::default()
        })
        .unwrap();
        assert_eq!(
            ask(
                &client,
                &endpoint(ProviderKind::OpenaiCompatible, &fake.base, None)
            ),
            Err(AssistError::Timeout)
        );
    }

    #[test]
    fn requests_and_settings_that_cannot_work_fail_before_any_request() {
        let client = client();
        let local = endpoint(ProviderKind::Ollama, "http://127.0.0.1:9", None);
        assert_eq!(
            client.generate(&local, "m", &ubuntu(), Language::De, "  "),
            Err(AssistError::Request)
        );
        assert_eq!(
            client.generate(&local, " ", &ubuntu(), Language::De, "x"),
            Err(AssistError::NoModel)
        );
        assert_eq!(
            Endpoint::new(ProviderKind::Anthropic, None, None).unwrap_err(),
            AssistError::NoKey
        );
        assert_eq!(
            Endpoint::new(ProviderKind::OpenaiCompatible, None, None).unwrap_err(),
            AssistError::Address("missing")
        );
        // The cloud providers' addresses are their own, whatever was typed.
        let openai = Endpoint::new(
            ProviderKind::Openai,
            Some("http://192.0.2.1"),
            Some(Zeroizing::new(KEY.into())),
        )
        .unwrap();
        assert_eq!(openai.base_url, "https://api.openai.com/v1");
        assert!(!format!("{openai:?}").contains(KEY));
    }

    #[test]
    fn addresses_are_checked() {
        use ProviderKind::{Ollama, OpenaiCompatible};
        assert_eq!(
            check_base_url(Ollama, "http://localhost:11434/v1/").unwrap(),
            "http://localhost:11434"
        );
        assert_eq!(
            check_base_url(OpenaiCompatible, "http://192.168.1.20:8080/v1").unwrap(),
            "http://192.168.1.20:8080/v1"
        );
        assert!(check_base_url(OpenaiCompatible, "http://llm.lan:1234/v1").is_ok());
        assert!(check_base_url(OpenaiCompatible, "http://gpubox:8000/v1").is_ok());
        assert!(check_base_url(OpenaiCompatible, "https://llm.example.com/v1").is_ok());
        assert_eq!(
            check_base_url(OpenaiCompatible, "http://llm.example.com/v1"),
            Err(AssistError::Address("insecure"))
        );
        assert_eq!(
            check_base_url(OpenaiCompatible, "http://203.0.113.7/v1"),
            Err(AssistError::Address("insecure"))
        );
        assert_eq!(
            check_base_url(OpenaiCompatible, "https://user:pw@llm.example.com"),
            Err(AssistError::Address("login"))
        );
        assert_eq!(
            check_base_url(OpenaiCompatible, "http://169.254.169.254/latest"),
            Err(AssistError::Address("refused-ip"))
        );
        assert_eq!(
            check_base_url(OpenaiCompatible, "ftp://192.0.2.2"),
            Err(AssistError::Address("scheme"))
        );
        assert_eq!(
            check_base_url(OpenaiCompatible, "https://llm.example.com/v1?x=1"),
            Err(AssistError::Address("query"))
        );
    }

    #[test]
    fn provider_names_round_trip() {
        for kind in ProviderKind::ALL {
            assert_eq!(ProviderKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                Value::String(kind.as_str().into())
            );
        }
        assert_eq!(ProviderKind::parse("gemini"), None);
    }
}
