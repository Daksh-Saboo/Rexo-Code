//! Installable plugins — a bundle of skills, custom commands, and hooks
//! under one name, the same idea `search_plugins`/`suggest_plugin_install`
//! cover for Claude's own catalog, but local and self-authored: drop a
//! folder at `<workspace>/.rexo/plugins/<name>/` with a `plugin.toml`
//! manifest plus any of `skills/`, `commands/`, `hooks.toml`, and
//! `/plugin enable <name>` makes all of it live.
//!
//! Deliberately reuses the existing skill (`cli::skills`), custom-command
//! (`cli::custom_commands`), and hook (`crate::hooks`) systems rather
//! than teaching each of them a second discovery path: enabling a plugin
//! *copies* its `skills/<n>/SKILL.md` files into `.rexo/skills/`, its
//! `commands/<n>.md` files into `.rexo/commands/`, and merges its
//! `hooks.toml` entries into `.rexo/hooks.toml`. Every plugin records
//! exactly what it installed (`.rexo/plugins/<name>/.installed.toml`),
//! so `/plugin disable` can remove precisely those files/hooks again —
//! not "everything in `.rexo/skills/`", which might include things the
//! user wrote by hand.
//!
//! What this doesn't do: no sandboxing (a plugin's hooks run exactly
//! like any other configured hook — real shell commands, same posture as
//! `/hooks` itself), no version resolution or dependency graph between
//! plugins, no remote install (a plugin is a local directory the user
//! put there themselves, same as a skill or a custom command).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
struct PluginManifest {
    #[serde(default)]
    description: String,
    #[serde(default)]
    version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct InstalledRecord {
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    commands: Vec<String>,
    /// The exact `HookDef`s this plugin contributed, so disable can
    /// remove precisely those entries from `.rexo/hooks.toml` (matched
    /// by equality) without touching hooks the user added by hand.
    #[serde(default)]
    hooks: Vec<crate::hooks::HookDef>,
}

#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub description: String,
    pub version: String,
    pub enabled: bool,
}

fn plugins_dir(workspace: &Path) -> PathBuf {
    workspace.join(".rexo").join("plugins")
}

fn enabled_list_path(workspace: &Path) -> PathBuf {
    plugins_dir(workspace).join(".enabled.toml")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct EnabledFile {
    #[serde(default)]
    enabled: Vec<String>,
}

fn load_enabled(workspace: &Path) -> Vec<String> {
    std::fs::read_to_string(enabled_list_path(workspace)).ok().and_then(|raw| toml::from_str::<EnabledFile>(&raw).ok()).map(|f| f.enabled).unwrap_or_default()
}

fn save_enabled(workspace: &Path, enabled: &[String]) -> Result<()> {
    let path = enabled_list_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(&EnabledFile { enabled: enabled.to_vec() })?)?;
    Ok(())
}

