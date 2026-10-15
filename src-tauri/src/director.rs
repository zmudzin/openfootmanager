//! One morning call to LM Studio, then the day carries on either way.
//!
//! The server is `http://127.0.0.1:1234/v1` unless `OFM_DIRECTOR_URL` says
//! otherwise. The model is `google/gemma-4-e4b` unless `OFM_DIRECTOR_MODEL` names
//! another key from `lms ls`. `OFM_DIRECTOR=0` turns the call off. A server
//! that is not there is remembered for the rest of the process, so a month of
//! days does not each wait on a refused connection. Skipping ahead does not
//! call the model at all: only a single day the player advances does.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use log::{info, warn};
use ofm_core::director::{
    apply_director_choices, choice_schema, choice_user_prompt, parse_director_choices,
    parse_director_voice, CHOICE_SYSTEM,
};
use ofm_core::state::StateManager;

const DEFAULT_URL: &str = "http://127.0.0.1:1234/v1";
const DEFAULT_MODEL: &str = "google/gemma-4-e4b";
static OFFLINE: AtomicBool = AtomicBool::new(false);

pub fn consult(state: &StateManager) {
    if !enabled() {
        return;
    }
    let Some(slate) = state
        .get_game(|game| ofm_core::director_voice::morning_slate(game, &game_language()))
        .flatten()
    else {
        return;
    };
    let Ok(reply) = complete(CHOICE_SYSTEM, &choice_user_prompt(&slate), choice_schema()) else {
        return;
    };
    let choices = parse_director_choices(&reply);
    let voice = parse_director_voice(&reply);
    info!(
        "[director] {} chose {} move(s) for {}",
        model_name(),
        choices.len(),
        slate.date
    );
    let _ = state.update_game(|game| {
        if game_date(game) != slate.date {
            return;
        }
        let accepted = apply_director_choices(game, &slate, &choices);
        info!("[director] {} bid(s) accepted", accepted.len());
        ofm_core::director_voice::remember_voice(game, &slate, &voice);
    });
}

fn game_date(game: &ofm_core::game::Game) -> String {
    game.clock.current_date.format("%Y-%m-%d").to_string()
}

fn game_language() -> String {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        });
    let Some(path) = base.map(|dir| dir.join("com.sturdyrobot.openfootmanager/settings.json")) else {
        return "en".to_string();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return "en".to_string();
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("language")?.as_str().map(str::to_string))
        .filter(|code| !code.is_empty())
        .unwrap_or_else(|| "en".to_string())
}

fn enabled() -> bool {
    if OFFLINE.load(Ordering::Relaxed) {
        return false;
    }
    match std::env::var("OFM_DIRECTOR") {
        Ok(value) if is_off(&value) => false,
        Ok(value) if is_on(&value) => true,
        _ => !cfg!(test),
    }
}

fn is_off(value: &str) -> bool {
    value == "0" || value.eq_ignore_ascii_case("off")
}

fn is_on(value: &str) -> bool {
    value == "1" || value.eq_ignore_ascii_case("on")
}

fn model_name() -> String {
    std::env::var("OFM_DIRECTOR_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

fn base_url() -> String {
    std::env::var("OFM_DIRECTOR_URL").unwrap_or_else(|_| DEFAULT_URL.to_string())
}

fn timeout() -> Duration {
    let seconds = std::env::var("OFM_DIRECTOR_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30);
    Duration::from_secs(seconds)
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_millis(500)))
            .timeout_global(Some(timeout()))
            .http_status_as_error(false)
            .proxy(None)
            .user_agent(format!("OpenFootManager/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    })
}

fn complete(system: &str, user: &str, schema: serde_json::Value) -> Result<String, ()> {
    match post(system, user, Some(schema)) {
        Ok(text) => Ok(text),
        Err(failure) if failure.status == Some(400) => post(system, user, None).map_err(|retry| {
            warn!("[director] LM Studio rejected the call: {}", retry.detail);
            ()
        }),
        Err(failure) => {
            if failure.unreachable {
                if !OFFLINE.swap(true, Ordering::Relaxed) {
                    warn!(
                        "[director] LM Studio is not reachable ({}). The market heuristic continues. Start it with `lms server start` and load {}.",
                        failure.detail,
                        model_name()
                    );
                }
            } else {
                warn!("[director] LM Studio call failed: {}", failure.detail);
            }
            Err(())
        }
    }
}

struct CallFailure {
    status: Option<u16>,
    unreachable: bool,
    detail: String,
}

fn post(
    system: &str,
    user: &str,
    schema: Option<serde_json::Value>,
) -> Result<String, CallFailure> {
    let url = format!("{}/chat/completions", base_url().trim_end_matches('/'));
    let mut body = serde_json::json!({
        "model": model_name(),
        "temperature": 0.2,
        "max_tokens": 700,
        // Qwen 3.5 spends the whole token budget on reasoning and returns an
        // empty content field unless this is set. The desk reply is the JSON.
        "reasoning_effort": "none",
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    });
    if let Some(schema) = schema {
        body["response_format"] = serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "director",
                "strict": true,
                "schema": schema
            }
        });
    }
    let mut response = agent()
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send(serde_json::to_vec(&body).unwrap_or_default())
        .map_err(|error| CallFailure {
            status: None,
            unreachable: is_unreachable(&error),
            detail: error.to_string(),
        })?;
    let status = response.status().as_u16();
    let bytes = response
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_vec()
        .map_err(|error| CallFailure {
            status: Some(status),
            unreachable: false,
            detail: error.to_string(),
        })?;
    if !(200..300).contains(&status) {
        let detail = String::from_utf8_lossy(&bytes).chars().take(300).collect();
        return Err(CallFailure {
            status: Some(status),
            unreachable: false,
            detail,
        });
    }
    let parsed: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| CallFailure {
            status: Some(status),
            unreachable: false,
            detail: error.to_string(),
        })?;
    let content_value = &parsed["choices"][0]["message"]["content"];
    let content = match content_value {
        serde_json::Value::Null => {
            return Err(CallFailure {
                status: Some(status),
                unreachable: false,
                detail: "reply had no message content".to_string(),
            });
        }
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Ok(content)
}

fn is_unreachable(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::HostNotFound
        | ureq::Error::ConnectionFailed
        | ureq::Error::BadUri(_)
        | ureq::Error::Timeout(ureq::Timeout::Resolve | ureq::Timeout::Connect) => true,
        ureq::Error::Io(io) => matches!(
            io.kind(),
            std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::AddrNotAvailable
                | std::io::ErrorKind::NetworkUnreachable
        ),
        _ => false,
    }
}
