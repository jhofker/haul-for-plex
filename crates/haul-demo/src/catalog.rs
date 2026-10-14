//! The made-up media the demo servers hold. Everything here is fictional.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Value, json};

/// Seconds since the epoch for 2025-06-01; added dates count forward from it.
const BASE_ADDED: i64 = 1_748_736_000;
const DAY: i64 = 86_400;
const GB: f64 = 1_000_000_000.0;

pub struct PartInfo {
    pub size: u64,
}

pub struct Item {
    pub kind: &'static str,
    pub title: String,
    pub sort_title: String,
    pub year: Option<u32>,
    pub added: i64,
    pub released: String,
    pub audience: f64,
    /// Rating keys of seasons (for a show) or episodes (for a season).
    pub children: Vec<String>,
    /// For episodes: the show's rating key, so `allLeaves` can find them.
    pub show: Option<String>,
    pub json: Value,
}

pub struct Library {
    pub key: String,
    pub title: String,
    pub kind: &'static str,
    /// Rating keys of the library's top-level items.
    pub items: Vec<String>,
}

pub struct Server {
    pub name: String,
    pub id: String,
    pub token: String,
    pub owned: bool,
    /// Who shared it, for servers that aren't yours.
    pub owner: Option<String>,
    pub libraries: Vec<Library>,
    pub items: BTreeMap<String, Item>,
    pub parts: HashMap<u64, PartInfo>,
    /// Parts the "owner" has switched downloads off for.
    pub forbidden: HashSet<u64>,
}

impl Server {
    /// Top-level item of the given title, e.g. to queue it from a script.
    pub fn find(&self, title: &str) -> Option<&str> {
        self.items
            .iter()
            .find(|(_, i)| i.title == title && matches!(i.kind, "movie" | "show"))
            .map(|(k, _)| k.as_str())
    }

    /// Rating key of a show's Nth season.
    pub fn season(&self, show: &str, n: usize) -> Option<&str> {
        self.items
            .get(self.find(show)?)?
            .children
            .get(n.checked_sub(1)?)
            .map(String::as_str)
    }
}

fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

fn tags(names: &[&str]) -> Value {
    Value::Array(names.iter().map(|n| json!({"tag": n})).collect())
}

fn date(year: u32, salt: u64) -> String {
    format!("{year}-{:02}-{:02}", 1 + salt % 12, 1 + (salt >> 8) % 28)
}

/// "The Hollow Lantern" is sorted under "Hollow Lantern", like Plex does.
fn sort_title(title: &str) -> String {
    for article in ["The ", "A ", "An "] {
        if let Some(rest) = title.strip_prefix(article) {
            return rest.to_string();
        }
    }
    title.to_string()
}

const PEOPLE: [&str; 20] = [
    "Ines Valcourt",
    "Tomas Brandt",
    "Priya Ramanathan",
    "Oskar Lindqvist",
    "Mei-Lin Okafor",
    "Dario Castellanos",
    "Hana Sorensen",
    "Felix Abara",
    "Noor Haddad",
    "Luca Ferreira",
    "Greta Vandermolen",
    "Idris Calloway",
    "Sunniva Aas",
    "Rafael Quill",
    "Odette Marsh",
    "Kofi Lindgren",
    "Anouk Delacroix",
    "Bram Oyelaran",
    "Yuki Santangelo",
    "Cormac Reyes",
];
const ROLES: [&str; 12] = [
    "Detective Marlow",
    "Captain Ines",
    "Dr. Okonkwo",
    "Wren",
    "The Stranger",
    "Aunt Pilar",
    "Jonah",
    "Mayor Hale",
    "Sister Agnes",
    "Teodor",
    "Lila",
    "Old Sam",
];

struct MovieSpec {
    title: &'static str,
    year: u32,
    genres: &'static [&'static str],
    tagline: &'static str,
    summary: &'static str,
    minutes: u32,
    rated: &'static str,
    /// (resolution, codec); the first is the default version.
    versions: &'static [(&'static str, &'static str)],
}

const H: (&str, &str) = ("1080", "h264");
const UHD: (&str, &str) = ("4k", "hevc");

