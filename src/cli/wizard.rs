//! First-launch setup wizard.
//!
//! Runs once, before the trust dialog and the main TUI even start, when
//! [`GlobalConfig::setup_completed`] is `false` — a fresh install, or a
//! v0.2 checkout that predates global config entirely. Never runs again
//! after that, whether the user actually configured something or
//! explicitly skipped: `setup_completed` is set either way.
//!
//! Deliberately plain blocking terminal I/O (`prompt_line`/hidden
//! password read), the same as `/connect` — this runs *before* the
//! alternate-screen TUI ever opens, so there's no raw-mode conflict to
//! work around here the way `cli::tui::run_suspended` has to for
//! mid-session commands.

use anyhow::Result;
use colored::Colorize;

use crate::cli::picker;
use crate::config::Config;

/// `Ok(true)` if the wizard ran (or the lightweight migration offer ran);
/// `Ok(false)` if it was skipped outright (non-interactive run, or
/// already set up). Mutates `config.global` in place and, on any path
/// that results in a saved profile, `config.model`/`display_name`/
/// `credential_key` too, so the caller can immediately build a provider
/// from the same `Config` without reloading.
pub async fn maybe_run(config: &mut Config, non_interactive: bool, auto_yes: bool) -> Result<bool> {
    if config.global.setup_completed {
        return Ok(false);
    }
    if non_interactive || auto_yes {
        // Don't nag a scripted/CI run; just remember we saw it so an
        // interactive run later gets the real wizard instead of a
        // migration-offer path that assumes prior interactive use.
        config.global.setup_completed = true;
        let _ = config.global.save();
        return Ok(false);
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        config.global.setup_completed = true;
        let _ = config.global.save();
        return Ok(false);
    }

    // If there's already a working setup via the old per-workspace
    // rexo.toml/.env mechanism, offer the lightweight migration instead
    // of the full wizard — this is the "detect it, migrate it, preserve
    // settings" path.
    if config.model.model.is_some() && config.api_key().is_ok() {
        return offer_migration(config);
    }

    run_full_wizard(config).await
}

fn offer_migration(config: &mut Config) -> Result<bool> {
    println!("{}", "Rexo Code".bold());
    println!();
    println!(
        "Found a working setup already active here (provider: {}, model: {}).",
        config.display_name().bold(),
        config.model.model.as_deref().unwrap_or("?").bold()
    );
    println!("Save it as your global default, so it's available from any workspace too?");
    let answer = prompt("Save globally? [Y/n] ")?;
    if answer.trim().eq_ignore_ascii_case("n") {
        println!("{}", "Not saved globally — this workspace's own settings still apply here.".dimmed());
        config.global.setup_completed = true;
        config.global.save()?;
        return Ok(true);
    }

    let (name, profile) = migrated_profile(config);
    config.global.providers.insert(name.clone(), profile.clone());
    config.global.default_provider = Some(name);
    config.global.setup_completed = true;
    config.global.save()?;
    config.display_name = Some(profile.display_name.clone());
    // Use the same credential key the profile itself resolves to (see
    // `migrated_profile`'s doc comment) — not a new "migrated" name —
    // so this session's credential resolution doesn't change either.
    config.credential_key = profile.credential_key.clone();

    println!("{} Saved as your global default.", "✓".green());
    println!(
        "{}",
        "(the key itself wasn't copied anywhere new — it's still resolved from the \
         environment/.env exactly as before; run /api set --permanent later if you'd \
         like REXO to hold onto it directly)"
            .dimmed()
    );
    Ok(true)
}

