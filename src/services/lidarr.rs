//! Lidarr's quality and metadata profiles: what each named profile allows.
//!
//! Lidarr ships three quality profiles (`Any`, `Lossless`, `Standard`) and two
//! metadata ones (`Standard`, `None`), and a root folder points at them by
//! name (design §11). What such a name *means* is the factory default until
//! somebody writes it down -- "not set" is no statement about behaviour, the
//! lesson Ghostfolio's `ENABLE_FEATURE_AUTH_TOKEN` taught the host on
//! 2026-09-10. These two tasks declare the contents, and converge creates no
//! profile: a name the service does not have is an error.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    client::{expect_status, expect_status_at, Transport},
    endpoint::{Endpoint, Shape},
    engine::{Change, Probe, Task},
    error::Error,
    services::servarr,
};

pub const QUALITY_PROFILE_UPDATE: Endpoint = Endpoint {
    method: "PUT",
    path: "/api/v1/qualityprofile/{id}",
    request: Some(Shape::One("QualityProfileResource")),
    response: None,
};
pub const METADATA_PROFILE_UPDATE: Endpoint = Endpoint {
    method: "PUT",
    path: "/api/v1/metadataprofile/{id}",
    request: Some(Shape::One("MetadataProfileResource")),
    response: None,
};

/// The two writes. The lists these tasks read are `servarr::V1`'s, which the
/// root folder task already declares; naming them here again would check the
/// same endpoint twice.
pub const ENDPOINTS: [Endpoint; 2] = [QUALITY_PROFILE_UPDATE, METADATA_PROFILE_UPDATE];

/// The wire types of both profiles, each named exactly like its OpenAPI
/// component. `servarr::lidarr_wire_types` hands them to `schema-check`.
pub fn wire_types() -> Vec<schemars::Schema> {
    vec![
        schemars::schema_for!(QualityProfileResource),
        schemars::schema_for!(MetadataProfileResource),
    ]
}

// --- wire types --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QualityProfileResource {
    pub id: i32,
    #[serde(default)]
    pub name: Option<String>,
    pub upgrade_allowed: bool,
    /// The rung a download need not be upgraded beyond, as an id: a single
    /// quality's `quality.id`, or a group's own `id`.
    pub cutoff: i32,
    #[serde(default)]
    pub items: Option<Vec<QualityProfileQualityItemResource>>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// One rung of a profile's ladder: a quality of its own, or a group with
/// `items` of its own. A group carries no `quality` key and a quality no
/// `name` key, and neither may gain one on the way back -- hence the
/// `skip_serializing_if` on every optional field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualityProfileQualityItemResource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<Quality>,
    /// A group's qualities. Left out of the schema on purpose: its element is
    /// this very component, so the comparison would descend into itself
    /// forever, and what it would compare there is what is compared here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub items: Option<Vec<QualityProfileQualityItemResource>>,
    pub allowed: bool,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// Lidarr's `Quality`, read for its id as well: the id is what `cutoff`
/// holds, the name is what a spec says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Quality {
    pub id: i32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetadataProfileResource {
    pub id: i32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub primary_album_types: Option<Vec<ProfilePrimaryAlbumTypeItemResource>>,
    #[serde(default)]
    pub secondary_album_types: Option<Vec<ProfileSecondaryAlbumTypeItemResource>>,
    #[serde(default)]
    pub release_statuses: Option<Vec<ProfileReleaseStatusItemResource>>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// One row of a metadata profile: something with a name that is allowed or
/// not. The three lists differ only in what that something is called -- and
/// each carries its own component name, which `schema-check` compares.
trait Choice {
    /// The field the answer carries this list under; it names the changes.
    const LIST: &'static str;
    fn name(&self) -> Option<&str>;
    fn allowed(&self) -> bool;
    fn set_allowed(&mut self, allowed: bool);
}

macro_rules! choice {
    ($item:ident, $inner:ident, $field:ident, $list:literal) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct $item {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub id: Option<i32>,
            pub $field: $inner,
            pub allowed: bool,
            #[serde(flatten)]
            #[schemars(skip)]
            pub rest: Map<String, Value>,
        }

        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        pub struct $inner {
            pub id: i32,
            #[serde(default)]
            pub name: Option<String>,
            #[serde(flatten)]
            #[schemars(skip)]
            pub rest: Map<String, Value>,
        }

        impl Choice for $item {
            const LIST: &'static str = $list;

            fn name(&self) -> Option<&str> {
                self.$field.name.as_deref()
            }

            fn allowed(&self) -> bool {
                self.allowed
            }

            fn set_allowed(&mut self, allowed: bool) {
                self.allowed = allowed;
            }
        }
    };
}

choice!(
    ProfilePrimaryAlbumTypeItemResource,
    PrimaryAlbumType,
    album_type,
    "primaryAlbumTypes"
);
choice!(
    ProfileSecondaryAlbumTypeItemResource,
    SecondaryAlbumType,
    album_type,
    "secondaryAlbumTypes"
);
choice!(
    ProfileReleaseStatusItemResource,
    ReleaseStatus,
    release_status,
    "releaseStatuses"
);

// --- what a spec declares ----------------------------------------------------

/// One quality profile: the ladder is Lidarr's, the flags on it are the
/// spec's. `allowed` names **qualities**, never groups -- a group follows the
/// qualities it holds.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityWish {
    pub upgrade_allowed: bool,
    /// A quality or a group, by name.
    pub cutoff: String,
    pub allowed: Vec<String>,
}