const MOVIES: &[MovieSpec] = &[
    MovieSpec {
        title: "The Lantern Keeper",
        year: 2012,
        genres: &["Drama", "Fantasy"],
        tagline: "Some lights are worth keeping.",
        summary: "On a cliff above a fishing village, a widowed lighthouse keeper discovers that the lamp answers questions, and that each answer costs a memory.",
        minutes: 118,
        rated: "PG-13",
        versions: &[H],
    },
    MovieSpec {
        title: "Orbit Seven",
        year: 2021,
        genres: &["Science Fiction", "Thriller"],
        tagline: "Seven crew. Six seats home.",
        summary: "When a failing station loses its relay to Earth, the seven people aboard must decide who gets the only working return capsule.",
        minutes: 126,
        rated: "PG-13",
        versions: &[H, UHD],
    },
    MovieSpec {
        title: "Midnight at the Observatory",
        year: 2019,
        genres: &["Mystery"],
        tagline: "The stars aren't the only thing watching.",
        summary: "A night-shift astronomer finds a pattern in the telescope logs that points to a crime committed forty years ago in the very same dome.",
        minutes: 104,
        rated: "PG-13",
        versions: &[H],
    },
    MovieSpec {
        title: "Paper Cranes",
        year: 2008,
        genres: &["Drama", "Romance"],
        tagline: "A thousand folds. One wish.",
        summary: "Two strangers keep leaving folded notes for each other in a borrowed library book, each convinced the other is long gone.",
        minutes: 109,
        rated: "PG",
        versions: &[H],
    },
    MovieSpec {
        title: "The Salt Road",
        year: 2015,
        genres: &["Adventure", "Western"],
        tagline: "Water is worth more than gold.",
        summary: "A retired courier agrees to guide a caravan across the salt flats one last time, with a rival outfit a day behind.",
        minutes: 131,
        rated: "R",
        versions: &[H],
    },
    MovieSpec {
        title: "Quiet Machines",
        year: 2023,
        genres: &["Science Fiction", "Drama"],
        tagline: "They only ever wanted to be useful.",
        summary: "In a care home staffed by robots, an aging engineer notices that her mechanical caretakers have started keeping secrets.",
        minutes: 112,
        rated: "PG-13",
        versions: &[H, UHD],
    },
    MovieSpec {
        title: "A Thousand Small Fires",
        year: 2017,
        genres: &["Thriller"],
        tagline: "Every blaze has a first spark.",
        summary: "A fire investigator links a string of tiny, harmless blazes across town to a disaster that hasn't happened yet.",
        minutes: 115,
        rated: "R",
        versions: &[H],
    },
    MovieSpec {
        title: "Gravel & Honey",
        year: 2005,
        genres: &["Comedy"],
        tagline: "Sweet. Sticky. Slightly illegal.",
        summary: "Two feuding beekeepers are forced to share a truck, a county fair booth and one very stubborn queen.",
        minutes: 97,
        rated: "PG-13",
        versions: &[H],
    },
    MovieSpec {
        title: "The Glass Orchard",
        year: 2014,
        genres: &["Fantasy", "Family"],
        tagline: "Grow something impossible.",
        summary: "A girl inherits her grandmother's orchard, where the fruit is made of glass and ripens only for people who tell the truth.",
        minutes: 101,
        rated: "PG",
        versions: &[H],
    },
    MovieSpec {
        title: "Northbound",
        year: 2020,
        genres: &["Adventure"],
        tagline: "The only way out is up.",
        summary: "Three estranged siblings drive their father's ashes to the arctic circle, arguing the whole way.",
        minutes: 122,
        rated: "PG-13",
        versions: &[H],
    },
    MovieSpec {
        title: "Echo Valley",
        year: 2011,
        genres: &["Horror"],
        tagline: "Don't answer back.",
        summary: "A hiking trip goes wrong when the canyon starts repeating things nobody said.",
        minutes: 94,
        rated: "R",
        versions: &[H],
    },
    MovieSpec {
        title: "Marigold Street",
        year: 2009,
        genres: &["Comedy", "Romance"],
        tagline: "Love thy neighbour. Eventually.",
        summary: "Two neighbours who have spent a decade feuding over a hedge are snowed in together for a week.",
        minutes: 102,
        rated: "PG-13",
        versions: &[H],
    },
    MovieSpec {
        title: "The Cartographer's Daughter",
        year: 2018,
        genres: &["Adventure", "Drama"],
        tagline: "Some maps are meant to be lost.",
        summary: "A young woman follows her late father's unfinished maps to an island that appears on none of them.",
        minutes: 128,
        rated: "PG-13",
        versions: &[H],
    },
    MovieSpec {
        title: "Static Bloom",
        year: 2022,
        genres: &["Science Fiction", "Thriller"],
        tagline: "The signal is spreading.",
        summary: "A radio technician traces a looping broadcast to a flower field that grows only where the signal is strongest.",
        minutes: 107,
        rated: "R",
        versions: &[H],
    },
    MovieSpec {
        title: "Velvet Harbor",
        year: 2016,
        genres: &["Crime", "Drama"],
        tagline: "Everyone owes someone.",
        summary: "A dockside bookkeeper cooks the ledgers for the wrong family and has one night to put the money back.",
        minutes: 119,
        rated: "R",
        versions: &[H],
    },
    MovieSpec {
        title: "The Last Ferry",
        year: 2013,
        genres: &["Drama"],
        tagline: "Every crossing has a last.",
        summary: "On the final evening before a bridge replaces the ferry, its regular passengers share what they never said.",
        minutes: 99,
        rated: "PG",
        versions: &[H],
    },
    MovieSpec {
        title: "Copper Moon",
        year: 2024,
        genres: &["Animation", "Family"],
        tagline: "Catch it before sunrise.",
        summary: "A tinkerer's apprentice and a runaway weathervane set off to bring the moon back to the village it fell on.",
        minutes: 92,
        rated: "G",
        versions: &[H, UHD],
    },
    MovieSpec {
        title: "Little Thunder",
        year: 2010,
        genres: &["Family", "Comedy"],
        tagline: "Small dog. Big storm.",
        summary: "A very small dog is convinced that he alone is responsible for the weather.",
        minutes: 88,
        rated: "G",
        versions: &[H],
    },
    MovieSpec {
        title: "Ashes of Meridian",
        year: 2019,
        genres: &["Action", "Science Fiction"],
        tagline: "Rebuild or burn it all.",
        summary: "A demolitions expert is the only one who knows where the city's old reactor was walled up, and everyone wants it.",
        minutes: 137,
        rated: "PG-13",
        versions: &[H, UHD],
    },
    MovieSpec {
        title: "The Understudy",
        year: 2007,
        genres: &["Comedy", "Drama"],
        tagline: "The show must go on. Somehow.",
        summary: "On opening night an understudy has forty minutes to learn the lead's part and hide that the lead has vanished.",
        minutes: 106,
        rated: "PG-13",
        versions: &[H],
    },
    MovieSpec {
        title: "Winter Signal",
        year: 2021,
        genres: &["Thriller"],
        tagline: "Somebody is still listening.",
        summary: "At a polar weather station, the crew receives a distress call in their own voices, dated next week.",
        minutes: 111,
        rated: "R",
        versions: &[H],
    },
    MovieSpec {
        title: "Kite Season",
        year: 2006,
        genres: &["Family", "Drama"],
        tagline: "Hold on and let go.",
        summary: "One windy spring, a boy and his grandfather build a kite big enough to carry a message across the bay.",
        minutes: 96,
        rated: "PG",
        versions: &[H],
    },
    MovieSpec {
        title: "Hollow Pines",
        year: 2018,
        genres: &["Horror", "Mystery"],
        tagline: "The forest remembers.",
        summary: "A surveyor mapping a protected forest keeps finding the same cabin, each time a little closer to the road.",
        minutes: 101,
        rated: "R",
        versions: &[H],
    },
    MovieSpec {
        title: "Tidewater",
        year: 2025,
        genres: &["Drama"],
        tagline: "What the sea returns.",
        summary: "After a flood, a family of salvagers finds a sealed room in the cellar that was never on any plan of the house.",
        minutes: 123,
        rated: "PG-13",
        versions: &[H, UHD],
    },
];