/// Build the global profile the migration offer saves, and the map name
/// it saves under. Pulled out as a pure function (no I/O — mirrors the
/// `persistable_model_profile`/`persist_model_choice` split from the
/// v0.4.1 picker fix) specifically so the credential-key logic below is
/// unit-testable without mocking blocking stdin reads.
///
/// **This is the root-caused fix for the wizard migration bug flagged in
/// v0.4.1 and left open through v0.5.0.** The original code always wrote
/// `credential_key: None` into the new profile and then set
/// `config.credential_key = Some("migrated")` directly — but `"migrated"`
/// is just this new profile's own map name, not the key the *currently
/// working* setup actually resolves its credential under (which, per
/// `Config::api_key`, could be an unnamespaced legacy var like
/// `NVIDIA_API_KEY`, a namespaced `REXO_<KEY>_API_KEY` under the
/// *previous* key name, or a credential-store entry saved under that
/// previous name). Renaming the effective key to `"migrated"` mid-flight
/// silently orphaned whichever of those the person was actually relying
/// on — `REXO_MIGRATED_API_KEY` never existed and nothing told them to
/// set it. The fix: capture `config.effective_credential_key()` — what's
/// *already* resolving successfully, which is the precondition for this
/// function being called at all (see `maybe_run`) — and carry that
/// forward explicitly as the new profile's `credential_key`, so
/// resolution keeps working exactly as it did the moment before
/// migration, just now also reachable from other workspaces.
fn migrated_profile(config: &Config) -> (String, crate::config::global::ProviderProfile) {
    let name = "migrated".to_string();
    let working_credential_key = config.effective_credential_key();
    let profile = crate::config::global::ProviderProfile {
        display_name: config.display_name(),
        kind: config.model.provider.clone(),
        base_url: config.model.base_url.clone(),
        model: config.model.model.clone(),
        temperature: Some(config.model.temperature),
        max_tokens: Some(config.model.max_tokens),
        reasoning_effort: config.model.reasoning_effort.clone(),
        credential_key: Some(working_credential_key),
        requires_key: config.model.provider != "local",
    };
    (name, profile)
}

async fn run_full_wizard(config: &mut Config) -> Result<bool> {
    // Runs before any `TuiCore` exists — see `cli::picker`'s module docs
    // on exactly this: open a short-lived terminal/event stream, reuse
    // the same picker functions `/connect` uses inside the real session,
    // tear both down before returning. This replaces the old
    // "type a number from a printed list" + a plain `read_line` for the
    // API key with real arrow-key selection and masked raw-mode input —
    // deferred since v0.4.0, done here.
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;
    // See trust_dialog.rs's matching comment: alt-screen buffers aren't
    // guaranteed blank on entry, and this wizard is (as of v0.7) the
    // *first* of potentially several alt-screen sessions in a row
    // (wizard → trust dialog → main TUI), so it's the one most likely to
    // leave stale content for whichever one runs next if it doesn't
    // clear on its own way in.
    terminal.clear()?;
    let mut events = crossterm::event::EventStream::new();
    let accent = ratatui::style::Color::Cyan;

    let outcome = run_full_wizard_inner(config, &mut terminal, &mut events, accent).await;

    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

    match outcome {
        Ok(ran) => {
            if ran {
                println!("{}", "Rexo Code is ready.".bold());
            }
            Ok(ran)
        }
        Err(e) => {
            // Whatever went wrong, don't leave setup_completed unset — a
            // half-finished wizard shouldn't nag on every future launch;
            // /connect is always there to finish the job.
            config.global.setup_completed = true;
            let _ = config.global.save();
            println!("{} Setup wizard hit an error ({e}) — run /connect once you're in.", "✗".red());
            Ok(true)
        }
    }
}