/// One metadata profile: what each of its three lists allows. What a list
/// does not name is not allowed -- which is how Lidarr's own `None` profile
/// is written down: three empty lists.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataWish {
    pub primary_album_types: Vec<String>,
    pub secondary_album_types: Vec<String>,
    pub release_statuses: Vec<String>,
}

fn named(names: &[String], at: &str) -> Result<(), String> {
    if let Some(index) = names.iter().position(String::is_empty) {
        return Err(format!("{at}: entry {index} is empty"));
    }
    let mut seen = BTreeSet::new();
    match names.iter().find(|n| !seen.insert(n.as_str())) {
        Some(twice) => Err(format!("{at} names {twice:?} twice")),
        None => Ok(()),
    }
}

/// Refuses a quality-profiles spec that cannot mean anything. The reason
/// alone; the caller names the spec.
pub fn check_quality_profiles(profiles: &BTreeMap<String, QualityWish>) -> Result<(), String> {
    if profiles.is_empty() {
        return Err("desired.profiles names no profile".to_string());
    }
    for (name, wish) in profiles {
        if name.is_empty() {
            return Err("desired.profiles: a profile name is empty".to_string());
        }
        let at = format!("desired.profiles.{name}");
        if wish.allowed.is_empty() {
            return Err(format!(
                "{at}.allowed names no quality, which would leave the profile nothing to grab"
            ));
        }
        named(&wish.allowed, &format!("{at}.allowed"))?;
        if wish.cutoff.is_empty() {
            return Err(format!("{at}.cutoff is empty"));
        }
    }
    Ok(())
}

/// The same for a metadata-profiles spec. An empty list is allowed here: the
/// `None` profile is three of them.
pub fn check_metadata_profiles(profiles: &BTreeMap<String, MetadataWish>) -> Result<(), String> {
    if profiles.is_empty() {
        return Err("desired.profiles names no profile".to_string());
    }
    for (name, wish) in profiles {
        if name.is_empty() {
            return Err("desired.profiles: a profile name is empty".to_string());
        }
        let at = format!("desired.profiles.{name}");
        named(
            &wish.primary_album_types,
            &format!("{at}.primary_album_types"),
        )?;
        named(
            &wish.secondary_album_types,
            &format!("{at}.secondary_album_types"),
        )?;
        named(&wish.release_statuses, &format!("{at}.release_statuses"))?;
    }
    Ok(())
}

// --- reading -----------------------------------------------------------------

fn list_endpoint(endpoint: Option<Endpoint>, what: &'static str) -> Result<Endpoint, Error> {
    endpoint.ok_or_else(|| Error::Request {
        method: "GET",
        path: format!("{what} profiles"),
        reason: "this API has no such list".to_string(),
    })
}

/// A list of profiles. An empty answer is an error, and so is a nameless
/// entry: the spec finds them by name.
fn read_list<T: for<'de> Deserialize<'de>>(
    t: &dyn Transport,
    endpoint: Endpoint,
    name_of: fn(&T) -> Option<&str>,
) -> Result<Vec<T>, Error> {
    let path = endpoint.path.to_string();
    let reply = t.get(endpoint.path)?;
    expect_status(&endpoint, &reply, &[200])?;
    let list: Vec<T> = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
        path: path.clone(),
        reason: crate::error::shape(&e),
    })?;
    if list.is_empty() {
        return Err(Error::EmptyList { path });
    }
    if let Some(index) = list.iter().position(|entry| name_of(entry).is_none()) {
        return Err(Error::MissingName { path, index });
    }
    Ok(list)
}

fn quality_name(profile: &QualityProfileResource) -> Option<&str> {
    profile.name.as_deref()
}

fn metadata_name(profile: &MetadataProfileResource) -> Option<&str> {
    profile.name.as_deref()
}

fn subject(name: Option<&str>) -> String {
    format!("profile {}", name.unwrap_or("?"))
}

fn metadata_subject(name: Option<&str>) -> String {
    format!("metadata profile {}", name.unwrap_or("?"))
}

/// What a run found wrong before it wrote anything: names the service does
/// not have, and a spec that contradicts itself.
#[derive(Default)]
struct Findings {
    missing: Vec<String>,
    refused: Vec<String>,
}

impl Findings {
    fn into_error(self) -> Option<Error> {
        if !self.missing.is_empty() {
            return Some(Error::NotFound(self.missing));
        }
        if !self.refused.is_empty() {
            return Some(Error::Mismatch(self.refused));
        }
        None
    }
}

/// The spec's profiles against the service's, by name. A profile the service
/// does not have is a finding: converge creates none.
fn pair<'a, T, W>(
    current: &'a [T],
    profiles: &'a BTreeMap<String, W>,
    name_of: fn(&T) -> Option<&str>,
    subject_of: fn(Option<&str>) -> String,
    found: &mut Findings,
) -> Vec<(&'a T, &'a W)> {
    let mut pairs = Vec::new();
    for (name, wish) in profiles {
        match current.iter().find(|p| name_of(p) == Some(name.as_str())) {
            Some(profile) => pairs.push((profile, wish)),
            None => found.missing.push(format!(
                "{}, which converge does not create",
                subject_of(Some(name))
            )),
        }
    }
    pairs
}