struct ShowSpec {
    title: &'static str,
    year: u32,
    genres: &'static [&'static str],
    summary: &'static str,
    studio: &'static str,
    seasons: usize,
    episodes: usize,
    minutes: u32,
    rated: &'static str,
}

const SHOWS: &[ShowSpec] = &[
    ShowSpec {
        title: "Harbor Lights",
        year: 2019,
        genres: &["Drama", "Mystery"],
        summary: "In a fog-bound port town, the new harbourmaster discovers that every ship that docks is carrying a secret.",
        studio: "Northwind Television",
        seasons: 3,
        episodes: 8,
        minutes: 48,
        rated: "TV-14",
    },
    ShowSpec {
        title: "Night Shift Radio",
        year: 2016,
        genres: &["Drama", "Comedy"],
        summary: "The overnight crew of a struggling local station keeps the town company through its strangest hours.",
        studio: "Kestrel Pictures",
        seasons: 2,
        episodes: 6,
        minutes: 34,
        rated: "TV-PG",
    },
    ShowSpec {
        title: "Brass & Bone",
        year: 2021,
        genres: &["Fantasy", "Adventure"],
        summary: "A band of mismatched mercenaries escorts a clockwork saint across a continent that wants her broken.",
        studio: "Ironwood Studios",
        seasons: 2,
        episodes: 8,
        minutes: 52,
        rated: "TV-MA",
    },
    ShowSpec {
        title: "The Quiet Wing",
        year: 2010,
        genres: &["Drama"],
        summary: "Nurses, orderlies and one very patient night doctor keep a small hospital's quietest ward running.",
        studio: "Kestrel Pictures",
        seasons: 4,
        episodes: 6,
        minutes: 44,
        rated: "TV-14",
    },
    ShowSpec {
        title: "Daybreak Diner",
        year: 2014,
        genres: &["Comedy"],
        summary: "The staff and regulars of an all-night diner argue about everything except the pie.",
        studio: "Marlow & Finch",
        seasons: 3,
        episodes: 6,
        minutes: 24,
        rated: "TV-PG",
    },
    ShowSpec {
        title: "Foxglove",
        year: 2022,
        genres: &["Thriller", "Crime"],
        summary: "A pharmacist realises her small town's prescriptions are being quietly rewritten, and by whom.",
        studio: "Ironwood Studios",
        seasons: 1,
        episodes: 8,
        minutes: 40,
        rated: "TV-MA",
    },
];

