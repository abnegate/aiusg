use anyhow::Result;
use chrono::{DateTime, Utc};
use rmcp::ErrorData as McpError;
use rmcp::ServerHandler;
use rmcp::ServiceExt;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::model::ContentBlock;
use rmcp::model::Implementation;
use rmcp::model::ServerCapabilities;
use rmcp::model::ServerInfo;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;
use rmcp::transport::stdio;
use serde::Deserialize;
use serde::Serialize;

use crate::app;
use crate::model::{Provider, Report, Usage};
use crate::store::Store;

pub async fn serve() -> Result<()> {
    let running = Server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}

#[derive(Clone)]
struct Server;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ProviderFilter {
    /// Only include accounts for this provider: claude, codex, gemini, copilot, grok, grokbot, cursor
    provider: Option<String>,
}

#[tool_router]
impl Server {
    #[tool(
        description = "Live usage for every stored account: windows with used percent, counts, and reset times. Optionally filtered to one provider."
    )]
    async fn usage(
        &self,
        Parameters(filter): Parameters<ProviderFilter>,
    ) -> Result<CallToolResult, McpError> {
        let reports = reports(parse(filter.provider)?).await?;
        json(&reports)
    }

    #[tool(
        description = "Pick the account with the most remaining usage, so a request can be routed to it. Returns the chosen account, ranked alternatives, and accounts that are unavailable (exhausted, signed out, or failing) with their reset times. Optionally scoped to one provider."
    )]
    async fn route(
        &self,
        Parameters(filter): Parameters<ProviderFilter>,
    ) -> Result<CallToolResult, McpError> {
        let reports = reports(parse(filter.provider)?).await?;
        let routing = routing(&reports);
        if routing.chosen.is_none() {
            let body = serde_json::to_string_pretty(&routing)
                .map_err(|error| McpError::internal_error(error.to_string(), None))?;
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "no account has remaining usage\n{body}"
            ))]));
        }
        json(&routing)
    }

    #[tool(description = "List stored accounts without fetching usage.")]
    async fn accounts(&self) -> Result<CallToolResult, McpError> {
        let store =
            Store::open().map_err(|error| McpError::internal_error(format!("{error:#}"), None))?;
        let accounts = store
            .accounts()
            .map_err(|error| McpError::internal_error(format!("{error:#}"), None))?;
        json(&accounts)
    }
}

#[tool_handler]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("aiusg", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Reads usage limits across all stored AI provider accounts (Claude, Codex, \
                 Gemini, Copilot, Grok, Cursor). Use `route` to pick the account with the most \
                 remaining usage before sending work to a provider; use `usage` for the full \
                 picture.",
            )
    }
}

fn parse(provider: Option<String>) -> Result<Option<Provider>, McpError> {
    provider
        .map(|value| value.parse())
        .transpose()
        .map_err(|error| McpError::invalid_params(format!("{error}"), None))
}

async fn reports(provider: Option<Provider>) -> Result<Vec<Report>, McpError> {
    let store =
        Store::open().map_err(|error| McpError::internal_error(format!("{error:#}"), None))?;
    app::collect(&store, provider)
        .await
        .map_err(|error| McpError::internal_error(format!("{error:#}"), None))
}

fn json(value: &impl Serialize) -> Result<CallToolResult, McpError> {
    let body = serde_json::to_string_pretty(value)
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(body)]))
}

#[derive(Debug, Serialize)]
struct Routing {
    chosen: Option<Candidate>,
    alternatives: Vec<Candidate>,
    unavailable: Vec<Unavailable>,
}

