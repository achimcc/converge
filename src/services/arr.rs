use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::endpoint::{Endpoint, Shape};

pub const SYSTEM_STATUS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/system/status",
    request: None,
    response: Some(Shape::One("SystemResource")),
};
pub const QUALITY_LIST: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/qualitydefinition",
    request: None,
    response: Some(Shape::List("QualityDefinitionResource")),
};
pub const QUALITY_UPDATE: Endpoint = Endpoint {
    method: "PUT",
    path: "/api/v3/qualitydefinition/update",
    request: Some(Shape::List("QualityDefinitionResource")),
    response: None,
};
pub const ENDPOINTS: [Endpoint; 3] = [SYSTEM_STATUS, QUALITY_LIST, QUALITY_UPDATE];

/// The wire types, each named exactly like its OpenAPI component. Their
/// fields are what `schema-check` compares -- derived, not listed by hand.
pub fn wire_types() -> Vec<schemars::Schema> {
    vec![
        schemars::schema_for!(SystemResource),
        schemars::schema_for!(QualityDefinitionResource),
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SystemResource {
    #[serde(default)]
    pub version: Option<String>,
}

/// Declares only what this program reads or writes. Everything else the
/// service sends is kept in `rest` and written back untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Quality {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QualityDefinitionResource {
    pub quality: Quality,
    // `default`: the service omits null values instead of writing `null`.
    #[serde(default)]
    pub min_size: Option<f64>,
    #[serde(default)]
    pub preferred_size: Option<f64>,
    #[serde(default)]
    pub max_size: Option<f64>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const RADARR_LIST: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/qualitydefinition.json");
    pub const SONARR_LIST: &str =
        include_str!("../../tests/fixtures/sonarr-4.0.19.2979/qualitydefinition.json");

    fn by_name<'a>(
        list: &'a [QualityDefinitionResource],
        name: &str,
    ) -> &'a QualityDefinitionResource {
        list.iter()
            .find(|d| d.quality.name.as_deref() == Some(name))
            .unwrap()
    }

    #[test]
    fn recorded_answers_parse() {
        let radarr: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        let sonarr: Vec<QualityDefinitionResource> = serde_json::from_str(SONARR_LIST).unwrap();
        assert!(radarr.len() > 20 && sonarr.len() > 20);
    }

    #[test]
    fn an_absent_nullable_field_is_none() {
        // Radarr omits null values: this entry has no maxSize key at all.
        let raw: Value = serde_json::from_str(RADARR_LIST).unwrap();
        let entry = raw
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["quality"]["name"] == "Bluray-1080p")
            .unwrap();
        assert!(
            entry.get("maxSize").is_none(),
            "fixture changed; pick another entry"
        );
        let list: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        assert_eq!(by_name(&list, "Bluray-1080p").max_size, None);
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        let list: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        let back = serde_json::to_value(by_name(&list, "Unknown")).unwrap();
        assert_eq!(back["quality"]["modifier"], "none");
        assert_eq!(back["id"], 1);
        assert!(back.get("weight").is_some() && back.get("title").is_some());
    }

    #[test]
    fn none_is_written_as_explicit_null() {
        let list: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        let back = serde_json::to_value(by_name(&list, "Bluray-1080p")).unwrap();
        assert_eq!(back.get("maxSize"), Some(&Value::Null));
    }

    #[test]
    fn a_missing_quality_object_is_an_error() {
        assert!(serde_json::from_str::<QualityDefinitionResource>(r#"{"minSize":1}"#).is_err());
    }

    #[test]
    fn wire_types_are_named_after_their_components() {
        let titles: Vec<String> = wire_types()
            .iter()
            .map(|s| s.as_value()["title"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(titles, ["SystemResource", "QualityDefinitionResource"]);
    }
}