const ADJ: [&str; 20] = [
    "Long",
    "Hollow",
    "Paper",
    "Salt",
    "Quiet",
    "Borrowed",
    "Second",
    "Glass",
    "Wandering",
    "Last",
    "Copper",
    "Velvet",
    "Winter",
    "Small",
    "Midnight",
    "Open",
    "Broken",
    "Golden",
    "Slow",
    "Distant",
];
const NOUN: [&str; 20] = [
    "Tide", "Lantern", "Letter", "Harbor", "Orchard", "Signal", "Promise", "Ferry", "Garden",
    "Key", "Window", "River", "Bell", "Door", "Map", "Season", "Station", "Echo", "Thread",
    "Crossing",
];
const BLURBS: [&str; 8] = [
    "An unexpected visitor forces an old secret into the open.",
    "A storm cuts the town off and tempers fray.",
    "Someone returns after years away, and not everyone is pleased.",
    "A small mistake at work threatens to become a large one.",
    "The group splits up to follow two very different leads.",
    "A long-buried letter changes what everyone thought they knew.",
    "An ordinary favour turns into a night nobody forgets.",
    "Old rivals are forced to work together against the clock.",
];

/// Builds one server's contents, handing out unique rating keys and part ids.
struct Builder {
    items: BTreeMap<String, Item>,
    parts: HashMap<u64, PartInfo>,
    next_key: u32,
    next_part: u64,
}

impl Builder {
    fn key(&mut self) -> String {
        self.next_key += 1;
        self.next_key.to_string()
    }

