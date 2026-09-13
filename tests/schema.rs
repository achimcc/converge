use converge::{schema::check, services::arr};
use schemars::JsonSchema;
use serde_json::Value;

fn doc(name: &str) -> Value {
    let path = format!("{}/openapi/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn findings(openapi: &Value) -> Vec<String> {
    check(openapi, &arr::ENDPOINTS, &arr::wire_types())
}

fn properties<'a>(d: &'a mut Value, component: &str) -> &'a mut serde_json::Map<String, Value> {
    d.pointer_mut(&format!("/components/schemas/{component}/properties"))
        .unwrap()
        .as_object_mut()
        .unwrap()
}

#[test]
fn the_deployed_versions_match() {
    assert_eq!(findings(&doc("radarr-6.3.0.10514")), Vec::<String>::new());
    assert_eq!(findings(&doc("sonarr-4.0.19.2979")), Vec::<String>::new());
}

#[test]
fn a_renamed_property_is_found() {
    let mut d = doc("radarr-6.3.0.10514");
    let props = properties(&mut d, "QualityDefinitionResource");
    let v = props.remove("minSize").unwrap();
    props.insert("minimumSize".into(), v);
    assert_eq!(
        findings(&d),
        ["QualityDefinitionResource.minSize: not in the OpenAPI description"]
    );
}

#[test]
fn a_nested_rename_is_found() {
    let mut d = doc("sonarr-4.0.19.2979");
    properties(&mut d, "Quality").remove("name");
    assert_eq!(
        findings(&d),
        ["Quality.name: not in the OpenAPI description"]
    );
}

#[test]
fn a_changed_type_is_found() {
    let mut d = doc("radarr-6.3.0.10514");
    properties(&mut d, "QualityDefinitionResource")["maxSize"]["type"] = "integer".into();
    assert_eq!(
        findings(&d),
        ["QualityDefinitionResource.maxSize: integer there, number here"]
    );
}

#[test]
fn a_missing_endpoint_or_wrong_body_is_found() {
    let mut d = doc("radarr-6.3.0.10514");
    d["paths"]
        .as_object_mut()
        .unwrap()
        .remove("/api/v3/qualitydefinition/update");
    assert_eq!(
        findings(&d),
        ["PUT /api/v3/qualitydefinition/update does not exist"]
    );

    let mut d = doc("radarr-6.3.0.10514");
    d["paths"]["/api/v3/qualitydefinition"]["get"]["responses"]["200"]["content"]
        ["application/json"]["schema"] =
        serde_json::json!({"$ref": "#/components/schemas/QualityDefinitionResource"});
    assert_eq!(
        findings(&d),
        ["GET /api/v3/qualitydefinition 200 response is not a list of QualityDefinitionResource"]
    );
}

mod strict {
    use super::JsonSchema;

    #[allow(dead_code)]
    #[derive(JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub struct QualityDefinitionResource {
        pub min_size: f64,
    }

    #[allow(dead_code)]
    #[derive(JsonSchema)]
    pub struct SystemResource {}
}

#[test]
fn nullable_there_but_required_here_is_found() {
    let wire = vec![
        schemars::schema_for!(strict::SystemResource),
        schemars::schema_for!(strict::QualityDefinitionResource),
    ];
    let found = check(&doc("radarr-6.3.0.10514"), &arr::ENDPOINTS, &wire);
    assert!(
        found.contains(
            &"QualityDefinitionResource.minSize: nullable there, but not an Option here"
                .to_string()
        ),
        "{found:?}"
    );
    // A wire type with no properties checks nothing and must say so.
    assert!(
        found.contains(&"wire type SystemResource declares no properties".to_string()),
        "{found:?}"
    );
}

#[test]
fn a_missing_wire_type_is_found() {
    let wire = vec![schemars::schema_for!(strict::SystemResource)];
    let found = check(&doc("radarr-6.3.0.10514"), &arr::ENDPOINTS, &wire);
    assert!(
        found.iter().any(|f| f.ends_with(
            "uses QualityDefinitionResource, but this program has no wire type of that name"
        )),
        "{found:?}"
    );
}
