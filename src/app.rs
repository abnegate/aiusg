use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::cli::StatusArgs;
use crate::model::{Account, AccountId, Provider, Report, Usage};
use crate::provider;
use crate::render;
use crate::store::{Credential, Store};

const REFRESH_MARGIN: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("building the HTTP client")
}

pub async fn collect(store: &Store, provider: Option<Provider>) -> Result<Vec<Report>> {
    let accounts = store.accounts_for(provider)?;
    let http = http()?;

    let reports = futures::future::join_all(
        accounts
            .iter()
            .map(|account| report_for(store, &http, account)),
    )
    .await;

    Ok(reports)
}

async fn report_for(store: &Store, http: &reqwest::Client, account: &Account) -> Report {
    match usage_for(store, http, account).await {
        Ok(usage) => Report::Ok(usage),
        Err(error) if error.downcast_ref::<provider::SignedOut>().is_some() => Report::SignedOut {
            account: account.id.clone(),
            provider: account.provider,
            label: account.label.clone(),
        },
        Err(error) => Report::Failed {
            account: account.id.clone(),
            provider: account.provider,
            label: account.label.clone(),
            message: format!("{error:#}"),
        },
    }
}

async fn usage_for(store: &Store, http: &reqwest::Client, account: &Account) -> Result<Usage> {
    let credential = ensure_fresh(store, http, account).await?;
    let fetched = provider::fetch(account.provider, http, &credential).await?;

    Ok(Usage {
        account: account.id.clone(),
        provider: account.provider,
        label: account.label.clone(),
        plan: fetched.plan.or_else(|| account.plan.clone()),
        windows: fetched.windows,
        fetched_at: Utc::now(),
    })
}

async fn ensure_fresh(
    store: &Store,
    http: &reqwest::Client,
    account: &Account,
) -> Result<Credential> {
    let credential = store.credential_async(&account.id).await?;
    let stale = credential.expires_at.is_some_and(|at| {
        at <= Utc::now() + chrono::Duration::from_std(REFRESH_MARGIN).unwrap_or_default()
    });

    if !stale {
        return Ok(credential);
    }

    match provider::refresh(account.provider, http, &credential).await {
        Ok(Some(refreshed)) => {
            store
                .write_credential_async(&account.id, &refreshed)
                .await?;
            Ok(refreshed)
        }
        Ok(None) => Ok(credential),
        Err(_) if credential.is_expired() => Err(provider::SignedOut(format!(
            "the stored token expired and could not be refreshed for {}",
            account.id
        ))
        .into()),
        Err(_) => Ok(credential),
    }
}

pub async fn status(args: StatusArgs) -> Result<()> {
    let store = Store::open()?;
    if store.accounts()?.is_empty() {
        render::table::empty();
        return Ok(());
    }

    if let Some(interval) = args.watch {
        if args.json {
            bail!("a live dashboard cannot emit JSON; drop `--json` or drop `--watch`");
        }
        return render::watch::run(interval, &store, &args).await;
    }

    let reports = collect(&store, args.provider).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        render::table::print(&reports, args.all);
    }
    Ok(())
}

pub async fn login(provider: Provider) -> Result<()> {
    let store = Store::open()?;
    let http = http()?;

    println!("Signing in to {}...", provider.display());
    let discovered = provider::login(provider, &http).await?;
    store.save(&discovered.account, &discovered.credential)?;
    println!("  Added {}", discovered.account.id);
    Ok(())
}

pub async fn import(provider: Option<Provider>) -> Result<()> {
    let store = Store::open()?;
    let targets = match provider {
        Some(provider) => vec![provider],
        None => Provider::ALL.to_vec(),
    };

    let existing = store.accounts()?;
    let mut added = 0_usize;

    for provider in targets {
        let discovered = match provider::discover(provider) {
            Ok(discovered) => discovered,
            Err(error) => {
                println!("  {} — skipped ({error:#})", provider.display());
                continue;
            }
        };

        for candidate in discovered {
            if existing
                .iter()
                .any(|account| account.id == candidate.account.id)
            {
                println!("  {} — already stored", candidate.account.id);
                continue;
            }
            store.save(&candidate.account, &candidate.credential)?;
            println!("  {} — imported", candidate.account.id);
            added += 1;
        }
    }

    if added == 0 {
        println!("\nNothing new to import. Use `aiusg login <provider>` to add an account.");
    }
    Ok(())
}

pub async fn list() -> Result<()> {
    let store = Store::open()?;
    let accounts = store.accounts()?;
    if accounts.is_empty() {
        render::table::empty();
        return Ok(());
    }
    for account in accounts {
        let plan = account.plan.unwrap_or_else(|| "-".to_owned());
        println!(
            "{:<10} {:<34} {}",
            account.provider.slug(),
            account.label,
            plan
        );
    }
    Ok(())
}

pub async fn remove(account: &str) -> Result<()> {
    let store = Store::open()?;
    let id = AccountId::from_display(account)
        .with_context(|| format!("'{account}' is not a valid account id"))?;
    if store.remove(&id)? {
        println!("Removed {id}");
        Ok(())
    } else {
        bail!("no stored account matches '{account}'")
    }
}