// --- quality-profiles --------------------------------------------------------

pub struct QualityProfiles {
    pub profiles: BTreeMap<String, QualityWish>,
}

fn item_name(item: &QualityProfileQualityItemResource) -> Option<&str> {
    match &item.quality {
        Some(quality) => quality.name.as_deref(),
        None => item.name.as_deref(),
    }
}

/// Every quality of these rungs, however deep it sits.
fn qualities<'a>(items: &'a [QualityProfileQualityItemResource], into: &mut Vec<&'a str>) {
    for item in items {
        match &item.quality {
            Some(quality) => into.extend(quality.name.as_deref()),
            None => qualities(item.items.as_deref().unwrap_or_default(), into),
        }
    }
}

/// Sets `allowed` from the named qualities and answers whether the rung ends
/// up allowed: a quality when the spec names it, a group when one of its
/// qualities is allowed.
fn allow(item: &mut QualityProfileQualityItemResource, wanted: &BTreeSet<&str>) -> bool {
    item.allowed = match &item.quality {
        Some(quality) => quality.name.as_deref().is_some_and(|n| wanted.contains(n)),
        None => {
            let mut any = false;
            for child in item.items.iter_mut().flatten() {
                any |= allow(child, wanted);
            }
            any
        }
    };
    item.allowed
}

/// The rungs of that name, each with the id `cutoff` would hold for it.
fn rungs<'a>(
    items: &'a [QualityProfileQualityItemResource],
    name: &str,
) -> Vec<(i32, &'a QualityProfileQualityItemResource)> {
    let mut found = Vec::new();
    for item in items {
        match (&item.quality, item.id) {
            (Some(quality), _) if quality.name.as_deref() == Some(name) => {
                found.push((quality.id, item));
            }
            (None, Some(id)) if item.name.as_deref() == Some(name) => found.push((id, item)),
            _ => {}
        }
        if item.quality.is_none() {
            found.extend(rungs(item.items.as_deref().unwrap_or_default(), name));
        }
    }
    found
}

fn named_by_id(items: &[QualityProfileQualityItemResource], id: i32) -> Vec<String> {
    let mut found = Vec::new();
    for item in items {
        let hit = match (&item.quality, item.id) {
            (Some(quality), _) => quality.id == id,
            (None, own) => own == Some(id),
        };
        if hit {
            found.push(item_name(item).unwrap_or("?").to_string());
        }
        if item.quality.is_none() {
            found.extend(named_by_id(item.items.as_deref().unwrap_or_default(), id));
        }
    }
    found
}

/// The name a cutoff id stands for. An id that matches no rung, or more than
/// one, is shown as itself rather than guessed at.
fn cutoff_name(items: &[QualityProfileQualityItemResource], id: i32) -> String {
    match named_by_id(items, id).as_slice() {
        [only] => only.clone(),
        _ => format!("id {id}"),
    }
}

/// One change per rung whose `allowed` differs, the group before its own
/// qualities.
fn flags(
    subject: &str,
    before: &[QualityProfileQualityItemResource],
    after: &[QualityProfileQualityItemResource],
    changes: &mut Vec<Change>,
) {
    for (was, is) in before.iter().zip(after) {
        if was.allowed != is.allowed {
            changes.push(Change {
                subject: subject.to_string(),
                field: format!("items.{}.allowed", item_name(was).unwrap_or("?")),
                current: was.allowed.to_string(),
                desired: is.allowed.to_string(),
            });
        }
        flags(
            subject,
            was.items.as_deref().unwrap_or_default(),
            is.items.as_deref().unwrap_or_default(),
            changes,
        );
    }
}

impl QualityProfiles {
    /// The profile as the spec wants it, or `None` with what stands in the
    /// way written into `found`.
    fn corrected(
        profile: &QualityProfileResource,
        wish: &QualityWish,
        found: &mut Findings,
    ) -> Option<QualityProfileResource> {
        let subject = subject(profile.name.as_deref());
        let mut known = Vec::new();
        qualities(profile.items.as_deref().unwrap_or_default(), &mut known);
        let known: BTreeSet<&str> = known.into_iter().collect();
        let unknown: Vec<&String> = wish
            .allowed
            .iter()
            .filter(|name| !known.contains(name.as_str()))
            .collect();
        if !unknown.is_empty() {
            for name in unknown {
                found
                    .missing
                    .push(format!("{subject}: it has no quality {name:?}"));
            }
            return None;
        }
        let wanted: BTreeSet<&str> = wish.allowed.iter().map(String::as_str).collect();
        let mut fixed = profile.clone();
        for item in fixed.items.iter_mut().flatten() {
            allow(item, &wanted);
        }
        let cutoff =
            match rungs(fixed.items.as_deref().unwrap_or_default(), &wish.cutoff).as_slice() {
                [(id, item)] if item.allowed => *id,
                [(_, _)] => {
                    // Lidarr refuses a cutoff nothing allows, and a spec that
                    // says both things means neither.
                    found.refused.push(format!(
                        "{subject}: cutoff {} is not one of the allowed qualities",
                        wish.cutoff
                    ));
                    return None;
                }
                [] => {
                    found.missing.push(format!(
                        "{subject}: it has no quality and no group {:?} for the cutoff",
                        wish.cutoff
                    ));
                    return None;
                }
                _ => {
                    found.refused.push(format!(
                        "{subject}: {:?} names more than one rung, so the cutoff is ambiguous",
                        wish.cutoff
                    ));
                    return None;
                }
            };
        fixed.cutoff = cutoff;
        fixed.upgrade_allowed = wish.upgrade_allowed;
        Some(fixed)
    }