    /// A media version with one file of `bytes`, with a believable stream list.
    fn media(&mut self, res: &str, codec: &str, minutes: u32, bytes: u64, file: &str) -> Value {
        self.next_part += 1;
        let id = self.next_part;
        self.parts.insert(id, PartInfo { size: bytes });
        let (w, h, label) = match res {
            "4k" => (3840, 2160, "4K"),
            "720" => (1280, 720, "720p"),
            _ => (1920, 1080, "1080p"),
        };
        let ms = u64::from(minutes) * 60_000;
        let kbps = bytes * 8 / (ms / 1000) / 1000;
        let (profile, depth) = if codec == "hevc" {
            ("main 10", 10)
        } else {
            ("high", 8)
        };
        let audio = if res == "4k" {
            ("truehd", "TrueHD Atmos 7.1", 8)
        } else {
            ("eac3", "EAC3 5.1", 6)
        };
        json!({
            "id": id, "container": "mkv", "videoResolution": res, "videoCodec": codec,
            "videoProfile": profile, "videoFrameRate": "24p", "audioCodec": audio.0,
            "audioChannels": audio.2, "width": w, "height": h, "aspectRatio": 1.78,
            "bitrate": kbps, "duration": ms,
            "Part": [{
                "id": id, "key": format!("/library/parts/{id}/1/file.mkv"), "file": file,
                "size": bytes, "container": "mkv", "duration": ms,
                "Stream": [
                    {"streamType": 1, "codec": codec, "profile": profile, "language": "English",
                     "displayTitle": format!("{label} ({})", codec.to_uppercase()), "bitrate": kbps,
                     "width": w, "height": h, "frameRate": 23.976, "bitDepth": depth,
                     "colorSpace": "bt709", "default": true},
                    {"streamType": 2, "codec": audio.0, "language": "English",
                     "displayTitle": format!("English ({})", audio.1), "bitrate": 640,
                     "channels": audio.2, "default": true},
                    {"streamType": 2, "codec": "aac", "profile": "lc", "language": "English",
                     "displayTitle": "English (AAC Stereo)", "bitrate": 128, "channels": 2},
                    {"streamType": 3, "codec": "pgs", "language": "English", "displayTitle": "English (PGS)", "bitrate": 40},
                    {"streamType": 3, "codec": "srt", "language": "Español", "displayTitle": "Español (SRT)"},
                    {"streamType": 3, "codec": "srt", "language": "Français", "displayTitle": "Français (SRT)"},
                ]
            }]
        })
    }

    fn people(seed: u64, n: usize) -> (Value, Value, Value) {
        let pick = |i: usize| PEOPLE[((seed >> (i % 8)) as usize + i * 7) % PEOPLE.len()];
        let cast: Vec<Value> = (0..n)
            .map(|i| json!({"tag": pick(i + 3), "role": ROLES[((seed >> 3) as usize + i * 5) % ROLES.len()]}))
            .collect();
        (
            json!([{"tag": pick(0)}]),
            json!([{"tag": pick(1)}, {"tag": pick(2)}]),
            Value::Array(cast),
        )
    }

    fn movie(&mut self, spec: &MovieSpec, section: &str, section_title: &str, n: usize) -> String {
        let key = self.key();
        let h = hash(spec.title);
        let added = BASE_ADDED + (n as i64 * 5 + (h % 4) as i64) * DAY;
        let (directors, writers, cast) = Self::people(h, 6);
        let audience = 6.0 + (h % 35) as f64 / 10.0;
        let slug = spec.title.replace(' ', ".");
        let mut media = Vec::new();
        for (i, (res, codec)) in spec.versions.iter().enumerate() {
            let factor = if *res == "4k" { 2.4 } else { 1.0 };
            let gb = (f64::from(spec.minutes) * 0.062 + (h % 20) as f64 / 10.0) * factor;
            let label = if *res == "4k" { "2160p" } else { "1080p" };
            let file = format!(
                "/media/movies/{0} ({1})/{slug}.{1}.{label}.mkv",
                spec.title, spec.year
            );
            media.push(self.media(res, codec, spec.minutes, (gb * GB) as u64, &file));
            let _ = i;
        }
        let item_json = json!({
            "ratingKey": key, "type": "movie", "title": spec.title, "titleSort": sort_title(spec.title),
            "year": spec.year, "tagline": spec.tagline, "summary": spec.summary,
            "duration": u64::from(spec.minutes) * 60_000, "contentRating": spec.rated,
            "rating": (audience - 0.4).max(1.0), "audienceRating": audience,
            "studio": "Lantern Hill Pictures", "originallyAvailableAt": date(spec.year, h),
            "addedAt": added, "librarySectionID": section.parse::<u64>().unwrap_or(0),
            "librarySectionTitle": section_title,
            "Genre": tags(spec.genres), "Director": directors, "Writer": writers, "Role": cast,
            "Country": tags(&["United States of America"]), "Media": media,
        });
        self.items.insert(
            key.clone(),
            Item {
                kind: "movie",
                title: spec.title.into(),
                sort_title: sort_title(spec.title),
                year: Some(spec.year),
                added,
                released: date(spec.year, h),
                audience,
                children: vec![],
                show: None,
                json: item_json,
            },
        );
        key
    }