async fn run_full_wizard_inner(
    config: &mut Config,
    terminal: &mut ratatui::Terminal<picker::Backend>,
    events: &mut crossterm::event::EventStream,
    accent: ratatui::style::Color,
) -> Result<bool> {
    let mut items: Vec<picker::PickerItem> = crate::providers::catalog::search("")
        .into_iter()
        .map(|p| {
            let sub = if p.notes.is_empty() { (if p.requires_key { "requires a key" } else { "no key needed" }).to_string() } else { p.notes.to_string() };
            picker::PickerItem::with_sub(p.display_name, sub, p.key)
        })
        .collect();
    items.push(picker::PickerItem::with_sub("Custom endpoint…", "any OpenAI-compatible API you point at yourself", "__custom__"));
    let item_count = items.len();

    let provider_outcome = picker::run_select(
        terminal,
        events,
        picker::SelectOptions {
            title: "Welcome to Rexo Code — connect a provider".to_string(),
            items,
            accent,
            allow_custom: false,
            subtitle: Some(format!("{item_count} providers — type to filter, Esc to skip setup for now", item_count = item_count)),
            search_placeholder: "search providers…",
        },
    )
    .await?;

    let key = match provider_outcome {
        picker::PickerOutcome::Selected(key) => key,
        _ => {
            let _ = terminal.clear();
            println!("{}", "Skipped — run /connect any time to configure a provider.".dimmed());
            config.global.setup_completed = true;
            config.global.save()?;
            return Ok(true);
        }
    };
    let _ = terminal.clear();

    let (kind, display_name, requires_key, fixed_base_url) = if key == "__custom__" {
        let Some(name) = picker::run_text_input(terminal, events, "Custom endpoint", "Display name:", Some("Custom"), false, accent).await? else {
            config.global.setup_completed = true;
            config.global.save()?;
            return Ok(true);
        };
        let kind = ask_wire_format(terminal, events, accent).await?;
        (kind, name, true, None)
    } else {
        let preset = crate::providers::catalog::find(&key).expect("picker only offers catalog keys");
        // The catalog's own `kind` is the source of truth here — this
        // used to re-derive it via `if preset.key == "nvidia" {...} else
        // if ... else { "openai_compatible" }`, which silently fell
        // through to `openai_compatible` for any preset the chain didn't
        // explicitly name. Gemini's preset (added after that chain was
        // written) was actually hitting that fallback: picking Gemini
        // from *this* first-launch wizard mis-routed it through the
        // OpenAI-compatible client — wrong auth header, wrong request
        // shape, guaranteed 401 — even though `/connect` (which already
        // read `preset.kind` directly) had no such bug. Reading the
        // field directly means a new native-driver preset can't skew
        // out of sync with this match again.
        (preset.kind.to_string(), preset.display_name.to_string(), preset.requires_key, preset.base_url)
    };
    let preset_key = if key == "__custom__" { None } else { Some(key.clone()) };

    let base_url = match fixed_base_url {
        Some(url) if kind == "nvidia" => Some(url.to_string()),
        Some(url) => picker::run_text_input(terminal, events, &display_name, "Base URL:", Some(url), false, accent).await?,
        None if kind == "local" => None,
        None => {
            let Some(url) = picker::run_text_input(terminal, events, &display_name, "Base URL:", None, false, accent).await? else {
                config.global.setup_completed = true;
                config.global.save()?;
                return Ok(true);
            };
            Some(url)
        }
    };
    let _ = terminal.clear();

    let api_key = if requires_key {
        let label = format!("API key for {display_name}:");
        picker::run_text_input(terminal, events, &display_name, &label, None, true, accent).await?.filter(|s| !s.is_empty())
    } else {
        None
    };
    let _ = terminal.clear();

    let model_default_hint = match kind.as_str() {
        "nvidia" => Some("moonshotai/kimi-k3"),
        "local" => Some("llama3"),
        "gemini" => Some("gemini-flash-latest"),
        _ => None, // no safe universal default for an arbitrary custom/openai_compatible endpoint
    };
    let model = loop {
        let entered = picker::run_text_input(
            terminal,
            events,
            &display_name,
            "Model (Enter to accept the suggestion, or type your own):",
            model_default_hint,
            false,
            accent,
        )
        .await?;
        let Some(entered) = entered else {
            // Esc — same "skip the rest of setup" behavior as every
            // other step, not specific to this one.
            config.global.setup_completed = true;
            config.global.save()?;
            return Ok(true);
        };
        let chosen = if entered.trim().is_empty() { model_default_hint.map(str::to_string) } else { Some(entered.trim().to_string()) };
        // REXO can't actually start the interactive session without a
        // model — see main.rs's startup, which needs one to build a
        // working provider before the TUI (and therefore /model) is
        // reachable at all. A truly blank model here would silently
        // produce a config that fails on the *next* launch, so this is
        // the one step in the wizard that doesn't accept empty: loop
        // back with an explanation instead.
        match chosen {
            Some(m) => break m,
            None => {
                let _ = terminal.clear();
                picker::run_confirm(terminal, events, &display_name, "A model is required to start REXO — press Enter to try again", true, accent).await?;
                let _ = terminal.clear();
            }
        }
    };
    let model = Some(model);
    let _ = terminal.clear();

    let permanent = picker::run_confirm(terminal, events, &display_name, "Save this permanently?", true, accent).await?;
    let _ = terminal.clear();

    let map_key = preset_key.unwrap_or_else(|| "custom".to_string());
    let mut profile = crate::config::global::ProviderProfile {
        display_name: display_name.clone(),
        kind,
        base_url,
        model,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        credential_key: None,
        requires_key,
    };

    if permanent {
        let credential_key = profile.resolved_credential_key(&map_key).to_string();
        if let Some(key) = &api_key {
            let store = crate::config::credentials::CredentialStore::open()?;
            store.set(&credential_key, key)?;
        }
        profile.credential_key = Some(credential_key.clone());
        config.global.providers.insert(map_key.clone(), profile.clone());
        config.global.default_provider = Some(map_key);
        config.model.provider = profile.kind.clone();
        config.model.model = profile.model.clone();
        config.model.base_url = profile.base_url.clone();
        config.display_name = Some(profile.display_name.clone());
        config.credential_key = Some(credential_key);
    } else {
        config.model.provider = profile.kind.clone();
        config.model.model = profile.model.clone();
        config.model.base_url = profile.base_url.clone();
        config.display_name = Some(profile.display_name.clone());
        config.credential_key = Some(map_key);
    }
    config.global.setup_completed = true;
    config.global.save()?;

    println!();
    println!("{} Configuration saved.", "✓".green());
    if !permanent {
        println!("{}", "  (session only — this won't be here next time; run /connect to save it for good)".dimmed());
    }
    Ok(true)
}

