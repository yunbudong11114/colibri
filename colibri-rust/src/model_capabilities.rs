use crate::config::ModelConfig;

#[derive(Clone, Copy, Debug)]
pub struct ModelCapabilities {
    pub context_window: usize,
    pub max_output_tokens: usize,
    pub source: &'static str,
}

impl ModelCapabilities {
    pub fn compaction_threshold(self) -> usize {
        let remaining = self.context_window.saturating_sub(self.max_output_tokens);
        let headroom = (self.context_window / 20).clamp(1, 8192);
        (self.context_window.saturating_mul(4) / 5)
            .min(remaining.saturating_sub(headroom))
            .max(1)
    }
}

pub fn resolve(config: &ModelConfig) -> ModelCapabilities {
    let url = url::Url::parse(&config.base_url).ok();
    let host = url.as_ref().and_then(|url| url.host_str()).unwrap_or("");
    let known = if config.provider == "openai_compatible" {
        // Exact official origins and model IDs only. A proxy may apply different limits.
        match (host, config.model.as_str()) {
            ("api.deepseek.com", "deepseek-flash" | "deepseek-v4-pro") => {
                Some((1_048_576, 393_216))
            }
            (
                "api.openai.com",
                "gpt-4.1"
                | "gpt-4.1-mini"
                | "gpt-4.1-nano"
                | "gpt-4.1-2025-04-14"
                | "gpt-4.1-mini-2025-04-14"
                | "gpt-4.1-nano-2025-04-14",
            ) => Some((1_047_576, 32_768)),
            (
                "api.openai.com",
                "gpt-4o"
                | "gpt-4o-mini"
                | "gpt-4o-2024-08-06"
                | "gpt-4o-2024-11-20"
                | "gpt-4o-mini-2024-07-18",
            ) => Some((128_000, 16_384)),
            ("api.openai.com", "gpt-5" | "gpt-5-2025-08-07" | "gpt-5.2" | "gpt-5.2-2025-12-11") => {
                Some((400_000, 128_000))
            }
            ("api.openai.com", "gpt-5.5" | "gpt-5.5-2026-04-23") => Some((1_050_000, 128_000)),
            _ => None,
        }
    } else {
        None
    };
    apply(
        config,
        known.map(|(window, output)| (Some(window), Some(output))),
        if known.is_some() {
            "catalog"
        } else {
            "fallback"
        },
    )
}

pub(crate) fn apply(
    config: &ModelConfig,
    advertised: Option<(Option<usize>, Option<usize>)>,
    source: &'static str,
) -> ModelCapabilities {
    let known_window = advertised.and_then(|(window, _)| window);
    let (context_window, source) = if config.input_context_tokens > 0 {
        (
            known_window.map_or(config.input_context_tokens, |window| {
                config.input_context_tokens.min(window)
            }),
            "override",
        )
    } else {
        (known_window.unwrap_or(48_000), source)
    };
    let output_limit = advertised.and_then(|(_, output)| output).unwrap_or(16_384);
    ModelCapabilities {
        context_window,
        max_output_tokens: config
            .max_output_tokens
            .min(output_limit)
            .min((context_window / 2).max(1))
            .max(1),
        source,
    }
}
