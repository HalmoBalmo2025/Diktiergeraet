//! Anfrage an ein lokales Modell über die OpenAI-kompatible Schnittstelle
//! von `llama-server` (`/v1/chat/completions`).

use serde::{Deserialize, Serialize};

use std::time::Duration;

use super::PolishError;
use crate::language::Language;

/// Anfrage an `/v1/chat/completions`.
#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    /// llama-server bedient nur ein Modell; der Name ist beliebig.
    model: &'static str,
    temperature: u8,
    max_tokens: usize,
    stream: bool,
    messages: [ChatMessage<'a>; 2],
}

/// Eine Nachricht der Anfrage.
#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: Role,
    content: &'a str,
}

/// Wer spricht.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    /// Die Anweisung.
    System,
    /// Der zu korrigierende Text.
    User,
}

/// Antwort von `/v1/chat/completions` (nur, was gebraucht wird).
#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

/// Eine Antwort des Modells.
#[derive(Debug, Deserialize)]
struct Choice {
    message: AnswerMessage,
    finish_reason: Option<FinishReason>,
}

/// Der Text der Antwort.
#[derive(Debug, Deserialize)]
struct AnswerMessage {
    #[serde(default)]
    content: String,
}

/// Warum das Modell aufgehört hat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum FinishReason {
    /// Längengrenze (`max_tokens`) erreicht.
    Length,
    /// Fertig oder ein anderer Grund.
    #[serde(other)]
    Other,
}

/// Fehlerantwort von llama-server (`{"error": {"message": …}}`).
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

/// Der Kern einer Fehlerantwort.
#[derive(Debug, Deserialize)]
struct ErrorDetail {
    message: String,
}

/// Über die OpenAI-kompatible Schnittstelle unter `url` (bis `/v1`), für
/// einen Text in `language`; höchstens `timeout` lang.
pub(super) fn ask(
    text: &str,
    instructions: &str,
    url: &str,
    language: Language,
    timeout: Duration,
) -> Result<String, PolishError> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        // Eine Ablehnung trägt die Begründung im Körper; die soll der
        // Nutzer lesen, nicht nur „http status: 400“.
        .http_status_as_error(false)
        .build()
        .new_agent();
    let request = ChatRequest {
        model: "local",
        temperature: 0,
        max_tokens: answer_limit(text, language),
        stream: false,
        messages: [
            ChatMessage {
                role: Role::System,
                content: instructions,
            },
            ChatMessage {
                role: Role::User,
                content: text,
            },
        ],
    };
    let mut response = agent
        .post(format!("{}/chat/completions", url.trim_end_matches('/')))
        .send_json(&request)
        .map_err(|error| PolishError::Request(Box::new(error)))?;
    let status = response.status();
    if !status.is_success() {
        let message = response
            .body_mut()
            .read_json::<ErrorResponse>()
            .ok()
            .map(|body| body.error.message);
        return Err(PolishError::Server {
            status: status.as_u16(),
            message,
        });
    }
    let reply: ChatResponse = response
        .body_mut()
        .read_json()
        .map_err(|error| PolishError::Response(Box::new(error)))?;
    let Some(choice) = reply.choices.into_iter().next() else {
        return Err(PolishError::Empty);
    };
    if choice.finish_reason == Some(FinishReason::Length) {
        return Err(PolishError::TooLong);
    }
    let answer = without_thinking(&choice.message.content).trim();
    if answer.is_empty() {
        return Err(PolishError::Empty);
    }
    Ok(answer.to_owned())
}

/// Höchstzahl Tokens für die Antwort auf `text` in `language`. Eine
/// Korrektur oder Übersetzung ist etwa so lang wie der Text (deutsch rund
/// ein Token je 3–4 Zeichen); das Doppelte reicht. Chinesisch und
/// Japanisch brauchen etwa ein Token je Zeichen; dort das Anderthalbfache
/// (so entschieden). Ohne Grenze kann ein Modell in eine Wiederholung
/// geraten und schreibt bis zur Zeitgrenze weiter (beobachtet mit Gemma 4
/// 12B).
fn answer_limit(text: &str, language: Language) -> usize {
    let chars = text.chars().count();
    let tokens = if language.without_spaces() {
        chars.saturating_mul(3) / 2
    } else {
        chars / 2
    };
    tokens.saturating_add(64)
}

/// Entfernt einen Denkabschnitt (`<think>…</think>`), den manche Modelle
/// vor die Antwort stellen.
fn without_thinking(content: &str) -> &str {
    content
        .rsplit_once("</think>")
        .map_or(content, |(_, answer)| answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_thinking() {
        assert_eq!(without_thinking("<think>hm</think>\nFertig."), "\nFertig.");
        assert_eq!(without_thinking("Fertig."), "Fertig.");
    }

    #[test]
    fn writes_chat_request() {
        let request = ChatRequest {
            model: "local",
            temperature: 0,
            max_tokens: 70,
            stream: false,
            messages: [
                ChatMessage {
                    role: Role::System,
                    content: "Anweisung",
                },
                ChatMessage {
                    role: Role::User,
                    content: "Text",
                },
            ],
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            concat!(
                r#"{"model":"local","temperature":0,"max_tokens":70,"stream":false,"#,
                r#""messages":[{"role":"system","content":"Anweisung"},"#,
                r#"{"role":"user","content":"Text"}]}"#,
            )
        );
    }

    #[test]
    fn reads_chat_response() {
        let parse = |json: &str| serde_json::from_str::<ChatResponse>(json).unwrap();
        let finished = parse(
            r#"{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"Fertig."},"finish_reason":"stop"}]}"#,
        );
        let choice = &finished.choices[0];
        assert_eq!(choice.message.content, "Fertig.");
        assert_eq!(choice.finish_reason, Some(FinishReason::Other));
        let cut = parse(r#"{"choices":[{"message":{},"finish_reason":"length"}]}"#);
        assert_eq!(cut.choices[0].finish_reason, Some(FinishReason::Length));
        assert_eq!(cut.choices[0].message.content, "");
    }

    #[test]
    fn unreachable_server_is_a_request_error() {
        let error = ask(
            "Text",
            "x",
            "http://127.0.0.1:9/v1",
            Language::German,
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(matches!(error, PolishError::Request(_)), "{error:?}");
    }

    #[test]
    fn the_answer_limit_grows_with_the_script() {
        assert_eq!(answer_limit(&"a".repeat(100), Language::German), 50 + 64);
        assert_eq!(answer_limit(&"字".repeat(100), Language::Chinese), 150 + 64);
        assert_eq!(
            answer_limit(&"字".repeat(100), Language::Japanese),
            150 + 64
        );
    }

    #[test]
    fn a_refusing_server_keeps_its_reason() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::Builder::new()
            .spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let _read = stream.read(&mut request).unwrap();
                let body = r#"{"error":{"code":400,"message":"the request exceeds the available context size"}}"#;
                write!(
                    stream,
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            })
            .unwrap();
        let error = ask(
            "Text",
            "x",
            &format!("http://127.0.0.1:{port}/v1"),
            Language::German,
            Duration::from_secs(5),
        )
        .unwrap_err();
        server.join().unwrap();
        assert!(
            error
                .to_string()
                .ends_with("(HTTP 400): the request exceeds the available context size"),
            "Grund des Servers fehlt: {error}"
        );
    }
}
