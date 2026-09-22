//! Koel (9.11): radio stations (design §18). Koel publishes an OpenAPI
//! description (`api-docs/api.yaml`) that still says version 5.1.0 and
//! describes neither radio stations nor anything else this module uses, so
//! field names come from Koel's source (`RadioStationResource`, the store and
//! update requests) and from recorded answers, as for Seerr (§17).
//!
//! The token is a Sanctum token that travels as `Authorization: Bearer`, and
//! every request says `Accept: application/json`: without it, Koel answers a
//! refused request with a redirect to its web page, which the HTTP agent
//! follows into an HTML answer with HTTP 200. An answer that is not JSON is
//! therefore an error, never an empty list.

use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    client::{expect_status_at, Transport},
    engine::{shortened, Change, Probe, Task},
    error::Error,
};

pub const STATIONS: &str = "/api/radio/stations";
pub const ME: &str = "/api/me";

/// The largest logo file converge sends: 2 MiB (design §18).
pub const LOGO_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Koel tells its version only in `GET /api/data`, which creates a queue
/// row for the account on its first call; it is not asked.
pub const VERSION_NOT_REPORTED: &str = "(not reported)";

/// A logo, read from a file and ready to send. `data_uri` is never printed;
/// changes name the file.
#[derive(Clone)]
pub struct Logo {
    pub file: String,
    pub data_uri: String,
}

/// One station as the spec wants it.
#[derive(Clone)]
pub struct StationTarget {
    pub name: String,
    pub url: String,
    pub description: String,
    pub is_public: bool,
    pub homepage_url: Option<String>,
    pub logo: Option<Logo>,
}

pub struct RadioStations {
    pub stations: Vec<StationTarget>,
    /// The spec names every station the account should have: its other ones
    /// are removed (design §39). False unless the spec says
    /// `"exactly": true`. It can only ever reach the account's **own**
    /// stations, because `read` refuses to run while the list holds
    /// anybody else's (`check_only_own_stations_listed`).
    pub exactly: bool,
}

/// A station as `GET /api/radio/stations` answers it
/// (`app/Http/Resources/RadioStationResource.php`). Only the fields read are
/// declared; nothing read is written back, since a write carries the whole
/// entry from the spec.
#[derive(Debug, Clone, Deserialize)]
pub struct RadioStationResource {
    pub id: String,
    pub name: String,
    pub url: String,
    /// `''` for a station created through the API without one; `null` is
    /// possible for rows created otherwise, and Koel's update turns `null`
    /// into `''` -- the same value here.
    #[serde(default)]
    pub description: Option<String>,
    pub is_public: bool,
    #[serde(default)]
    pub homepage_url: Option<String>,
    /// A URL of the stored image, or `null`.
    #[serde(default)]
    pub logo: Option<String>,
}

/// Decodes the station list. An empty body or an HTML page is an error that
/// says so; `[]` is a list without stations.
pub fn decode_stations(path: &str, body: &str) -> Result<Vec<RadioStationResource>, Error> {
    let decode = |reason: String| Error::Decode {
        path: path.to_string(),
        reason,
    };
    let value: Value = serde_json::from_str(body).map_err(|e| {
        decode(format!(
            "not JSON ({}) -- a web page instead of the API? Koel needs Accept: application/json",
            crate::error::shape(&e)
        ))
    })?;
    let Value::Array(entries) = value else {
        return Err(decode("not a list".to_string()));
    };
    entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let Some(name) = entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return Err(Error::MissingName {
                    path: path.to_string(),
                    index,
                });
            };
            serde_json::from_value(entry)
                .map_err(|e| decode(format!("station {name}: {}", crate::error::shape(&e))))
        })
        .collect()
}

/// Refuses unless the token's account has `include_public_media` off.
///
/// `GET /api/radio/stations` lists the account's own stations and, with that
/// preference on (Koel's default), every public station of its organization
/// (`RadioStationBuilder::accessible`). The answer names no owner
/// (`RadioStationResource`), and `permissions.edit` is true for an
/// administrator on every station -- so a spec station whose name matches a
/// person's public station would be written onto that person's station.
/// With the preference off the list is `whereBelongsTo` the account: every
/// station in it is its own.
///
/// `GET /api/me` also answers the account's Subsonic key and e-mail address;
/// only the one field is read, and no part of the body reaches an error.
fn check_only_own_stations_listed(t: &dyn Transport) -> Result<(), Error> {
    let reply = t.get(ME)?;
    expect_status_at("GET", ME, &reply, &[200])?;
    // A syntax error from serde_json names a line and a column, never text.
    let account: Value = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
        path: ME.to_string(),
        reason: format!("not JSON ({})", crate::error::shape(&e)),
    })?;
    match account
        .pointer("/preferences/include_public_media")
        .and_then(Value::as_bool)
    {
        Some(false) => Ok(()),
        Some(true) => Err(Error::Refused(format!(
            "the account's preference include_public_media is on, so GET {STATIONS} also lists other people's public stations, and Koel does not say whose a station is -- set include_public_media to false for the account the token belongs to"
        ))),
        None => Err(Error::MissingField(vec![format!(
            "{ME}: preferences.include_public_media (a boolean)"
        )])),
    }
}

