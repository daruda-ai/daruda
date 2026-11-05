use super::{Result, TransportError};
use crate::remote_channel::cards::EditFailure;
use serde_json::Value;
use std::{io::Read, time::Duration};
use ureq::Body;
use ureq::http::Response;

pub const MAX_BODY_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const ACK_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RETRIES: usize = 3;
const MAX_RETRY_DELAY: f64 = 60.0;

/// A request that got no usable answer, keeping the HTTP status apart so a
/// caller can tell a refusal from a fault.
#[derive(Debug)]
pub enum Failure {
    /// The platform answered with this error status; 429 once retries ran out.
    Status(u16),
    /// No response, or one that could not be read or decoded.
    Transport(TransportError),
}

impl From<Failure> for TransportError {
    fn from(failure: Failure) -> Self {
        match failure {
            // The budget is spent. Reported as such rather than as a bare
            // "HTTP 429", which reads as a single rejection.
            Failure::Status(429) => Self::Message("Remote API retry limit reached".into()),
            Failure::Status(status) => Self::Message(format!("Remote API returned HTTP {status}")),
            Failure::Transport(error) => error,
        }
    }
}

impl From<TransportError> for Failure {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

/// Classify a failed message edit by what its status says about a retry:
/// rate limits, timeouts and server errors may pass; any other refusal names a
/// message, conversation or credential that is gone, or a request that will
/// stay malformed.
pub(crate) fn edit_failure(failure: Failure) -> EditFailure {
    let retryable = match failure {
        Failure::Status(status) => matches!(status, 408 | 429 | 500..),
        Failure::Transport(_) => true,
    };
    let reason = TransportError::from(failure).to_string();
    if retryable {
        EditFailure::Transient(reason)
    } else {
        EditFailure::Gone(reason)
    }
}

pub fn request(
    method: &str,
    url: &str,
    authorization: Option<&str>,
    payload: &Value,
) -> Result<Value> {
    Ok(request_with_status(method, url, authorization, payload)?)
}

/// [`request`], with an error status kept as [`Failure::Status`].
pub fn request_with_status(
    method: &str,
    url: &str,
    authorization: Option<&str>,
    payload: &Value,
) -> std::result::Result<Value, Failure> {
    call(
        method,
        url,
        authorization,
        payload,
        REQUEST_TIMEOUT,
        MAX_RETRIES,
    )
}

pub fn acknowledge(url: &str, payload: &Value) -> Result<Value> {
    Ok(call("POST", url, None, payload, ACK_TIMEOUT, 0)?)
}

fn call(
    method: &str,
    url: &str,
    authorization: Option<&str>,
    payload: &Value,
    timeout: Duration,
    retries: usize,
) -> std::result::Result<Value, Failure> {
    if cfg!(test) {
        return Err(Failure::Transport(TransportError::Message(
            "Network calls are disabled in tests".into(),
        )));
    }
    // Statuses come back as responses so a 429 can read its retry hint.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into();
    let mut attempt = 0;
    loop {
        let response = match send(&agent, method, url, authorization, payload) {
            Ok(response) => response,
            // URLs can contain live Socket Mode tickets or interaction tokens.
            Err(_) => {
                return Err(Failure::Transport(TransportError::Message(
                    "Remote API connection failed".into(),
                )));
            }
        };
        match response.status().as_u16() {
            429 if attempt < retries => {
                let header = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|s| s.parse::<f64>().ok());
                let body = read(response).unwrap_or(Value::Null);
                let seconds = header
                    .or_else(|| body["retry_after"].as_f64())
                    .unwrap_or(1.0);
                if !seconds.is_finite() || !(0.0..=MAX_RETRY_DELAY).contains(&seconds) {
                    return Err(Failure::Transport(TransportError::Message(
                        "Remote API requested a long retry delay".into(),
                    )));
                }
                std::thread::sleep(Duration::from_secs_f64(seconds.max(0.1)));
                attempt += 1;
                continue;
            }
            status @ 400.. => return Err(Failure::Status(status)),
            _ => {}
        }
        if response.status() == 204 {
            return Ok(Value::Null);
        }
        return Ok(read(response)?);
    }
}

/// `GET` carries no body; every other method sends `payload` as JSON.
fn send(
    agent: &ureq::Agent,
    method: &str,
    url: &str,
    authorization: Option<&str>,
    payload: &Value,
) -> std::result::Result<Response<Body>, ureq::Error> {
    let mut request = ureq::http::Request::builder().method(method).uri(url);
    if let Some(auth) = authorization {
        request = request.header("Authorization", auth);
    }
    if method == "GET" {
        return agent.run(request.body(())?);
    }
    let body = serde_json::to_vec(payload).map_err(std::io::Error::other)?;
    agent.run(
        request
            .header("Content-Type", "application/json; charset=utf-8")
            .body(body)?,
    )
}

fn read(response: Response<Body>) -> Result<Value> {
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take((MAX_BODY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| TransportError::Message("Remote API response could not be read".into()))?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(TransportError::Message(
            "Remote API response exceeded the size limit".into(),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| TransportError::Message("Remote API returned invalid JSON".into()))
}

pub fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| TransportError::Message(format!("Remote API response is missing {key}")))
}

#[cfg(test)]
mod tests {
    #[test]
    fn invalid_required_fields_do_not_expose_payloads() {
        let result = super::required(&serde_json::json!({"token": "secret"}), "url").unwrap_err();
        assert!(!result.to_string().contains("secret"));
    }

    #[test]
    fn a_status_failure_reads_as_before_for_callers_that_do_not_classify() {
        use super::{Failure, TransportError};
        let shown = |failure: Failure| TransportError::from(failure).to_string();
        assert_eq!(shown(Failure::Status(404)), "Remote API returned HTTP 404");
        assert_eq!(
            shown(Failure::Status(429)),
            "Remote API retry limit reached"
        );
        assert_eq!(
            shown(Failure::Transport(TransportError::Message(
                "Remote API connection failed".into()
            ))),
            "Remote API connection failed"
        );
    }
}
