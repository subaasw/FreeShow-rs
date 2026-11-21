// SPDX-License-Identifier: GPL-3.0-only

use super::{SyncReport, file_id};
use freeshow_data::{Catalog, Library, ProjectsFile, ShowFile, is_lesson_document, new_id};
use serde_json::{Map, Value, json};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_MEDIA: u64 = 2 * 1024 * 1024 * 1024;

fn error(error: impl ToString) -> String {
    error.to_string()
}

pub(super) fn import_sources(root: &Path) -> SyncReport {
    let library = Library::open(root);
    let mut report = SyncReport::none();
    let Ok(entries) = fs::read_dir(library.imports_dir()) else {
        return report;
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !path.is_file() || !["olp", "olf", "json"].contains(&extension.as_str()) {
            continue;
        }
        let found = (|| {
            if fs::metadata(&path).map_err(error)?.len() > 16 * 1024 * 1024 {
                return Err("lesson document exceeds 16 MiB".into());
            }
            let value: Value =
                serde_json::from_slice(&fs::read(&path).map_err(error)?).map_err(error)?;
            if extension == "json" && !is_lesson_document(&value) {
                return Ok(SyncReport::none());
            }
            import_lesson_document(root, &value)
        })();
        match found {
            Ok(found) => {
                report.shows += found.shows;
                report.project_id = found.project_id.or(report.project_id);
            }
            Err(message) => tracing::error!("lesson {}: {message}", path.display()),
        }
    }
    report
}