    fn show(&mut self, spec: &ShowSpec, section: &str, section_title: &str, n: usize) -> String {
        let show_key = self.key();
        let h = hash(spec.title);
        let added = BASE_ADDED + (n as i64 * 9 + 20) * DAY;
        let (directors, writers, cast) = Self::people(h, 6);
        let audience = 7.0 + (h % 25) as f64 / 10.0;
        let mut season_keys = Vec::new();
        let mut all_eps = 0u32;
        for s in 1..=spec.seasons {
            let season_key = self.key();
            let mut ep_keys = Vec::new();
            for e in 1..=spec.episodes {
                let ep_key = self.key();
                let i = (s - 1) * spec.episodes + e;
                let eh = hash(&format!("{}{i}", spec.title));
                let title = format!(
                    "The {} {}",
                    ADJ[(eh % 20) as usize],
                    NOUN[((eh >> 8) % 20) as usize]
                );
                let blurb = BLURBS[((eh >> 16) % 8) as usize];
                let year = spec.year + s as u32 - 1;
                let aired = format!(
                    "{year}-{:02}-{:02}",
                    1 + (e as u64 * 2) % 12,
                    1 + (eh >> 24) % 28
                );
                let gb = f64::from(spec.minutes) * 0.011 + (eh % 8) as f64 / 20.0;
                let file = format!(
                    "/media/tv/{0} ({1})/Season {s:02}/{0} - S{s:02}E{e:02}.mkv",
                    spec.title, spec.year
                );
                let media = self.media("1080", "h264", spec.minutes, (gb * GB) as u64, &file);
                let ep_added = added + (i as i64) * DAY / 2;
                let item_json = json!({
                    "ratingKey": ep_key, "type": "episode", "title": title, "titleSort": title,
                    "year": year, "index": e, "parentIndex": s, "summary": blurb,
                    "parentTitle": format!("Season {s}"), "parentRatingKey": season_key,
                    "grandparentTitle": spec.title, "grandparentRatingKey": show_key,
                    "duration": u64::from(spec.minutes) * 60_000, "contentRating": spec.rated,
                    "audienceRating": 6.5 + (eh % 30) as f64 / 10.0,
                    "originallyAvailableAt": aired, "addedAt": ep_added,
                    "librarySectionID": section.parse::<u64>().unwrap_or(0),
                    "librarySectionTitle": section_title,
                    "Director": directors, "Writer": writers, "Role": cast,
                    "Media": [media],
                });
                self.items.insert(
                    ep_key.clone(),
                    Item {
                        kind: "episode",
                        title: title.clone(),
                        sort_title: title,
                        year: Some(year),
                        added: ep_added,
                        released: aired,
                        audience: 6.5 + (eh % 30) as f64 / 10.0,
                        children: vec![],
                        show: Some(show_key.clone()),
                        json: item_json,
                    },
                );
                ep_keys.push(ep_key);
                all_eps += 1;
            }
            let season_json = json!({
                "ratingKey": season_key, "type": "season", "title": format!("Season {s}"),
                "index": s, "parentTitle": spec.title, "parentRatingKey": show_key,
                "leafCount": spec.episodes, "librarySectionID": section.parse::<u64>().unwrap_or(0),
                "librarySectionTitle": section_title, "addedAt": added,
            });
            self.items.insert(
                season_key.clone(),
                Item {
                    kind: "season",
                    title: format!("Season {s}"),
                    sort_title: format!("Season {s}"),
                    year: None,
                    added,
                    released: String::new(),
                    audience: 0.0,
                    children: ep_keys,
                    show: Some(show_key.clone()),
                    json: season_json,
                },
            );
            season_keys.push(season_key);
        }
        let show_json = json!({
            "ratingKey": show_key, "type": "show", "title": spec.title, "titleSort": sort_title(spec.title),
            "year": spec.year, "summary": spec.summary, "studio": spec.studio,
            "contentRating": spec.rated, "audienceRating": audience, "rating": audience - 0.3,
            "originallyAvailableAt": date(spec.year, h), "addedAt": added,
            "childCount": spec.seasons, "leafCount": all_eps,
            "librarySectionID": section.parse::<u64>().unwrap_or(0), "librarySectionTitle": section_title,
            "Genre": tags(spec.genres), "Director": directors, "Writer": writers, "Role": cast,
            "Country": tags(&["United States of America"]),
        });
        self.items.insert(
            show_key.clone(),
            Item {
                kind: "show",
                title: spec.title.into(),
                sort_title: sort_title(spec.title),
                year: Some(spec.year),
                added,
                released: date(spec.year, h),
                audience,
                children: season_keys,
                show: None,
                json: show_json,
            },
        );
        show_key
    }
}

