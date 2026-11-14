//! Host only: what the host setup page edits, in a JSON file of its own (`settings_file` in
//! the host configuration): model cards for the control panel, and a project with its rule
//! sets and roles. The host configuration itself, with its keys, is never written.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;
use url::Url;

pub const MODEL_CARD_README_BYTES: u64 = 32 * 1024;
const SETTINGS_BYTES: u64 = 1024 * 1024;
/// The tools a role may name; nothing runs them yet.
pub const ROLE_TOOLS: [&str; 5] = [
    "archive_search",
    "email_read",
    "image_create",
    "video_create",
    "hand_over",
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub model_cards: Vec<ModelCard>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    #[serde(default)]
    pub rule_sets: Vec<RuleSet>,
    #[serde(default)]
    pub roles: Vec<Role>,
}

/// A model's card for the control panel. The links open in the user's browser; the README
/// is read once from this computer when the host starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelCard {
    /// The model ID as `GET /v1/models` lists it.
    pub id: String,
    /// The model's page, e.g. on Hugging Face.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub page: String,
    /// Where its benchmark results are published.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub benchmarks: String,
    /// A .md or .txt file with the model's description, absolute path, at most 32 KiB.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub readme: String,
}

/// What a group agreed, e.g. for code or for design; a role names the sets that apply to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleSet {
    pub name: String,
    #[serde(default)]
    pub rules: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    /// Shown to clients. The instructions never are.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The model behind the role; empty: the host's default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    #[serde(default)]
    pub rule_sets: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
}

/// A link a model card may carry: an https address without credentials.
pub fn card_link(value: &str) -> bool {
    value.len() <= 512
        && Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

/// The key a model card's README is fetched by: the first 16 hex digits of the ID's SHA-256.
pub fn model_card_key(id: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(id.as_bytes())[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unique<'a>(names: impl Iterator<Item = &'a str>, what: &str) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for name in names {
        ensure!(seen.insert(name), "two {what} are called {name}");
    }
    Ok(())
}

impl ModelCard {
    fn check(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty() && self.id.chars().count() <= 256,
            "a model card needs a model ID of at most 256 characters"
        );
        for (name, link) in [("page", &self.page), ("benchmarks", &self.benchmarks)] {
            ensure!(
                link.is_empty() || card_link(link),
                "the {name} link of {} must be an https address",
                self.id
            );
        }
        if !self.readme.is_empty() {
            let readme = Path::new(&self.readme);
            ensure!(
                readme.is_absolute(),
                "the README of {} needs an absolute path",
                self.id
            );
            // The README goes to the paired client: only text files meant for it, never a
            // configuration or state file by mistake.
            ensure!(
                readme
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "md" | "markdown" | "txt"
                    )),
                "the README of {} must be a .md or .txt file",
                self.id
            );
        }
        Ok(())
    }
}

impl Settings {
    pub fn check(&self) -> Result<()> {
        ensure!(self.model_cards.len() <= 64, "at most 64 model cards");
        for card in &self.model_cards {
            card.check()?;
        }
        unique(
            self.model_cards.iter().map(|c| c.id.as_str()),
            "model cards",
        )?;
        ensure!(
            self.project.chars().count() <= 60,
            "the project name is too long"
        );
        ensure!(self.rule_sets.len() <= 32, "at most 32 rule sets");
        for set in &self.rule_sets {
            ensure!(
                !set.name.trim().is_empty() && set.name.chars().count() <= 40,
                "every rule set needs a name of at most 40 characters"
            );
            ensure!(
                set.rules.chars().count() <= 8000,
                "the rules of {} are too long",
                set.name
            );
        }
        unique(self.rule_sets.iter().map(|s| s.name.as_str()), "rule sets")?;
        ensure!(self.roles.len() <= 32, "at most 32 roles");
        for role in &self.roles {
            ensure!(
                !role.name.trim().is_empty() && role.name.chars().count() <= 40,
                "every role needs a name of at most 40 characters"
            );
            ensure!(
                role.description.chars().count() <= 500
                    && role.instructions.chars().count() <= 8000
                    && role.model.chars().count() <= 256,
                "the role {} has too long a text",
                role.name
            );
            for set in &role.rule_sets {
                ensure!(
                    self.rule_sets.iter().any(|known| &known.name == set),
                    "the role {} names an unknown rule set {set}",
                    role.name
                );
            }
            for tool in &role.tools {
                ensure!(
                    ROLE_TOOLS.contains(&tool.as_str()),
                    "the role {} names an unknown tool {tool}",
                    role.name
                );
            }
        }
        unique(self.roles.iter().map(|r| r.name.as_str()), "roles")
    }