    fn changes(before: &QualityProfileResource, after: &QualityProfileResource) -> Vec<Change> {
        let subject = subject(before.name.as_deref());
        let mut changes = Vec::new();
        if before.upgrade_allowed != after.upgrade_allowed {
            changes.push(Change {
                subject: subject.clone(),
                field: "upgradeAllowed".to_string(),
                current: before.upgrade_allowed.to_string(),
                desired: after.upgrade_allowed.to_string(),
            });
        }
        if before.cutoff != after.cutoff {
            let items = before.items.as_deref().unwrap_or_default();
            changes.push(Change {
                subject: subject.clone(),
                field: "cutoff".to_string(),
                current: cutoff_name(items, before.cutoff),
                desired: cutoff_name(items, after.cutoff),
            });
        }
        flags(
            &subject,
            before.items.as_deref().unwrap_or_default(),
            after.items.as_deref().unwrap_or_default(),
            &mut changes,
        );
        changes
    }

    /// The profiles that differ, or everything that is wrong with the spec.
    fn pending(
        &self,
        current: &[QualityProfileResource],
    ) -> Result<Vec<QualityProfileResource>, Error> {
        let mut found = Findings::default();
        let mut writes = Vec::new();
        for (profile, wish) in pair(current, &self.profiles, quality_name, subject, &mut found) {
            if let Some(fixed) = Self::corrected(profile, wish, &mut found) {
                if !Self::changes(profile, &fixed).is_empty() {
                    writes.push(fixed);
                }
            }
        }
        // Nothing is written while anything at all is wrong: half a list of
        // profiles is worse than none.
        match found.into_error() {
            Some(e) => Err(e),
            None => Ok(writes),
        }
    }
}

impl Task for QualityProfiles {
    type Current = Vec<QualityProfileResource>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        servarr::probe(t, &servarr::V1)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let endpoint = list_endpoint(servarr::V1.quality_profiles, "quality")?;
        read_list(t, endpoint, quality_name)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut found = Findings::default();
        let mut changes = Vec::new();
        for (profile, wish) in pair(current, &self.profiles, quality_name, subject, &mut found) {
            if let Some(fixed) = Self::corrected(profile, wish, &mut found) {
                changes.extend(Self::changes(profile, &fixed));
            }
        }
        match found.into_error() {
            Some(e) => Err(e),
            None => Ok(changes),
        }
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        current
            .iter()
            .filter_map(quality_name)
            .filter(|name| !self.profiles.contains_key(*name))
            .map(|name| format!("not in the spec: {}", subject(Some(name))))
            .collect()
    }

    /// One `PUT` per differing profile: the whole profile as the service sent
    /// it, with the flags the spec sets. A profile that already agrees is not
    /// written.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for fixed in self.pending(current)? {
            let path = QUALITY_PROFILE_UPDATE
                .path
                .replace("{id}", &fixed.id.to_string());
            let body = serde_json::to_string(&fixed).map_err(|e| Error::Request {
                method: QUALITY_PROFILE_UPDATE.method,
                path: path.clone(),
                reason: format!("cannot serialize: {e}"),
            })?;
            let reply = t.put_json(&path, &body)?;
            // 202 as everywhere in Servarr: accepted, not saved -- the engine
            // reads back.
            expect_status_at(QUALITY_PROFILE_UPDATE.method, &path, &reply, &[200, 202])?;
        }
        Ok(())
    }
}

// --- metadata-profiles -------------------------------------------------------

pub struct MetadataProfiles {
    pub profiles: BTreeMap<String, MetadataWish>,
}

/// Sets `allowed` on every row of one list from the names the spec gives, and
/// reports the names that list does not have.
fn choose<T: Choice>(
    subject: &str,
    rows: &mut [T],
    wanted: &[String],
    found: &mut Findings,
) -> bool {
    let known: BTreeSet<&str> = rows.iter().filter_map(Choice::name).collect();
    let unknown: Vec<&String> = wanted
        .iter()
        .filter(|name| !known.contains(name.as_str()))
        .collect();
    if !unknown.is_empty() {
        for name in unknown {
            found
                .missing
                .push(format!("{subject}: {} has no entry {name:?}", T::LIST));
        }
        return false;
    }
    for row in rows {
        let allowed = row.name().is_some_and(|n| wanted.iter().any(|w| w == n));
        row.set_allowed(allowed);
    }
    true
}

/// One change per row whose `allowed` differs, named by its list.
fn chosen<T: Choice>(subject: &str, before: &[T], after: &[T], changes: &mut Vec<Change>) {
    for (was, is) in before.iter().zip(after) {
        if was.allowed() != is.allowed() {
            changes.push(Change {
                subject: subject.to_string(),
                field: format!("{}.{}.allowed", T::LIST, was.name().unwrap_or("?")),
                current: was.allowed().to_string(),
                desired: is.allowed().to_string(),
            });
        }
    }
}