#[derive(Clone, Debug, Serialize)]
struct Candidate {
    account: String,
    provider: Provider,
    label: String,
    plan: Option<String>,
    headroom_percent: f64,
    limiting_window: Option<String>,
    resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
struct Unavailable {
    account: String,
    provider: Provider,
    label: String,
    reason: String,
    resets_at: Option<DateTime<Utc>>,
}

fn routing(reports: &[Report]) -> Routing {
    let mut candidates = Vec::new();
    let mut unavailable = Vec::new();

    for report in reports {
        match report {
            Report::Ok(usage) => {
                let candidate = candidate(usage);
                if candidate.headroom_percent > 0.0 {
                    candidates.push(candidate);
                } else {
                    unavailable.push(Unavailable {
                        account: candidate.account,
                        provider: candidate.provider,
                        label: candidate.label,
                        reason: "exhausted".to_owned(),
                        resets_at: usage.usable_at(),
                    });
                }
            }
            Report::SignedOut {
                account,
                provider,
                label,
            } => unavailable.push(Unavailable {
                account: account.as_str().to_owned(),
                provider: *provider,
                label: label.clone(),
                reason: "signed_out".to_owned(),
                resets_at: None,
            }),
            Report::Failed {
                account,
                provider,
                label,
                message,
            } => unavailable.push(Unavailable {
                account: account.as_str().to_owned(),
                provider: *provider,
                label: label.clone(),
                reason: format!("failed: {message}"),
                resets_at: None,
            }),
        }
    }

    candidates.sort_by(|left, right| {
        right
            .headroom_percent
            .total_cmp(&left.headroom_percent)
            .then_with(|| left.account.cmp(&right.account))
    });

    let mut ranked = candidates.into_iter();
    Routing {
        chosen: ranked.next(),
        alternatives: ranked.collect(),
        unavailable,
    }
}

fn candidate(usage: &Usage) -> Candidate {
    let limiting = usage.limiting_window();

    Candidate {
        account: usage.account.as_str().to_owned(),
        provider: usage.provider,
        label: usage.label.clone(),
        plan: usage.plan.clone(),
        headroom_percent: usage.headroom(),
        limiting_window: limiting.map(|window| window.name.clone()),
        resets_at: limiting.and_then(|window| window.resets_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountId, Window};

    fn usage(label: &str, windows: Vec<Window>) -> Report {
        Report::Ok(Usage {
            account: AccountId::new(Provider::Claude, label),
            provider: Provider::Claude,
            label: label.to_owned(),
            plan: None,
            windows,
            fetched_at: Utc::now(),
        })
    }

    #[test]
    fn routes_to_the_account_with_the_most_headroom() {
        let reports = vec![
            usage(
                "busy@example.com",
                vec![
                    Window::from_percent("Session", 20.0),
                    Window::from_percent("Weekly", 80.0),
                ],
            ),
            usage(
                "idle@example.com",
                vec![Window::from_percent("Weekly", 10.0)],
            ),
        ];

        let routing = routing(&reports);
        let chosen = routing.chosen.expect("an account should be chosen");
        assert_eq!(chosen.account, "claude:idle@example.com");
        assert_eq!(chosen.headroom_percent, 90.0);
        assert_eq!(routing.alternatives.len(), 1);
        assert_eq!(
            routing.alternatives[0].limiting_window.as_deref(),
            Some("Weekly")
        );
    }

    #[test]
    fn exhausted_accounts_are_unavailable() {
        let reports = vec![usage(
            "spent@example.com",
            vec![
                Window::from_percent("Session", 40.0),
                Window::from_percent("Weekly", 100.0),
            ],
        )];

        let routing = routing(&reports);
        assert!(routing.chosen.is_none());
        assert_eq!(routing.unavailable[0].reason, "exhausted");
    }

    #[test]
    fn an_exhausted_account_reports_when_every_spent_window_is_back() {
        let now = Utc::now();
        let reports = vec![usage(
            "spent@example.com",
            vec![
                Window::from_percent("Session", 150.0)
                    .resetting_at(Some(now + chrono::Duration::hours(1))),
                Window::from_percent("Weekly", 100.0)
                    .resetting_at(Some(now + chrono::Duration::days(3))),
            ],
        )];

        let routing = routing(&reports);
        let waiting = &routing.unavailable[0];
        assert_eq!(waiting.reason, "exhausted");
        assert_eq!(
            waiting.resets_at,
            Some(now + chrono::Duration::days(3)),
            "the caller waits on this, and the account is not back until every spent window is"
        );
    }

    #[test]
    fn signed_out_and_failed_accounts_are_unavailable() {
        let id = AccountId::new(Provider::Codex, "gone@example.com");
        let reports = vec![
            Report::SignedOut {
                account: id.clone(),
                provider: Provider::Codex,
                label: "gone@example.com".to_owned(),
            },
            Report::Failed {
                account: AccountId::new(Provider::Grok, "broken@example.com"),
                provider: Provider::Grok,
                label: "broken@example.com".to_owned(),
                message: "timeout".to_owned(),
            },
        ];

        let routing = routing(&reports);
        assert!(routing.chosen.is_none());
        assert_eq!(routing.unavailable[0].reason, "signed_out");
        assert_eq!(routing.unavailable[1].reason, "failed: timeout");
    }

    #[test]
    fn accounts_without_percentages_have_full_headroom() {
        let routing = routing(&[usage("fresh@example.com", vec![])]);
        let chosen = routing.chosen.expect("an account should be chosen");
        assert_eq!(chosen.headroom_percent, 100.0);
        assert!(chosen.limiting_window.is_none());
    }
}
