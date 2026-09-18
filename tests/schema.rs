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

/// The two documents added in v0.19.0: their field paths are checked against
/// the OpenAPI of the deployed version, and a renamed field is found.
#[test]
fn the_indexer_config_and_delay_profile_fields_are_checked_against_the_openapi() {
    use converge::{schema::check_paths, services::servarr};
    let gewuenscht = |felder: &[(&str, Value)]| -> std::collections::BTreeMap<String, Value> {
        felder
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    };
    for version in [
        "radarr-6.3.0.10514",
        "sonarr-4.0.19.2979",
        "lidarr-3.1.0.4875",
    ] {
        let d = doc(version);
        assert_eq!(
            check_paths(
                &d,
                servarr::INDEXER_CONFIG,
                &gewuenscht(&[
                    ("minimumAge", 0.into()),
                    ("retention", 0.into()),
                    ("maximumSize", 0.into()),
                    ("rssSyncInterval", 15.into()),
                ])
            ),
            Vec::<String>::new(),
            "{version}"
        );
        assert_eq!(
            check_paths(
                &d,
                servarr::DELAY_PROFILE,
                &gewuenscht(&[
                    ("preferredProtocol", "usenet".into()),
                    ("usenetDelay", 0.into()),
                    ("torrentDelay", 30.into()),
                    ("enableUsenet", true.into()),
                    ("enableTorrent", true.into()),
                    ("bypassIfHighestQuality", true.into()),
                ])
            ),
            Vec::<String>::new(),
            "{version}"
        );
    }

    // A renamed field is found, in both components.
    let mut d = doc("radarr-6.3.0.10514");
    let props = properties(&mut d, "DelayProfileResource");
    let v = props.remove("torrentDelay").unwrap();
    props.insert("torrentDelayMinutes".to_string(), v);
    assert_eq!(
        check_paths(
            &d,
            servarr::DELAY_PROFILE,
            &gewuenscht(&[("torrentDelay", 30.into())])
        )
        .len(),
        1
    );
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

#[test]
fn a_renamed_property_inside_a_list_is_found() {
    // QualityProfileResource.items is a list of QualityProfileQualityItemResource;
    // the check has to follow the list, not stop at "array".
    let mut d = doc("radarr-6.3.0.10514");
    let props = properties(&mut d, "QualityProfileQualityItemResource");
    let v = props.remove("allowed").unwrap();
    props.insert("isAllowed".into(), v);
    assert_eq!(
        findings(&d),
        ["QualityProfileQualityItemResource.allowed: not in the OpenAPI description"]
    );
}

#[test]
fn a_list_of_the_wrong_component_is_found() {
    let mut d = doc("sonarr-4.0.19.2979");
    properties(&mut d, "QualityProfileResource")["items"]["items"]["$ref"] =
        "#/components/schemas/Quality".into();
    assert_eq!(
        findings(&d),
        ["QualityProfileResource.items: a list of Quality there, of QualityProfileQualityItemResource here"]
    );
}

fn jellyfin() -> Value {
    doc("jellyfin-10.11.11")
}

fn paths(component: &str, desired: serde_json::Value) -> Vec<String> {
    let map = desired.as_object().unwrap().clone().into_iter().collect();
    converge::schema::check_paths(&jellyfin(), component, &map)
}

#[test]
fn a_known_path_with_the_right_type_passes() {
    let found = paths(
        "ServerConfiguration",
        serde_json::json!({
            "TrickplayOptions.EnableKeyFrameOnlyExtraction": true,
            "TrickplayOptions.EnableHwAcceleration": true
        }),
    );
    assert_eq!(found, Vec::<String>::new());
    let found = paths(
        "LibraryOptions",
        serde_json::json!({"EnableTrickplayImageExtraction": true}),
    );
    assert_eq!(found, Vec::<String>::new());
}

#[test]
fn named_configuration_components_are_checked_like_documents() {
    let found = paths(
        "NetworkConfiguration",
        serde_json::json!({"KnownProxies": ["10.0.20.11"], "EnableUPnP": false}),
    );
    assert_eq!(found, Vec::<String>::new());
    let found = paths(
        "BrandingOptionsDto",
        serde_json::json!({"LoginDisclaimer": "<a href=\"/sso\">x</a>", "SplashscreenEnabled": false}),
    );
    assert_eq!(found, Vec::<String>::new());
    let found = paths(
        "NetworkConfiguration",
        serde_json::json!({"KnownProxies": "10.0.20.11"}),
    );
    assert_eq!(
        found,
        ["NetworkConfiguration.KnownProxies: expects array, the spec has a string"]
    );
}

#[test]
fn a_misspelt_segment_is_found_with_the_component_it_is_missing_from() {
    let found = paths(
        "ServerConfiguration",
        serde_json::json!({"TrickplayOptions.EnableHwAccel": true}),
    );
    assert_eq!(
        found,
        ["ServerConfiguration.TrickplayOptions.EnableHwAccel: TrickplayOptions has no property EnableHwAccel"]
    );
}

#[test]
fn a_wrong_value_type_is_found() {
    let found = paths(
        "ServerConfiguration",
        serde_json::json!({"TrickplayOptions.EnableHwAcceleration": "yes"}),
    );
    assert_eq!(
        found,
        ["ServerConfiguration.TrickplayOptions.EnableHwAcceleration: expects boolean, the spec has a string"]
    );
}

#[test]
fn a_path_through_a_plain_value_is_found() {
    let found = paths(
        "LibraryOptions",
        serde_json::json!({"EnableTrickplayImageExtraction.Deeper": true}),
    );
    assert_eq!(
        found,
        ["LibraryOptions.EnableTrickplayImageExtraction.Deeper: EnableTrickplayImageExtraction is not an object"]
    );
}

#[test]
fn a_trigger_type_outside_the_enum_is_found() {
    // 2026-09-07 on the host: a trigger type the API accepted silently and
    // never fired. The enum in the OpenAPI file turns it into a build error.
    let list = serde_json::json!([{"Type": "Daily", "TimeOfDayTicks": 198000000000i64}]);
    let found = converge::schema::check_objects(&jellyfin(), "TaskTriggerInfo", &list);
    assert_eq!(
        found,
        ["TaskTriggerInfo[0].Type: \"Daily\" is not one of DailyTrigger, WeeklyTrigger, IntervalTrigger, StartupTrigger"]
    );
    let good = serde_json::json!([{"Type": "DailyTrigger", "TimeOfDayTicks": 198000000000i64}]);
    assert_eq!(
        converge::schema::check_objects(&jellyfin(), "TaskTriggerInfo", &good),
        Vec::<String>::new()
    );
    let unknown_key = serde_json::json!([{"Type": "DailyTrigger", "TimeOfDay": 1}]);
    assert_eq!(
        converge::schema::check_objects(&jellyfin(), "TaskTriggerInfo", &unknown_key),
        ["TaskTriggerInfo[0].TimeOfDay: TaskTriggerInfo has no property TimeOfDay"]
    );
}

#[test]
fn null_needs_a_nullable_property() {
    let found = paths(
        "ServerConfiguration",
        serde_json::json!({"TrickplayOptions.EnableHwAcceleration": null}),
    );
    assert_eq!(
        found,
        ["ServerConfiguration.TrickplayOptions.EnableHwAcceleration: is not nullable, the spec has null"]
    );
}

#[test]
fn the_jellyfin_endpoints_and_wire_types_match() {
    let found = check(
        &jellyfin(),
        &converge::services::jellyfin::ENDPOINTS,
        &converge::services::jellyfin::wire_types(),
    );
    assert_eq!(found, Vec::<String>::new());
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

fn trailarr() -> Value {
    doc("trailarr-0.11.5")
}

fn trailarr_findings(openapi: &Value) -> Vec<String> {
    use converge::services::trailarr;
    check(openapi, &trailarr::ENDPOINTS, &trailarr::wire_types())
}

fn trailarr_paths(component: &str, desired: Value) -> Vec<String> {
    let map = desired.as_object().unwrap().clone().into_iter().collect();
    converge::schema::check_paths(&trailarr(), component, &map)
}

#[test]
fn the_trailarr_endpoints_and_wire_types_match() {
    assert_eq!(trailarr_findings(&trailarr()), Vec::<String>::new());
}

#[test]
fn a_renamed_trailarr_property_is_found_directly_and_inside_a_list() {
    let mut d = trailarr();
    let props = properties(&mut d, "ConnectionRead");
    let v = props.remove("api_key").unwrap();
    props.insert("apikey".into(), v);
    // PathMappingCRU is only reached through the list in the request bodies.
    properties(&mut d, "PathMappingCRU").remove("path_to");
    assert_eq!(
        trailarr_findings(&d),
        [
            "ConnectionRead.api_key: not in the OpenAPI description",
            "PathMappingCRU.path_to: not in the OpenAPI description"
        ]
    );
}

#[test]
fn the_setting_value_must_keep_its_three_types() {
    let mut d = trailarr();
    properties(&mut d, "UpdateSetting")["value"]["anyOf"]
        .as_array_mut()
        .unwrap()
        .remove(2);
    assert_eq!(
        trailarr_findings(&d),
        ["UpdateSetting.value: one of integer, string there, one of boolean, integer, string here"]
    );
}

#[test]
fn nullable_as_any_of_needs_an_option() {
    // OpenAPI 3.1 writes nullable as anyOf [..., {type: null}].
    let mut d = trailarr();
    properties(&mut d, "ConnectionRead")["name"] =
        serde_json::json!({"anyOf": [{"type": "string"}, {"type": "null"}]});
    assert_eq!(
        trailarr_findings(&d),
        ["ConnectionRead.name: nullable there, but not an Option here"]
    );
}

fn host_connection() -> Value {
    serde_json::json!({
        "arr_type": "radarr", "url": "http://127.0.0.1:7878",
        "monitor_new_media": true, "external_url": "", "path_mappings": []
    })
}

#[test]
fn the_hosts_connection_fields_are_properties_of_both_bodies() {
    for component in ["ConnectionCreate", "ConnectionUpdate"] {
        assert_eq!(
            trailarr_paths(component, host_connection()),
            Vec::<String>::new(),
            "{component}"
        );
    }
    let mapping = serde_json::json!({"path_mappings": [{"path_from": "/a", "path_to": "/b"}]});
    assert_eq!(
        trailarr_paths("ConnectionUpdate", mapping),
        Vec::<String>::new()
    );
}

#[test]
fn monitor_is_not_a_trailarr_field() {
    // What the host's shell unit sent for years; pydantic ignored it.
    for component in ["ConnectionCreate", "ConnectionUpdate"] {
        assert_eq!(
            trailarr_paths(component, serde_json::json!({"monitor": true})),
            [format!(
                "{component}.monitor: {component} has no property monitor"
            )]
        );
    }
}

#[test]
fn an_arr_type_outside_the_enum_and_a_wrong_type_are_found() {
    for component in ["ConnectionCreate", "ConnectionUpdate"] {
        assert_eq!(
            trailarr_paths(component, serde_json::json!({"arr_type": "lidarr"})),
            [format!(
                "{component}.arr_type: \"lidarr\" is not one of radarr, sonarr, plex"
            )]
        );
        assert_eq!(
            trailarr_paths(component, serde_json::json!({"monitor_new_media": "yes"})),
            [format!(
                "{component}.monitor_new_media: expects boolean, the spec has a string"
            )]
        );
    }
    // null is allowed where the update says anyOf [..., null], not on create.
    assert_eq!(
        trailarr_paths("ConnectionUpdate", serde_json::json!({"url": null})),
        Vec::<String>::new()
    );
    assert_eq!(
        trailarr_paths("ConnectionCreate", serde_json::json!({"url": null})),
        ["ConnectionCreate.url: is not nullable, the spec has null"]
    );
    let bad_mapping = serde_json::json!({"path_mappings": [{"from": "/a"}]});
    assert_eq!(
        trailarr_paths("ConnectionCreate", bad_mapping),
        ["ConnectionCreate.path_mappings[0].from: PathMappingCRU has no property from"]
    );
}

#[test]
fn trailer_profile_fields_are_checked_by_name_and_type() {
    let host = serde_json::json!({
        "search_query": "{title} {year} deutscher trailer", "always_search": true,
        "exclude_words": "reaction,review", "file_format": "mp4",
        "video_format": "h264", "audio_format": "aac"
    });
    assert_eq!(
        trailarr_paths("TrailerProfileRead", host),
        Vec::<String>::new()
    );
    assert_eq!(
        trailarr_paths(
            "TrailerProfileRead",
            serde_json::json!({"retry_count": "2", "search": "x"})
        ),
        [
            "TrailerProfileRead.retry_count: expects integer, the spec has a string",
            "TrailerProfileRead.search: TrailerProfileRead has no property search"
        ]
    );
}

// --- Servarr providers --------------------------------------------------------

#[test]
fn the_provider_endpoints_match_for_all_four_services() {
    use converge::{services::providers::ProviderApi, spec::Service};
    for (file, service) in [
        ("radarr-6.3.0.10514", Service::Radarr),
        ("sonarr-4.0.19.2979", Service::Sonarr),
        ("lidarr-3.1.0.4875", Service::Lidarr),
        ("prowlarr-2.5.2.5491", Service::Prowlarr),
    ] {
        let endpoints = ProviderApi::endpoints_of(service);
        assert!(!endpoints.is_empty());
        assert_eq!(
            check(&doc(file), &endpoints, &[]),
            Vec::<String>::new(),
            "{file}"
        );
    }
}

#[test]
fn prowlarr_indexer_and_proxy_top_level_fields_are_checked() {
    assert_eq!(
        servarr_paths(
            "prowlarr-2.5.2.5491",
            "IndexerResource",
            serde_json::json!({"enable": true, "appProfileId": 1, "priority": 25, "redirect": true})
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        servarr_paths(
            "prowlarr-2.5.2.5491",
            "IndexerResource",
            serde_json::json!({"priority": "25", "syncLevel": "fullSync"})
        ),
        [
            "IndexerResource.priority: expects integer, the spec has a string",
            "IndexerResource.syncLevel: IndexerResource has no property syncLevel"
        ]
    );
    assert_eq!(
        servarr_paths(
            "prowlarr-2.5.2.5491",
            "IndexerProxyResource",
            serde_json::json!({"onHealthIssue": false})
        ),
        Vec::<String>::new()
    );
}

#[test]
fn provider_top_level_fields_are_checked_per_service() {
    // Lidarr's webhook fires on a release import; Radarr's on a download.
    assert_eq!(
        servarr_paths(
            "lidarr-3.1.0.4875",
            "NotificationResource",
            serde_json::json!({"onReleaseImport": true, "onUpgrade": true, "includeHealthWarnings": false})
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        servarr_paths(
            "lidarr-3.1.0.4875",
            "NotificationResource",
            serde_json::json!({"onDownload": true})
        ),
        ["NotificationResource.onDownload: NotificationResource has no property onDownload"]
    );
    assert_eq!(
        servarr_paths(
            "radarr-6.3.0.10514",
            "DownloadClientResource",
            serde_json::json!({"enable": true, "priority": "1"})
        ),
        ["DownloadClientResource.priority: expects integer, the spec has a string"]
    );
}

// --- Servarr: naming, media management, root folders ------------------------

fn servarr_findings(openapi: &Value, lidarr: bool) -> Vec<String> {
    use converge::services::servarr;
    if lidarr {
        let mut endpoints = vec![servarr::V1.status];
        endpoints.extend(servarr::V1.task_endpoints());
        check(openapi, &endpoints, &servarr::lidarr_wire_types())
    } else {
        let mut endpoints = arr::ENDPOINTS.to_vec();
        endpoints.extend(servarr::V3.task_endpoints());
        check(openapi, &endpoints, &arr::wire_types())
    }
}

fn servarr_paths(file: &str, component: &str, desired: Value) -> Vec<String> {
    let map = desired.as_object().unwrap().clone().into_iter().collect();
    converge::schema::check_paths(&doc(file), component, &map)
}

#[test]
fn the_servarr_endpoints_match_for_all_three_services() {
    assert_eq!(
        servarr_findings(&doc("radarr-6.3.0.10514"), false),
        Vec::<String>::new()
    );
    assert_eq!(
        servarr_findings(&doc("sonarr-4.0.19.2979"), false),
        Vec::<String>::new()
    );
    assert_eq!(
        servarr_findings(&doc("lidarr-3.1.0.4875"), true),
        Vec::<String>::new()
    );
}

#[test]
fn a_root_folder_list_of_the_wrong_component_and_a_renamed_profile_name_are_found() {
    let mut d = doc("lidarr-3.1.0.4875");
    d["paths"]["/api/v1/rootfolder"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"]["items"]["$ref"] = "#/components/schemas/TagResource".into();
    let props = properties(&mut d, "QualityProfileResource");
    let v = props.remove("name").unwrap();
    props.insert("title".into(), v);
    assert_eq!(
        servarr_findings(&d, true),
        [
            "GET /api/v1/rootfolder 200 response is not a list of RootFolderResource",
            "QualityProfileResource.name: not in the OpenAPI description"
        ]
    );
}

#[test]
fn the_hosts_documents_are_checked_by_name_type_and_enum() {
    // Radarr's colon replacement is an enum of strings, Sonarr's of numbers.
    assert_eq!(
        servarr_paths(
            "radarr-6.3.0.10514",
            "NamingConfigResource",
            serde_json::json!({"colonReplacementFormat": "smart", "renameMovies": true})
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        servarr_paths(
            "sonarr-4.0.19.2979",
            "NamingConfigResource",
            serde_json::json!({"colonReplacementFormat": 4, "multiEpisodeStyle": 5})
        ),
        Vec::<String>::new()
    );
    let found = servarr_paths(
        "radarr-6.3.0.10514",
        "NamingConfigResource",
        serde_json::json!({"colonReplacementFormat": "clever", "renameEpisodes": true}),
    );
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found[0]
            .starts_with("NamingConfigResource.colonReplacementFormat: \"clever\" is not one of"),
        "{found:?}"
    );
    assert_eq!(
        found[1],
        "NamingConfigResource.renameEpisodes: NamingConfigResource has no property renameEpisodes"
    );
    // tofu's provider called it hardlinks_copy; the API does not.
    assert_eq!(
        servarr_paths(
            "lidarr-3.1.0.4875",
            "MediaManagementConfigResource",
            serde_json::json!({"hardlinksCopy": false})
        ),
        ["MediaManagementConfigResource.hardlinksCopy: MediaManagementConfigResource has no property hardlinksCopy"]
    );
    assert_eq!(
        servarr_paths(
            "lidarr-3.1.0.4875",
            "RootFolderResource",
            serde_json::json!({"name": "Musik", "path": "/tank/data/media/music",
                               "defaultMonitorOption": "all", "defaultNewItemMonitorOption": "all",
                               "defaultQualityProfileId": 1, "defaultMetadataProfileId": 1})
        ),
        Vec::<String>::new()
    );
}

fn kavita() -> Value {
    doc("kavita-0.9.1.4")
}

#[test]
fn the_kavita_endpoints_and_wire_types_match() {
    let found = check(
        &kavita(),
        &converge::services::kavita::ENDPOINTS,
        &converge::services::kavita::wire_types(),
    );
    assert_eq!(found, Vec::<String>::new());
}

#[test]
fn kavitas_oidc_switches_are_paths_of_its_settings_and_a_typo_is_not() {
    use converge::services::kavita::SERVER_SETTINGS;
    let desired = serde_json::json!({
        "oidcConfig.provisionAccounts": true,
        "oidcConfig.syncUserSettings": true,
        "oidcConfig.requireVerifiedEmail": false,
        "oidcConfig.disablePasswordAuthentication": true,
        "oidcConfig.autoLogin": true,
        "oidcConfig.rolesClaim": "kavita_roles",
        "oidcConfig.rolesPrefix": ""
    });
    let map = desired.as_object().unwrap().clone().into_iter().collect();
    assert_eq!(
        converge::schema::check_paths(&kavita(), SERVER_SETTINGS, &map),
        Vec::<String>::new()
    );
    let typo = serde_json::json!({"oidcConfig.autoLogn": true, "oidcConfig.autoLogin": "yes"});
    let map = typo.as_object().unwrap().clone().into_iter().collect();
    let found = converge::schema::check_paths(&kavita(), SERVER_SETTINGS, &map);
    assert_eq!(found.len(), 2, "{found:?}");
}