impl MetadataProfiles {
    fn corrected(
        profile: &MetadataProfileResource,
        wish: &MetadataWish,
        found: &mut Findings,
    ) -> Option<MetadataProfileResource> {
        let subject = metadata_subject(profile.name.as_deref());
        let mut fixed = profile.clone();
        // Every list, even after one of them failed: a run says everything
        // that is wrong, not the first thing.
        let mut whole = choose(
            &subject,
            fixed.primary_album_types.as_deref_mut().unwrap_or_default(),
            &wish.primary_album_types,
            found,
        );
        whole &= choose(
            &subject,
            fixed
                .secondary_album_types
                .as_deref_mut()
                .unwrap_or_default(),
            &wish.secondary_album_types,
            found,
        );
        whole &= choose(
            &subject,
            fixed.release_statuses.as_deref_mut().unwrap_or_default(),
            &wish.release_statuses,
            found,
        );
        whole.then_some(fixed)
    }

    fn changes(before: &MetadataProfileResource, after: &MetadataProfileResource) -> Vec<Change> {
        let subject = metadata_subject(before.name.as_deref());
        let mut changes = Vec::new();
        chosen(
            &subject,
            before.primary_album_types.as_deref().unwrap_or_default(),
            after.primary_album_types.as_deref().unwrap_or_default(),
            &mut changes,
        );
        chosen(
            &subject,
            before.secondary_album_types.as_deref().unwrap_or_default(),
            after.secondary_album_types.as_deref().unwrap_or_default(),
            &mut changes,
        );
        chosen(
            &subject,
            before.release_statuses.as_deref().unwrap_or_default(),
            after.release_statuses.as_deref().unwrap_or_default(),
            &mut changes,
        );
        changes
    }

    fn pending(
        &self,
        current: &[MetadataProfileResource],
    ) -> Result<Vec<MetadataProfileResource>, Error> {
        let mut found = Findings::default();
        let mut writes = Vec::new();
        for (profile, wish) in pair(
            current,
            &self.profiles,
            metadata_name,
            metadata_subject,
            &mut found,
        ) {
            if let Some(fixed) = Self::corrected(profile, wish, &mut found) {
                if !Self::changes(profile, &fixed).is_empty() {
                    writes.push(fixed);
                }
            }
        }
        match found.into_error() {
            Some(e) => Err(e),
            None => Ok(writes),
        }
    }
}