    /// The settings in `path`; a file that does not exist yet means none.
    pub fn load(path: &Path) -> Result<Self> {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error).context("cannot read the settings file"),
        };
        ensure!(
            metadata.len() <= SETTINGS_BYTES,
            "the settings file is larger than 1 MiB"
        );
        let settings: Self = serde_json::from_slice(&std::fs::read(path)?)
            .context("the settings file is not valid")?;
        settings.check().context("the settings file is not valid")?;
        Ok(settings)
    }

    /// Writes the settings privately, keeping the previous file as `<name>.bak`.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.check()?;
        ensure!(
            path.is_absolute(),
            "the settings file needs an absolute path"
        );
        if path.is_file() {
            std::fs::copy(path, path.with_extension("json.bak"))
                .context("cannot keep a copy of the previous settings")?;
        }
        let draft = path.with_extension("json.new");
        crate::config::private_file(&draft, &serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&draft, path).context("cannot write the settings file")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(page: &str, readme: &str) -> ModelCard {
        ModelCard {
            id: "model".into(),
            page: page.into(),
            benchmarks: String::new(),
            readme: readme.into(),
        }
    }

    #[test]
    fn model_cards_carry_only_https_links_and_a_text_readme() {
        for (page, readme, valid) in [
            (
                "https://huggingface.co/org/model",
                "/models/README.md",
                true,
            ),
            ("", "", true),
            ("", "/models/notes.TXT", true),
            ("http://huggingface.co/org/model", "", false),
            ("https://user:pw@example.org/model", "", false),
            ("javascript:alert(1)", "", false),
            ("", "README.md", false),
            ("", "/etc/k-lani-link/host.toml", false),
        ] {
            let settings = Settings {
                model_cards: vec![card(page, readme)],
                ..Settings::default()
            };
            assert_eq!(settings.check().is_ok(), valid, "{page} {readme}");
        }
        let twice = Settings {
            model_cards: vec![card("", ""), card("", "")],
            ..Settings::default()
        };
        assert!(twice.check().is_err());
        assert_eq!(model_card_key("model").len(), 16);
    }

    #[test]
    fn roles_name_only_known_rule_sets_and_tools_and_saving_keeps_a_copy() {
        let mut settings: Settings = serde_json::from_str(
            r#"{"version": 2, "project": "Demo",
                "rule_sets": [{"name": "Code", "rules": "KISS"}],
                "roles": [{"name": "Coder", "model": "m", "instructions": "secret task",
                           "rule_sets": ["Code"], "tools": ["hand_over"]}]}"#,
        )
        .unwrap();
        settings.check().unwrap();
        settings.roles[0].rule_sets.push("Design".into());
        assert!(settings.check().is_err());
        settings.roles[0].rule_sets.pop();
        settings.roles[0].tools.push("shell".into());
        assert!(settings.check().is_err());
        settings.roles[0].tools.pop();

        let folder = std::env::temp_dir().join(format!("kl-settings-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("host-settings.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        settings.project = "Changed".into();
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap().project, "Changed");
        assert!(folder.join("host-settings.json.bak").is_file());
        std::fs::write(&path, "{not json").unwrap();
        assert!(Settings::load(&path).is_err());
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