/// Readiness and the token in one request: the list itself. A refused token
/// and an answer that is not JSON stay that way however long one waits.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t.get(STATIONS).map_err(|e| Probe::NotYet(e.to_string()))?;
    match reply.status {
        200 => decode_stations(STATIONS, &reply.body)
            .map(|_| VERSION_NOT_REPORTED.to_string())
            .map_err(Probe::Fatal),
        401 | 403 => Err(Probe::Fatal(Error::Status {
            method: "GET",
            path: STATIONS.to_string(),
            status: reply.status,
            validation: vec!["the token was refused".to_string()],
        })),
        other => Err(Probe::NotYet(format!("HTTP {other}"))),
    }
}

/// The file as a `data:` URI, which Koel's `ValidImageData` rule and its
/// image storage take (an SVG goes through its sanitizer only with its own
/// type). The type comes from the content: a store path's name says little.
pub fn logo(path: &Path) -> Result<Logo, String> {
    let file = path.display().to_string();
    let unreadable = |e: std::io::Error| format!("logo_file {file} cannot be read ({})", e.kind());
    let size = std::fs::metadata(path).map_err(unreadable)?.len();
    if size > LOGO_MAX_BYTES {
        return Err(format!(
            "logo_file {file} is {size} bytes, more than the {LOGO_MAX_BYTES} (2 MiB) converge sends -- Koel scales a logo to 640 pixels wide anyway"
        ));
    }
    let bytes = std::fs::read(path).map_err(unreadable)?;
    if bytes.is_empty() {
        return Err(format!("logo_file {file} is empty"));
    }
    let text_start = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).into_owned();
    let text_start = text_start.trim_start();
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if text_start.starts_with("<svg")
        || (text_start.starts_with("<?xml") && text_start.contains("<svg"))
    {
        "image/svg+xml"
    } else {
        return Err(format!(
            "logo_file {file} is not a PNG, JPEG, GIF, WebP or SVG image"
        ));
    };
    Ok(Logo {
        file,
        data_uri: format!("data:{mime};base64,{}", base64(&bytes)),
    })
}

/// Standard base64 with padding (RFC 4648, section 4).
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn subject(name: &str) -> String {
    format!("station {name}")
}

impl RadioStations {
    /// The one station of that name, `None` when there is none. `read` has
    /// made sure the list holds only the account's own stations
    /// (`check_only_own_stations_listed`); two of them with one name are an
    /// error, since nothing tells them apart.
    fn find<'a>(
        current: &'a [RadioStationResource],
        name: &str,
    ) -> Result<Option<&'a RadioStationResource>, String> {
        let mut found = current.iter().filter(|s| s.name == name);
        match (found.next(), found.count()) {
            (None, _) => Ok(None),
            (Some(station), 0) => Ok(Some(station)),
            (Some(_), more) => Err(format!(
                "Koel has {} stations named {name} and converge cannot tell which one is meant",
                more + 1
            )),
        }
    }

    fn changes_of(target: &StationTarget, station: &RadioStationResource) -> Vec<Change> {
        let subject = subject(&target.name);
        let change = |field: &str, current: Value, desired: Value| Change {
            subject: subject.clone(),
            field: field.to_string(),
            current: shortened(&current),
            desired: shortened(&desired),
        };
        let mut changes = Vec::new();
        if station.url != target.url {
            changes.push(change("url", json!(station.url), json!(target.url)));
        }
        let description = station.description.as_deref().unwrap_or("");
        if description != target.description {
            changes.push(change(
                "description",
                json!(description),
                json!(target.description),
            ));
        }
        if station.is_public != target.is_public {
            changes.push(change(
                "is_public",
                json!(station.is_public),
                json!(target.is_public),
            ));
        }
        if station.homepage_url != target.homepage_url {
            changes.push(change(
                "homepage_url",
                json!(station.homepage_url),
                json!(target.homepage_url),
            ));
        }
        if let Some(logo) = Self::logo_to_send(target, Some(station)) {
            changes.push(Change {
                subject: subject.clone(),
                field: "logo".to_string(),
                current: "(none)".to_string(),
                desired: format!("(from {})", logo.file),
            });
        }
        changes
    }

    /// A logo goes out when a station is added, or when the station has
    /// none: a stored image has a random name, so no other comparison exists.
    fn logo_to_send<'a>(
        target: &'a StationTarget,
        station: Option<&RadioStationResource>,
    ) -> Option<&'a Logo> {
        let has_logo = station
            .and_then(|s| s.logo.as_deref())
            .is_some_and(|l| !l.is_empty());
        target.logo.as_ref().filter(|_| !has_logo)
    }

    /// The account's stations the spec does not name.
    fn not_in_the_spec<'a>(
        &self,
        current: &'a [RadioStationResource],
    ) -> Vec<&'a RadioStationResource> {
        current
            .iter()
            .filter(|s| !self.stations.iter().any(|t| t.name == s.name))
            .collect()
    }

    /// The whole entry: Koel's update resets what a body leaves out.
    fn body(target: &StationTarget, logo: Option<&Logo>) -> String {
        let mut body = Map::new();
        body.insert("name".to_string(), json!(target.name));
        body.insert("url".to_string(), json!(target.url));
        body.insert("description".to_string(), json!(target.description));
        body.insert("is_public".to_string(), json!(target.is_public));
        body.insert("homepage_url".to_string(), json!(target.homepage_url));
        if let Some(logo) = logo {
            body.insert("logo".to_string(), json!(logo.data_uri));
        }
        Value::Object(body).to_string()
    }
}