/// Every plugin found under `.rexo/plugins/*/plugin.toml`, each marked
/// with whether it's currently enabled.
pub fn discover(workspace: &Path) -> Vec<PluginInfo> {
    let dir = plugins_dir(workspace);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let enabled = load_enabled(workspace);
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let manifest_path = path.join("plugin.toml");
        let Ok(raw) = std::fs::read_to_string(&manifest_path) else { continue };
        let manifest: PluginManifest = toml::from_str(&raw).unwrap_or_default();
        out.push(PluginInfo { name: name.to_string(), description: manifest.description, version: manifest.version, enabled: enabled.iter().any(|e| e == name) });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn installed_record_path(workspace: &Path, name: &str) -> PathBuf {
    plugins_dir(workspace).join(name).join(".installed.toml")
}

/// Copy `name`'s skills/commands into the standard discovered locations
/// and merge its hooks, recording exactly what was installed so
/// [`disable`] can reverse it precisely. Returns a one-line human-
/// readable summary of what got installed.
pub fn enable(workspace: &Path, name: &str) -> Result<String> {
    let plugin_dir = plugins_dir(workspace).join(name);
    if !plugin_dir.join("plugin.toml").exists() {
        anyhow::bail!("no plugin named '{name}' (looked for {})", plugin_dir.join("plugin.toml").display());
    }

    let mut record = InstalledRecord::default();

    let skills_src = plugin_dir.join("skills");
    if skills_src.is_dir() {
        let skills_dst = workspace.join(".rexo").join("skills");
        std::fs::create_dir_all(&skills_dst)?;
        for entry in std::fs::read_dir(&skills_src)?.flatten() {
            let src = entry.path();
            if !src.is_dir() {
                continue;
            }
            let Some(skill_name) = src.file_name().and_then(|n| n.to_str()) else { continue };
            let dst = skills_dst.join(skill_name);
            copy_dir(&src, &dst)?;
            record.skills.push(skill_name.to_string());
        }
    }

    let commands_src = plugin_dir.join("commands");
    if commands_src.is_dir() {
        let commands_dst = workspace.join(".rexo").join("commands");
        std::fs::create_dir_all(&commands_dst)?;
        for entry in std::fs::read_dir(&commands_src)?.flatten() {
            let src = entry.path();
            if src.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(file_name) = src.file_name().and_then(|n| n.to_str()) else { continue };
            std::fs::copy(&src, commands_dst.join(file_name)).with_context(|| format!("copying {file_name}"))?;
            record.commands.push(file_name.trim_end_matches(".md").to_string());
        }
    }

    let hooks_src = plugin_dir.join("hooks.toml");
    if hooks_src.exists() {
        let raw = std::fs::read_to_string(&hooks_src)?;
        #[derive(Deserialize)]
        struct HooksFileShape {
            #[serde(default)]
            hooks: Vec<crate::hooks::HookDef>,
        }
        let plugin_hooks: Vec<crate::hooks::HookDef> = toml::from_str::<HooksFileShape>(&raw)?.hooks;
        let mut all_hooks = crate::hooks::load(workspace);
        for h in &plugin_hooks {
            all_hooks.push(h.clone());
        }
        crate::hooks::save(workspace, &all_hooks)?;
        record.hooks = plugin_hooks;
    }

    std::fs::write(installed_record_path(workspace, name), toml::to_string_pretty(&record)?)?;

    let mut enabled = load_enabled(workspace);
    if !enabled.iter().any(|e| e == name) {
        enabled.push(name.to_string());
    }
    save_enabled(workspace, &enabled)?;

    Ok(format!("{} skill(s), {} command(s), {} hook(s) installed", record.skills.len(), record.commands.len(), record.hooks.len()))
}

/// Reverses exactly what [`enable`] installed for `name`, using its
/// recorded `.installed.toml` — never a blanket wipe of
/// `.rexo/skills/`/`.rexo/commands/`, which might hold things the user
/// added directly.
pub fn disable(workspace: &Path, name: &str) -> Result<String> {
    let record_path = installed_record_path(workspace, name);
    let record: InstalledRecord = std::fs::read_to_string(&record_path).ok().and_then(|raw| toml::from_str(&raw).ok()).unwrap_or_default();

    for skill_name in &record.skills {
        let _ = std::fs::remove_dir_all(workspace.join(".rexo").join("skills").join(skill_name));
    }
    for cmd_name in &record.commands {
        let _ = std::fs::remove_file(workspace.join(".rexo").join("commands").join(format!("{cmd_name}.md")));
    }
    if !record.hooks.is_empty() {
        let mut all_hooks = crate::hooks::load(workspace);
        all_hooks.retain(|h| !record.hooks.iter().any(|ph| ph.event == h.event && ph.command == h.command && ph.matcher == h.matcher));
        crate::hooks::save(workspace, &all_hooks)?;
    }
    let _ = std::fs::remove_file(&record_path);

    let mut enabled = load_enabled(workspace);
    enabled.retain(|e| e != name);
    save_enabled(workspace, &enabled)?;

    Ok(format!("{} skill(s), {} command(s), {} hook(s) removed", record.skills.len(), record.commands.len(), record.hooks.len()))
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &dest)?;
        } else {
            std::fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rexo_plugin_test_{}_{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rand_suffix() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as u64
    }

    fn write_sample_plugin(workspace: &Path, name: &str) {
        let dir = workspace.join(".rexo").join("plugins").join(name);
        std::fs::create_dir_all(dir.join("skills").join("demo-skill")).unwrap();
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::write(dir.join("plugin.toml"), "description = \"a test plugin\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(dir.join("skills").join("demo-skill").join("SKILL.md"), "---\ndescription: demo\n---\nBody.\n").unwrap();
        std::fs::write(dir.join("commands").join("demo.md"), "---\ndescription: demo command\n---\nDo the thing.\n").unwrap();
        std::fs::write(dir.join("hooks.toml"), "[[hooks]]\nevent = \"post_tool\"\ncommand = \"echo plugin-hook\"\n").unwrap();
    }

    #[test]
    fn discover_finds_a_plugin_with_manifest() {
        let ws = temp_workspace();
        write_sample_plugin(&ws, "demo");
        let found = discover(&ws);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "demo");
        assert_eq!(found[0].description, "a test plugin");
        assert!(!found[0].enabled);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn enable_installs_skill_command_and_hook() {
        let ws = temp_workspace();
        write_sample_plugin(&ws, "demo");
        enable(&ws, "demo").unwrap();

        assert!(ws.join(".rexo/skills/demo-skill/SKILL.md").exists());
        assert!(ws.join(".rexo/commands/demo.md").exists());
        let hooks = crate::hooks::load(&ws);
        assert!(hooks.iter().any(|h| h.command == "echo plugin-hook"));

        let found = discover(&ws);
        assert!(found[0].enabled);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn disable_removes_exactly_what_enable_installed() {
        let ws = temp_workspace();
        write_sample_plugin(&ws, "demo");
        // Something the user added by hand — must survive disable.
        std::fs::create_dir_all(ws.join(".rexo/skills/own-skill")).unwrap();
        std::fs::write(ws.join(".rexo/skills/own-skill/SKILL.md"), "---\ndescription: mine\n---\nMine.\n").unwrap();

        enable(&ws, "demo").unwrap();
        disable(&ws, "demo").unwrap();

        assert!(!ws.join(".rexo/skills/demo-skill").exists());
        assert!(!ws.join(".rexo/commands/demo.md").exists());
        assert!(ws.join(".rexo/skills/own-skill").exists(), "hand-written skill must survive disabling an unrelated plugin");
        let hooks = crate::hooks::load(&ws);
        assert!(!hooks.iter().any(|h| h.command == "echo plugin-hook"));

        let found = discover(&ws);
        assert!(!found[0].enabled);
        let _ = std::fs::remove_dir_all(&ws);
    }
}
