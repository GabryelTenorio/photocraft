//! HTTPS POST for File › Send to Telegram. The bot token is in the URL; it is stripped from
//! any error returned to the shell.

use std::time::Duration;

/// Post a `sendDocument` body. Rejects any URL that is not Telegram's bot method, so this
/// cannot be reused as a general request helper.
pub fn post_send_document(url: &str, content_type: &str, body: &[u8]) -> Result<String, String> {
    let token = bot_token(url).ok_or_else(|| "Telegram address was rejected".to_string())?;
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build();
    match agent.post(url).set("Content-Type", content_type).send_bytes(body) {
        Ok(response) => {
            let status = response.status();
            let text = response.into_string().unwrap_or_default();
            interpret(status, &text, token)
        }
        Err(ureq::Error::Status(status, response)) => {
            let text = response.into_string().unwrap_or_default();
            interpret(status, &text, token)
        }
        Err(error) => Err(redact(&error.to_string(), token)),
    }
}

/// GET `getUpdates`. Same host check as [`post_send_document`].
pub fn get(url: &str) -> Result<String, String> {
    let token = bot_token(url).ok_or_else(|| "Telegram address was rejected".to_string())?;
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
    match agent.get(url).call() {
        Ok(response) => {
            let status = response.status();
            let text = response.into_string().unwrap_or_default();
            interpret(status, &text, token)
        }
        Err(ureq::Error::Status(status, response)) => {
            let text = response.into_string().unwrap_or_default();
            interpret(status, &text, token)
        }
        Err(error) => Err(redact(&error.to_string(), token)),
    }
}

fn bot_token(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://api.telegram.org/bot")?;
    let (token, method) = rest.split_once('/')?;
    if matches!(method, "sendDocument" | "getUpdates") && !token.is_empty() { Some(token) } else { None }
}

fn interpret(status: u16, text: &str, token: &str) -> Result<String, String> {
    if (200..300).contains(&status) {
        let ok = serde_json::from_str::<serde_json::Value>(text).ok().and_then(|v| v.get("ok").and_then(|b| b.as_bool()));
        if ok != Some(false) {
            return Ok(text.to_string());
        }
    }
    let description = serde_json::from_str::<serde_json::Value>(text).ok().and_then(|v| v.get("description").and_then(|d| d.as_str()).map(str::to_string));
    Err(redact(&description.unwrap_or_else(|| format!("HTTP {status}")), token))
}

fn redact(text: &str, token: &str) -> String {
    if token.len() < 8 { text.to_string() } else { text.replace(token, "…") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_send_document_url_is_accepted() {
        assert!(bot_token("https://api.telegram.org/bot123:abc/sendDocument").is_some());
        assert!(bot_token("https://api.telegram.org/bot123:abc/getUpdates").is_some());
        assert!(bot_token("https://example.com/bot123:abc/sendDocument").is_none());
        assert!(bot_token("https://api.telegram.org/bot123:abc/getMe").is_none());
    }

    #[test]
    fn errors_drop_the_token() {
        let token = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
        assert_eq!(redact(&format!("refused https://api.telegram.org/bot{token}/sendDocument"), token), "refused https://api.telegram.org/bot…/sendDocument");
    }
}