impl Task for RadioStations {
    type Current = Vec<RadioStationResource>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    /// The account first: only with `include_public_media` off is every
    /// station in the list the account's own (design §18). Then the list; an
    /// empty one is valid -- a fresh Koel has no station, and every one the
    /// spec names then shows up as a change.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        check_only_own_stations_listed(t)?;
        let reply = t.get(STATIONS)?;
        expect_status_at("GET", STATIONS, &reply, &[200])?;
        decode_stations(STATIONS, &reply.body)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut changes = Vec::new();
        let mut ambiguous = Vec::new();
        for target in &self.stations {
            match Self::find(current, &target.name) {
                Err(reason) => ambiguous.push(reason),
                Ok(None) => changes.push(Change {
                    subject: subject(&target.name),
                    field: String::new(),
                    current: "(missing)".to_string(),
                    desired: "(added)".to_string(),
                }),
                Ok(Some(station)) => changes.extend(Self::changes_of(target, station)),
            }
        }
        if ambiguous.is_empty() {
            Ok(changes)
        } else {
            Err(Error::Mismatch(ambiguous))
        }
    }

    /// With `exactly` nothing is left as it is, so the note would be a lie;
    /// those stations are removals instead.
    fn notes(&self, current: &Self::Current) -> Vec<String> {
        if self.exactly {
            return Vec::new();
        }
        self.not_in_the_spec(current)
            .into_iter()
            .map(|s| format!("not in the spec, left as it is: {}", subject(&s.name)))
            .collect()
    }

    /// The account's own stations the spec does not name -- and never half
    /// the list or more.
    ///
    /// Koel finds a station by its name, and a name is a string somebody
    /// typed: another Unicode normalisation on either side, and no spec
    /// station would match any of Koel's, making every one of them look
    /// like a surplus. A spec that would take away half the account's
    /// stations is therefore an error rather than a removal -- in `plan`
    /// too, and before a single `DELETE`.
    fn surplus(&self, current: &Self::Current) -> Result<Vec<String>, Error> {
        if !self.exactly {
            return Ok(Vec::new());
        }
        let surplus = self.not_in_the_spec(current);
        if !surplus.is_empty() && 2 * surplus.len() >= current.len() {
            return Err(Error::Refused(format!(
                "would remove {} of {} stations (half or more) -- nothing removed",
                surplus.len(),
                current.len()
            )));
        }
        Ok(surplus
            .into_iter()
            .map(|s| subject(&s.name))
            .collect::<Vec<_>>())
    }

    /// One `DELETE /api/radio/stations/{id}` per surplus station. Koel's
    /// controller deletes the Eloquent model, so its observer takes the
    /// stored logo file with it (design §39); the answer is 204 and empty.
    fn remove(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        if !self.exactly {
            return Ok(());
        }
        // Nothing is removed while the guard above says no.
        self.surplus(current)?;
        for station in self.not_in_the_spec(current) {
            let path = format!("{STATIONS}/{}", station.id);
            let reply = t.delete(&path)?;
            // `response()->noContent()` is 204; 200 is taken as well.
            if ![200, 204].contains(&reply.status) {
                return Err(Error::Status {
                    method: "DELETE",
                    path,
                    status: reply.status,
                    validation: vec![format!("{} was not removed", subject(&station.name))],
                });
            }
        }
        Ok(())
    }

    /// `POST` for a missing station, `PUT` with the whole entry for one that
    /// differs. converge never deletes a station.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        // Nothing is written while any name is ambiguous.
        self.diff(current)?;
        for target in &self.stations {
            match Self::find(current, &target.name).map_err(|r| Error::Mismatch(vec![r]))? {
                None => {
                    let body = Self::body(target, target.logo.as_ref());
                    let reply = t.post_json(STATIONS, &body)?;
                    expect_status_at("POST", STATIONS, &reply, &[201])?;
                }
                Some(station) => {
                    if Self::changes_of(target, station).is_empty() {
                        continue;
                    }
                    // A ULID; anything else would not be a path segment.
                    if station.id.is_empty()
                        || !station.id.chars().all(|c| c.is_ascii_alphanumeric())
                    {
                        return Err(Error::Decode {
                            path: STATIONS.to_string(),
                            reason: format!("{} has no usable id", subject(&target.name)),
                        });
                    }
                    let path = format!("{STATIONS}/{}", station.id);
                    let body = Self::body(target, Self::logo_to_send(target, Some(station)));
                    let reply = t.put_json(&path, &body)?;
                    expect_status_at("PUT", &path, &reply, &[200])?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        clock::Clock,
        engine::{run, Mode, Outcome, Timing},
        testing::{ok, FakeClock, FakeTransport, Step},
    };

    const RECORDED: &str = include_str!("../../tests/fixtures/koel-9.11.3/radio-stations.json");
    const CONSTRUCTED: &str =
        include_str!("../../tests/fixtures/koel-9.11.3/constructed-radio-stations.json");
    const UNAUTHENTICATED: &str =
        include_str!("../../tests/fixtures/koel-9.11.3/unauthenticated.json");
    const INVALID: &str =
        include_str!("../../tests/fixtures/koel-9.11.3/constructed-validation-error.json");

    /// A logo nobody would type, so a leak into output is easy to find.
    const LOGO_DATA: &str = "data:image/png;base64,bG9nby03ZjNhOWMtbmV2ZXItcHJpbnQtbWU=";

    fn rdl(logo: bool) -> StationTarget {
        StationTarget {
            name: "Radio Dreyeckland".to_string(),
            url: "https://stream.rdl.de/rdl".to_string(),
            description: "Free radio from Freiburg.".to_string(),
            is_public: true,
            homepage_url: Some("https://rdl.de/".to_string()),
            logo: logo.then(|| Logo {
                file: "/nix/store/x-rdl.png".to_string(),
                data_uri: LOGO_DATA.to_string(),
            }),
        }
    }

    fn fsk() -> StationTarget {
        StationTarget {
            name: "FSK".to_string(),
            url: "https://streaming.fueralle.org/fsk.mp3".to_string(),
            description: String::new(),
            is_public: false,
            homepage_url: None,
            logo: None,
        }
    }

    const ME_RECORDED: &str = include_str!("../../tests/fixtures/koel-9.11.3/me.json");

    /// The recorded `GET /api/me` with `include_public_media` as given.
    fn me(include_public_media: bool) -> String {
        let mut me: Value = serde_json::from_str(ME_RECORDED).unwrap();
        me["preferences"]["include_public_media"] = json!(include_public_media);
        me.to_string()
    }

    /// An account whose list holds only its own stations.
    fn koel(list: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(ME, vec![ok(&me(false))])
            .on_get(STATIONS, vec![ok(list)])
    }

    #[test]
    fn the_recorded_account_lists_public_stations_so_nothing_is_read_or_written() {
        // Recorded: the library account has the default, `true`. The list
        // then carries a person's public station, and a spec station of the
        // same name must not become a PUT onto it.
        let mut theirs = fsk();
        theirs.name = "Somebody's own".to_string();
        theirs.url = "https://example.org/other.mp3".to_string();
        let task = RadioStations {
            stations: vec![theirs],
            exactly: false,
        };
        for mode in [Mode::Apply, Mode::Plan] {
            let t = FakeTransport::default()
                .on_get(ME, vec![ok(ME_RECORDED)])
                .on_get(STATIONS, vec![ok(CONSTRUCTED)])
                .on_put(vec![ok("{}")]);
            let err = run(mode, &task, &t, &FakeClock::new(), Timing::default())
                .err()
                .unwrap()
                .to_string();
            assert_eq!(
                err,
                "refused: the account's preference include_public_media is on, so GET /api/radio/stations also lists other people's public stations, and Koel does not say whose a station is -- set include_public_media to false for the account the token belongs to"
            );
            assert!(t.written.borrow().is_empty(), "{mode:?} wrote");
        }
    }

    #[test]
    fn the_account_answer_is_checked_and_never_shown() {
        let task = RadioStations {
            stations: vec![fsk()],
            exactly: false,
        };
        let mark = "subsonic-key-7f3a9c-never-print-me";
        let mut without: Value = serde_json::from_str(ME_RECORDED).unwrap();
        without["subsonic_api_key"] = json!(mark);
        without["preferences"]
            .as_object_mut()
            .unwrap()
            .remove("include_public_media");
        let t = koel(RECORDED).on_get(ME, vec![ok(&without.to_string())]);
        let err = task.read(&t).err().unwrap().to_string();
        assert_eq!(
            err,
            "the answer has no such field: /api/me: preferences.include_public_media (a boolean)"
        );
        without["preferences"]["include_public_media"] = json!(mark);
        let t = koel(RECORDED).on_get(ME, vec![ok(&without.to_string())]);
        let err = task.read(&t).err().unwrap().to_string();
        assert!(!err.contains(mark), "{err}");

        let t = koel(RECORDED).on_get(ME, vec![ok("<!DOCTYPE html>")]);
        let err = task.read(&t).err().unwrap().to_string();
        assert!(
            err.starts_with("/api/me: the answer does not have the expected shape: not JSON"),
            "{err}"
        );
        let t = koel(RECORDED).on_get(ME, vec![Step::Answer(401, UNAUTHENTICATED.into())]);
        let err = task.read(&t).err().unwrap().to_string();
        assert_eq!(err, "GET /api/me answered HTTP 401");
    }

    fn sent(t: &FakeTransport, index: usize) -> (String, Value) {
        let written = t.written.borrow();
        (
            written[index].0.clone(),
            serde_json::from_str(&written[index].1).unwrap(),
        )
    }

    fn shown(changes: &[Change]) -> Vec<String> {
        changes.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_probe_wants_the_list_as_json_and_a_token_koel_takes() {
        assert_eq!(probe(&koel(RECORDED)).ok().unwrap(), VERSION_NOT_REPORTED);

        let t = koel("").on_get(STATIONS, vec![Step::Answer(401, UNAUTHENTICATED.into())]);
        match probe(&t) {
            Err(Probe::Fatal(e)) => assert_eq!(
                e.to_string(),
                "GET /api/radio/stations answered HTTP 401: the token was refused"
            ),
            _ => panic!("401 is fatal"),
        }
        // The web page a redirect leads to: waiting does not make it JSON.
        let t = koel("<!DOCTYPE html><html></html>");
        match probe(&t) {
            Err(Probe::Fatal(e)) => {
                let e = e.to_string();
                assert!(e.contains("not JSON"), "{e}");
                assert!(e.contains("Accept: application/json"), "{e}");
            }
            _ => panic!("an HTML answer is fatal"),
        }
        let t = FakeTransport::default().on_get(STATIONS, vec![Step::Answer(502, String::new())]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
        let t = FakeTransport::default().on_get(STATIONS, vec![Step::Refused]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    #[test]
    fn an_empty_body_is_an_error_and_an_empty_list_is_not() {
        let task = RadioStations {
            stations: vec![fsk()],
            exactly: false,
        };
        assert!(task.read(&koel(RECORDED)).unwrap().is_empty());
        let err = task.read(&koel("")).err().unwrap().to_string();
        assert!(
            err.starts_with(
                "/api/radio/stations: the answer does not have the expected shape: not JSON"
            ),
            "{err}"
        );
        let err = task.read(&koel("{}")).err().unwrap().to_string();
        assert!(err.contains("not a list"), "{err}");
        let err = task
            .read(&koel(
                r#"[{"id":"01X","url":"https://a/x","is_public":true}]"#,
            ))
            .err()
            .unwrap()
            .to_string();
        assert_eq!(err, "entry 0 of /api/radio/stations has no name");
        let err = task
            .read(&koel(r#"[{"id":"01X","name":"A","url":"https://a/x"}]"#))
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("station A: missing field `is_public`"),
            "{err}"
        );
    }

    #[test]
    fn the_recorded_empty_list_adds_every_station_with_all_its_fields() {
        let task = RadioStations {
            stations: vec![rdl(true), fsk()],
            exactly: false,
        };
        let t = koel(RECORDED).on_put(vec![Step::Answer(201, "{}".into())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(
            shown(&changes),
            [
                "station Radio Dreyeckland: (missing) -> (added)",
                "station FSK: (missing) -> (added)"
            ]
        );
        assert!(!format!("{changes:?}").contains("bG9nby"));
        task.write(&t, &current).unwrap();
        assert_eq!(t.written.borrow().len(), 2);
        let (path, body) = sent(&t, 0);
        assert_eq!(path, STATIONS);
        assert_eq!(
            body,
            json!({"name": "Radio Dreyeckland", "url": "https://stream.rdl.de/rdl",
                   "description": "Free radio from Freiburg.", "is_public": true,
                   "homepage_url": "https://rdl.de/", "logo": LOGO_DATA})
        );
        let (_, body) = sent(&t, 1);
        assert_eq!(
            body,
            json!({"name": "FSK", "url": "https://streaming.fueralle.org/fsk.mp3",
                   "description": "", "is_public": false, "homepage_url": null}),
            "no logo key without a logo file"
        );
    }

    #[test]
    fn stations_as_wanted_are_unchanged_and_the_others_only_noted() {
        let mut own = fsk();
        own.name = "Somebody's own".to_string();
        own.url = "https://radio.example.org/live.ogg".to_string();
        own.is_public = true;
        // Its description is null on the service: the same as empty.
        let task = RadioStations {
            stations: vec![rdl(true), fsk(), own],
            exactly: false,
        };
        let t = koel(CONSTRUCTED);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);

        let task = RadioStations {
            stations: vec![rdl(false)],
            exactly: false,
        };
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(
            task.notes(&current),
            [
                "not in the spec, left as it is: station FSK",
                "not in the spec, left as it is: station Somebody's own"
            ]
        );
    }

    // --- exactly (design §39) ---------------------------------------------

    /// The station of the recording the spec below does not name.
    const SURPLUS_ID: &str = "01K52Z6P7B8C9D0E1F2G3H4J5K";

    /// Two of the three recorded stations, so removing the third is under
    /// the half the guard allows.
    fn two_of_three(exactly: bool) -> RadioStations {
        RadioStations {
            stations: vec![rdl(false), fsk()],
            exactly,
        }
    }

    #[test]
    fn without_exactly_a_station_outside_the_spec_stays_a_note() {
        let task = two_of_three(false);
        let t = koel(CONSTRUCTED).on_delete(vec![Step::Answer(204, String::new())]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.surplus(&current).unwrap(), Vec::<String>::new());
        task.remove(&t, &current).unwrap();
        assert!(t.deleted.borrow().is_empty());
        assert_eq!(
            task.notes(&current),
            ["not in the spec, left as it is: station Somebody's own"]
        );
    }

    #[test]
    fn with_exactly_a_station_outside_the_spec_is_a_removal_and_plan_removes_nothing() {
        let task = two_of_three(true);
        let t = koel(CONSTRUCTED);
        let current = task.read(&t).unwrap();
        assert_eq!(task.surplus(&current).unwrap(), ["station Somebody's own"]);
        assert_eq!(task.notes(&current), Vec::<String>::new());

        let t = koel(CONSTRUCTED);
        let report = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        match report.outcome {
            Outcome::Differs(changes) => assert_eq!(
                shown(&changes),
                ["station Somebody's own: (present) -> (removed)"]
            ),
            other => panic!("expected Differs, got {other:?}"),
        }
        assert!(t.deleted.borrow().is_empty(), "plan removes nothing");
    }

    /// If the spec's names and Koel's do not line up at all -- another
    /// Unicode normalisation, say -- every station looks like a surplus.
    /// Half or more is therefore refused, before a single `DELETE`.
    #[test]
    fn removing_half_or_more_is_refused_before_anything_is_removed() {
        let task = RadioStations {
            stations: vec![rdl(false)],
            exactly: true,
        };
        for mode in [Mode::Plan, Mode::Apply] {
            let t = koel(CONSTRUCTED)
                .on_put(vec![ok("{}")])
                .on_delete(vec![Step::Answer(204, String::new())]);
            let err = run(mode, &task, &t, &FakeClock::new(), Timing::default())
                .err()
                .unwrap()
                .to_string();
            assert_eq!(
                err,
                "refused: would remove 2 of 3 stations (half or more) -- nothing removed"
            );
            assert!(t.deleted.borrow().is_empty(), "{mode:?} removed something");
            assert!(t.written.borrow().is_empty(), "{mode:?} wrote something");
        }
    }

    /// A station renamed in the spec but keeping its URL: Koel's URL is
    /// unique per account, so the `POST` that adds the new name would be
    /// refused with 422 while the old station is still there.
    #[test]
    fn a_renamed_station_is_removed_before_the_new_one_is_added() {
        let mut renamed = fsk();
        renamed.name = "Radio 100,7".to_string();
        renamed.url = "https://radio.example.org/live.ogg".to_string();
        renamed.is_public = true;
        let task = RadioStations {
            stations: vec![rdl(false), fsk(), renamed],
            exactly: true,
        };
        // After the run Koel lists the new station instead of the old one.
        let after = {
            let mut v: Value = serde_json::from_str(CONSTRUCTED).unwrap();
            let list = v.as_array_mut().unwrap();
            for station in list.iter_mut() {
                if station["id"] == SURPLUS_ID {
                    station["name"] = json!("Radio 100,7");
                    station["description"] = json!("");
                }
            }
            v.to_string()
        };
        let t = FakeTransport::default()
            .on_get(ME, vec![ok(&me(false))])
            // The probe reads the list too, so: probe, read, read back.
            .on_get(STATIONS, vec![ok(CONSTRUCTED), ok(CONSTRUCTED), ok(&after)])
            .on_put(vec![Step::Answer(201, "{}".into())])
            .on_delete(vec![Step::Answer(204, String::new())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        match report.outcome {
            Outcome::Changed(changes) => assert_eq!(
                shown(&changes),
                [
                    "station Somebody's own: (present) -> (removed)",
                    "station Radio 100,7: (missing) -> (added)"
                ]
            ),
            other => panic!("expected Changed, got {other:?}"),
        }
        let calls = t.calls.borrow();
        assert_eq!(
            calls[0],
            ("DELETE", format!("{STATIONS}/{SURPLUS_ID}")),
            "{calls:?}"
        );
        assert_eq!(calls[1], ("POST", STATIONS.to_string()), "{calls:?}");
    }

    #[test]
    fn a_refused_removal_names_the_station() {
        let task = two_of_three(true);
        let t = koel(CONSTRUCTED).on_delete(vec![Step::Answer(403, INVALID.into())]);
        let current = task.read(&t).unwrap();
        let err = task.remove(&t, &current).err().unwrap().to_string();
        assert_eq!(
            err,
            format!(
                "DELETE {STATIONS}/{SURPLUS_ID} answered HTTP 403: station Somebody's own was not removed"
            )
        );
    }

    #[test]
    fn a_differing_station_is_put_whole_and_a_logo_only_where_there_is_none() {
        let mut changed = fsk();
        changed.url = "https://streaming.fueralle.org/fsk.ogg".to_string();
        changed.is_public = true;
        changed.homepage_url = Some("https://fsk-hh.org/".to_string());
        changed.logo = Some(Logo {
            file: "/nix/store/x-fsk.png".to_string(),
            data_uri: LOGO_DATA.to_string(),
        });
        // A logo file for a station that has a logo changes nothing.
        let task = RadioStations {
            stations: vec![rdl(true), changed],
            exactly: false,
        };
        let t = koel(CONSTRUCTED).on_put(vec![ok("{}")]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(
            shown(&changes),
            [
                r#"station FSK: url "https://streaming.fueralle.org/fsk.mp3" -> "https://streaming.fueralle.org/fsk.ogg""#,
                "station FSK: is_public false -> true",
                r#"station FSK: homepage_url null -> "https://fsk-hh.org/""#,
                "station FSK: logo (none) -> (from /nix/store/x-fsk.png)",
            ]
        );
        assert!(!format!("{changes:?}").contains("bG9nby"));
        task.write(&t, &current).unwrap();
        assert_eq!(t.written.borrow().len(), 1, "only the station that differs");
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/radio/stations/01K52Z6N0R5S6T7V8W9X0Y1Z2A");
        assert_eq!(
            body,
            json!({"name": "FSK", "url": "https://streaming.fueralle.org/fsk.ogg",
                   "description": "", "is_public": true,
                   "homepage_url": "https://fsk-hh.org/", "logo": LOGO_DATA})
        );

        // Only the description differs: the PUT still carries every field,
        // or Koel would make the station private -- but no logo.
        let mut described = rdl(true);
        described.description = "Neu.".to_string();
        let task = RadioStations {
            stations: vec![described],
            exactly: false,
        };
        let t = koel(CONSTRUCTED).on_put(vec![ok("{}")]);
        task.write(&t, &task.read(&t).unwrap()).unwrap();
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/radio/stations/01K52Z6M3Q8V4T7X1B9N0C2D5E");
        assert_eq!(
            body,
            json!({"name": "Radio Dreyeckland", "url": "https://stream.rdl.de/rdl",
                   "description": "Neu.", "is_public": true,
                   "homepage_url": "https://rdl.de/"})
        );
    }

    #[test]
    fn a_name_koel_has_twice_is_an_error_before_anything_is_written() {
        let mut list: Vec<Value> = serde_json::from_str(CONSTRUCTED).unwrap();
        list[2]["name"] = json!("FSK");
        let task = RadioStations {
            stations: vec![fsk()],
            exactly: false,
        };
        let t = koel(&Value::Array(list).to_string());
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "the spec does not fit the service: Koel has 2 stations named FSK and converge cannot tell which one is meant"
        );
    }

    #[test]
    fn a_refused_write_shows_the_validation_fields() {
        let task = RadioStations {
            stations: vec![fsk()],
            exactly: false,
        };
        let t = koel(RECORDED).on_put(vec![Step::Answer(422, INVALID.into())]);
        let err = task
            .write(&t, &task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "POST /api/radio/stations answered HTTP 422: logo: Invalid image for logo; url: The url field must be a valid URL."
        );
    }

    #[test]
    fn apply_adds_then_reads_back_until_the_station_is_there() {
        let task = RadioStations {
            stations: vec![fsk()],
            exactly: false,
        };
        let after =
            json!([{"type": "radio-stations", "name": "FSK", "id": "01K52Z6N0R5S6T7V8W9X0Y1Z2A",
            "url": "https://streaming.fueralle.org/fsk.mp3", "homepage_url": null, "logo": null,
            "description": "", "is_public": false, "created_at": "2026-09-14T18:00:01.000000Z",
            "favorite": false, "permissions": {"edit": true, "delete": true}}])
            .to_string();
        let t = FakeTransport::default()
            .on_get(ME, vec![ok(&me(false))])
            .on_get(
                STATIONS,
                vec![ok(RECORDED), ok(RECORDED), ok(RECORDED), ok(&after)],
            )
            .on_put(vec![Step::Answer(201, "{}".into())]);
        let clock = FakeClock::new();
        let start = clock.now();
        let report = run(Mode::Apply, &task, &t, &clock, Timing::default()).unwrap();
        assert!(matches!(report.outcome, Outcome::Changed(ref c) if c.len() == 1));
        assert_eq!(report.version, VERSION_NOT_REPORTED);
        assert_eq!(t.written.borrow().len(), 1, "no second POST");
        assert!(clock.now() > start, "it waited for the read-back");
    }

    #[test]
    fn base64_follows_rfc_4648() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded, "{plain}");
        }
        assert_eq!(base64(&[0xfb, 0xff, 0xfe]), "+//+");
    }

    #[test]
    fn a_logo_file_above_the_ceiling_is_refused_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize(LOGO_MAX_BYTES as usize, 0);
        std::fs::write(&path, &bytes).unwrap();
        assert!(logo(&path).is_ok(), "exactly the ceiling is fine");
        bytes.push(0);
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            logo(&path).err().unwrap(),
            format!(
                "logo_file {} is 2097153 bytes, more than the 2097152 (2 MiB) converge sends -- Koel scales a logo to 640 pixels wide anyway",
                path.display()
            )
        );
    }

    #[test]
    fn a_logo_file_becomes_a_data_uri_by_its_content_not_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let png = write("logo.jpg", b"\x89PNG\r\n\x1a\n");
        let logo = logo(&png).unwrap();
        assert_eq!(logo.data_uri, "data:image/png;base64,iVBORw0KGgo=");
        assert_eq!(logo.file, png.display().to_string());
        for (bytes, mime) in [
            (&b"\xff\xd8\xff\xe0"[..], "image/jpeg"),
            (&b"GIF89a"[..], "image/gif"),
            (&b"RIFF\x00\x00\x00\x00WEBPVP8 "[..], "image/webp"),
            (
                &b"  <svg xmlns=\"http://www.w3.org/2000/svg\"/>"[..],
                "image/svg+xml",
            ),
            (&b"<?xml version=\"1.0\"?>\n<svg/>"[..], "image/svg+xml"),
        ] {
            let path = write("x", bytes);
            let uri = super::logo(&path).unwrap().data_uri;
            assert!(uri.starts_with(&format!("data:{mime};base64,")), "{uri}");
        }
        let err = super::logo(&write("icon.ico", b"\x00\x00\x01\x00"))
            .err()
            .unwrap();
        assert!(
            err.contains("is not a PNG, JPEG, GIF, WebP or SVG image"),
            "{err}"
        );
        let err = super::logo(&write("empty.png", b"")).err().unwrap();
        assert!(err.contains("is empty"), "{err}");
        let err = super::logo(&dir.path().join("missing.png")).err().unwrap();
        assert!(err.contains("cannot be read (entity not found)"), "{err}");
    }
}