/// Import an Open Lesson Playlist or Open Lesson Format document. All downloads
/// finish before either the show or its Lessons.church project is published.
pub fn import_lesson_document(root: &Path, lesson: &Value) -> Result<SyncReport, String> {
    if !is_lesson_document(lesson) {
        return Err("not an Open Lesson Playlist/Format document".into());
    }
    let library = Library::open(root);
    let name = lesson["lessonName"].as_str().unwrap();
    let title = lesson["lessonTitle"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(name);
    let display = if title == name {
        title.to_string()
    } else {
        format!("{title} - {name}")
    };
    let base = file_id(&display);
    fs::create_dir_all(library.shows_dir()).map_err(error)?;
    let mut existing = None;
    let mut destination = PathBuf::new();
    for suffix in 0..10_000 {
        let suffix = if suffix == 0 {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let candidate = library
            .shows_dir()
            .join(format!("lessons_{base}{suffix}.show"));
        if !candidate.exists() {
            destination = candidate;
            break;
        }
        let show = ShowFile::load(&candidate).map_err(error)?;
        if show.show["origin"] == "lessons"
            && show.show["meta"]["title"] == title
            && show.show["meta"]["name"] == name
        {
            existing = Some(show);
            destination = candidate;
            break;
        }
    }
    if destination.as_os_str().is_empty() {
        return Err("too many lesson filename collisions".into());
    }
    // Reopening an imported lesson keeps operator edits and requires no network.
    let show = if let Some(show) = existing {
        show
    } else {
        let mut show = ShowFile::blank(&new_id(), &display);
        show.show["origin"] = json!("lessons");
        show.show["category"] = json!("lessons");
        show.show["meta"] = json!({"title":title,"name":name,"venue":lesson["venueName"]});
        show.show["reference"] = json!({"type":"lessons","data":{"about":lesson["programAbout"],"studyName":lesson["studyName"]}});
        show.show["layouts"]["main"]["notes"] = lesson
            .get("lessonDescription")
            .cloned()
            .unwrap_or(json!(""));
        show.show["slides"] = json!({});
        show.show["media"] = json!({});
        let files = lesson_groups(lesson)?;
        let count: usize = files.iter().map(Vec::len).sum();
        if count == 0 {
            return Err("lesson contains no media".into());
        }
        let mut cache = MediaCache::open(&library)?;
        let mut order = Vec::<Value>::new();
        let mut index = 0;
        for group in files {
            let mut parent = None;
            for file in group {
                index += 1;
                let id = format!("s{index}");
                let media_id = format!("m{index}");
                let source = file["url"].as_str().unwrap();
                let file_name = file["name"].as_str().unwrap_or("Media");
                let extension = media_extension(&file)?;
                let loops = file["loopVideo"].as_bool() == Some(true)
                    || file["loop"].as_bool() == Some(true)
                    || file_name.contains("Title");
                tracing::info!("preparing lesson media {index}/{count}: {file_name}");
                let path = cache.download(source, extension)?;
                show.show["media"][&media_id] =
                    json!({"name":file_name,"path":path,"muted":false,"loop":loops});
                show.show["slides"][&id] = json!({"group":file_name,"color":"","settings":{},"notes":"","items":[],"children":[]});
                let mut node = json!({"background":media_id});
                if !loops && is_video(extension) && index < count {
                    node["actions"] = json!({"nextAfterMedia":true});
                }
                if let Some(parent_id) = &parent {
                    show.show["slides"][parent_id]["children"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!(id));
                    show.show["slides"][&id]["group"] = Value::Null;
                    order.last_mut().unwrap()["children"][&id] = node;
                } else {
                    node["id"] = json!(id);
                    order.push(node);
                    parent = Some(id);
                }
            }
        }
        show.show["layouts"]["main"]["slides"] = json!(order);
        show.save(&destination).map_err(error)?;
        show
    };
    let mut projects = ProjectsFile::load(&library.projects_path()).map_err(error)?;
    projects.add_project("lessons", "Lessons.church");
    if !projects
        .projects()
        .iter()
        .find(|p| p.id == "lessons")
        .is_some_and(|p| p.shows.iter().any(|entry| entry.id == show.id))
    {
        projects.append_show("lessons", &show.id);
        projects.save(&library.projects_path()).map_err(error)?;
    }
    Ok(SyncReport {
        shows: 1,
        project_id: Some("lessons".into()),
    })
}

fn lesson_groups(lesson: &Value) -> Result<Vec<Vec<Value>>, String> {
    let mut groups = Vec::new();
    if let Some(image) = lesson["lessonImage"].as_str().filter(|url| !url.is_empty()) {
        groups.push(vec![json!({"name":"Lesson Image","url":image})]);
    }
    let messages: Vec<_> = if let Some(sections) = lesson["sections"].as_array() {
        sections
            .iter()
            .flat_map(|section| section["actions"].as_array().into_iter().flatten())
            .filter(|action| action["actionType"] == "play")
            .collect()
    } else {
        lesson["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .collect()
    };
    for message in messages {
        let files: Vec<_> = message["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|file| file["url"].as_str().is_some_and(|url| !url.is_empty()))
            .cloned()
            .collect();
        if !files.is_empty() {
            groups.push(files);
        }
    }
    if groups.iter().map(Vec::len).sum::<usize>() > 2000 {
        return Err("lesson contains more than 2000 media slides".into());
    }
    // Validate every media type/URL before starting the first download.
    for file in groups.iter().flatten() {
        media_extension(file)?;
        let url = file["url"].as_str().unwrap();
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Err("lesson media must use HTTP or HTTPS".into());
        }
    }
    Ok(groups)
}

pub(super) fn plan_media_show(
    library: &Library,
    id: &str,
    name: &str,
    notes: &str,
    files: &[Value],
) -> Result<ShowFile, String> {
    let mut files = files.to_vec();
    for file in &mut files {
        if !file.is_object() {
            return Err("service-plan media entry is not an object".into());
        }
        if !file["url"].as_str().is_some_and(|url| !url.is_empty()) {
            let stream = file["streamUrl"].as_str().unwrap_or("");
            let path = stream
                .split(['?', '#'])
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            if ![".mp4", ".mov", ".m4v", ".webm"]
                .iter()
                .any(|extension| path.ends_with(extension))
            {
                return Err("stream-only lesson media needs a downloadable video URL".into());
            }
            file["url"] = file["streamUrl"].clone();
        }
    }
    let groups = lesson_groups(&json!({"messages":[{"files":files}]}))?;
    if groups.first().map(Vec::len) != Some(files.len()) || files.is_empty() {
        return Err("service-plan media is missing a downloadable URL".into());
    }
    let mut cache = MediaCache::open(library)?;
    let mut show = ShowFile::blank(id, name);
    show.show["origin"] = json!("churchApps");
    show.show["category"] = json!("churchapps");
    show.show["meta"]["title"] = json!(name);
    show.show["layouts"]["main"]["notes"] = json!(notes);
    show.show["slides"] = json!({});
    show.show["media"] = json!({});
    let mut order = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let slide_id = format!("s{}", index + 1);
        let media_id = format!("m{}", index + 1);
        let extension = media_extension(file)?;
        let path = cache.download(file["url"].as_str().unwrap(), extension)?;
        show.show["media"][&media_id] = json!({"name":file["name"],"path":path,
            "type":if is_video(extension) {"video"} else {"image"},
            "loop":file["loopVideo"].as_bool().unwrap_or(false),"muted":false});
        show.show["slides"][&slide_id] =
            json!({"group":name,"color":null,"settings":{},"notes":"","items":[]});
        order.push(json!({"id":slide_id,"background":media_id}));
    }
    show.show["layouts"]["main"]["slides"] = json!(order);
    Ok(show)
}

fn media_extension(file: &Value) -> Result<&'static str, String> {
    let mime = file["fileType"].as_str().unwrap_or("");
    let extension = match mime.split(';').next().unwrap_or("") {
        "video" | "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "video/webm" => "webm",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "",
    };
    if !extension.is_empty() {
        return Ok(extension);
    }
    let url = file["url"]
        .as_str()
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    for extension in [
        "mp4", "mov", "m4v", "webm", "png", "jpg", "jpeg", "webp", "gif",
    ] {
        if url.ends_with(&format!(".{extension}")) {
            return Ok(extension);
        }
    }
    if file["streamUrl"]
        .as_str()
        .is_some_and(|url| !url.is_empty())
    {
        return Ok("mp4");
    }
    Err("lesson media has no supported file extension or MIME type".into())
}

fn is_video(extension: &str) -> bool {
    ["mp4", "mov", "m4v", "webm"].contains(&extension)
}

struct MediaCache {
    directory: PathBuf,
    // ponytail: one JSON cache index; use SQLite if it grows to thousands of URLs.
    index: Map<String, Value>,
    agent: ureq::Agent,
}

impl MediaCache {
    fn open(library: &Library) -> Result<Self, String> {
        let directory = library.media_dir().join("Lessons");
        fs::create_dir_all(&directory).map_err(error)?;
        let directory = directory.canonicalize().map_err(error)?;
        let catalog = Catalog::load(&directory.join("index.json")).map_err(error)?;
        Ok(Self {
            directory,
            index: catalog.entries().into_iter().collect(),
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(30))
                .timeout(Duration::from_secs(480))
                .build(),
        })
    }

    fn download(&mut self, url: &str, extension: &str) -> Result<PathBuf, String> {
        if let Some(entry) = self.index.get(url) {
            let name = entry["file"].as_str().unwrap_or("");
            let size = entry["bytes"].as_u64().unwrap_or(0);
            // Cached paths must be single filenames, never traversal or symlinks.
            if !name.is_empty() && !name.contains(['/', '\\', ':']) && name != "." && name != ".." {
                let path = self.directory.join(name);
                if fs::symlink_metadata(&path)
                    .is_ok_and(|meta| meta.is_file() && meta.len() == size && size > 0)
                {
                    return Ok(path);
                }
            }
        }
        // Media requests carry no ChurchApps account token, including redirects.
        let response = self.agent.get(url).call().map_err(|err| match err {
            ureq::Error::Status(code, _) => format!("lesson media returned HTTP {code}"),
            ureq::Error::Transport(_) => "lesson media download failed".into(),
        })?;
        if response.status() != 200 {
            return Err(format!(
                "lesson media returned unexpected HTTP {}",
                response.status()
            ));
        }
        let expected = response
            .header("Content-Length")
            .and_then(|value| value.parse::<u64>().ok());
        if expected.is_some_and(|size| size > MAX_MEDIA) {
            return Err("lesson media exceeds 2 GiB".into());
        }
        let name = format!("{}.{}", new_id(), extension);
        let final_path = self.directory.join(&name);
        let partial = final_path.with_extension(format!("{extension}.part"));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)
                .map_err(error)?;
            let size = std::io::copy(&mut response.into_reader().take(MAX_MEDIA + 1), &mut file)
                .map_err(error)?;
            if size == 0 || size > MAX_MEDIA || expected.is_some_and(|expected| expected != size) {
                return Err("lesson media download is empty, incomplete, or too large".into());
            }
            file.flush().map_err(error)?;
            file.sync_all().map_err(error)?;
            drop(file);
            fs::rename(&partial, &final_path).map_err(error)?;
            self.index
                .insert(url.to_string(), json!({"file":name,"bytes":size}));
            Catalog::parse(&Value::Object(self.index.clone()).to_string())
                .map_err(error)?
                .save(&self.directory.join("index.json"))
                .map_err(error)?;
            Ok(final_path.clone())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&partial);
        }
        result
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use freeshow_data::SlideBackdrop;
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    pub(crate) fn server(responses: Vec<Vec<u8>>) -> (String, JoinHandle<Vec<String>>) {
        server_with(|_| responses)
    }

    pub(crate) fn server_with(
        responses: impl FnOnce(&str) -> Vec<Vec<u8>>,
    ) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let responses = responses(&url);
        let thread = thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(err)
                            if err.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(err) => panic!("test listener: {err}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") && request.len() < 16384 {
                    let mut byte = [0];
                    if stream.read(&mut byte).unwrap() == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                requests.push(String::from_utf8(request).unwrap());
                stream.write_all(&response).unwrap();
            }
            requests
        });
        (url, thread)
    }

    pub(crate) fn response(body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("freeshow-lesson-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn playlist_downloads_grouped_media_and_reopens_offline_without_losing_edits() {
        let root = root("playlist");
        let (base, server) = server(vec![
            response(b"cover"),
            response(b"clip"),
            response(b"last clip"),
        ]);
        let mut lesson = json!({
            "lessonName":"Week 1", "lessonTitle":"Faith", "lessonDescription":"Leader notes",
            "lessonImage":format!("{base}/cover.png?token=test"), "venueName":"Kids",
            "messages":[
                {"name":"Welcome","files":[
                    {"name":"Title Card","url":format!("{base}/cover.png?token=test")},
                    {"name":"Intro","url":format!("{base}/clip.mp4"),"loopVideo":false}
                ]},
                {"name":"Goodbye","files":[{"name":"Ending","url":format!("{base}/last"),"fileType":"video/mp4"}]}
            ]
        });
        let report = import_lesson_document(&root, &lesson).unwrap();
        assert_eq!(
            report,
            SyncReport {
                shows: 1,
                project_id: Some("lessons".into())
            }
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].starts_with("GET /cover.png?token=test "));
        assert!(
            requests
                .iter()
                .all(|request| !request.to_ascii_lowercase().contains("authorization:"))
        );
        let library = Library::open(&root);
        let mut show = library.list_shows().unwrap().remove(0);
        assert_eq!(show.name(), "Faith - Week 1");
        assert_eq!(show.ordered_slides().len(), 4);
        assert_eq!(show.show["slides"]["s2"]["children"], json!(["s3"]));
        assert!(show.nexts_after_media_at(2));
        assert!(!show.nexts_after_media_at(3));
        for (index, expected) in [b"cover".as_slice(), b"cover", b"clip", b"last clip"]
            .into_iter()
            .enumerate()
        {
            let SlideBackdrop::Picture { path, .. } = show.backdrop_at(index) else {
                panic!("missing media")
            };
            assert!(Path::new(&path).starts_with(library.media_dir().canonicalize().unwrap()));
            assert_eq!(fs::read(path).unwrap(), expected);
        }
        assert_eq!(show.show["media"]["m2"]["loop"], true);
        assert_eq!(show.show["media"]["m3"]["muted"], false);
        assert_eq!(show.show["layouts"]["main"]["notes"], "Leader notes");
        show.show["layouts"]["main"]["notes"] = json!("Operator edit");
        show.save(show.path.as_ref().unwrap()).unwrap();
        import_lesson_document(&root, &lesson).unwrap();
        assert_eq!(
            library.list_shows().unwrap()[0].show["layouts"]["main"]["notes"],
            "Operator edit"
        );
        // A new lesson using the same URLs must use the completed cache too.
        lesson["lessonName"] = json!("Week 2");
        import_lesson_document(&root, &lesson).unwrap();
        assert_eq!(library.list_shows().unwrap().len(), 2);
        let projects = ProjectsFile::load(&library.projects_path()).unwrap();
        assert_eq!(projects.projects()[0].shows.len(), 2);
        assert_eq!(
            fs::read_dir(library.media_dir().join("Lessons"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext != "json"))
                .count(),
            3
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn full_lesson_imports_play_actions_from_the_imports_folder() {
        let root = root("format");
        let (base, server) = server(vec![response(b"video")]);
        let lesson = json!({"lessonName":"Story", "lessonDescription":"Notes", "studyName":"Trust",
        "sections":[{"name":"Opening","actions":[
            {"actionType":"say","content":"Leader text","files":[{"url":"file:///private/skip.mp4"}]},
            {"actionType":"play","content":"Story video","files":[{"name":"Story","url":format!("{base}/story.mp4"),"loopVideo":true}]}
        ]}]});
        let library = Library::open(&root);
        fs::create_dir_all(library.imports_dir()).unwrap();
        fs::write(library.imports_dir().join("story.olf"), lesson.to_string()).unwrap();
        assert_eq!(import_sources(&root).shows, 1);
        server.join().unwrap();
        assert!(library.import_show_sources().unwrap().is_empty());
        let shows = library.list_shows().unwrap();
        assert_eq!(shows.len(), 1);
        assert_eq!(shows[0].show["reference"]["data"]["studyName"], "Trust");
        assert_eq!(shows[0].show["media"]["m1"]["loop"], true);
        assert!(!shows[0].nexts_after_media_at(0));
        assert_eq!(import_sources(&root).shows, 1);
        assert_eq!(library.list_shows().unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_downloads_are_not_cached_and_failure_publishes_no_lesson() {
        let root = root("failure");
        let (base, server) = server(vec![
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort".to_vec(),
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nConnection: close\r\n\r\npart"
                .to_vec(),
            response(b"complete"),
        ]);
        let lesson = json!({"lessonName":"Retry", "messages":[{"files":[{"name":"Clip","url":format!("{base}/clip.mp4")}]}]});
        let library = Library::open(&root);
        assert!(import_lesson_document(&root, &lesson).is_err());
        assert!(library.list_shows().unwrap().is_empty());
        assert!(!library.projects_path().exists());
        assert_eq!(
            fs::read_dir(library.media_dir().join("Lessons"))
                .unwrap()
                .count(),
            0
        );
        assert!(import_lesson_document(&root, &lesson).is_err());
        assert!(library.list_shows().unwrap().is_empty());
        import_lesson_document(&root, &lesson).unwrap();
        server.join().unwrap();
        assert_eq!(library.list_shows().unwrap().len(), 1);
        let bad =
            json!({"lessonName":"Unsafe","messages":[{"files":[{"url":"file:///outside.png"}]}]});
        assert!(import_lesson_document(&root, &bad).is_err());
        assert_eq!(library.list_shows().unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn redirects_work_and_http_errors_leave_previous_lessons_unchanged() {
        let root = root("redirect");
        let (base, server) = server(vec![
            b"HTTP/1.1 302 Found\r\nLocation: /actual.png\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            response(b"picture"),
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        ]);
        let mut lesson = json!({"lessonName":"Redirect", "lessonImage":format!("{base}/cover.png"), "messages":[]});
        import_lesson_document(&root, &lesson).unwrap();
        let library = Library::open(&root);
        let project = fs::read(library.projects_path()).unwrap();
        lesson["lessonName"] = json!("Missing");
        lesson["lessonImage"] = json!(format!("{base}/missing.png"));
        assert!(
            import_lesson_document(&root, &lesson)
                .unwrap_err()
                .contains("404")
        );
        let requests = server.join().unwrap();
        assert!(requests[1].starts_with("GET /actual.png "));
        assert!(
            requests
                .iter()
                .all(|request| !request.to_ascii_lowercase().contains("authorization:"))
        );
        assert_eq!(fs::read(library.projects_path()).unwrap(), project);
        assert_eq!(library.list_shows().unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
