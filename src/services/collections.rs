//! `collections`: Jellyfin collections fed from a Radarr import list
//! (design §51).
//!
//! A collection the spec names holds exactly the films of one Jellyfin
//! library whose TMDb id is on one Radarr import list -- no more, no less.
//! What a list holds is Radarr's answer to `GET /api/v3/importlist/movie`:
//! the films it stored at its last sync, each with the ids of the lists that
//! named it. A film that leaves the list leaves the collection; a film that
//! is on the list but not yet in the library is a note, not a change, and
//! arrives with the next run after it is imported.
//!
//! **Two services, two transports.** The collection lives in Jellyfin, the
//! list in Radarr; the task holds the second transport itself, built from
//! the spec's `source` with its own credential. Nothing is written to
//! Radarr.
//!
//! **Collections the spec does not name are left alone**, and a named one
//! is owned completely: a member the list does not account for is removed,
//! as a format's specifications are in `custom-formats` (§41).
//!
//! Jellyfin answers a collection's members to an API key without a user
//! only with `recursive=true` (10.11.11, measured: 0 without, 211 with).

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    client::{expect_status, Transport},
    endpoint::{is_path_segment, Endpoint, Shape},
    engine::{Change, Probe, Task},
    error::Error,
    services::{jellyfin, providers::IMPORT_LISTS_V3},
};

pub const ITEMS: Endpoint = Endpoint {
    method: "GET",
    path: "/Items",
    request: None,
    response: Some(Shape::One("BaseItemDtoQueryResult")),
};
pub const COLLECTION_CREATE: Endpoint = Endpoint {
    method: "POST",
    path: "/Collections",
    request: None,
    response: Some(Shape::One("CollectionCreationResult")),
};
pub const COLLECTION_ADD: Endpoint = Endpoint {
    method: "POST",
    path: "/Collections/{collectionId}/Items",
    request: None,
    response: None,
};
pub const COLLECTION_REMOVE: Endpoint = Endpoint {
    method: "DELETE",
    path: "/Collections/{collectionId}/Items",
    request: None,
    response: None,
};
pub const ENDPOINTS: [Endpoint; 4] = [ITEMS, COLLECTION_CREATE, COLLECTION_ADD, COLLECTION_REMOVE];

/// Radarr's stored list films. The description declares no body for it
/// (`200 OK` only); the fields read are named in [`ListMovie`].
pub const LIST_MOVIES: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/importlist/movie",
    request: None,
    response: None,
};

/// The query parameters this task sends, by endpoint -- `tests/schema.rs`
/// holds them against the description, since `schema-check` does not look
/// at a query.
pub const QUERY_PARAMETERS: [(&str, &str, &[&str]); 4] = [
    (
        "/Items",
        "get",
        &["parentId", "includeItemTypes", "recursive", "fields"],
    ),
    ("/Collections", "post", &["name", "ids"]),
    ("/Collections/{collectionId}/Items", "post", &["ids"]),
    ("/Collections/{collectionId}/Items", "delete", &["ids"]),
];

pub fn wire_types() -> Vec<schemars::Schema> {
    vec![
        schemars::schema_for!(BaseItemDtoQueryResult),
        schemars::schema_for!(CollectionCreationResult),
    ]
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemDtoQueryResult {
    #[serde(default)]
    pub items: Vec<BaseItemDto>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemDto {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub provider_ids: Option<BTreeMap<String, Option<String>>>,
}

impl BaseItemDto {
    fn tmdb(&self) -> Option<i64> {
        self.provider_ids
            .as_ref()?
            .get("Tmdb")?
            .as_deref()?
            .parse()
            .ok()
    }

    fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.id.clone())
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct CollectionCreationResult {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListMovie {
    tmdb_id: i64,
    #[serde(default)]
    lists: Vec<i64>,
}

/// Where a collection's films come from: one Radarr import list by name.
pub struct Source<'a> {
    pub transport: &'a dyn Transport,
    pub import_list: String,
}

pub struct Wanted<'a> {
    pub name: String,
    pub library: String,
    pub source: Source<'a>,
}

pub struct Collections<'a> {
    pub collections: Vec<Wanted<'a>>,
}

