mod mock;

use std::path::Path;
use std::time::Duration;

use aiusg::model::Availability;
use aiusg::provider::{claude, codex, is_signed_out};
use aiusg::store::Credential;
use chrono::{DateTime, TimeZone, Utc};
use mock::{Mock, Reply};

const CLAUDE_USAGE: &str = "/api/oauth/usage";
const CLAUDE_PROFILE: &str = "/api/oauth/profile";
const CODEX_USAGE: &str = "/backend-api/wham/usage";
const CLAUDE_TOKEN: &str = "sk-ant-oat01-zone";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const TIMEOUT: Duration = Duration::from_secs(5);

const CLAUDE_USAGE_BODY: &str = r#"{"limits":[{"kind":"session","percent":62,"resets_at":"2026-09-23T06:10:00Z","scope":null},{"kind":"weekly_all","percent":31,"resets_at":"2026-09-28T04:00:00Z","scope":null}],"extra_usage":{"is_enabled":false}}"#;
const CLAUDE_EXHAUSTED_BODY: &str = r#"{"limits":[{"kind":"session","percent":40,"resets_at":"2026-10-01T05:00:00Z"},{"kind":"weekly_all","percent":100,"resets_at":"2026-10-08T04:00:00Z"}]}"#;
const PROFILE_BODY: &str = r#"{"account":{"uuid":"abc12345-ffff","email":"jake@example.com","display_name":"Jake"},"organization":{"organization_type":"claude_max","billing_type":"stripe_subscription","rate_limit_tier":"default_claude_max_20x"}}"#;
const CODEX_USAGE_BODY: &str = r#"{"plan_type":"pro","rate_limit":{"primary_window":{"used_percent":62,"limit_window_seconds":18000,"reset_at":1790000000},"secondary_window":{"used_percent":31,"limit_window_seconds":604800,"reset_at":1790500000}},"additional_rate_limits":null}"#;
const CODEX_AUTH: &str = r#"{"tokens":{"access_token":"codex-zone","refresh_token":"codex-refresh","account_id":"acct-42"}}"#;
const CLAUDE_CREDENTIALS: &str = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-home","refreshToken":"sk-ant-ort01-home","expiresAt":1790000000000,"rateLimitTier":"default_claude_max_5x"}}"#;

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(TIMEOUT)
        .build()
        .expect("building the HTTP client")
}

fn at(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .expect("a valid timestamp")
        .with_timezone(&Utc)
}

fn home_with(file: &str, contents: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("creating a temp home");
    write(home.path(), file, contents);
    home
}

fn write(home: &Path, file: &str, contents: &str) {
    std::fs::write(home.join(file), contents).expect("writing the credentials file");
}

#[tokio::test]
async fn a_zone_shaped_caller_reads_usage_and_profile_from_a_base_it_names() {
    let mock = Mock::serve([
        (CLAUDE_USAGE, Reply::json(CLAUDE_USAGE_BODY)),
        (CLAUDE_PROFILE, Reply::json(PROFILE_BODY)),
        (CODEX_USAGE, Reply::json(CODEX_USAGE_BODY)),
    ])
    .await;
    let http = http();

    let fetched = claude::fetch_at(&http, mock.base(), &Credential::bearer(CLAUDE_TOKEN))
        .await
        .expect("Claude usage from the named base");

    assert_eq!(
        fetched.plan.as_deref(),
        Some("default_claude_max_20x"),
        "the plan comes from the profile on the same base"
    );
    assert_eq!(fetched.windows.len(), 2);
    assert_eq!(fetched.windows[0].name, "5h");
    assert_eq!(fetched.windows[0].used_percent, Some(62.0));
    assert_eq!(
        fetched.windows[0].resets_at,
        Some(at("2026-09-23T06:10:00Z"))
    );
    assert_eq!(fetched.windows[1].name, "7d");
    assert_eq!(fetched.windows[1].used_percent, Some(31.0));
    assert_eq!(
        fetched.windows[1].resets_at,
        Some(at("2026-09-28T04:00:00Z"))
    );

    for path in [CLAUDE_USAGE, CLAUDE_PROFILE] {
        let request = mock.request(path);
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {CLAUDE_TOKEN}").as_str()),
            "{path} carries the access token"
        );
        assert_eq!(
            request.header("anthropic-beta"),
            Some(OAUTH_BETA),
            "{path} opts into the OAuth beta"
        );
        assert!(
            request
                .header("user-agent")
                .is_some_and(|agent| agent.starts_with("claude-code/")),
            "{path} identifies as Claude Code"
        );
    }

    let profile = claude::profile_at(&http, &format!("{}/", mock.base()), CLAUDE_TOKEN)
        .await
        .expect("a base with a trailing slash still reaches the profile");
    assert_eq!(profile.label(), Some("jake@example.com"));
    assert_eq!(profile.plan(), Some("default_claude_max_20x"));

    let home = home_with("auth.json", CODEX_AUTH);
    let login = codex::discover_in(home.path())
        .expect("reading the login's codex home")
        .remove(0);
    let fetched = codex::fetch_at(&http, mock.base(), &login.credential)
        .await
        .expect("Codex usage from the named base");

    assert_eq!(fetched.plan.as_deref(), Some("pro"));
    assert_eq!(fetched.windows.len(), 2);
    assert_eq!(fetched.windows[0].name, "5h");
    assert_eq!(fetched.windows[0].used_percent, Some(62.0));
    assert_eq!(
        fetched.windows[0].resets_at,
        Utc.timestamp_opt(1_790_000_000, 0).single()
    );
    assert_eq!(fetched.windows[1].name, "7d");
    assert_eq!(
        fetched.windows[1].resets_at,
        Utc.timestamp_opt(1_790_500_000, 0).single()
    );

    let request = mock.request(CODEX_USAGE);
    assert_eq!(request.method, "GET");
    assert_eq!(request.header("authorization"), Some("Bearer codex-zone"));
    assert_eq!(
        request.header("chatgpt-account-id"),
        Some("acct-42"),
        "the account id discovered in the home selects the workspace"
    );
    assert_eq!(request.header("originator"), Some("codex_cli_rs"));
    assert_eq!(request.header("accept"), Some("application/json"));
}