impl Task for MetadataProfiles {
    type Current = Vec<MetadataProfileResource>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        servarr::probe(t, &servarr::V1)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let endpoint = list_endpoint(servarr::V1.metadata_profiles, "metadata")?;
        read_list(t, endpoint, metadata_name)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut found = Findings::default();
        let mut changes = Vec::new();
        for (profile, wish) in pair(
            current,
            &self.profiles,
            metadata_name,
            metadata_subject,
            &mut found,
        ) {
            if let Some(fixed) = Self::corrected(profile, wish, &mut found) {
                changes.extend(Self::changes(profile, &fixed));
            }
        }
        match found.into_error() {
            Some(e) => Err(e),
            None => Ok(changes),
        }
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        current
            .iter()
            .filter_map(metadata_name)
            .filter(|name| !self.profiles.contains_key(*name))
            .map(|name| format!("not in the spec: {}", metadata_subject(Some(name))))
            .collect()
    }

    /// The whole profile back to its own id, with the three lists set.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for fixed in self.pending(current)? {
            let path = METADATA_PROFILE_UPDATE
                .path
                .replace("{id}", &fixed.id.to_string());
            let body = serde_json::to_string(&fixed).map_err(|e| Error::Request {
                method: METADATA_PROFILE_UPDATE.method,
                path: path.clone(),
                reason: format!("cannot serialize: {e}"),
            })?;
            let reply = t.put_json(&path, &body)?;
            expect_status_at(METADATA_PROFILE_UPDATE.method, &path, &reply, &[200, 202])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::{
        engine::Task,
        testing::{ok, FakeTransport, Step},
    };

    const QUALITY: &str =
        include_str!("../../tests/fixtures/lidarr-3.1.0.4875/qualityprofile.json");
    const METADATA: &str =
        include_str!("../../tests/fixtures/lidarr-3.1.0.4875/metadataprofile.json");

    /// Profile `Standard` exactly as Lidarr holds it (recorded 2026-09-22).
    const STANDARD: &str = r#"{
        "upgrade_allowed": false,
        "cutoff": "Low Quality Lossy",
        "allowed": ["MP3-192", "OGG Vorbis Q6", "AAC-192", "WMA", "MP3-224",
                    "OGG Vorbis Q7", "MP3-VBR-V2", "MP3-256", "OGG Vorbis Q8", "AAC-256",
                    "MP3-VBR-V0", "AAC-VBR", "MP3-320", "OGG Vorbis Q9", "AAC-320",
                    "OGG Vorbis Q10"]
    }"#;

    /// Metadata profile `Standard`, likewise.
    const META_STANDARD: &str = r#"{
        "primary_album_types": ["Album"],
        "secondary_album_types": ["Studio"],
        "release_statuses": ["Official"]
    }"#;

    fn quality_task(profiles: &[(&str, &str)]) -> QualityProfiles {
        QualityProfiles {
            profiles: profiles
                .iter()
                .map(|(name, wish)| ((*name).to_string(), serde_json::from_str(wish).unwrap()))
                .collect(),
        }
    }

    fn metadata_task(profiles: &[(&str, &str)]) -> MetadataProfiles {
        MetadataProfiles {
            profiles: profiles
                .iter()
                .map(|(name, wish)| ((*name).to_string(), serde_json::from_str(wish).unwrap()))
                .collect(),
        }
    }

    fn quality_listing(body: &str) -> FakeTransport {
        FakeTransport::default().on_get("/api/v1/qualityprofile", vec![ok(body)])
    }

    fn metadata_listing(body: &str) -> FakeTransport {
        FakeTransport::default().on_get("/api/v1/metadataprofile", vec![ok(body)])
    }

    /// The spec with `name` taken out of `Standard`'s allowed qualities.
    fn standard_without(name: &str) -> String {
        let mut wish: Value = serde_json::from_str(STANDARD).unwrap();
        wish["allowed"]
            .as_array_mut()
            .unwrap()
            .retain(|q| q != name);
        wish.to_string()
    }

    /// The recorded profile of that name, as JSON.
    fn recorded(body: &str, name: &str) -> Value {
        serde_json::from_str::<Value>(body)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap()
            .clone()
    }

    // --- quality profiles ---------------------------------------------------

    /// The spec written from the recording changes nothing -- which is the
    /// point of a guard: it pins what is already there.
    #[test]
    fn the_recorded_profile_is_what_the_spec_says() {
        let task = quality_task(&[("Standard", STANDARD)]);
        let current = task.read(&quality_listing(QUALITY)).unwrap();
        assert_eq!(current.len(), 3);
        assert_eq!(task.diff(&current).unwrap(), vec![]);
    }

    /// All three, among them the two whose cutoff is a group and a quality.
    #[test]
    fn all_three_recorded_profiles_can_be_declared() {
        let any = r#"{"upgrade_allowed": false, "cutoff": "Unknown",
            "allowed": ["Unknown", "MP3-8", "MP3-16", "MP3-24", "MP3-32", "MP3-40", "MP3-48",
                        "MP3-56", "MP3-64", "MP3-80", "MP3-96", "MP3-112", "MP3-128",
                        "OGG Vorbis Q5", "MP3-160", "MP3-192", "OGG Vorbis Q6", "AAC-192",
                        "WMA", "MP3-224", "OGG Vorbis Q7", "MP3-VBR-V2", "MP3-256",
                        "OGG Vorbis Q8", "AAC-256", "MP3-VBR-V0", "AAC-VBR", "MP3-320",
                        "OGG Vorbis Q9", "AAC-320", "OGG Vorbis Q10", "FLAC", "ALAC", "APE",
                        "WavPack", "FLAC 24bit", "ALAC 24bit"]}"#;
        let lossless = r#"{"upgrade_allowed": false, "cutoff": "Lossless",
            "allowed": ["FLAC", "ALAC", "APE", "WavPack", "FLAC 24bit", "ALAC 24bit"]}"#;
        let task = quality_task(&[("Any", any), ("Lossless", lossless), ("Standard", STANDARD)]);
        let current = task.read(&quality_listing(QUALITY)).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(task.notes(&current), Vec::<String>::new());
    }

    #[test]
    fn a_quality_the_spec_leaves_out_is_one_change_and_one_put() {
        let task = quality_task(&[("Standard", &standard_without("MP3-320"))]);
        let transport = quality_listing(QUALITY).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "profile Standard: items.MP3-320.allowed true -> false"
        );

        task.write(&transport, &current).unwrap();
        let written = transport.written.borrow();
        assert_eq!(written.len(), 1, "only Standard differs");
        assert_eq!(written[0].0, "/api/v1/qualityprofile/3");

        // The whole profile as it was recorded with that one flag flipped:
        // groups keep their shape, `formatItems` and the ids travel back.
        let mut expected = recorded(QUALITY, "Standard");
        for group in expected["items"].as_array_mut().unwrap() {
            for leaf in group["items"].as_array_mut().unwrap() {
                if leaf["quality"]["name"] == "MP3-320" {
                    leaf["allowed"] = false.into();
                }
            }
        }
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent, expected);
    }

    /// A group is allowed as long as one of its qualities is: take the last
    /// one away and the group goes too, in the same PUT.
    #[test]
    fn a_group_whose_last_quality_goes_is_a_change_of_its_own() {
        let high = [
            "MP3-VBR-V0",
            "AAC-VBR",
            "MP3-320",
            "OGG Vorbis Q9",
            "AAC-320",
            "OGG Vorbis Q10",
        ];
        let mut wish: Value = serde_json::from_str(STANDARD).unwrap();
        wish["allowed"]
            .as_array_mut()
            .unwrap()
            .retain(|q| !high.iter().any(|h| q == h));
        let task = quality_task(&[("Standard", &wish.to_string())]);
        let transport = quality_listing(QUALITY).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        let lines: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(lines.len(), high.len() + 1, "{lines:?}");
        assert!(
            lines.contains(
                &"profile Standard: items.High Quality Lossy.allowed true -> false".to_string()
            ),
            "{lines:?}"
        );

        task.write(&transport, &current).unwrap();
        let written = transport.written.borrow();
        assert_eq!(written.len(), 1);
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let group = sent["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == "High Quality Lossy")
            .unwrap();
        assert_eq!(group["allowed"], Value::Bool(false));
        assert!(group["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|leaf| leaf["allowed"] == Value::Bool(false)));
    }

    #[test]
    fn the_cutoff_and_the_upgrade_switch_are_changes_that_name_the_rung() {
        let mut wish: Value = serde_json::from_str(STANDARD).unwrap();
        wish["cutoff"] = "MP3-320".into();
        wish["upgrade_allowed"] = true.into();
        let task = quality_task(&[("Standard", &wish.to_string())]);
        let transport = quality_listing(QUALITY).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        let lines: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            lines,
            [
                "profile Standard: upgradeAllowed false -> true",
                "profile Standard: cutoff Low Quality Lossy -> MP3-320",
            ]
        );
        task.write(&transport, &current).unwrap();
        let sent: Value = serde_json::from_str(&transport.written.borrow()[0].1).unwrap();
        // The id of the quality, not of the group it sits in.
        assert_eq!(sent["cutoff"], Value::from(4));
        assert_eq!(sent["upgradeAllowed"], Value::Bool(true));
    }

    #[test]
    fn a_quality_the_profile_does_not_know_is_an_error_and_writes_nothing() {
        let mut wish: Value = serde_json::from_str(STANDARD).unwrap();
        wish["allowed"]
            .as_array_mut()
            .unwrap()
            .push("FLAC-24".into());
        let task = quality_task(&[("Standard", &wish.to_string())]);
        let transport = quality_listing(QUALITY).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.contains("profile Standard"), "{err}");
        assert!(err.contains("FLAC-24"), "{err}");
        assert!(task.write(&transport, &current).is_err());
        assert!(transport.written.borrow().is_empty());
    }

    #[test]
    fn a_profile_lidarr_does_not_have_is_an_error_naming_it() {
        let task = quality_task(&[("Standard", STANDARD), ("Hi-Res", STANDARD)]);
        let transport = quality_listing(QUALITY).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.contains("profile Hi-Res"), "{err}");
        assert!(task.write(&transport, &current).is_err());
        assert!(transport.written.borrow().is_empty());
    }

    /// A cutoff nothing allows is a spec that contradicts itself. It is
    /// refused here, before the PUT.
    #[test]
    fn a_cutoff_outside_the_allowed_qualities_is_refused() {
        let mut wish: Value = serde_json::from_str(STANDARD).unwrap();
        wish["cutoff"] = "FLAC".into();
        let task = quality_task(&[("Standard", &wish.to_string())]);
        let transport = quality_listing(QUALITY).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.contains("cutoff FLAC"), "{err}");
        assert!(task.write(&transport, &current).is_err());
        assert!(transport.written.borrow().is_empty());
    }

    #[test]
    fn a_cutoff_the_profile_does_not_have_is_an_error() {
        let mut wish: Value = serde_json::from_str(STANDARD).unwrap();
        wish["cutoff"] = "Hi-Res Lossy".into();
        let task = quality_task(&[("Standard", &wish.to_string())]);
        let current = task.read(&quality_listing(QUALITY)).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.contains(r#""Hi-Res Lossy" for the cutoff"#), "{err}");
    }

    #[test]
    fn profiles_the_spec_does_not_name_are_a_note() {
        let task = quality_task(&[("Standard", STANDARD)]);
        let current = task.read(&quality_listing(QUALITY)).unwrap();
        assert_eq!(
            task.notes(&current),
            [
                "not in the spec: profile Any",
                "not in the spec: profile Lossless"
            ]
        );
    }

    #[test]
    fn an_empty_list_and_a_nameless_profile_are_errors() {
        let task = quality_task(&[("Standard", STANDARD)]);
        let err = task.read(&quality_listing("[]")).err().unwrap().to_string();
        assert!(err.contains("empty list"), "{err}");
        let nameless = r#"[{"id":1,"upgradeAllowed":false,"cutoff":0,"items":[]}]"#;
        let err = task
            .read(&quality_listing(nameless))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("has no name"), "{err}");
    }

    /// A refused write names the path and the status -- and nothing the
    /// service put in its body.
    #[test]
    fn a_refused_write_names_the_path_and_nothing_from_the_body() {
        let task = quality_task(&[("Standard", &standard_without("MP3-320"))]);
        let transport = quality_listing(QUALITY)
            .on_put(vec![Step::Answer(500, "ERFUNDENES-GEHEIMNIS-XYZ".into())]);
        let current = task.read(&transport).unwrap();
        let err = task.write(&transport, &current).err().unwrap().to_string();
        assert_eq!(err, "PUT /api/v1/qualityprofile/3 answered HTTP 500");
    }

    // --- metadata profiles --------------------------------------------------

    #[test]
    fn the_recorded_metadata_profiles_are_what_the_spec_says() {
        let none = r#"{"primary_album_types": [], "secondary_album_types": [],
                       "release_statuses": []}"#;
        let task = metadata_task(&[("Standard", META_STANDARD), ("None", none)]);
        let current = task.read(&metadata_listing(METADATA)).unwrap();
        assert_eq!(current.len(), 2);
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(task.notes(&current), Vec::<String>::new());
    }

    #[test]
    fn one_more_album_type_is_one_change_and_one_put() {
        let mut wish: Value = serde_json::from_str(META_STANDARD).unwrap();
        wish["primary_album_types"]
            .as_array_mut()
            .unwrap()
            .push("EP".into());
        let task = metadata_task(&[("Standard", &wish.to_string())]);
        let transport = metadata_listing(METADATA).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "metadata profile Standard: primaryAlbumTypes.EP.allowed false -> true"
        );

        task.write(&transport, &current).unwrap();
        let written = transport.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/v1/metadataprofile/1");
        let mut expected = recorded(METADATA, "Standard");
        for item in expected["primaryAlbumTypes"].as_array_mut().unwrap() {
            if item["albumType"]["name"] == "EP" {
                item["allowed"] = true.into();
            }
        }
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent, expected);
    }

    #[test]
    fn a_release_status_and_a_secondary_type_are_named_by_their_list() {
        let mut wish: Value = serde_json::from_str(META_STANDARD).unwrap();
        wish["secondary_album_types"] = serde_json::json!([]);
        wish["release_statuses"] = serde_json::json!(["Official", "Promotion"]);
        let task = metadata_task(&[("Standard", &wish.to_string())]);
        let current = task.read(&metadata_listing(METADATA)).unwrap();
        let lines: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            lines,
            [
                "metadata profile Standard: secondaryAlbumTypes.Studio.allowed true -> false",
                "metadata profile Standard: releaseStatuses.Promotion.allowed false -> true",
            ]
        );
    }

    #[test]
    fn an_unknown_type_name_is_an_error_and_writes_nothing() {
        let mut wish: Value = serde_json::from_str(META_STANDARD).unwrap();
        wish["secondary_album_types"]
            .as_array_mut()
            .unwrap()
            .push("Bootleg".into());
        let task = metadata_task(&[("Standard", &wish.to_string())]);
        let transport = metadata_listing(METADATA).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.contains("Bootleg"), "{err}");
        assert!(err.contains("secondaryAlbumTypes"), "{err}");
        assert!(task.write(&transport, &current).is_err());
        assert!(transport.written.borrow().is_empty());
    }

    #[test]
    fn a_metadata_profile_lidarr_does_not_have_is_an_error_naming_it() {
        let task = metadata_task(&[("Bootlegs", META_STANDARD)]);
        let transport = metadata_listing(METADATA).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.contains("metadata profile Bootlegs"), "{err}");
        assert!(transport.written.borrow().is_empty());
    }

    #[test]
    fn a_metadata_profile_the_spec_does_not_name_is_a_note() {
        let task = metadata_task(&[("Standard", META_STANDARD)]);
        let current = task.read(&metadata_listing(METADATA)).unwrap();
        assert_eq!(
            task.notes(&current),
            ["not in the spec: metadata profile None"]
        );
    }

    #[test]
    fn an_empty_metadata_list_is_an_error() {
        let task = metadata_task(&[("Standard", META_STANDARD)]);
        let err = task
            .read(&metadata_listing("[]"))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("empty list"), "{err}");
    }

    // --- specs, wire types, probe -------------------------------------------

    #[test]
    fn a_spec_that_cannot_mean_anything_is_refused() {
        let quality = |wish: &str| {
            let profiles = BTreeMap::from([("Standard".to_string(), from(wish))]);
            check_quality_profiles(&profiles).unwrap_err()
        };
        fn from<T: for<'de> Deserialize<'de>>(text: &str) -> T {
            serde_json::from_str(text).unwrap()
        }
        assert!(
            quality(r#"{"upgrade_allowed": false, "cutoff": "FLAC", "allowed": []}"#)
                .contains("names no quality"),
        );
        assert!(
            quality(r#"{"upgrade_allowed": false, "cutoff": "", "allowed": ["FLAC"]}"#)
                .contains("cutoff is empty")
        );
        assert!(quality(
            r#"{"upgrade_allowed": false, "cutoff": "FLAC", "allowed": ["FLAC", "FLAC"]}"#
        )
        .contains(r#"names "FLAC" twice"#));
        assert_eq!(
            check_quality_profiles(&BTreeMap::new()).unwrap_err(),
            "desired.profiles names no profile"
        );

        // Three empty lists are a metadata profile, not an error: Lidarr's
        // own `None` is exactly that.
        let none: MetadataWish = from(
            r#"{"primary_album_types": [], "secondary_album_types": [], "release_statuses": []}"#,
        );
        let profiles = BTreeMap::from([("None".to_string(), none)]);
        assert_eq!(check_metadata_profiles(&profiles), Ok(()));
        let twice: MetadataWish = from(
            r#"{"primary_album_types": ["Album", "Album"], "secondary_album_types": [],
                "release_statuses": []}"#,
        );
        let profiles = BTreeMap::from([("Standard".to_string(), twice)]);
        assert!(check_metadata_profiles(&profiles)
            .unwrap_err()
            .contains(r#"primary_album_types names "Album" twice"#));
    }

    #[test]
    fn the_wire_types_carry_the_component_names() {
        let titles: Vec<String> = wire_types()
            .iter()
            .map(|s| s.as_value()["title"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            titles,
            ["QualityProfileResource", "MetadataProfileResource"]
        );
    }

    /// Both readiness probes go through the Servarr status endpoint.
    #[test]
    fn the_probe_is_the_servarr_one() {
        const STATUS: &str =
            include_str!("../../tests/fixtures/lidarr-3.1.0.4875/system-status.json");
        let t = FakeTransport::default().on_get("/api/v1/system/status", vec![ok(STATUS)]);
        assert_eq!(
            quality_task(&[("Standard", STANDARD)]).probe(&t).ok(),
            Some("3.1.0.4875".to_string())
        );
        assert_eq!(
            metadata_task(&[("Standard", META_STANDARD)]).probe(&t).ok(),
            Some("3.1.0.4875".to_string())
        );
    }
}
