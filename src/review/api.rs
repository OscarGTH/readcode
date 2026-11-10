//! BYOK: grade with an OpenAI- or Anthropic-compatible endpoint using the
//! user's own key. Nothing about the key or the model is baked in — the shape
//! of the request is the only thing this module knows.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::json;

use super::{Annotation, Grade, Reviewer, contract, parse_json_object, truncate};
use crate::snippet::{Prompt, Snippet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// `POST {base}/chat/completions` with a bearer token.
    OpenAi,
    /// `POST {base}/v1/messages` with `x-api-key`.
    Anthropic,
}

#[derive(Debug, Clone)]
pub struct ApiReviewer {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub style: Style,
    pub timeout: Duration,
}

impl ApiReviewer {
    /// `READCODE_API_KEY` / `OPENAI_API_KEY` / `ANTHROPIC_API_KEY`, plus optional
    /// `READCODE_API_BASE`, `READCODE_MODEL`, `READCODE_API_STYLE`.
    pub fn from_env() -> Option<Self> {
        let anthropic_only = first_env(&["READCODE_API_KEY", "OPENAI_API_KEY"]).is_none();
        let api_key = first_env(&["READCODE_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"])?;

        let style = match first_env(&["READCODE_API_STYLE"]).as_deref() {
            Some("anthropic") | Some("claude") => Style::Anthropic,
            Some("openai") | Some("openai-compatible") => Style::OpenAi,
            Some(other) => {
                eprintln!(
                    "readcode: unknown READCODE_API_STYLE `{other}`, using {}",
                    if anthropic_only {
                        "anthropic"
                    } else {
                        "openai"
                    }
                );
                if anthropic_only {
                    Style::Anthropic
                } else {
                    Style::OpenAi
                }
            }
            None => {
                if anthropic_only {
                    Style::Anthropic
                } else {
                    Style::OpenAi
                }
            }
        };

        let base_url = first_env(&["READCODE_API_BASE"]).unwrap_or_else(|| match style {
            Style::OpenAi => "https://api.openai.com/v1".to_string(),
            Style::Anthropic => "https://api.anthropic.com".to_string(),
        });

        let model = first_env(&["READCODE_MODEL"]).unwrap_or_else(|| match style {
            Style::OpenAi => "gpt-4o-mini".to_string(),
            Style::Anthropic => "claude-sonnet-4-5".to_string(),
        });

        Some(Self {
            base_url,
            model,
            api_key,
            style,
            timeout: Duration::from_secs(120),
        })
    }

    fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        match self.style {
            Style::OpenAi => format!("{base}/chat/completions"),
            Style::Anthropic => {
                let base = base.strip_suffix("/v1").unwrap_or(base);
                format!("{base}/v1/messages")
            }
        }
    }

    fn body(&self, system: &str, user: &str) -> serde_json::Value {
        match self.style {
            Style::OpenAi => json!({
                "model": self.model,
                "temperature": 0,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
            }),
            Style::Anthropic => json!({
                "model": self.model,
                "max_tokens": 2048,
                "temperature": 0,
                "system": system,
                "messages": [{"role": "user", "content": user}],
            }),
        }
    }

    fn complete(&self, system: &str, user: &str) -> Result<String> {
        let request = match self.style {
            Style::OpenAi => ureq::post(&self.endpoint())
                .set("authorization", &format!("Bearer {}", self.api_key)),
            Style::Anthropic => ureq::post(&self.endpoint())
                .set("x-api-key", &self.api_key)
                .set("anthropic-version", "2023-06-01"),
        };

        let response = request
            .set("content-type", "application/json")
            .timeout(self.timeout)
            .send_json(self.body(system, user));

        let text = match response {
            Ok(response) => response
                .into_string()
                .context("could not read the reviewer's response body")?,
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_string().unwrap_or_default();
                bail!(
                    "the reviewer endpoint returned {code}: {}",
                    truncate(body.trim(), 400)
                );
            }
            Err(error) => bail!("cannot reach {} ({}): {error}", self.endpoint(), self.model),
        };

        let value: serde_json::Value = serde_json::from_str(&text).with_context(|| {
            format!(
                "the reviewer endpoint returned invalid JSON: {}",
                truncate(&text, 300)
            )
        })?;

        let content = match self.style {
            Style::OpenAi => value["choices"][0]["message"]["content"].as_str(),
            Style::Anthropic => value["content"][0]["text"].as_str(),
        };
        let content = content.with_context(|| {
            format!(
                "the reviewer endpoint returned no text content: {}",
                truncate(&text, 300)
            )
        })?;
        Ok(content.to_string())
    }
}

impl Reviewer for ApiReviewer {
    fn name(&self) -> String {
        format!(
            "api:{}:{}",
            match self.style {
                Style::OpenAi => "openai",
                Style::Anthropic => "anthropic",
            },
            self.model
        )
    }

    fn grade(
        &self,
        snippet: &Snippet,
        prompts: &[Prompt],
        annotations: &[Annotation],
    ) -> Result<Grade> {
        let task = contract::task(snippet, prompts, annotations);
        let text = self.complete(contract::CONTRACT, &task)?;
        let raw = parse_json_object(&text)?;
        Grade::from_raw(prompts, raw, &self.name())
    }

    fn ask(&self, task: &str) -> Result<String> {
        self.complete(contract::CONTRACT, task)
    }
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| std::env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reviewer(style: Style, base: &str) -> ApiReviewer {
        ApiReviewer {
            base_url: base.to_string(),
            model: "m".into(),
            api_key: "k".into(),
            style,
            timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn openai_endpoint_is_built_from_the_base() {
        assert_eq!(
            reviewer(Style::OpenAi, "https://api.openai.com/v1/").endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn anthropic_endpoint_does_not_double_the_version_segment() {
        assert_eq!(
            reviewer(Style::Anthropic, "https://api.anthropic.com").endpoint(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            reviewer(Style::Anthropic, "https://api.anthropic.com/v1").endpoint(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn anthropic_body_puts_the_system_prompt_outside_the_messages() {
        let body = reviewer(Style::Anthropic, "https://x").body("contract", "task");
        assert_eq!(body["system"], "contract");
        assert_eq!(body["messages"][0]["content"], "task");
        assert!(body.get("max_tokens").is_some());
    }

    #[test]
    fn openai_body_uses_role_messages() {
        let body = reviewer(Style::OpenAi, "https://x").body("contract", "task");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "task");
        assert_eq!(body["temperature"], 0);
    }
}