/// The same "which wire format does this endpoint actually speak"
/// question `/connect`'s custom-endpoint flow asks — factored out so the
/// first-launch wizard's own custom-endpoint path asks it too, instead
/// of silently assuming OpenAI-compatible the way both paths used to.
pub(crate) async fn ask_wire_format(terminal: &mut ratatui::Terminal<picker::Backend>, events: &mut crossterm::event::EventStream, accent: ratatui::style::Color) -> Result<String> {
    let items = vec![
        picker::PickerItem::with_sub("OpenAI-compatible", "the /chat/completions shape most endpoints speak", "openai_compatible"),
        picker::PickerItem::with_sub("Anthropic-native", "Messages API shape (x-api-key, content blocks)", "anthropic"),
        picker::PickerItem::with_sub("Google Gemini-native", "generateContent shape (x-goog-api-key)", "gemini"),
    ];
    let outcome = picker::run_select(
        terminal,
        events,
        picker::SelectOptions {
            title: "Wire format".to_string(),
            items,
            accent,
            allow_custom: false,
            subtitle: Some("Which API shape does this endpoint actually speak?".to_string()),
            search_placeholder: "",
        },
    )
    .await?;
    Ok(match outcome {
        picker::PickerOutcome::Selected(k) => k,
        _ => "openai_compatible".to_string(),
    })
}