#[tokio::test]
async fn a_refused_usage_token_is_signed_out() {
    let refusing = Mock::serve([
        (CLAUDE_USAGE, Reply::status(401)),
        (CLAUDE_PROFILE, Reply::status(401)),
        (CODEX_USAGE, Reply::status(403)),
    ])
    .await;
    let http = http();
    let credential = Credential::bearer(CLAUDE_TOKEN);

    let refused = claude::fetch_at(&http, refusing.base(), &credential)
        .await
        .expect_err("a 401 from the usage endpoint fails the fetch");
    assert!(is_signed_out(&refused), "{refused:#}");
    let wrapped = refused.context("refreshing usage for login 3f2b9c1e");
    assert!(
        is_signed_out(&wrapped),
        "a caller's context keeps the sign-out visible: {wrapped:#}"
    );

    let refused = claude::profile_at(&http, refusing.base(), CLAUDE_TOKEN)
        .await
        .expect_err("a 401 from the profile endpoint fails the read");
    assert!(is_signed_out(&refused), "{refused:#}");

    let refused = codex::fetch_at(&http, refusing.base(), &Credential::bearer("codex-zone"))
        .await
        .expect_err("a 403 from the usage endpoint fails the fetch");
    assert!(is_signed_out(&refused), "{refused:#}");

    let failing = Mock::serve([(CLAUDE_USAGE, Reply::status(500))]).await;
    let failed = claude::fetch_at(&http, failing.base(), &credential)
        .await
        .expect_err("a 500 fails the fetch");
    assert!(
        !is_signed_out(&failed),
        "a server error is not a sign-out: {failed:#}"
    );
}

#[tokio::test]
async fn a_discovered_home_becomes_usage_with_its_reset() {
    let mock = Mock::serve([(CLAUDE_USAGE, Reply::json(CLAUDE_EXHAUSTED_BODY))]).await;
    let home = home_with(".credentials.json", CLAUDE_CREDENTIALS);

    let login = claude::discover_in(home.path())
        .expect("reading the login's config dir")
        .remove(0);
    let fetched = claude::fetch_at(&http(), mock.base(), &login.credential)
        .await
        .expect("usage for the discovered login");

    assert_eq!(
        mock.request(CLAUDE_USAGE).header("authorization"),
        Some("Bearer sk-ant-oat01-home"),
        "the discovered token is the one sent"
    );
    assert!(
        fetched.plan.is_none(),
        "the profile is unavailable on this base, so no plan came back"
    );

    let usage = fetched.into_usage(&login.account);

    assert_eq!(usage.account, login.account.id);
    assert_eq!(usage.provider, login.account.provider);
    assert_eq!(usage.label, login.account.label);
    assert_eq!(
        usage.plan.as_deref(),
        Some("default_claude_max_5x"),
        "the plan stored at discovery stays"
    );
    assert_eq!(usage.headroom(), 0.0);
    assert_eq!(
        usage.availability(),
        Availability::At(at("2026-10-08T04:00:00Z")),
        "the login is usable again when its spent weekly window resets"
    );
}
