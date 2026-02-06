use serde::{Deserialize, Serialize};
use strum::Display;

#[derive(Copy, Clone, Debug, Display, Serialize, Deserialize)]
pub enum Model {
    Haiku,
    Sonnet,
    Opus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_variants() {
        assert_eq!(Model::Haiku.to_string(), "Haiku");
        assert_eq!(Model::Sonnet.to_string(), "Sonnet");
        assert_eq!(Model::Opus.to_string(), "Opus");
    }

    #[test]
    fn deserialize_valid() {
        let h: Model = serde_yaml::from_str("Haiku").unwrap();
        assert!(matches!(h, Model::Haiku));
        let s: Model = serde_yaml::from_str("Sonnet").unwrap();
        assert!(matches!(s, Model::Sonnet));
        let o: Model = serde_yaml::from_str("Opus").unwrap();
        assert!(matches!(o, Model::Opus));
    }

    #[test]
    fn deserialize_lowercase() {
        // serde default is case-sensitive — "haiku" should fail
        let result: Result<Model, _> = serde_yaml::from_str("haiku");
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_invalid() {
        let result: Result<Model, _> = serde_yaml::from_str("gpt4");
        assert!(result.is_err());
    }
}