fn prompt(label: &str) -> Result<String> {
    use std::io::Write;
    print!("{label}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::global::with_temp_global_dir;

    /// `maybe_run` is `async` (the full wizard now drives real picker
    /// I/O), but `with_temp_global_dir`'s closure is plain `FnOnce() -> T`
    /// — shared by every test in the crate that touches global config, so
    /// it doesn't need an async-aware signature just for this module.
    /// A tiny throwaway runtime bridges the two; these tests never hit
    /// `run_full_wizard` anyway (every case here returns before the
    /// picker terminal ever opens), so there's no actual raw-mode/TTY
    /// concern — just a plain `Future` to drive to completion.
    fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Runtime::new().unwrap().block_on(fut)
    }

    #[test]
    fn non_interactive_skips_without_prompting_and_marks_completed() {
        with_temp_global_dir("wizard_noninteractive", || {
            let dir = std::env::temp_dir().join("rexo_test_wizard_noninteractive");
            std::fs::create_dir_all(&dir).unwrap();
            let mut config = Config::load(&dir).unwrap();
            assert!(!config.global.setup_completed);
            let ran = block_on(maybe_run(&mut config, true, false)).unwrap();
            assert!(!ran);
            assert!(config.global.setup_completed);
        });
    }

    #[test]
    fn already_completed_short_circuits() {
        with_temp_global_dir("wizard_already_done", || {
            let dir = std::env::temp_dir().join("rexo_test_wizard_already_done");
            std::fs::create_dir_all(&dir).unwrap();
            let mut config = Config::load(&dir).unwrap();
            config.global.setup_completed = true;
            let ran = block_on(maybe_run(&mut config, false, false)).unwrap();
            assert!(!ran);
        });
    }

    #[test]
    fn auto_yes_skips_like_non_interactive() {
        with_temp_global_dir("wizard_auto_yes", || {
            let dir = std::env::temp_dir().join("rexo_test_wizard_auto_yes");
            std::fs::create_dir_all(&dir).unwrap();
            let mut config = Config::load(&dir).unwrap();
            let ran = block_on(maybe_run(&mut config, false, true)).unwrap();
            assert!(!ran);
            assert!(config.global.setup_completed);
        });
    }

    // ---- Regression tests for the wizard migration credential-rename bug ----
    // (flagged in v0.4.1, left open through v0.5.0, root-caused and fixed here
    // — see `migrated_profile`'s doc comment for the full story.)

    #[test]
    fn migration_preserves_the_legacy_unnamespaced_env_var_key() {
        with_temp_global_dir("wizard_migration_legacy_env", || {
            let dir = std::env::temp_dir().join("rexo_test_wizard_migration_legacy_env");
            std::fs::create_dir_all(&dir).unwrap();
            let mut config = Config::load(&dir).unwrap();
            config.model.provider = "nvidia".to_string();
            config.model.model = Some("some-model".to_string());
            config.credential_key = None; // resolving purely via the legacy NVIDIA_API_KEY path

            let (name, profile) = migrated_profile(&config);

            assert_eq!(name, "migrated");
            // The bug: this used to always be `None` (which then made
            // `effective_credential_key()` fall back to the new profile's
            // own map name, "migrated", instead of the provider kind
            // "nvidia" that `REXO_NVIDIA_API_KEY`/`NVIDIA_API_KEY`/the
            // credential store were actually keyed on).
            assert_eq!(profile.credential_key.as_deref(), Some("nvidia"));
            assert_ne!(profile.credential_key.as_deref(), Some("migrated"));
        });
    }

    #[test]
    fn migration_preserves_an_existing_named_profiles_credential_key() {
        with_temp_global_dir("wizard_migration_named_profile", || {
            let dir = std::env::temp_dir().join("rexo_test_wizard_migration_named_profile");
            std::fs::create_dir_all(&dir).unwrap();
            let mut config = Config::load(&dir).unwrap();
            config.model.provider = "openai_compatible".to_string();
            config.model.model = Some("some-model".to_string());
            // Simulates a config that already resolved through a named
            // profile/credential-store entry before migration ran.
            config.credential_key = Some("work-openrouter".to_string());

            let (_name, profile) = migrated_profile(&config);

            assert_eq!(profile.credential_key.as_deref(), Some("work-openrouter"));
        });
    }

    #[test]
    fn migrated_profile_credential_key_matches_what_effective_credential_key_already_resolves_to() {
        with_temp_global_dir("wizard_migration_matches_effective", || {
            let dir = std::env::temp_dir().join("rexo_test_wizard_migration_matches_effective");
            std::fs::create_dir_all(&dir).unwrap();
            let mut config = Config::load(&dir).unwrap();
            config.model.provider = "nvidia".to_string();
            config.model.model = Some("some-model".to_string());

            // Whatever `effective_credential_key()` resolves to *before*
            // migration must be exactly what the migrated profile carries
            // forward — that's the whole fix, stated as an invariant.
            let before = config.effective_credential_key();
            let (_name, profile) = migrated_profile(&config);
            assert_eq!(profile.credential_key.as_deref(), Some(before.as_str()));
        });
    }
}
