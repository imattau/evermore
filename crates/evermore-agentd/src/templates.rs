//! Curated starting points for the Agents gallery.
//!
//! A template is metadata over a built-in recipe: the nscript/TOML recipe
//! owns the behaviour, while the template supplies the product name, default
//! cadence, expiry, and a short explanation. Keeping them separate avoids
//! encoding presentation in recipe comments and lets a recipe be reused by
//! more than one template.

use std::collections::BTreeSet;

use evermore_common::agent::AgentTemplate;
use serde::Deserialize;

use crate::recipe;

#[derive(Deserialize)]
struct TemplateFile {
    #[serde(default)]
    template: Vec<AgentTemplate>,
}

const DEFAULT_TEMPLATES: &str = include_str!("../templates/default-agents.toml");

pub fn load() -> Vec<AgentTemplate> {
    let parsed: TemplateFile = match toml::from_str(DEFAULT_TEMPLATES) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("evermore-agentd: default agent templates are invalid: {error}");
            return Vec::new();
        }
    };
    let mut seen = BTreeSet::new();
    let mut templates = Vec::new();
    for template in parsed.template {
        if let Err(error) = validate(&template) {
            eprintln!(
                "evermore-agentd: ignoring agent template '{}': {error}",
                template.id
            );
            continue;
        }
        if !seen.insert(template.id.clone()) {
            eprintln!(
                "evermore-agentd: ignoring duplicate agent template '{}'",
                template.id
            );
            continue;
        }
        templates.push(template);
    }
    templates
}

fn validate(template: &AgentTemplate) -> Result<(), String> {
    if template.id.trim().is_empty() {
        return Err("id must not be empty".to_string());
    }
    if template.name.trim().is_empty() {
        return Err("name must not be empty".to_string());
    }
    if template.summary.trim().is_empty() {
        return Err("summary must not be empty".to_string());
    }
    if template.cadence_seconds == 0 {
        return Err("cadence_seconds must be greater than zero".to_string());
    }
    if template.expiry_days == 0 || template.expiry_days > 365 {
        return Err("expiry_days must be between 1 and 365".to_string());
    }
    if template.requires.iter().any(|item| item.trim().is_empty()) {
        return Err("requires entries must not be empty".to_string());
    }
    recipe::find(&template.recipe_id)
        .map_err(|error| format!("recipe '{}': {error}", template.recipe_id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_templates_reference_real_recipes() {
        let templates = load();
        assert!(!templates.is_empty(), "default templates should parse");
        let mut ids = BTreeSet::new();
        for template in &templates {
            assert!(ids.insert(template.id.clone()), "duplicate id");
            assert!(
                recipe::find(&template.recipe_id).is_ok(),
                "{} references {}",
                template.id,
                template.recipe_id
            );
            assert!(template.cadence_seconds > 0);
            assert!((1..=365).contains(&template.expiry_days));
        }
    }
}