fn server(name: &str, id: &str, owner: Option<&str>) -> (Builder, Server) {
    (
        Builder {
            items: BTreeMap::new(),
            parts: HashMap::new(),
            next_key: 100,
            next_part: 0,
        },
        Server {
            name: name.into(),
            id: id.into(),
            token: format!("{id}-token"),
            owned: owner.is_none(),
            owner: owner.map(String::from),
            libraries: vec![],
            items: BTreeMap::new(),
            parts: HashMap::new(),
            forbidden: HashSet::new(),
        },
    )
}

/// The servers your demo account can see: one you own, one a friend shared.
pub fn build() -> Vec<Server> {
    // Atlas: yours, with every movie and show.
    let (mut b, mut atlas) = server("Atlas", "demo-atlas", None);
    let movies: Vec<String> = MOVIES
        .iter()
        .enumerate()
        .map(|(n, m)| b.movie(m, "1", "Movies", n))
        .collect();
    let shows: Vec<String> = SHOWS
        .iter()
        .enumerate()
        .map(|(n, s)| b.show(s, "2", "TV Shows", n))
        .collect();
    atlas.libraries = vec![
        Library {
            key: "1".into(),
            title: "Movies".into(),
            kind: "movie",
            items: movies,
        },
        Library {
            key: "2".into(),
            title: "TV Shows".into(),
            kind: "show",
            items: shows,
        },
    ];
    atlas.items = b.items;
    atlas.parts = b.parts;

    // Lighthouse: shared by a friend, a smaller selection, one title with
    // downloads switched off.
    let (mut b, mut lighthouse) = server("Lighthouse", "demo-lighthouse", Some("Margot"));
    let pick = |titles: &[&str]| -> Vec<&'static MovieSpec> {
        MOVIES
            .iter()
            .filter(|m| titles.contains(&m.title))
            .collect()
    };
    let films = pick(&[
        "Paper Cranes",
        "Northbound",
        "Velvet Harbor",
        "The Last Ferry",
        "Copper Moon",
        "Little Thunder",
        "Tidewater",
        "Kite Season",
        "Marigold Street",
    ]);
    let movies: Vec<String> = films
        .iter()
        .enumerate()
        .map(|(n, m)| b.movie(m, "1", "Movies", n))
        .collect();
    let forbidden_key = movies[films.iter().position(|m| m.title == "Tidewater").unwrap()].clone();
    let shows: Vec<String> = [3usize, 4]
        .iter()
        .enumerate()
        .map(|(n, i)| b.show(&SHOWS[*i], "2", "TV Shows", n))
        .collect();
    lighthouse.libraries = vec![
        Library {
            key: "1".into(),
            title: "Movies".into(),
            kind: "movie",
            items: movies,
        },
        Library {
            key: "2".into(),
            title: "TV Shows".into(),
            kind: "show",
            items: shows,
        },
    ];
    // The part ids of the forbidden movie.
    let parts: Vec<u64> = b.items[&forbidden_key].json["Media"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| {
            m["Part"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["id"].as_u64().unwrap())
        })
        .collect();
    lighthouse.forbidden = parts.into_iter().collect();
    lighthouse.items = b.items;
    lighthouse.parts = b.parts;

    vec![atlas, lighthouse]
}