/// One named collection as it stands.
#[derive(Debug)]
pub struct Found {
    /// `None` while the collection does not exist.
    pub id: Option<String>,
    pub members: Vec<BaseItemDto>,
    /// The library's films whose TMDb id is on the list.
    pub wanted: Vec<BaseItemDto>,
    /// How many films the list holds, and how many the library lacks.
    pub on_list: usize,
    pub not_in_library: usize,
}

pub struct Current {
    pub found: Vec<Found>,
}

/// Percent-encodes a query value: everything but unreserved characters.
fn query_value(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A write Jellyfin is still busy with when the request gives up: creating
/// a collection writes its folder, metadata and images, and on the host this
/// was written for took longer than the ten seconds a request may take
/// (2026-10-10: `timeout: global`, and the collection stood complete when it
/// was read). Such a write is not known to have failed; the engine reads back
/// until the state is there or its deadline ends, and that decides.
fn sent(
    result: Result<crate::client::Reply, Error>,
) -> Result<Option<crate::client::Reply>, Error> {
    match result {
        Ok(reply) => Ok(Some(reply)),
        Err(Error::Request { reason, .. }) if reason.contains("timeout") => Ok(None),
        Err(e) => Err(e),
    }
}

fn decode<T: for<'de> Deserialize<'de>>(path: &str, body: &str) -> Result<T, Error> {
    serde_json::from_str(body).map_err(|e| Error::Decode {
        path: path.to_string(),
        reason: crate::error::shape(&e),
    })
}

fn get_items(t: &dyn Transport, query: &str) -> Result<Vec<BaseItemDto>, Error> {
    let path = format!("{}?{query}", ITEMS.path);
    let reply = t.get(&path)?;
    expect_status(&ITEMS, &reply, &[200])?;
    let result: BaseItemDtoQueryResult = decode(ITEMS.path, &reply.body)?;
    if let Some(bad) = result.items.iter().find(|i| !is_path_segment(&i.id)) {
        return Err(Error::Decode {
            path: ITEMS.path.to_string(),
            reason: format!("an item id is not a plain id: {:?}", bad.label()),
        });
    }
    Ok(result.items)
}

/// The TMDb ids on the import list called `name`. An unknown list and an
/// empty one are errors: a collection fed from nothing would be emptied.
fn list_members(source: &Source) -> Result<(usize, BTreeSet<i64>), Error> {
    let t = source.transport;
    let reply = t.get(IMPORT_LISTS_V3.list.path)?;
    expect_status(&IMPORT_LISTS_V3.list, &reply, &[200])?;
    let lists: Vec<serde_json::Map<String, Value>> =
        decode(IMPORT_LISTS_V3.list.path, &reply.body)?;
    let ids: Vec<i64> = lists
        .iter()
        .filter(|l| l.get("name").and_then(Value::as_str) == Some(source.import_list.as_str()))
        .filter_map(|l| l.get("id").and_then(Value::as_i64))
        .collect();
    let id = match ids.as_slice() {
        [id] => *id,
        [] => {
            return Err(Error::NotFound(vec![format!(
                "import list {} on the source",
                source.import_list
            )]))
        }
        _ => {
            return Err(Error::Mismatch(vec![format!(
                "the source has {} import lists named {} -- converge does not pick one",
                ids.len(),
                source.import_list
            )]))
        }
    };
    let reply = t.get(LIST_MOVIES.path)?;
    expect_status(&LIST_MOVIES, &reply, &[200])?;
    let movies: Vec<ListMovie> = decode(LIST_MOVIES.path, &reply.body)?;
    let members: BTreeSet<i64> = movies
        .iter()
        .filter(|m| m.lists.contains(&id))
        .map(|m| m.tmdb_id)
        .collect();
    if members.is_empty() {
        return Err(Error::EmptyList {
            path: format!("{} (import list {})", LIST_MOVIES.path, source.import_list),
        });
    }
    Ok((members.len(), members))
}

impl Collections<'_> {
    fn subject(name: &str) -> String {
        format!("collection {name}")
    }

    /// Members to add and to remove, by item id, each sorted by label.
    fn delta(found: &Found) -> (Vec<&BaseItemDto>, Vec<&BaseItemDto>) {
        let have: BTreeSet<&str> = found.members.iter().map(|m| m.id.as_str()).collect();
        let want: BTreeSet<&str> = found.wanted.iter().map(|m| m.id.as_str()).collect();
        let mut add: Vec<&BaseItemDto> = found
            .wanted
            .iter()
            .filter(|w| !have.contains(w.id.as_str()))
            .collect();
        let mut remove: Vec<&BaseItemDto> = found
            .members
            .iter()
            .filter(|m| !want.contains(m.id.as_str()))
            .collect();
        add.sort_by_key(|i| i.label());
        remove.sort_by_key(|i| i.label());
        (add, remove)
    }

    fn ids(items: &[&BaseItemDto]) -> String {
        items
            .iter()
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl Task for Collections<'_> {
    type Current = Current;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        jellyfin::probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(jellyfin::FOLDERS.path)?;
        expect_status(&jellyfin::FOLDERS, &reply, &[200])?;
        let folders: Vec<serde_json::Map<String, Value>> =
            decode(jellyfin::FOLDERS.path, &reply.body)?;
        let boxsets = get_items(t, "includeItemTypes=BoxSet&recursive=true")?;
        let mut found = Vec::new();
        for wanted in &self.collections {
            let library = folders
                .iter()
                .filter(|f| f.get("Name").and_then(Value::as_str) == Some(wanted.library.as_str()))
                .find_map(|f| f.get("ItemId").and_then(Value::as_str))
                .ok_or_else(|| {
                    Error::NotFound(vec![format!(
                        "{}: library {}",
                        Self::subject(&wanted.name),
                        wanted.library
                    )])
                })?;
            if !is_path_segment(library) {
                return Err(Error::Decode {
                    path: jellyfin::FOLDERS.path.to_string(),
                    reason: format!("library {} has no plain id", wanted.library),
                });
            }
            let (on_list, tmdb) = list_members(&wanted.source)?;
            let films = get_items(
                t,
                &format!(
                    "parentId={library}&includeItemTypes=Movie&recursive=true&fields=ProviderIds"
                ),
            )?;
            let wanted_items: Vec<BaseItemDto> = films
                .into_iter()
                .filter(|f| f.tmdb().is_some_and(|id| tmdb.contains(&id)))
                .collect();
            let present: BTreeSet<i64> =
                wanted_items.iter().filter_map(BaseItemDto::tmdb).collect();
            let same_name: Vec<&BaseItemDto> = boxsets
                .iter()
                .filter(|b| b.name.as_deref() == Some(wanted.name.as_str()))
                .collect();
            let (id, members) = match same_name.as_slice() {
                [] => (None, Vec::new()),
                [one] => {
                    let members = get_items(
                        t,
                        &format!("parentId={}&recursive=true&fields=ProviderIds", one.id),
                    )?;
                    (Some(one.id.clone()), members)
                }
                more => {
                    return Err(Error::Mismatch(vec![format!(
                        "Jellyfin has {} collections named {} -- converge does not pick one",
                        more.len(),
                        wanted.name
                    )]))
                }
            };
            found.push(Found {
                id,
                members,
                wanted: wanted_items,
                on_list,
                not_in_library: on_list - present.len(),
            });
        }
        Ok(Current { found })
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut changes = Vec::new();
        for (wanted, found) in self.collections.iter().zip(&current.found) {
            let subject = Self::subject(&wanted.name);
            if found.id.is_none() {
                // Nothing to show yet is nothing to create: Jellyfin would
                // keep an empty folder nobody can tell from a broken one.
                if !found.wanted.is_empty() {
                    changes.push(Change {
                        subject: subject.clone(),
                        field: String::new(),
                        current: "(missing)".to_string(),
                        desired: format!("(added with {} films)", found.wanted.len()),
                    });
                }
                continue;
            }
            let (add, remove) = Self::delta(found);
            for item in add {
                changes.push(Change {
                    subject: subject.clone(),
                    field: item.label(),
                    current: "(not a member)".to_string(),
                    desired: "(member)".to_string(),
                });
            }
            for item in remove {
                changes.push(Change {
                    subject: subject.clone(),
                    field: item.label(),
                    current: "(member)".to_string(),
                    desired: "(not on the list)".to_string(),
                });
            }
        }
        Ok(changes)
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        self.collections
            .iter()
            .zip(&current.found)
            .filter(|(_, f)| f.not_in_library > 0)
            .map(|(w, f)| {
                format!(
                    "{}: {} of {} films on the list are not in library {} yet",
                    Self::subject(&w.name),
                    f.not_in_library,
                    f.on_list,
                    w.library
                )
            })
            .collect()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for (wanted, found) in self.collections.iter().zip(&current.found) {
            match &found.id {
                None if found.wanted.is_empty() => {}
                None => {
                    let all: Vec<&BaseItemDto> = found.wanted.iter().collect();
                    let path = format!(
                        "{}?name={}&ids={}",
                        COLLECTION_CREATE.path,
                        query_value(&wanted.name),
                        Self::ids(&all)
                    );
                    if let Some(reply) = sent(t.post_json(&path, ""))? {
                        expect_status(&COLLECTION_CREATE, &reply, &[200])?;
                        let created: CollectionCreationResult =
                            decode(COLLECTION_CREATE.path, &reply.body)?;
                        if created.id.is_empty() {
                            return Err(Error::Decode {
                                path: COLLECTION_CREATE.path.to_string(),
                                reason: "the new collection has no id".to_string(),
                            });
                        }
                    }
                }
                Some(id) => {
                    let (add, remove) = Self::delta(found);
                    let base = COLLECTION_ADD.path.replace("{collectionId}", id);
                    if !add.is_empty() {
                        let path = format!("{base}?ids={}", Self::ids(&add));
                        if let Some(reply) = sent(t.post_json(&path, ""))? {
                            expect_status(&COLLECTION_ADD, &reply, &[200, 204])?;
                        }
                    }
                    if !remove.is_empty() {
                        let path = format!("{base}?ids={}", Self::ids(&remove));
                        if let Some(reply) = sent(t.delete(&path))? {
                            expect_status(&COLLECTION_REMOVE, &reply, &[200, 204])?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const FOLDERS: &str =
        include_str!("../../tests/fixtures/jellyfin-10.11.11/collections-virtualfolders.json");
    const FILMS: &str =
        include_str!("../../tests/fixtures/jellyfin-10.11.11/collections-library-films.json");
    const BOXSETS: &str =
        include_str!("../../tests/fixtures/jellyfin-10.11.11/collections-boxsets.json");
    const LISTS: &str =
        include_str!("../../tests/fixtures/radarr-6.4.4.10685/importlist-stored.json");
    const LIST_FILMS: &str =
        include_str!("../../tests/fixtures/radarr-6.4.4.10685/importlist-movie.json");

    const NAME: &str = "50 politisch linke Filme";
    const LIBRARY: &str = "7a2175bccb1f1a94152cbd2b2bae8f6d";
    const FILMS_PATH: &str =
        "/Items?parentId=7a2175bccb1f1a94152cbd2b2bae8f6d&includeItemTypes=Movie&recursive=true&fields=ProviderIds";
    const BOXSETS_PATH: &str = "/Items?includeItemTypes=BoxSet&recursive=true";
    const COLLECTION: &str = "c0ffee00c0ffee00c0ffee00c0ffee00";

    fn radarr() -> FakeTransport {
        FakeTransport::default()
            .on_get("/api/v3/importlist", vec![ok(LISTS)])
            .on_get("/api/v3/importlist/movie", vec![ok(LIST_FILMS)])
    }

    fn task<'a>(source: &'a FakeTransport, list: &str) -> Collections<'a> {
        Collections {
            collections: vec![Wanted {
                name: NAME.to_string(),
                library: "Filme".to_string(),
                source: Source {
                    transport: source,
                    import_list: list.to_string(),
                },
            }],
        }
    }

    fn jellyfin(boxsets: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get("/Library/VirtualFolders", vec![ok(FOLDERS)])
            .on_get(BOXSETS_PATH, vec![ok(boxsets)])
            .on_get(FILMS_PATH, vec![ok(FILMS)])
    }

    /// The recorded library holds nine of the fifty films.
    fn wanted_ids() -> Vec<String> {
        let films: BaseItemDtoQueryResult = serde_json::from_str(FILMS).unwrap();
        let list: Vec<ListMovie> = serde_json::from_str(LIST_FILMS).unwrap();
        let tmdb: BTreeSet<i64> = list.iter().map(|m| m.tmdb_id).collect();
        let mut ids: Vec<String> = films
            .items
            .into_iter()
            .filter(|f| f.tmdb().is_some_and(|t| tmdb.contains(&t)))
            .map(|f| f.id)
            .collect();
        ids.sort();
        ids
    }

    fn with_collection(members: &[String]) -> (String, String) {
        let mut boxsets: Value = serde_json::from_str(BOXSETS).unwrap();
        boxsets["Items"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"Name": NAME, "Id": COLLECTION, "Type": "BoxSet"}));
        let films: Value = serde_json::from_str(FILMS).unwrap();
        let items: Vec<Value> = films["Items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| members.iter().any(|m| i["Id"] == m.as_str()))
            .cloned()
            .collect();
        (
            boxsets.to_string(),
            serde_json::json!({"Items": items, "TotalRecordCount": items.len()}).to_string(),
        )
    }

    fn members_path() -> String {
        format!("/Items?parentId={COLLECTION}&recursive=true&fields=ProviderIds")
    }

    #[test]
    fn the_fixtures_are_what_the_tests_say() {
        assert_eq!(wanted_ids().len(), 9);
        let folders: Vec<Value> = serde_json::from_str(FOLDERS).unwrap();
        assert!(folders.iter().any(|f| f["ItemId"] == LIBRARY));
    }

    #[test]
    fn a_missing_collection_is_created_with_the_films_the_library_has() {
        let source = radarr();
        let task = task(&source, NAME);
        let t = jellyfin(BOXSETS).on_put(vec![ok(r#"{"Id":"c0ffee00c0ffee00c0ffee00c0ffee00"}"#)]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "collection 50 politisch linke Filme: (missing) -> (added with 9 films)"
        );
        assert_eq!(
            task.notes(&current),
            ["collection 50 politisch linke Filme: 41 of 50 films on the list are not in library Filme yet"]
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        let (path, body) = &written[0];
        assert!(
            path.starts_with("/Collections?name=50%20politisch%20linke%20Filme&ids="),
            "{path}"
        );
        let mut sent: Vec<String> = path
            .split("&ids=")
            .nth(1)
            .unwrap()
            .split(',')
            .map(str::to_string)
            .collect();
        sent.sort();
        assert_eq!(sent, wanted_ids());
        assert_eq!(body, "");
    }

    #[test]
    fn a_complete_collection_is_unchanged_and_nothing_is_written() {
        let source = radarr();
        let task = task(&source, NAME);
        let (boxsets, members) = with_collection(&wanted_ids());
        let t = jellyfin(&boxsets).on_get(&members_path(), vec![ok(&members)]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        task.write(&t, &current).unwrap();
        assert!(t.written.borrow().is_empty());
        assert!(t.deleted.borrow().is_empty());
    }

    /// One film missing, one that is not on the list: one POST, one DELETE,
    /// each carrying exactly that id.
    #[test]
    fn a_missing_member_is_added_and_a_stranger_removed() {
        let source = radarr();
        let task = task(&source, NAME);
        let wanted = wanted_ids();
        let films: BaseItemDtoQueryResult = serde_json::from_str(FILMS).unwrap();
        let stranger = films
            .items
            .iter()
            .find(|f| !wanted.contains(&f.id))
            .unwrap()
            .id
            .clone();
        let mut members: Vec<String> = wanted[1..].to_vec();
        members.push(stranger.clone());
        let (boxsets, answer) = with_collection(&members);
        let t = jellyfin(&boxsets)
            .on_get(&members_path(), vec![ok(&answer)])
            .on_put(vec![Step::Answer(204, String::new())])
            .on_delete(vec![Step::Answer(204, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 2, "{changes:?}");
        assert!(changes[0]
            .to_string()
            .ends_with("(not a member) -> (member)"));
        assert!(changes[1]
            .to_string()
            .ends_with("(member) -> (not on the list)"));
        task.write(&t, &current).unwrap();
        assert_eq!(
            t.written.borrow()[0].0,
            format!("/Collections/{COLLECTION}/Items?ids={}", wanted[0])
        );
        assert_eq!(
            t.deleted.borrow()[0],
            format!("/Collections/{COLLECTION}/Items?ids={stranger}")
        );
    }

    #[test]
    fn an_unknown_list_or_library_is_an_error_before_any_write() {
        let source = radarr();
        let task = task(&source, "Gibt es nicht");
        let t = jellyfin(BOXSETS);
        let err = task.read(&t).err().unwrap().to_string();
        assert!(err.contains("import list Gibt es nicht"), "{err}");
        assert!(t.written.borrow().is_empty());

        let mut task = self::task(&source, NAME);
        task.collections[0].library = "Keine".to_string();
        let err = task.read(&t).err().unwrap().to_string();
        assert!(err.contains("library Keine"), "{err}");
    }

    /// A list that holds nothing would empty the collection: an error.
    #[test]
    fn an_empty_list_is_an_error_not_an_empty_collection() {
        let source = FakeTransport::default()
            .on_get("/api/v3/importlist", vec![ok(LISTS)])
            .on_get("/api/v3/importlist/movie", vec![ok("[]")]);
        let task = task(&source, NAME);
        let (boxsets, members) = with_collection(&wanted_ids());
        let t = jellyfin(&boxsets).on_get(&members_path(), vec![ok(&members)]);
        let err = task.read(&t).err().unwrap().to_string();
        assert!(err.contains("50 politisch linke Filme"), "{err}");
    }

    #[test]
    fn two_collections_of_one_name_are_an_error() {
        let source = radarr();
        let task = task(&source, NAME);
        let (boxsets, _) = with_collection(&[]);
        let mut twice: Value = serde_json::from_str(&boxsets).unwrap();
        let again = twice["Items"].as_array().unwrap().last().unwrap().clone();
        twice["Items"].as_array_mut().unwrap().push(again);
        let t = jellyfin(&twice.to_string());
        let err = task.read(&t).err().unwrap().to_string();
        assert!(err.contains("2 collections named"), "{err}");
    }

    /// The case measured on the host: the create times out, the read-back
    /// finds the collection. Through the engine: changed, not an error. A
    /// refused connection stays an error.
    #[test]
    fn a_create_that_times_out_is_decided_by_the_read_back() {
        use crate::{engine, testing::FakeClock};
        let source = radarr();
        let task = task(&source, NAME);
        let (boxsets, members) = with_collection(&wanted_ids());
        let world = |write: Step| {
            FakeTransport::default()
                .on_get("/System/Info", vec![ok(r#"{"Version":"10.11.11"}"#)])
                .on_get("/Library/VirtualFolders", vec![ok(FOLDERS)])
                .on_get(BOXSETS_PATH, vec![ok(BOXSETS), ok(&boxsets)])
                .on_get(FILMS_PATH, vec![ok(FILMS)])
                .on_get(&members_path(), vec![ok(&members)])
                .on_put(vec![write])
        };
        let t = world(Step::TimedOut);
        let report = engine::run(
            engine::Mode::Apply,
            &task,
            &t,
            &FakeClock::new(),
            engine::Timing::default(),
        );
        assert!(report.is_ok(), "{:?}", report.err());
        assert_eq!(t.written.borrow().len(), 1);

        let t = world(Step::Refused);
        let report = engine::run(
            engine::Mode::Apply,
            &task,
            &t,
            &FakeClock::new(),
            engine::Timing::default(),
        );
        assert!(report.is_err());
    }

    #[test]
    fn a_query_value_is_percent_encoded() {
        assert_eq!(query_value("a b,c&d/ä"), "a%20b%2Cc%26d%2F%C3%A4");
    }
}
